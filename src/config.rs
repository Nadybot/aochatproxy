use crate::communication::SendMode;

use log::warn;
use nanoserde::DeJson;
use std::{
    env::{args, set_var, var},
    fmt::{Display, Formatter, Result as FmtResult},
    fs::read_to_string,
    path::Path,
};

#[derive(Clone, Debug, DeJson)]
pub struct AccountData {
    pub username: String,
    pub password: String,
    pub character: String,
}

#[derive(Clone, DeJson, Debug)]
pub struct Config {
    #[nserde(default = "info")]
    pub rust_log: String,
    #[nserde(default = 9993)]
    pub port_number: u32,
    #[nserde(default)]
    pub accounts: Vec<AccountData>,
    #[nserde(default = "chat.d1.funcom.com:7105")]
    pub server_address: String,
    #[nserde(default = "true")]
    pub spam_bot_support: bool,
    #[nserde(default = "true")]
    pub send_tells_over_main: bool,
    #[nserde(default = "false")]
    pub relay_worker_tells: bool,
    #[nserde(default)]
    pub default_mode: SendMode,
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
        set_var("RUST_LOG", &*self.rust_log);
        env_logger::builder().format_timestamp_millis().init();

        match self.default_mode {
            SendMode::ByCharId | SendMode::RoundRobin => {}
            _ => {
                return Err(ConfigError::InvalidConfig(String::from(
                    "default_mode must be round-robin or by-charid",
                )))
            }
        };

        if self.spam_bot_support && (!self.send_tells_over_main && self.accounts.is_empty()) {
            return Err(ConfigError::InvalidConfig(String::from(
                "When SPAM_BOT_SUPPORT is true and SEND_TELLS_OVER_MAIN is disabled, at least one worker needs to be configured",
            )));
        }
        Ok(())
    }
}

pub fn load_from_file(path: &str) -> Result<Config, ConfigError> {
    let config: Config;
    let content = read_to_string(path).map_err(|_| ConfigError::NotFound(path.to_string()))?;
    config = DeJson::deserialize_json(&content)
        .map_err(|e| ConfigError::InvalidConfig(e.to_string()))?;
    Ok(config)
}

pub fn try_load() -> Result<Config, ConfigError> {
    let file = args().nth(1).unwrap_or_else(|| String::from("config.json"));
    let mut used_config = false;
    let mut conf: Config = {
        if Path::new(&file).exists() {
            let conf = load_from_file(&file)?;
            used_config = true;
            conf
        } else {
            DeJson::deserialize_json("{}").unwrap()
        }
    };
    let changed_from_env = conf.mash_with_env()?;

    conf.validate_self()?;

    if !used_config && changed_from_env == 0 {
        warn!("{} not found, proceeding with defaults", file);
    }

    Ok(conf)
}
