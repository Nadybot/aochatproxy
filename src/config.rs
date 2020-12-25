use std::{
    env::var,
    fmt::{Display, Formatter, Result as FmtResult},
};

#[derive(Clone)]
pub struct AccountData {
    pub username: String,
    pub password: String,
    pub character: String,
}

#[derive(Clone)]
pub struct Config {
    pub port_number: u32,
    pub accounts: Vec<AccountData>,
    pub server_address: String,
    pub spam_bot_support: bool,
    pub send_tells_over_main: bool,
    pub relay_worker_tells: bool,
    pub relay_by_id: bool,
}

pub enum ConfigError {
    NotNumber(String),
    NotBoolean(String),
    InvalidConfig(String),
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
        }
    }
}

pub fn load_config() -> Result<Config, ConfigError> {
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
    let relay_by_id: bool = var("RELAY_BY_ID")
        .unwrap_or_else(|_| String::from("false"))
        .parse()
        .map_err(|_| ConfigError::NotBoolean(String::from("RELAY_BY_ID")))?;

    // We cannot send tells in this case
    if spam_bot_support & (!send_tells_over_main && account_data.is_empty()) {
        return Err(ConfigError::InvalidConfig(String::from(
            "When SPAM_BOT_SUPPORT is true and SEND_TELLS_OVER_MAIN is disabled, at least one worker needs to be configured",
        )));
    }

    Ok(Config {
        port_number,
        accounts: account_data,
        server_address,
        spam_bot_support,
        send_tells_over_main,
        relay_worker_tells,
        relay_by_id,
    })
}
