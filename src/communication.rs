use nanoserde::{DeJson, SerJson};

#[derive(DeJson, Debug)]
pub enum Command {
    #[nserde(rename = "capabilities")]
    Capabilities,
    #[nserde(rename = "ping")]
    Ping,
}

#[derive(DeJson, SerJson, Debug, PartialEq, Copy, Clone)]
pub enum SendMode {
    #[nserde(rename = "round-robin")]
    RoundRobin,
    #[nserde(rename = "by-charid")]
    ByCharId,
    #[nserde(rename = "by-msgid")]
    ByMsgId,
    #[nserde(rename = "by-worker")]
    ByWorker,
    #[nserde(rename = "proxy-default")]
    Default,
}

impl Default for SendMode {
    fn default() -> Self {
        Self::RoundRobin
    }
}

impl SendMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::ByCharId => "by-charid",
            Self::ByMsgId => "by-msgid",
            Self::ByWorker => "by-worker",
            Self::Default => "proxy-default",
        }
    }
}

#[derive(Debug, DeJson)]
pub struct CommandPayload {
    pub cmd: Command,
    pub worker: Option<usize>,
    pub payload: Option<String>,
}

#[derive(Debug, DeJson)]
pub struct SendMessagePayload {
    pub mode: SendMode,
    pub msgid: Option<usize>,
    pub worker: Option<usize>,
}

#[derive(Debug, DeJson)]
pub struct BuddyAddPayload {
    pub worker: usize,
}
