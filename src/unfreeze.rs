use std::time::Duration;

use hyper::{
    client::HttpConnector,
    header::{CONTENT_TYPE, COOKIE, ORIGIN, REFERER, SET_COOKIE, USER_AGENT},
    Body, Client, Method, Request, StatusCode, Uri,
};
use hyper_proxy::{Intercept, Proxy, ProxyConnector};
use tokio::time::timeout;

const PROXY_URL: &str = "http://proxy.nadybot.org:22222";
const LOGIN_URL: &str = "https://register.funcom.com/account";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub enum Unfreezer {
    Proxied(Client<ProxyConnector<HttpConnector>>),
    Unproxied(Client<hyper_rustls::HttpsConnector<HttpConnector>>),
}

pub enum UnfreezeResult {
    Unfrozen,
    NotFrozen,
    Failed,
}

impl UnfreezeResult {
    pub fn should_continue(&self) -> bool {
        matches!(self, Self::Unfrozen | Self::NotFrozen)
    }
}

impl Unfreezer {
    pub fn new(with_proxy: bool) -> Self {
        if with_proxy {
            let mut proxy = Proxy::new(Intercept::All, Uri::from_static(PROXY_URL));
            proxy.force_connect();

            let mut connector = HttpConnector::new();
            connector.set_connect_timeout(Some(Duration::from_secs(5)));

            let proxy_connector = ProxyConnector::from_proxy(connector, proxy).unwrap();

            let client = Client::builder().build(proxy_connector);

            Self::Proxied(client)
        } else {
            let connector = hyper_rustls::HttpsConnectorBuilder::new()
                .with_webpki_roots()
                .https_or_http()
                .enable_http1()
                .build();

            let client = Client::builder().build(connector);

            Self::Unproxied(client)
        }
    }

    pub async fn unfreeze(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UnfreezeResult, hyper::Error> {
        let username = username.to_lowercase();

        // Request login page to get an aca cookie
        let payload = serde_urlencoded::to_string([
            ("__ac_name", username.as_str()),
            ("__ac_password", &password),
        ])
        .unwrap();

        let mut res = loop {
            let req = Request::builder()
                .method(Method::POST)
                .uri(LOGIN_URL)
                .header(
                    USER_AGENT,
                    concat!("aochatproxy/", env!("CARGO_PKG_VERSION")),
                )
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(ORIGIN, "https://register.funcom.com")
                .header(REFERER, "https://register.funcom.com/account")
                .body(Body::from(payload.clone()))
                .unwrap();

            match timeout(REQUEST_TIMEOUT, {
                match self {
                    Self::Proxied(c) => c.request(req),
                    Self::Unproxied(c) => c.request(req),
                }
            })
            .await
            {
                Ok(Ok(res)) => {
                    if res.status() == StatusCode::FOUND {
                        break res;
                    } else {
                        log::error!("Login to account did not return a HTTP 302. Retrying");
                    }
                }
                Ok(Err(e)) => {
                    log::error!("Failed to log in to account due to {e}, retrying");
                }
                Err(_) => {
                    log::error!("Timeout when logging in to account, retrying");
                }
            };
        };

        let cookies = res
            .headers()
            .get_all(SET_COOKIE)
            .into_iter()
            .filter_map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|value| value.split(';').next())
            });

        let mut cookies_value = String::new();

        for (idx, cookie) in cookies.enumerate() {
            if idx != 0 {
                cookies_value.push_str("; ");
            }

            cookies_value.push_str(cookie);
        }

        res = loop {
            let reactivate_url = format!("https://register.funcom.com/account/subscription/ctrl/anarchy/{username}/reactivate");

            let req = Request::builder()
                .method(Method::POST)
                .uri(&reactivate_url)
                .header(
                    USER_AGENT,
                    concat!("aochatproxy/", env!("CARGO_PKG_VERSION")),
                )
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(ORIGIN, "https://register.funcom.com")
                .header(REFERER, reactivate_url)
                .header(COOKIE, &cookies_value)
                .body(Body::from("process=submit"))
                .unwrap();

            match timeout(
                REQUEST_TIMEOUT,
                match self {
                    Self::Proxied(c) => c.request(req),
                    Self::Unproxied(c) => c.request(req),
                },
            )
            .await
            {
                Ok(Ok(res)) => {
                    if res.status() == StatusCode::OK {
                        break res;
                    } else {
                        log::error!("Unfreezing account did not return a HTTP 200. Retrying");
                    }
                }
                Ok(Err(e)) => {
                    log::error!("Failed to unfreeze account due to {e}, retrying");
                }
                Err(_) => {
                    log::error!("Timeout when unfreezing account, retrying");
                }
            };
        };

        let bytes = hyper::body::to_bytes(res.body_mut()).await?;
        let body = String::from_utf8_lossy(&bytes);

        if body.contains("<div>Subscription Reactivated</div>") {
            log::info!("Successfully unfroze account.");
            Ok(UnfreezeResult::Unfrozen)
        } else if body.contains("<div>This account is not cancelled or frozen</div>") {
            log::info!("Account was not frozen.");
            Ok(UnfreezeResult::NotFrozen)
        } else {
            log::error!("There was an error when unfreezing the account.");
            Ok(UnfreezeResult::Failed)
        }
    }
}
