use crate::communication::SendMode;

use log::warn;
use nanoserde::{DeJson, DeJsonErr, DeJsonState};
use std::{
    env::{args, set_var, var},
    fmt::{Display, Formatter, Result as FmtResult},
    fs::read_to_string,
    ops::Deref,
    path::Path,
    str::Chars,
};

#[derive(Clone, Debug, DeJson)]
pub struct AccountData {
    pub username: String,
    pub password: String,
    pub character: String,
}

#[derive(Clone, Debug)]
pub struct RustLog(String);

impl Default for RustLog {
    fn default() -> Self {
        Self(String::from("info"))
    }
}

impl DeJson for RustLog {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        String::de_json(state, input).map(Self)
    }
}

impl Deref for RustLog {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct Port(u32);

impl Default for Port {
    fn default() -> Self {
        Self(9993)
    }
}

impl DeJson for Port {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        u32::de_json(state, input).map(Self)
    }
}

impl Deref for Port {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct ServerAddress(String);

impl Default for ServerAddress {
    fn default() -> Self {
        Self(String::from("chat.d1.funcom.com:7105"))
    }
}

impl DeJson for ServerAddress {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        String::de_json(state, input).map(Self)
    }
}

impl Deref for ServerAddress {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Copy)]
pub struct DefaultTrueBool(bool);

impl Default for DefaultTrueBool {
    fn default() -> Self {
        Self(true)
    }
}

impl DeJson for DefaultTrueBool {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        bool::de_json(state, input).map(Self)
    }
}

impl Deref for DefaultTrueBool {
    type Target = bool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Copy)]
pub struct DefaultFalseBool(bool);

impl Default for DefaultFalseBool {
    fn default() -> Self {
        Self(false)
    }
}

impl DeJson for DefaultFalseBool {
    fn de_json(state: &mut DeJsonState, input: &mut Chars) -> Result<Self, DeJsonErr> {
        bool::de_json(state, input).map(Self)
    }
}

impl Deref for DefaultFalseBool {
    type Target = bool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Default, DeJson, Debug)]
pub struct Config {
    #[nserde(default)]
    pub rust_log: RustLog,
    #[nserde(default)]
    pub port_number: Port,
    #[nserde(default)]
    pub accounts: Vec<AccountData>,
    #[nserde(default)]
    pub server_address: ServerAddress,
    #[nserde(default)]
    pub spam_bot_support: DefaultTrueBool,
    #[nserde(default)]
    pub send_tells_over_main: DefaultTrueBool,
    #[nserde(default)]
    pub relay_worker_tells: DefaultFalseBool,
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
            self.server_address = ServerAddress(addr);
            changed += 1;
        }
        if let Ok(port) = var("PROXY_PORT_NUMBER") {
            if let Ok(p) = port.parse() {
                self.port_number = Port(p);
                changed += 1;
            }
        }
        if let Ok(spam_bot_support) = var("SPAM_BOT_SUPPORT") {
            if let Ok(s) = spam_bot_support.parse() {
                self.spam_bot_support = DefaultTrueBool(s);
                changed += 1;
            }
        }
        if let Ok(send_tells_over_main) = var("SEND_TELLS_OVER_MAIN") {
            if let Ok(s) = send_tells_over_main.parse() {
                self.send_tells_over_main = DefaultTrueBool(s);
                changed += 1;
            }
        }
        if let Ok(relay_worker_tells) = var("RELAY_WORKER_TELLS") {
            if let Ok(r) = relay_worker_tells.parse() {
                self.relay_worker_tells = DefaultFalseBool(r);
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
            self.rust_log = RustLog(level);
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

        if *self.spam_bot_support && (!*self.send_tells_over_main && self.accounts.is_empty()) {
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
            Config::default()
        }
    };
    let changed_from_env = conf.mash_with_env()?;

    conf.validate_self()?;

    if !used_config && changed_from_env == 0 {
        warn!("{} not found, proceeding with defaults", file);
    }

    Ok(conf)
}
