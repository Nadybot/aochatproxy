use crate::config::Config;

use dashmap::{DashMap, DashSet};
use log::{debug, error, info, trace};
use nadylib::{
    client_socket::SocketSendHandle,
    packets::{
        BuddyAddPacket, BuddyRemovePacket, BuddyStatusPacket, IncomingPacket, LoginCharlistPacket,
        LoginSeedPacket, LoginSelectPacket, MsgPrivatePacket, OutgoingPacket, PacketType,
        PingPacket, SerializedPacket,
    },
    AOSocket, Result, SocketConfig,
};
use tokio::{
    spawn,
    sync::{mpsc, oneshot, Notify},
    task::JoinHandle,
};

use std::{fmt, sync::Arc};

// An actor-like struct
struct Worker {
    receiver: mpsc::Receiver<WorkerMessage>,
    buddies: Arc<DashSet<u32>>,
    pending_buddies: Arc<DashMap<u32, u32>>,
    packet_sender: SocketSendHandle,
    counter: u32,
}

enum WorkerMessage {
    GetTotalBuddies {
        respond_to: oneshot::Sender<usize>,
    },
    SendPacket {
        packet: SerializedPacket,
    },
    HasBuddy {
        id: u32,
        respond_to: oneshot::Sender<bool>,
    },
}

impl Worker {
    async fn new(
        id: usize,
        config: Config,
        receiver: mpsc::Receiver<WorkerMessage>,
        packet_sender: SocketSendHandle,
        logged_in: Arc<Notify>,
        worker_ids: Arc<DashSet<u32>>,
    ) -> (Self, JoinHandle<Result<()>>) {
        let conf = SocketConfig::default().keepalive(id != 0);

        let socket = AOSocket::connect(&*config.server_address.clone(), conf)
            .await
            .unwrap();

        let sender = socket.get_sender();

        let buddies = Arc::new(DashSet::new());
        let pending_buddies = Arc::new(DashMap::new());

        let task = if id == 0 {
            spawn(main_receive_loop(
                socket,
                logged_in,
                packet_sender.clone(),
                buddies.clone(),
                pending_buddies.clone(),
            ))
        } else {
            spawn(worker_receive_loop(
                id,
                config,
                socket,
                packet_sender.clone(),
                buddies.clone(),
                pending_buddies.clone(),
                worker_ids.clone(),
            ))
        };

        (
            Worker {
                receiver,
                buddies,
                packet_sender: sender,
                pending_buddies,
                counter: 0,
            },
            task,
        )
    }

    async fn handle_message(&mut self, msg: WorkerMessage) {
        match msg {
            WorkerMessage::GetTotalBuddies { respond_to } => {
                trace!("Pending buddies are {}", self.pending_buddies.len());
                let count = self.buddies.len() + self.pending_buddies.len();
                let _ = respond_to.send(count);
            }
            WorkerMessage::SendPacket { mut packet } => {
                if let PacketType::BuddyAdd = packet.0 {
                    if let Ok(mut pack) = BuddyAddPacket::load(&packet.1) {
                        let current = self.counter;
                        pack.send_tag = current.to_string();
                        packet = pack.serialize();
                        self.counter = self.counter.wrapping_add(1);
                        self.pending_buddies.insert(pack.character_id, current);
                    }
                }

                let _res = self.packet_sender.send_raw(packet.0, packet.1).await;
            }
            WorkerMessage::HasBuddy { id, respond_to } => {
                let has = self.buddies.contains(&id) || self.pending_buddies.contains_key(&id);
                let _ = respond_to.send(has);
            }
        }
    }
}

async fn main_receive_loop(
    mut socket: AOSocket,
    logged_in: Arc<Notify>,
    packet_sender: SocketSendHandle,
    buddies: Arc<DashSet<u32>>,
    pending_buddies: Arc<DashMap<u32, u32>>,
) -> Result<()> {
    loop {
        let (packet_type, body) = socket.read_raw_packet().await?;
        debug!("Received {:?} packet for main", packet_type);
        trace!("Packet body: {:?}", body);

        match packet_type {
            PacketType::LoginOk => logged_in.notify_waiters(),
            PacketType::BuddyAdd => {
                let b = BuddyStatusPacket::load(&body).unwrap();
                let num = b.send_tag.parse().unwrap_or_default();
                debug!("Main: Buddy {} is online: {}", b.character_id, b.online);
                pending_buddies.remove(&b.character_id);
                pending_buddies.retain(|_, v| *v > num || num - *v > 1_000_000);
                buddies.insert(b.character_id);
            }
            PacketType::BuddyRemove => {
                let b = BuddyRemovePacket::load(&body).unwrap();
                debug!("Main: Buddy {} removed", b.character_id);
                buddies.remove(&b.character_id);
            }
            _ => {}
        }

        let _res = packet_sender.send_raw(packet_type, body).await;
    }
}

async fn worker_receive_loop(
    id: usize,
    config: Config,
    mut socket: AOSocket,
    packet_sender: SocketSendHandle,
    buddies: Arc<DashSet<u32>>,
    pending_buddies: Arc<DashMap<u32, u32>>,
    worker_ids: Arc<DashSet<u32>>,
) -> Result<()> {
    let account = config.accounts[id - 1].clone();
    let identifier = format!(r#"{{"id": {}, "name": {:?}}}"#, id, account.character);

    loop {
        let (packet_type, body) = socket.read_raw_packet().await?;
        debug!("Received {:?} packet for worker #{}", packet_type, id);
        trace!("Packet body: {:?}", body);

        match packet_type {
            PacketType::LoginOk => {
                info!("{} logged in", account.character);
                debug!("Sending LoginOk packet from worker #{} to main", id);
                packet_sender.send_raw(packet_type, body).await?;
            }
            PacketType::LoginError => {
                error!("{} failed to log in", account.character);
                break;
            }
            PacketType::ClientName | PacketType::MsgSystem | PacketType::ClientLookup => {
                debug!(
                    "Sending {:?} packet from worker #{} to main",
                    packet_type, id
                );
                packet_sender.send_raw(packet_type, body).await?;
            }
            PacketType::LoginSeed => {
                let l = LoginSeedPacket::load(&body)?;
                socket
                    .login(&account.username, &account.password, &l.login_seed)
                    .await?;
            }
            PacketType::LoginCharlist => {
                let c = LoginCharlistPacket::load(&body)?;
                if let Some(character) = c.characters.iter().find(|i| i.name == account.character) {
                    let pack = LoginSelectPacket {
                        character_id: character.id,
                    };
                    worker_ids.insert(character.id);
                    socket.send(pack).await?;
                } else {
                    error!(
                        "Character {} is not on account {}",
                        account.character, account.username
                    );
                    break;
                }
            }
            PacketType::BuddyAdd => {
                let mut b = BuddyStatusPacket::load(&body)?;
                debug!(
                    "Worker #{}: Buddy {} is online: {}",
                    id, b.character_id, b.online
                );
                debug!("Sending BuddyAdd packet from worker #{} to main", id);
                let num = b.send_tag.parse().unwrap_or_default();
                b.send_tag = identifier.clone();
                buddies.insert(b.character_id);
                pending_buddies.remove(&b.character_id);
                pending_buddies.retain(|_, v| *v > num || num - *v > 1_000_000);
                packet_sender.send(b).await?;
            }
            PacketType::BuddyRemove => {
                let b = BuddyRemovePacket::load(&body)?;
                debug!("Worker #{}: Buddy {} removed", id, b.character_id);
                debug!("Sending BuddyRemove packet from worker #{} to main", id);
                buddies.remove(&b.character_id);
                packet_sender.send_raw(packet_type, body).await?;
            }
            PacketType::MsgPrivate => {
                if config.relay_worker_tells {
                    let mut m = MsgPrivatePacket::load(&body)?;
                    debug!("Relaying tell message from worker #{} to main", id);
                    m.message.send_tag = identifier.clone();
                    packet_sender.send(m).await?;
                }
            }
            PacketType::Ping => {
                let p = PingPacket::load(&body)?;
                if p.client != "nadylib" {
                    packet_sender.send_raw(packet_type, body).await?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn run_worker(mut worker: Worker) {
    while let Some(msg) = worker.receiver.recv().await {
        worker.handle_message(msg).await;
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Clone)]
pub struct WorkerHandle {
    pub id: usize,
    sender: mpsc::Sender<WorkerMessage>,
}

impl fmt::Display for WorkerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.id == 0 {
            write!(f, "main")
        } else {
            write!(f, "worker #{}", self.id)
        }
    }
}

impl WorkerHandle {
    pub async fn new(
        id: usize,
        config: Config,
        packet_sender: SocketSendHandle,
        logged_in: Arc<Notify>,
        worker_ids: Arc<DashSet<u32>>,
    ) -> (Self, JoinHandle<Result<()>>) {
        let (sender, receiver) = mpsc::channel(1000);
        let (worker, task) =
            Worker::new(id, config, receiver, packet_sender, logged_in, worker_ids).await;
        spawn(run_worker(worker));

        (Self { id, sender }, task)
    }

    pub async fn get_total_buddies(&self) -> usize {
        let (send, recv) = oneshot::channel();
        let msg = WorkerMessage::GetTotalBuddies { respond_to: send };

        let _res = self.sender.send(msg).await;
        recv.await.unwrap()
    }

    pub async fn send_packet(&self, packet: SerializedPacket) {
        let msg = WorkerMessage::SendPacket { packet };
        let _res = self.sender.send(msg).await;
    }

    pub async fn has_buddy(&self, id: u32) -> bool {
        let (send, recv) = oneshot::channel();
        let msg = WorkerMessage::HasBuddy {
            id,
            respond_to: send,
        };
        let _res = self.sender.send(msg).await;
        recv.await.unwrap()
    }
}
