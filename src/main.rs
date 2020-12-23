use dashmap::DashMap;
use dotenv::dotenv;
use futures_util::stream::{FuturesUnordered, StreamExt};
use log::{debug, error, info, log_enabled, trace, warn, Level::Trace};
use nadylib::{
    models::Channel,
    packets::{
        BuddyRemovePacket, BuddyStatusPacket, IncomingPacket, MsgPrivatePacket, OutgoingPacket,
        PacketType, ReceivedPacket,
    },
    AOSocket, Result,
};
use tokio::{
    net::TcpListener,
    spawn,
    sync::{mpsc::unbounded_channel, Notify},
    time::{sleep, Duration, Instant},
};

use std::{
    collections::{HashMap, VecDeque},
    convert::TryFrom,
    process::exit,
    sync::Arc,
};

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
        let tcp_server = TcpListener::bind(format!("0.0.0.0:{}", config.port_number)).await?;

        info!("Listening on port {}", config.port_number);
        info!("Waiting for client to connect...");
        let (client, addr) = tcp_server.accept().await?;
        info!("Client connected from {}", addr);

        // Create the main connection to the chat servers
        let mut sock = AOSocket::from_stream(client);
        let mut real_sock = AOSocket::connect(config.server_address.clone()).await?;

        // List of all buddies
        let buddies: Arc<DashMap<usize, DashMap<u32, ()>>> =
            Arc::new(DashMap::with_capacity(account_num + 1));
        // The main bot buddies
        buddies.insert(0, DashMap::new());
        let task1_buddies = buddies.clone();
        let task2_buddies = buddies.clone();
        // Pending buddies per account
        let pending_buddies: Arc<DashMap<usize, VecDeque<Instant>>> =
            Arc::new(DashMap::with_capacity(account_num + 1));
        let pending_buddies_clone = pending_buddies.clone();
        let pending_buddies_clone_2 = pending_buddies.clone();
        // List of communication channels to the workers
        let mut senders = HashMap::with_capacity(account_num + 1);
        let mut receivers = HashMap::with_capacity(account_num);
        senders.insert(0, real_sock.get_sender());
        pending_buddies.insert(0, VecDeque::new());
        for i in 0..account_num {
            let (s, r) = unbounded_channel();
            senders.insert(i + 1, s);
            pending_buddies.insert(i + 1, VecDeque::new());
            receivers.insert(i, r);
        }

        let sock_sender = sock.get_sender();
        let duplicate_sock_sender = sock_sender.clone();
        let real_sock_sender = real_sock.get_sender();

        let logged_in = Arc::new(Notify::new());
        let logged_in_setter = logged_in.clone();

        let mut tasks = FuturesUnordered::new();

        // Remove pending buddy adds after 10s
        tasks.push(spawn(async move {
            let dur = Duration::from_secs(10);
            loop {
                sleep(dur).await;
                let now = Instant::now();
                for i in 0..account_num + 1 {
                    pending_buddies_clone.update_get(&i, |_, v| {
                        v.into_iter().filter(|i| **i + dur > now).cloned().collect()
                    });
                }
            }
        }));

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
                    PacketType::LoginOk => logged_in_setter.notify_one(),
                    PacketType::BuddyAdd => {
                        let b = BuddyStatusPacket::load(&packet.1).unwrap();
                        debug!("Buddy {} is online: {}", b.character_id, b.online);
                        task1_buddies.get(&0).unwrap().insert(b.character_id, ());
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1).unwrap();
                        debug!("Buddy {} removed", b.character_id);
                        task1_buddies.get(&0).unwrap().remove(&b.character_id);
                    }
                    _ => {}
                }

                let _ = sock_sender.send(packet);
            }

            Ok(())
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
                        let mut buddy_count = task2_buddies.get(&0).unwrap().value().len()
                            + pending_buddies_clone_2.get(&0).unwrap().value().len();

                        for key in 1..task2_buddies.len() {
                            let elem = task2_buddies.get(&key).unwrap();
                            let val = elem.value().len()
                                + pending_buddies_clone_2.get(&key).unwrap().value().len();
                            if val < buddy_count {
                                buddy_count = val;
                                least_buddies = *elem.key();
                            }
                        }

                        pending_buddies_clone_2.update(&least_buddies, |_, v| {
                            let mut w = v.clone();
                            w.push_back(Instant::now());
                            w
                        });
                        if least_buddies == 0 {
                            debug!("Adding buddy on main ({} current buddies)", buddy_count);
                        } else {
                            debug!(
                                "Adding buddy on worker #{} ({} current buddies)",
                                least_buddies, buddy_count
                            );
                        }
                        let _ = senders.get(&least_buddies).unwrap().send(packet);
                    }
                    PacketType::BuddyRemove => {
                        let b = BuddyRemovePacket::load(&packet.1).unwrap();
                        // Remove the buddy on the slaves that have it on the buddy list
                        for elem in task2_buddies.iter() {
                            if elem.value().get(&b.character_id).is_some() {
                                let worker_id = elem.key();
                                if worker_id == &0 {
                                    debug!("Removing buddy {} on main", b.character_id);
                                } else {
                                    debug!(
                                        "Removing buddy {} on worker #{}",
                                        b.character_id,
                                        worker_id + 1
                                    );
                                }
                                let _ = senders.get(worker_id).unwrap().send(packet.clone());
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
                                let _ = senders.get(&current_buddy).unwrap().send(serialized);
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

            Ok(())
        }));

        // Wait until logged in
        logged_in.notified().await;

        // Create all slaves
        for (idx, acc) in config.accounts.iter().enumerate() {
            info!("Spawning worker for {}", acc.character);
            tasks.push(spawn(worker::worker_main(
                idx + 1,
                config.clone(),
                acc.clone(),
                duplicate_sock_sender.clone(),
                buddies.clone(),
                pending_buddies.clone(),
                receivers.remove(&idx).unwrap(),
            )));
        }

        let _ = tasks.next().await;

        warn!("Lost a connection (probably from client), restarting...");

        for task in tasks.iter() {
            task.abort();
        }
    }
}
