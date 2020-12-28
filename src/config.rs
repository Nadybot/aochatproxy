use crate::communication::SendMode;

use dotenv::dotenv;
use serde::Deserialize;
use serde_json::from_str;

use std::{
    env::{args, set_var, var},
    fmt::{Display, Formatter, Result as FmtResult},
    fs::read_to_string,
    path::Path,
};

#[derive(Clone, Deserialize, Debug)]
pub struct AccountData {
    pub username: String,
    pub password: String,
    pub character: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct Config {
    #[serde(default = "default_log")]
    pub rust_log: Option<String>,
    #[serde(default = "default_port")]
    pub port_number: u32,
    pub accounts: Vec<AccountData>,
    #[serde(default = "default_server_address")]
    pub server_address: String,
    #[serde(default = "default_spam")]
    pub spam_bot_support: bool,
    #[serde(default = "default_main")]
    pub send_tells_over_main: bool,
    #[serde(default = "default_relay")]
    pub relay_worker_tells: bool,
    #[serde(default = "default_mode")]
    pub default_mode: SendMode,
}

// Serde needs functions for defaults
fn default_log() -> Option<String> {
    Some(String::from("info"))
}
fn default_port() -> u32 {
    9993
}
fn default_server_address() -> String {
    String::from("chat.d1.funcom.com:7105")
}
fn default_spam() -> bool {
    false
}
fn default_main() -> bool {
    false
}
fn default_relay() -> bool {
    false
}
fn default_mode() -> SendMode {
    SendMode::RoundRobin
}

pub enum ConfigError {
    NotNumber(String),
    NotBoolean(String),
    InvalidConfig(String),
    NotFound(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::NotNumber(s) => {
                write!(f, "{} is not a valid number", s)
            }
            Self::NotBoolean(b) => {
                write!(f, "{} must be true or false", b)
            }
            Self::InvalidConfig(s) => {
                write!(f, "{}", s)
            }
            Self::NotFound(s) => {
                write!(f, "file {} not found or access denied", s)
            }
        }
    }
}

pub fn load_from_env() -> Result<Config, ConfigError> {
    dotenv().ok();
    let mut next_number = 1;
    let mut account_data: Vec<AccountData> = Vec::new();

    loop {
        let mut username = var(format!("WORKER{}_USERNAME", next_number));
        let mut password = var(format!("WORKER{}_PASSWORD", next_number));
        let character = var(format!("WORKER{}_CHARACTERNAME", next_number));

        if character.is_err() {
            break;
        }
        if username.is_err() {
            if account_data.is_empty() {
                break;
            }
            username = Ok(account_data.last().unwrap().username.clone());
        }
        if password.is_err() {
            if account_data.is_empty() {
                break;
            }
            password = Ok(account_data.last().unwrap().password.clone())
        }
        let account = AccountData {
            username: username.unwrap(),
            password: password.unwrap(),
            character: character.unwrap(),
        };
        account_data.push(account);
        next_number += 1;
    }

    let server_address =
        var("SERVER_ADDRESS").unwrap_or_else(|_| String::from("chat.d1.funcom.com:7105"));
    let port_number: u32 = var("PROXY_PORT_NUMBER")
        .unwrap_or_else(|_| String::from("9993"))
        .parse()
        .map_err(|_| ConfigError::NotNumber(String::from("PROXY_PORT_NUMBER")))?;
    let spam_bot_support: bool = var("SPAM_BOT_SUPPORT")
        .unwrap_or_else(|_| String::from("false"))
        .parse()
        .map_err(|_| ConfigError::NotBoolean(String::from("SPAM_BOT_SUPPORT")))?;
    let send_tells_over_main: bool = var("SEND_TELLS_OVER_MAIN")
        .unwrap_or_else(|_| {
            if account_data.is_empty() {
                String::from("true")
            } else {
                String::from("false")
            }
        })
        .parse()
        .map_err(|_| ConfigError::NotBoolean(String::from("SEND_TELLS_OVER_MAIN")))?;
    let relay_worker_tells: bool = var("RELAY_WORKER_TELLS")
        .unwrap_or_else(|_| String::from("false"))
        .parse()
        .map_err(|_| ConfigError::NotBoolean(String::from("RELAY_WORKER_TELLS")))?;

    let default_mode = match var("DEFAULT_MODE") {
        Ok(v) => match v.as_str() {
            "round-robin" => SendMode::RoundRobin,
            "by-charid" => SendMode::ByCharId,
            _ => Err(ConfigError::InvalidConfig(String::from(
                "DEFAULT_MODE must be round-robin or by-charid",
            )))?,
        },
        Err(_) => {
            let relay_by_id: bool = var("RELAY_BY_ID")
                .unwrap_or_else(|_| String::from("false"))
                .parse()
                .map_err(|_| ConfigError::NotBoolean(String::from("RELAY_BY_ID")))?;
            if relay_by_id {
                SendMode::ByCharId
            } else {
                SendMode::RoundRobin
            }
        }
    };

    // We cannot send tells in this case
    if spam_bot_support & (!send_tells_over_main && account_data.is_empty()) {
        return Err(ConfigError::InvalidConfig(String::from(
            "When SPAM_BOT_SUPPORT is true and SEND_TELLS_OVER_MAIN is disabled, at least one worker needs to be configured",
        )));
    }

    Ok(Config {
        rust_log: None,
        port_number,
        accounts: account_data,
        server_address,
        spam_bot_support,
        send_tells_over_main,
        relay_worker_tells,
        default_mode,
    })
}

pub fn load_from_file(path: String) -> Result<Config, ConfigError> {
    let content = read_to_string(&path).map_err(|_| ConfigError::NotFound(path))?;
    let config: Config =
        from_str(&content).map_err(|e| ConfigError::InvalidConfig(e.to_string()))?;
    if let Some(level) = &config.rust_log {
        set_var("RUST_LOG", level);
    }

    match config.default_mode {
        SendMode::ByCharId | SendMode::RoundRobin => {}
        _ => Err(ConfigError::InvalidConfig(String::from(
            "default_mode must be round-robin or by-charid",
        )))?,
    };

    // We cannot send tells in this case
    if config.spam_bot_support & (!config.send_tells_over_main && config.accounts.is_empty()) {
        return Err(ConfigError::InvalidConfig(String::from(
            "When spam_bot_support is true and send_tells_over_main is disabled, at least one worker needs to be configured",
        )));
    }

    Ok(config)
}

pub fn try_load() -> Result<Config, ConfigError> {
    let file = args().nth(1).unwrap_or_else(|| String::from("config.json"));
    if Path::new(&file).exists() {
        return load_from_file(file);
    }
    load_from_env()
}
