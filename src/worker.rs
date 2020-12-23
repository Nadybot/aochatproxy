use crate::config::{AccountData, Config};

use dashmap::DashMap;
use log::{debug, error, info, log_enabled, trace, Level::Trace};
use nadylib::{
    packets::{LoginSelectPacket, OutgoingPacket, PacketType, SerializedPacket},
    AOSocket, ReceivedPacket, Result,
};
use tokio::{
    spawn,
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        Notify,
    },
    time::Instant,
};

use std::{collections::VecDeque, convert::TryFrom, sync::Arc};

async fn worker_reader(
    id: usize,
    mut packet_reader: UnboundedReceiver<SerializedPacket>,
    socket_sender: UnboundedSender<SerializedPacket>,
    notify_wait: Arc<Notify>,
) {
    notify_wait.notified().await;

    while let Some(packet) = packet_reader.recv().await {
        debug!("Sending {:?} packet from worker #{}", packet.0, id);

        if log_enabled!(Trace) {
            let loaded = ReceivedPacket::try_from((packet.0, packet.1.as_slice()));
            if let Ok(pack) = loaded {
                trace!("Packet body: {:?}", pack);
            }
        }
        let _ = socket_sender.send(packet);
    }
}

// The main helper bot task
pub async fn worker_main(
    id: usize,
    config: Config,
    account: AccountData,
    sender: UnboundedSender<SerializedPacket>,
    buddies: Arc<DashMap<usize, DashMap<u32, ()>>>,
    pending_buddies: Arc<DashMap<usize, VecDeque<Instant>>>,
    packet_reader: UnboundedReceiver<SerializedPacket>,
) -> Result<()> {
    let mut socket = AOSocket::connect(config.server_address).await?;
    let socket_sender = socket.get_sender();
    buddies.insert(id, DashMap::new());
    let notify = Arc::new(Notify::new());
    let notify2 = notify.clone();
    spawn(worker_reader(id, packet_reader, socket_sender, notify2));

    while let Ok((packet_type, body)) = socket.read_raw_packet().await {
        // Read a packet and handle it if interested
        debug!("Received {:?} packet for worker #{}", packet_type, id);

        if log_enabled!(Trace) {
            let loaded = ReceivedPacket::try_from((packet_type, body.as_slice()));
            if let Ok(pack) = loaded {
                trace!("Packet body: {:?}", pack);
            }
        }

        match packet_type {
            PacketType::LoginSeed
            | PacketType::LoginCharlist
            | PacketType::LoginOk
            | PacketType::LoginError
            | PacketType::BuddyAdd
            | PacketType::BuddyRemove
            | PacketType::ClientName
            | PacketType::MsgPrivate => {
                let packet = ReceivedPacket::try_from((packet_type, body.as_slice()))?;

                match packet {
                    ReceivedPacket::LoginSeed(l) => {
                        socket.login(&account.username, &account.password, &l.login_seed)?;
                    }
                    ReceivedPacket::LoginCharlist(c) => {
                        if let Some(character) =
                            c.characters.iter().find(|i| i.name == account.character)
                        {
                            let pack = LoginSelectPacket {
                                character_id: character.id,
                            };
                            socket.send(pack)?;
                        } else {
                            error!(
                                "Character {} is not on account {}",
                                account.character, account.username
                            );
                            break;
                        }
                    }
                    ReceivedPacket::LoginOk => {
                        info!("{} logged in", account.character);
                        debug!("Sending LoginOk packet from worker #{} to main", id);
                        notify.notify_one();
                        sender.send((packet_type, body))?;
                    }
                    ReceivedPacket::LoginError(e) => {
                        error!("{} failed to log in: {}", account.character, e.message);
                        break;
                    }
                    ReceivedPacket::ClientName(_) => {
                        debug!("Sending ClientName packet from worker #{} to main", id);
                        sender.send((packet_type, body))?;
                    }
                    ReceivedPacket::BuddyStatus(b) => {
                        debug!(
                            "Worker #{}: Buddy {} is online: {}",
                            id, b.character_id, b.online
                        );
                        debug!("Sending BuddyAdd packet from worker #{} to main", id);
                        buddies.get(&id).unwrap().insert(b.character_id, ());
                        pending_buddies.update_get(&id, |_, v| {
                            let mut w = v.clone();
                            w.pop_front();
                            w
                        });
                        sender.send((packet_type, body))?;
                    }
                    ReceivedPacket::BuddyRemove(b) => {
                        debug!("Worker #{}: Buddy {} removed", id, b.character_id);
                        debug!("Sending BuddyRemove packet from worker #{} to main", id);
                        buddies.get(&id).unwrap().remove(&b.character_id);
                        sender.send((packet_type, body))?;
                    }
                    ReceivedPacket::MsgPrivate(mut m) => {
                        if config.relay_slave_tells {
                            debug!("Relaying tell message from worker #{} to main", id);
                            m.message.send_tag =
                                format!("{{\"id\": {}, \"name\": {:?}}}", id, account.character);
                            sender.send(m.serialize())?;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    Ok(())
}
