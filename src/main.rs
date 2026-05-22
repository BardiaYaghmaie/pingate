mod settings;
use settings::{Settings, SettingsError};

use async_trait::async_trait;
use log::{debug, error};
use pingora::prelude::*;
use std::{error::Error as StdError, fmt, net::TcpListener, sync::Arc, time::Duration};

pub struct LB {
    lb: Arc<LoadBalancer<RoundRobin>>,
    upstream_tls: bool,
    upstream_sni: String,
}

#[async_trait]
impl ProxyHttp for LB {
    type CTX = ();
    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>> {
        match self.lb.select(b"", 256) {
            Some(upstream) => {
                debug!("selected upstream peer: {upstream:?}");
                let peer = Box::new(HttpPeer::new(
                    upstream,
                    self.upstream_tls,
                    self.upstream_sni.clone(),
                ));
                Ok(peer)
            }
            None => {
                error!("no healthy upstream servers available");
                Err(Error::new(Custom("No healthy upstreams available")))
            }
        }
    }
}

#[derive(Debug)]
enum StartupError {
    Settings(SettingsError),
    ListenAddrUnavailable { addr: String, source: std::io::Error },
    ServerInit(String),
    Upstreams(String),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settings(err) => write!(f, "{err}"),
            Self::ListenAddrUnavailable { addr, source } => {
                write!(f, "cannot listen on `{addr}`: {source}")
            }
            Self::ServerInit(err) => write!(f, "failed to initialize Pingora server: {err}"),
            Self::Upstreams(err) => write!(f, "failed to configure upstreams: {err}"),
        }
    }
}

impl StdError for StartupError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Settings(err) => Some(err),
            Self::ListenAddrUnavailable { source, .. } => Some(source),
            Self::ServerInit(_) | Self::Upstreams(_) => None,
        }
    }
}

impl From<SettingsError> for StartupError {
    fn from(err: SettingsError) -> Self {
        Self::Settings(err)
    }
}

fn main() {
    env_logger::init();

    match build_server() {
        Ok(server) => server.run_forever(),
        Err(err) => {
            error!("{err}");
            std::process::exit(1);
        }
    }
}

fn build_server() -> std::result::Result<Server, StartupError> {
    let settings = Settings::new()?;
    let listen_addr = settings.server.listen_addr;
    let upstream_addresses = settings.upstreams.addresses;
    let health_check_frequency = settings.upstreams.health_check_frequency;
    let proxy = settings.proxy;

    ensure_listen_addr_available(&listen_addr)?;

    let mut my_server =
        Server::new(None).map_err(|err| StartupError::ServerInit(err.to_string()))?;
    my_server.bootstrap();

    let mut upstreams = LoadBalancer::try_from_iter(upstream_addresses.iter().map(String::as_str))
        .map_err(|err| StartupError::Upstreams(err.to_string()))?;

    let hc = TcpHealthCheck::new();
    upstreams.set_health_check(hc);
    upstreams.health_check_frequency = Some(Duration::from_secs(health_check_frequency));
    let background = background_service("health check", upstreams);
    let upstreams = background.task();

    let lb = LB {
        lb: upstreams,
        upstream_tls: proxy.upstream_tls,
        upstream_sni: proxy.upstream_sni,
    };
    let mut proxy_service = http_proxy_service(&my_server.configuration, lb);
    proxy_service.add_tcp(&listen_addr);

    my_server.add_service(proxy_service);
    my_server.add_service(background);

    Ok(my_server)
}

fn ensure_listen_addr_available(addr: &str) -> std::result::Result<(), StartupError> {
    TcpListener::bind(addr)
        .map(|listener| drop(listener))
        .map_err(|source| StartupError::ListenAddrUnavailable {
            addr: addr.to_string(),
            source,
        })
}
