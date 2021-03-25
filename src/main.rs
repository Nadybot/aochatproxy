#![deny(clippy::all)]
use dashmap::DashSet;
use log::{debug, error, info, trace, warn};
use nadylib::{
    client_socket::SocketSendHandle,
    models::Channel,
    packets::{
        BuddyAddPacket, BuddyRemovePacket, IncomingPacket, LoginSelectPacket, MsgPrivatePacket,
        OutgoingPacket, PacketType, PingPacket,
    },
    AOSocket, Result, SocketConfig,
};
#[cfg(not(feature = "simd-json"))]
use serde_json::{from_str, to_string};
#[cfg(feature = "simd-json")]
use simd_json::{from_str, to_string};
use tokio::{
    net::TcpListener,
    select, spawn,
    sync::{mpsc::unbounded_channel, Notify},
};
use worker::WorkerHandle;

use std::{
    process::exit,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

mod communication;
mod config;
mod worker;

#[tokio::main]
async fn main() -> Result<()> {
    let conf = config::try_load();
    let config = conf.unwrap_or_else(|e| {
        let _ = env_logger::builder().format_timestamp_millis().try_init();
        error!("Configuration Error: {}", e);
        exit(1)
    });

    let started_at = SystemTime::now();
    let started_at_unix = started_at.duration_since(UNIX_EPOCH).unwrap().as_secs();

    let spam_bot_support = config.spam_bot_support;
    let send_tells_over_main = config.send_tells_over_main;
    let account_num = config.accounts.len();
    let worker_ids: Arc<DashSet<u32>> = Arc::new(DashSet::new());
    let worker_names: Vec<String> = config
        .accounts
        .iter()
        .map(|i| i.character.clone())
        .collect();

    let default_mode = config.default_mode;

    let identifier = format!(
        concat!(
            r#"{{"name": "aochatproxy", "version": ""#,
            env!("CARGO_PKG_VERSION"),
            r#"", "type": "capabilities", "supported-cmds": ["capabilities", "ping"], "rate-limited": true, "default-mode": {}, "workers": {:?}, "started-at": {}, "send-modes": ["round-robin", "by-charid", "by-msgid", "proxy-default", "by-worker"], "buddy-modes": ["by-worker"]}}"#
        ),
        to_string(&default_mode).unwrap(),
        worker_names,
        started_at_unix
    );

    loop {
        let identifier_clone = identifier.clone();

        let logged_in = Arc::new(Notify::new());
        let logged_in_waiter = logged_in.clone();

        let (worker_sender, mut worker_receiver) = unbounded_channel();
        let command_reply = worker_sender.clone();

        // List of workers
        let mut workers: Vec<worker::WorkerHandle> = Vec::with_capacity(account_num + 1);

        // Create all workers
        for (idx, acc) in config.accounts.iter().enumerate() {
            info!("Spawning worker for {}", acc.character);
            let handle = SocketSendHandle::new(worker_sender.clone(), None);

            let worker = WorkerHandle::new(
                idx + 1,
                config.clone(),
                handle,
                logged_in.clone(),
                worker_ids.clone(),
            )
            .await;
            workers.push(worker);
        }

        let tcp_server = TcpListener::bind(format!("0.0.0.0:{}", config.port_number)).await?;
        info!("Listening on port {}", config.port_number);
        info!("Waiting for client to connect...");
        let (client, addr) = tcp_server.accept().await?;
        info!("Client connected from {}", addr);

        // Create a socket from the client
        let mut sock = AOSocket::from_stream(
            client,
            SocketConfig::default().keepalive(false).limit_tells(false),
        );

        let main_worker = WorkerHandle::new(
            0,
            config.clone(),
            sock.get_sender(),
            logged_in.clone(),
            worker_ids.clone(),
        )
        .await;
        workers.insert(0, main_worker);

        let sock_to_workers = sock.get_sender();

        // Forward stuff from the workers to the main
        let worker_read_task = spawn(async move {
            logged_in_waiter.notified().await;
            info!("Main logged in, relaying packets now");
            while let Some(msg) = worker_receiver.recv().await {
                let _ = sock_to_workers.send_raw(msg.0, msg.1).await;
            }
        });

        let worker_id_clone = worker_ids.clone();

        // Loop over incoming packets and depending on the type, round robin them
        // If not, we just send them over the normal FC connection
        let proxy_task = spawn(async move {
            // For round robin on private msgs
            let start_at = {
                if send_tells_over_main {
                    0
                } else {
                    1
                }
            };
            let mut current_buddy = start_at;
            let mut current_lookup = 0;
            while let Ok(packet) = sock.read_raw_packet().await {
                debug!("Received {:?} packet from main", packet.0);
                trace!("Packet body: {:?}", packet.1);

                match packet.0 {
                    PacketType::LoginSelect => {
                        let pack = LoginSelectPacket::load(&packet.1).unwrap();
                        if worker_id_clone.contains(&pack.character_id) {
                            error!("Main attempted to log in as an existing worker. Remove the worker from the config and restart.");
                            break;
                        }
                        workers[0].send_packet(packet).await;
                    }
                    PacketType::BuddyAdd => {
                        let mut pack = BuddyAddPacket::load(&packet.1).unwrap();

                        let send_on =
                            match from_str::<communication::BuddyAddPayload>(&mut pack.send_tag) {
                                Ok(v) => {
                                    // Do not send the packet at all if the worker is invalid
                                    if v.worker > account_num {
                                        continue;
                                    }
                                    debug!(
                                        "Adding buddy on {} (forced by packet)",
                                        workers[v.worker]
                                    );
                                    v.worker
                                }
                                Err(_) => {
                                    // Add the buddy on the worker with least buddies
                                    let mut least_buddies = 0;
                                    let mut buddy_count = workers[0].get_total_buddies().await;

                                    for (id, worker) in workers.iter().enumerate().skip(1) {
                                        let worker_buddy_count = worker.get_total_buddies().await;
                                        if worker_buddy_count < buddy_count {
                                            least_buddies = id;
                                            buddy_count = worker_buddy_count;
                                        }
                                    }

                                    debug!(
                                        "Adding buddy on {} ({} current buddies)",
                                        workers[least_buddies], buddy_count
                                    );
                                    least_buddies
                                }
                            };

                        workers[send_on].send_packet(packet).await;
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1).unwrap();

                        // Remove the buddy on the workers that have it on the buddy list
                        for worker in workers.iter() {
                            if worker.has_buddy(b.character_id).await {
                                debug!("Removing buddy {} on {}", b.character_id, worker);
                                worker.send_packet(packet.clone()).await;
                            }
                        }
                    }
                    PacketType::MsgPrivate => {
                        let mut m = MsgPrivatePacket::load(&packet.1).unwrap();

                        if spam_bot_support && m.message.send_tag != "\u{0}" {
                            // We assume legacy aka spam tag
                            let mut send_mode = default_mode;
                            let charid = {
                                if let Channel::Tell(id) = m.message.channel {
                                    id as usize
                                } else {
                                    unreachable!();
                                }
                            };
                            let mut msgid = None;
                            let mut worker = None;

                            if let Ok(payload) = from_str::<communication::SendMessagePayload>(
                                &mut m.message.send_tag,
                            ) {
                                if payload.mode != communication::SendMode::Default {
                                    send_mode = payload.mode;
                                }
                                msgid = payload.msgid;
                                worker = payload.worker;
                            } else if m.message.send_tag != "spam" {
                                // If it is neither new or legacy, use the main
                                send_mode = communication::SendMode::ByWorker;
                                worker = Some(0);
                            }

                            current_buddy = {
                                if send_mode == communication::SendMode::ByCharId
                                    || (send_mode == communication::SendMode::ByMsgId
                                        && default_mode == communication::SendMode::ByCharId)
                                {
                                    if send_tells_over_main {
                                        (charid) % (account_num + 1)
                                    } else {
                                        (charid) % account_num + 1
                                    }
                                } else if let (Some(m), communication::SendMode::ByMsgId) =
                                    (msgid, send_mode)
                                {
                                    if send_tells_over_main {
                                        m % (account_num + 1)
                                    } else {
                                        m % account_num + 1
                                    }
                                } else if let (Some(w), communication::SendMode::ByWorker) =
                                    (worker, send_mode)
                                {
                                    if send_tells_over_main {
                                        w % (account_num + 1)
                                    } else {
                                        (w - 1) % (account_num) + 1
                                    }
                                } else if send_mode == communication::SendMode::RoundRobin {
                                    current_buddy += 1;
                                    if current_buddy > account_num {
                                        current_buddy = start_at;
                                    }
                                    current_buddy
                                } else {
                                    current_buddy
                                }
                            };
                        } else {
                            current_buddy = 0;
                        }

                        m.message.send_tag = String::from("\u{0}");
                        let serialized = m.serialize();
                        workers[current_buddy].send_packet(serialized).await;
                    }
                    PacketType::Ping => {
                        let mut p = PingPacket::load(&packet.1).unwrap();

                        match from_str::<communication::CommandPayload>(&mut p.client) {
                            Ok(v) => match v.cmd {
                                communication::Command::Capabilities => {
                                    p.client = identifier_clone.clone();
                                    let serialized = p.serialize();
                                    let _ = command_reply.send(serialized);
                                }
                                communication::Command::Ping => {
                                    if let (Some(worker), Some(payload)) = (v.worker, v.payload) {
                                        let packet = PingPacket { client: payload };
                                        workers[worker].send_packet(packet.serialize()).await;
                                    }
                                }
                            },
                            Err(_) => workers[0].send_packet(packet).await,
                        };
                    }
                    PacketType::ClientLookup => {
                        workers[current_lookup].send_packet(packet).await;
                        current_lookup += 1;
                        if current_lookup > account_num {
                            current_lookup = 0;
                        }
                    }
                    _ => {
                        workers[0].send_packet(packet).await;
                    }
                }
            }
        });

        select! {
            _ = worker_read_task => {},
            _ = proxy_task => {},
        }

        warn!("Lost a connection (probably from client), restarting...");
    }
}
