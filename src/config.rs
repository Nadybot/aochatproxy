use crate::communication::SendMode;

use deser_hjson::from_str;
use log::warn;
use serde::Deserialize;
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
    pub rust_log: String,
    #[serde(default = "default_port")]
    pub port_number: u32,
    #[serde(default)]
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
fn default_log() -> String {
    String::from("info")
}
fn default_port() -> u32 {
    9993
}
fn default_server_address() -> String {
    String::from("chat.d1.funcom.com:7105")
}
fn default_spam() -> bool {
    true
}
fn default_main() -> bool {
    true
}
fn default_relay() -> bool {
    false
}
fn default_mode() -> SendMode {
    SendMode::RoundRobin
}

pub enum ConfigError {
    InvalidConfig(String),
    NotFound(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::InvalidConfig(s) => f.write_str(s),
            Self::NotFound(s) => f.write_fmt(format_args!("file {} not found or access denied", s)),
        }
    }
}

impl Config {
    pub fn mash_with_env(&mut self) -> Result<u8, ConfigError> {
        let mut changed = 0;
        let mut next_number = 1;

        loop {
            let mut username = var(format!("WORKER{}_USERNAME", next_number));
            let mut password = var(format!("WORKER{}_PASSWORD", next_number));
            let character = var(format!("WORKER{}_CHARACTERNAME", next_number));

            if character.is_err() {
                break;
            }
            if username.is_err() {
                if self.accounts.is_empty() {
                    break;
                }
                username = Ok(self.accounts.last().unwrap().username.clone());
            }
            if password.is_err() {
                if self.accounts.is_empty() {
                    break;
                }
                password = Ok(self.accounts.last().unwrap().password.clone())
            }
            let account = AccountData {
                username: username.unwrap(),
                password: password.unwrap(),
                character: character.unwrap(),
            };
            self.accounts.push(account);
            next_number += 1;
            changed += 1;
        }

        if let Ok(addr) = var("SERVER_ADDRESS") {
            self.server_address = addr;
            changed += 1;
        }
        if let Ok(port) = var("PROXY_PORT_NUMBER") {
            if let Ok(p) = port.parse() {
                self.port_number = p;
                changed += 1;
            }
        }
        if let Ok(spam_bot_support) = var("SPAM_BOT_SUPPORT") {
            if let Ok(s) = spam_bot_support.parse() {
                self.spam_bot_support = s;
                changed += 1;
            }
        }
        if let Ok(send_tells_over_main) = var("SEND_TELLS_OVER_MAIN") {
            if let Ok(s) = send_tells_over_main.parse() {
                self.send_tells_over_main = s;
                changed += 1;
            }
        }
        if let Ok(relay_worker_tells) = var("RELAY_WORKER_TELLS") {
            if let Ok(r) = relay_worker_tells.parse() {
                self.relay_worker_tells = r;
                changed += 1;
            }
        }
        if let Ok(default_mode) = var("DEFAULT_MODE") {
            changed += 1;
            match default_mode.as_str() {
                "round-robin" => self.default_mode = SendMode::RoundRobin,
                "by-charid" => self.default_mode = SendMode::ByCharId,
                _ => {}
            }
        }
        if let Ok(level) = var("RUST_LOG") {
            self.rust_log = level;
            changed += 1;
        }

        Ok(changed)
    }

    fn validate_self(&self) -> Result<(), ConfigError> {
        set_var("RUST_LOG", &self.rust_log);
        env_logger::builder().format_timestamp_millis().init();

        if self.spam_bot_support && (!self.send_tells_over_main && self.accounts.is_empty()) {
            return Err(ConfigError::InvalidConfig(String::from(
                "When SPAM_BOT_SUPPORT is true and SEND_TELLS_OVER_MAIN is disabled, at least one worker needs to be configured",
            )));
        }
        Ok(())
    }
}

pub fn load_from_file(path: String) -> Result<Config, ConfigError> {
    let content = read_to_string(&path).map_err(|_| ConfigError::NotFound(path))?;
    let config: Config =
        from_str(&content).map_err(|e| ConfigError::InvalidConfig(e.to_string()))?;
    set_var("RUST_LOG", &config.rust_log);

    match config.default_mode {
        SendMode::ByCharId | SendMode::RoundRobin => {}
        _ => {
            return Err(ConfigError::InvalidConfig(String::from(
                "default_mode must be round-robin or by-charid",
            )))
        }
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
    let mut used_config = false;
    let mut conf: Config = {
        if Path::new(&file).exists() {
            let conf = load_from_file(file)?;
            used_config = true;
            conf
        } else {
            from_str("{}").unwrap()
        }
    };
    let changed_from_env = conf.mash_with_env()?;

    conf.validate_self()?;

    if !used_config && changed_from_env == 0 {
        warn!("config.json not found, proceeding with defaults");
    }

    Ok(conf)
}
