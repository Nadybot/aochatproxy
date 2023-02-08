#![deny(clippy::pedantic)]
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    process::exit,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use dashmap::DashSet;
use libc::{c_int, sighandler_t, signal, SIGINT, SIGTERM};
use log::{debug, error, info, trace, warn};
use nadylib::{
    client_socket::SocketSendHandle,
    models::Channel,
    packets::{
        BuddyAddPacket, BuddyRemovePacket, IncomingPacket, LoginSelectPacket, MsgPrivatePacket,
        OutgoingPacket, PacketType, PingPacket,
    },
    AOSocket, Result as NadylibResult, SocketConfig,
};
use nanoserde::DeJson;
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
    sync::{mpsc::unbounded_channel, Notify},
    task::JoinHandle,
    time::{sleep, Duration},
};
use worker::WorkerHandle;

mod communication;
mod config;
mod select;
mod unfreeze;
mod worker;

async fn wait_server_ready(addr: &str) {
    while TcpStream::connect(addr).await.is_err() {
        sleep(Duration::from_secs(10)).await;
    }
}

#[allow(clippy::too_many_lines)]
async fn run_proxy() -> NadylibResult<()> {
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

    let unfreezer = unfreeze::Unfreezer::new(config.unfreeze_accounts_with_proxy);

    let identifier = format!(
        concat!(
            r#"{{"name": "aochatproxy", "version": ""#,
            env!("CARGO_PKG_VERSION"),
            r#"", "type": "capabilities", "supported-cmds": ["capabilities", "ping"], "rate-limited": true, "default-mode": {:?}, "workers": {:?}, "started-at": {}, "send-modes": ["round-robin", "by-charid", "by-msgid", "proxy-default", "by-worker"], "buddy-modes": ["by-worker"]}}"#
        ),
        default_mode.as_str(),
        worker_names,
        started_at_unix
    );

    loop {
        info!("Waiting for chat server to be available");
        wait_server_ready(&config.server_address).await;

        let identifier_clone = identifier.clone();

        let logged_in = Arc::new(Notify::new());
        let logged_in_waiter = logged_in.clone();

        let (worker_sender, mut worker_receiver) = unbounded_channel();
        let command_reply = worker_sender.clone();

        // List of workers
        let mut workers: Vec<worker::WorkerHandle> = Vec::with_capacity(account_num + 1);
        let mut worker_tasks: Vec<JoinHandle<NadylibResult<()>>> =
            Vec::with_capacity(account_num + 1);

        // Create all workers
        for (idx, acc) in config.accounts.iter().enumerate() {
            info!("Spawning worker for {}", acc.character);
            let handle = SocketSendHandle::new(worker_sender.clone(), None);

            let (worker, task) = WorkerHandle::new(
                idx + 1,
                config.clone(),
                handle,
                logged_in.clone(),
                worker_ids.clone(),
                unfreezer.clone(),
            )
            .await;
            workers.push(worker);
            worker_tasks.push(task);
        }

        let addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), config.port_number);
        let tcp_server = TcpListener::bind(addr).await?;
        info!("Listening on port {}", config.port_number);
        info!("Waiting for client to connect...");
        let (client, addr) = tcp_server.accept().await?;
        info!("Client connected from {}", addr);

        // Create a socket from the client
        let mut sock = AOSocket::from_stream(
            client,
            SocketConfig::default().keepalive(false).limit_tells(false),
        );

        let (main_worker, main_task) = WorkerHandle::new(
            0,
            config.clone(),
            sock.get_sender(),
            logged_in.clone(),
            worker_ids.clone(),
            unfreezer.clone(),
        )
        .await;
        workers.insert(0, main_worker);
        worker_tasks.insert(0, main_task);

        let sock_to_workers = sock.get_sender();

        // Forward stuff from the workers to the main
        let worker_read_task = spawn(async move {
            logged_in_waiter.notified().await;
            info!("Main logged in, relaying packets now");
            while let Some(msg) = worker_receiver.recv().await {
                let _res = sock_to_workers.send_raw(msg.0, msg.1).await;
            }

            Ok(())
        });

        let worker_id_clone = worker_ids.clone();

        // Loop over incoming packets and depending on the type, round robin them
        // If not, we just send them over the normal FC connection
        let proxy_task: JoinHandle<NadylibResult<()>> = spawn(async move {
            // For round robin on private msgs
            let start_at = { usize::from(!send_tells_over_main) };
            let mut current_buddy = start_at;
            let mut current_lookup = 0;
            loop {
                let packet = sock.read_raw_packet().await?;
                debug!("Received {:?} packet from main", packet.0);
                trace!("Packet body: {:?}", packet.1);

                match packet.0 {
                    PacketType::LoginSelect => {
                        let pack = LoginSelectPacket::load(&packet.1)?;
                        if worker_id_clone.contains(&pack.character_id) {
                            error!("Main attempted to log in as an existing worker. Remove the worker from the config and restart.");
                            break;
                        }
                        workers[0].send_packet(packet).await;
                    }
                    PacketType::BuddyAdd => {
                        let pack = BuddyAddPacket::load(&packet.1)?;

                        let send_on = if let Ok(v) =
                            communication::BuddyAddPayload::deserialize_json(&pack.send_tag)
                        {
                            // Do not send the packet at all if the worker is invalid
                            if v.worker > account_num {
                                continue;
                            }
                            debug!("Adding buddy on {} (forced by packet)", workers[v.worker]);
                            v.worker
                        } else {
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
                        };

                        workers[send_on].send_packet(packet).await;
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1)?;

                        // Remove the buddy on the workers that have it on the buddy list
                        for worker in &workers {
                            if worker.has_buddy(b.character_id).await {
                                debug!("Removing buddy {} on {}", b.character_id, worker);
                                worker.send_packet(packet.clone()).await;
                            }
                        }
                    }
                    PacketType::MsgPrivate => {
                        let mut m = MsgPrivatePacket::load(&packet.1)?;

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

                            if let Ok(payload) = communication::SendMessagePayload::deserialize_json(
                                &m.message.send_tag,
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
                        let mut p = PingPacket::load(&packet.1)?;

                        match communication::CommandPayload::deserialize_json(&p.client) {
                            Ok(v) => match v.cmd {
                                communication::Command::Capabilities => {
                                    p.client = identifier_clone.clone();
                                    let serialized = p.serialize();
                                    let _res = command_reply.send(serialized);
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

            Ok(())
        });

        worker_tasks.push(worker_read_task);
        worker_tasks.push(proxy_task);
        let (result, idx, others) = select::select_all(worker_tasks).await;
        for fut in others {
            fut.abort();
        }

        warn!(
            "Lost a connection (due to {:?}, {}), restarting...",
            result, idx
        );
    }
}

pub extern "C" fn handler(_: c_int) {
    std::process::exit(0);
}

unsafe fn set_os_handlers() {
    signal(SIGINT, handler as extern "C" fn(_) as sighandler_t);
    signal(SIGTERM, handler as extern "C" fn(_) as sighandler_t);
}

fn main() -> NadylibResult<()> {
    unsafe { set_os_handlers() };

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_proxy())
}
