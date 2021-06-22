use nanoserde::{DeJson, DeJsonErr, DeJsonState, SerJson, SerJsonState};
use std::str::Chars;

#[derive(Debug)]
pub enum Command {
    Capabilities,
    Ping,
}

impl DeJson for Command {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        let s = String::de_json(state, input)?;
        match s.as_ref() {
            "capabilities" => Ok(Self::Capabilities),
            "ping" => Ok(Self::Ping),
            _ => Err(DeJsonErr {
                msg: "Invalid Command value".to_string(),
                line: state.line,
                col: state.col,
            }),
        }
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum SendMode {
    RoundRobin,
    ByCharId,
    ByMsgId,
    ByWorker,
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

impl DeJson for SendMode {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        let s = String::de_json(state, input)?;
        match s.as_ref() {
            "round-robin" => Ok(Self::RoundRobin),
            "by-charid" => Ok(Self::ByCharId),
            "by-msgid" => Ok(Self::ByMsgId),
            "by-worker" => Ok(Self::ByWorker),
            "proxy-default" => Ok(Self::Default),
            _ => Err(DeJsonErr {
                msg: "Invalid SendMode value".to_string(),
                line: state.line,
                col: state.col,
            }),
        }
    }
}

impl SerJson for SendMode {
    fn ser_json(&self, d: usize, state: &mut SerJsonState) {
        let val = self.as_str().to_string();
        val.ser_json(d, state);
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
