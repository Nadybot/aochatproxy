use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub enum Command {
    #[serde(rename = "capabilities")]
    Capabilities,
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Copy, Clone)]
pub enum SendMode {
    #[serde(rename = "round-robin")]
    RoundRobin,
    #[serde(rename = "by-charid")]
    ByCharId,
    #[serde(rename = "by-msgid")]
    ByMsgId,
    #[serde(rename = "by-worker")]
    ByWorker,
    #[serde(rename = "proxy-default")]
    Default,
}

#[derive(Deserialize, Debug)]
pub struct CommandPayload {
    pub cmd: Command,
    pub worker: Option<usize>,
    pub payload: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct SendMessagePayload {
    pub mode: SendMode,
    pub msgid: Option<usize>,
    pub worker: Option<usize>,
}

#[derive(Deserialize, Debug)]
pub struct BuddyAddPayload {
    pub worker: usize,
}
