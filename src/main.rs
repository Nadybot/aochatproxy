use dashmap::DashMap;
use dotenv::dotenv;
use log::{debug, error, info, log_enabled, trace, warn, Level::Trace};
use nadylib::{
    models::Channel,
    packets::{
        BuddyAddPacket, BuddyRemovePacket, BuddyStatusPacket, IncomingPacket, MsgPrivatePacket,
        OutgoingPacket, PacketType, ReceivedPacket,
    },
    AOSocket, Result,
};
use tokio::{
    net::TcpListener,
    spawn,
    sync::{mpsc::unbounded_channel, Notify},
    time::Instant,
};
use unicycle::FuturesUnordered;
use worker::WorkerHandle;

use std::{convert::TryFrom, process::exit, sync::Arc};

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

    loop {
        let spam_bot_support = config.spam_bot_support;
        let send_tells_over_main = config.send_tells_over_main;
        let relay_by_id = config.relay_by_id;
        let account_num = config.accounts.len();

        let logged_in = Arc::new(Notify::new());
        let logged_in_waiter = logged_in.clone();
        let logged_in_setter = logged_in.clone();

        let (worker_sender, mut worker_receiver) = unbounded_channel();

        // List of workers
        let mut workers: Vec<worker::WorkerHandle> = Vec::with_capacity(account_num);

        // Create all workers
        for (idx, acc) in config.accounts.iter().enumerate() {
            info!("Spawning worker for {}", acc.character);

            let worker = WorkerHandle::new(idx + 1, config.clone(), worker_sender.clone()).await;
            workers.push(worker);
        }

        let tcp_server = TcpListener::bind(format!("0.0.0.0:{}", config.port_number)).await?;
        info!("Listening on port {}", config.port_number);
        info!("Waiting for client to connect...");
        let (client, addr) = tcp_server.accept().await?;
        info!("Client connected from {}", addr);

        let mut tasks = Vec::with_capacity(4);

        // Create a socket from the client
        let mut sock = AOSocket::from_stream(client);
        let sock_to_workers = sock.get_sender();
        let sock_sender = sock.get_sender();
        let mut real_sock = AOSocket::connect(config.server_address.clone()).await?;
        let real_sock_sender = real_sock.get_sender();

        // Forward stuff from the workers to the main
        tasks.push(spawn(async move {
            logged_in_waiter.notified().await;
            while let Some(msg) = worker_receiver.recv().await {
                let _ = sock_to_workers.send(msg);
            }
        }));

        let main_buddies = Arc::new(DashMap::new());
        let main_buddies_clone = main_buddies.clone();
        let main_pending_buddies = Arc::new(DashMap::new());
        let main_pending_buddies_clone = main_pending_buddies.clone();
        let main_pending_buddies_clone_2 = main_pending_buddies.clone();

        tasks.push(spawn(worker::remove_pending_buddies(main_pending_buddies)));

        // Forward all incoming packets to the client
        tasks.push(spawn(async move {
            while let Ok(packet) = real_sock.read_raw_packet().await {
                debug!("Received {:?} packet for main", packet.0);

                if log_enabled!(Trace) {
                    let loaded = ReceivedPacket::try_from((packet.0, packet.1.as_slice()));
                    if let Ok(pack) = loaded {
                        trace!("Packet body: {:?}", pack);
                    }
                }

                match packet.0 {
                    PacketType::LoginOk => logged_in_setter.notify_waiters(),
                    PacketType::BuddyAdd => {
                        let b = BuddyStatusPacket::load(&packet.1).unwrap();
                        debug!("Buddy {} is online: {}", b.character_id, b.online);
                        main_pending_buddies_clone.remove(&b.character_id);
                        main_buddies_clone.insert(b.character_id, ());
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1).unwrap();
                        debug!("Buddy {} removed", b.character_id);
                        main_buddies_clone.remove(&b.character_id);
                    }
                    _ => {}
                }

                let _ = sock_sender.send(packet);
            }
        }));

        // Loop over incoming packets and depending on the type, round robin them
        // If not, we just send them over the normal FC connection
        tasks.push(spawn(async move {
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

                if log_enabled!(Trace) {
                    let loaded = ReceivedPacket::try_from((packet.0, packet.1.as_slice()));
                    if let Ok(pack) = loaded {
                        trace!("Packet body: {:?}", pack);
                    }
                }

                match packet.0 {
                    PacketType::BuddyAdd => {
                        // Add the buddy on the slave with least buddies
                        let mut least_buddies = 0;
                        let mut buddy_count =
                            main_buddies.len() + main_pending_buddies_clone_2.len();

                        for (id, worker) in workers.clone().iter().enumerate() {
                            let worker_buddy_count = worker.get_total_buddies().await;
                            if worker_buddy_count < buddy_count {
                                least_buddies = id + 1;
                                buddy_count = worker_buddy_count;
                            }
                        }

                        if least_buddies == 0 {
                            debug!("Adding buddy on main ({} current buddies)", buddy_count);
                            if let Ok(pack) = BuddyAddPacket::load(&packet.1) {
                                main_pending_buddies_clone_2
                                    .insert(pack.character_id, Instant::now());
                            };
                            let _ = real_sock_sender.send(packet);
                        } else {
                            debug!(
                                "Adding buddy on worker #{} ({} current buddies)",
                                least_buddies, buddy_count
                            );
                            workers[least_buddies - 1].send_packet(packet).await;
                        }
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1).unwrap();

                        if main_buddies.get(&b.character_id).is_some() {
                            debug!("Removing buddy {} on main", b.character_id);
                            let _ = real_sock_sender.send(packet.clone());
                        }
                        // Remove the buddy on the slaves that have it on the buddy list
                        for (id, worker) in workers.iter().enumerate() {
                            if worker.has_buddy(b.character_id).await {
                                debug!("Removing buddy {} on worker #{}", b.character_id, id + 1);
                                worker.send_packet(packet.clone()).await;
                            }
                        }
                    }
                    PacketType::MsgPrivate => {
                        let mut m = MsgPrivatePacket::load(&packet.1).unwrap();
                        if m.message.send_tag.starts_with("spam") {
                            let split_parts: Vec<&str> =
                                m.message.send_tag.splitn(2, "spam-").collect();

                            // If a worker ID is provided via spam-N, use that one next
                            if split_parts.len() == 2 {
                                let num: usize = split_parts[1].parse().unwrap_or(current_buddy);
                                if num <= account_num {
                                    current_buddy = num;
                                }
                            } else if relay_by_id {
                                // If we are using modulo strategy, calculate it
                                if let Channel::Tell(id) = m.message.channel {
                                    if send_tells_over_main {
                                        current_buddy = (id as usize) % (account_num + 1);
                                    } else {
                                        current_buddy = (id as usize) % account_num + 1;
                                    }
                                }
                            }

                            m.message.send_tag = String::from("\u{0}");
                            let serialized = m.serialize();

                            if spam_bot_support && current_buddy != 0 {
                                workers[current_buddy].send_packet(serialized).await;
                            } else {
                                let _ = real_sock_sender.send(serialized);
                            }
                            if current_buddy == account_num {
                                current_buddy = start_at;
                            } else {
                                current_buddy += 1;
                            }
                        } else {
                            let _ = real_sock_sender.send(packet);
                        }
                    }
                    _ => {
                        let _ = real_sock_sender.send(packet);
                    }
                }
            }
        }));

        let num_tasks = tasks.len();
        let mut task_collection = tasks.into_iter().collect::<FuturesUnordered<_>>();

        let _ = task_collection.next().await;

        for i in 0..num_tasks {
            if let Some(fut) = task_collection.get_mut(i) {
                fut.abort();
            }
        }

        warn!("Lost a connection (probably from client), restarting...");
    }
}
