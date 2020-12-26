use communication::SendMode;
use dotenv::dotenv;
use log::{debug, error, info, trace, warn};
use nadylib::{
    models::Channel,
    packets::{
        BuddyRemovePacket, IncomingPacket, MsgPrivatePacket, OutgoingPacket, PacketType, PingPacket,
    },
    AOSocket, Result, SocketConfig,
};
use serde_json::{from_str, to_string};
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
    dotenv().ok();
    env_logger::builder().format_timestamp_millis().init();

    let config = config::load_config().unwrap_or_else(|e| {
        error!("Configuration Error: {}", e);
        exit(1)
    });
    let started_at = SystemTime::now();
    let started_at_unix = started_at.duration_since(UNIX_EPOCH).unwrap().as_secs();

    loop {
        let spam_bot_support = config.spam_bot_support;
        let send_tells_over_main = config.send_tells_over_main;
        let account_num = config.accounts.len();
        let worker_names: Vec<String> = config
            .accounts
            .iter()
            .map(|i| i.character.clone())
            .collect();

        let default_mode = match config.relay_by_id {
            true => communication::SendMode::ByCharId,
            false => communication::SendMode::RoundRobin,
        };

        let logged_in = Arc::new(Notify::new());
        let logged_in_waiter = logged_in.clone();

        let (worker_sender, mut worker_receiver) = unbounded_channel();
        let command_reply = worker_sender.clone();

        // List of workers
        let mut workers: Vec<worker::WorkerHandle> = Vec::with_capacity(account_num + 1);

        // Create all workers
        for (idx, acc) in config.accounts.iter().enumerate() {
            info!("Spawning worker for {}", acc.character);

            let worker = WorkerHandle::new(
                idx + 1,
                config.clone(),
                worker_sender.clone(),
                logged_in.clone(),
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
        let mut sock = AOSocket::from_stream(client, SocketConfig::default().keepalive(false));

        let main_worker =
            WorkerHandle::new(0, config.clone(), sock.get_sender(), logged_in.clone()).await;
        workers.insert(0, main_worker);

        let sock_to_workers = sock.get_sender();

        // Forward stuff from the workers to the main
        let worker_read_task = spawn(async move {
            logged_in_waiter.notified().await;
            info!("Main logged in, relaying packets now");
            while let Some(msg) = worker_receiver.recv().await {
                let _ = sock_to_workers.send(msg);
            }
        });

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
            while let Ok(packet) = sock.read_raw_packet().await {
                debug!("Received {:?} packet from main", packet.0);
                trace!("Packet body: {:?}", packet.1);

                match packet.0 {
                    PacketType::BuddyAdd => {
                        // Add the buddy on the worker with least buddies
                        let mut least_buddies = workers[0].clone();
                        let mut buddy_count = workers[0].get_total_buddies().await;

                        for worker in workers.iter().skip(1) {
                            let worker_buddy_count = worker.get_total_buddies().await;
                            if worker_buddy_count < buddy_count {
                                least_buddies = worker.clone();
                                buddy_count = worker_buddy_count;
                            }
                        }

                        debug!(
                            "Adding buddy on {} ({} current buddies)",
                            least_buddies, buddy_count
                        );
                        least_buddies.send_packet(packet).await;
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
                            let mut charid = 0;
                            if let Channel::Tell(id) = m.message.channel {
                                charid = id as usize;
                            }
                            let mut msgid = None;
                            let mut worker = None;

                            if let Ok(payload) =
                                from_str::<communication::SendMessagePayload>(&m.message.send_tag)
                            {
                                if payload.mode != SendMode::Default {
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
                                } else if send_mode == communication::SendMode::ByMsgId
                                    && msgid.is_some()
                                {
                                    if send_tells_over_main {
                                        msgid.unwrap() % (account_num + 1)
                                    } else {
                                        msgid.unwrap() % account_num + 1
                                    }
                                } else if send_mode == communication::SendMode::ByWorker
                                    && worker.is_some()
                                {
                                    if send_tells_over_main {
                                        worker.unwrap() % (account_num + 1)
                                    } else {
                                        (worker.unwrap() - 1) % (account_num) + 1
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

                        match from_str::<communication::CommandPayload>(&p.client) {
                            Ok(v) => match v.cmd {
                                communication::Command::Capabilities => {
                                    let string = format!(
                                        r#"{{"name": "aochatproxy", "version": "0.1.0", "default-mode": {}, "workers": {:?}, "started-at": {}, "send_modes": ["round-robin", "by-charid", "by-msgid", "proxy-default", "by-worker"]}}"#,
                                        to_string(&default_mode).unwrap(),
                                        worker_names,
                                        started_at_unix
                                    );
                                    p.client = string;
                                    let serialized = p.serialize();
                                    let _ = command_reply.send(serialized);
                                }
                            },
                            Err(_) => workers[0].send_packet(packet).await,
                        };
                    }
                    _ => {
                        let _ = workers[0].send_packet(packet).await;
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
