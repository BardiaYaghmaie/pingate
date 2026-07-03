mod admin;
mod docker;
mod routes;
mod settings;

use admin::AdminApp;
use async_trait::async_trait;
use docker::DockerDiscovery;
use log::{debug, error, info};
use pingora::{apps::http_app::HttpServer, prelude::*, services::listening::Service};
use routes::{normalize_host, RouteSelection, RouteTable, Upstream};
use settings::{Mode, Settings, SettingsError};
use std::{
    error::Error as StdError,
    fmt,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

pub struct StaticLb {
    lb: Arc<LoadBalancer<RoundRobin>>,
    upstream_tls: bool,
    upstream_sni: String,
}

#[async_trait]
impl ProxyHttp for StaticLb {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>> {
        match self.lb.select(b"", 256) {
            Some(upstream) => {
                debug!("selected static upstream peer: {upstream:?}");
                Ok(Box::new(HttpPeer::new(
                    upstream,
                    self.upstream_tls,
                    self.upstream_sni.clone(),
                )))
            }
            None => {
                error!("no healthy static upstream servers available");
                Err(Error::new(Custom("No healthy upstreams available")))
            }
        }
    }
}

pub struct DockerLb {
    routes: RouteTable,
}

#[async_trait]
impl ProxyHttp for DockerLb {
    type CTX = Option<Upstream>;

    fn new_ctx(&self) -> Self::CTX {
        None
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool>
    where
        Self::CTX: Send + Sync,
    {
        let Some(host) = request_host(session) else {
            debug!("request rejected because Host header is missing or invalid");
            session.respond_error(400).await?;
            return Ok(true);
        };

        match self.routes.select(&host) {
            RouteSelection::Unknown => {
                debug!("no Docker route configured for host `{host}`");
                session.respond_error(404).await?;
                Ok(true)
            }
            RouteSelection::Unavailable => {
                debug!("Docker route `{host}` has no running tasks");
                session.respond_error(503).await?;
                Ok(true)
            }
            RouteSelection::Available(upstream) => {
                *ctx = Some(upstream);
                Ok(false)
            }
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let upstream = ctx
            .take()
            .ok_or_else(|| Error::new(Custom("missing upstream in request context")))?;
        debug!("selected Docker upstream peer: {}", upstream.addr);
        Ok(Box::new(HttpPeer::new(
            upstream.addr.as_str(),
            false,
            String::new(),
        )))
    }
}

#[derive(Debug)]
enum StartupError {
    Settings(SettingsError),
    ListenAddrUnavailable {
        name: &'static str,
        addr: SocketAddr,
        source: std::io::Error,
    },
    ServerInit(String),
    Upstreams(String),
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settings(err) => write!(f, "invalid environment configuration: {err}"),
            Self::ListenAddrUnavailable { name, addr, source } => {
                write!(f, "cannot bind {name} listener `{addr}`: {source}")
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args();
    let _binary = args.next();
    if matches!(args.next().as_deref(), Some("healthcheck")) {
        if let Err(err) = run_healthcheck() {
            error!("healthcheck failed: {err}");
            std::process::exit(1);
        }
        return;
    }

    match build_server() {
        Ok(server) => server.run_forever(),
        Err(err) => {
            error!("{err}");
            std::process::exit(1);
        }
    }
}

fn build_server() -> std::result::Result<Server, StartupError> {
    let settings = Settings::from_env()?;
    ensure_listen_addrs_available(
        settings.server.listen_addr,
        settings.server.admin_listen_addr,
    )?;

    let mut server = Server::new(None).map_err(|err| StartupError::ServerInit(err.to_string()))?;
    server.bootstrap();

    let ready = Arc::new(AtomicBool::new(settings.mode == Mode::Static));
    let public_addr = settings.server.listen_addr.to_string();
    match settings.mode {
        Mode::Static => add_static_proxy(&mut server, &settings, &public_addr)?,
        Mode::Compose | Mode::Swarm => {
            info!(
                "starting {:?} discovery via {}",
                settings.mode, settings.docker.host
            );
            let routes = RouteTable::default();
            DockerDiscovery::new(
                settings.docker.clone(),
                settings.mode,
                routes.clone(),
                ready.clone(),
            )
            .spawn();

            let mut proxy_service = http_proxy_service(&server.configuration, DockerLb { routes });
            proxy_service.add_tcp(&public_addr);
            server.add_service(proxy_service);
        }
    }

    let admin_app = HttpServer::new_app(AdminApp::new(ready));
    let mut admin_service = Service::new("Pingate admin".into(), admin_app);
    admin_service.add_tcp(&settings.server.admin_listen_addr.to_string());
    server.add_service(admin_service);

    Ok(server)
}

fn add_static_proxy(
    server: &mut Server,
    settings: &Settings,
    listen_addr: &str,
) -> std::result::Result<(), StartupError> {
    let static_config = &settings.static_upstreams;
    let addresses = static_config
        .addresses
        .iter()
        .map(SocketAddr::to_string)
        .collect::<Vec<_>>();
    let mut upstreams = LoadBalancer::try_from_iter(addresses.iter().map(String::as_str))
        .map_err(|err| StartupError::Upstreams(err.to_string()))?;
    upstreams.set_health_check(TcpHealthCheck::new());
    upstreams.health_check_frequency = Some(Duration::from_secs(
        static_config.health_check_interval_seconds,
    ));
    let background = background_service("static upstream health check", upstreams);
    let upstreams = background.task();

    let lb = StaticLb {
        lb: upstreams,
        upstream_tls: static_config.upstream_tls,
        upstream_sni: static_config.upstream_sni.clone(),
    };
    let mut proxy_service = http_proxy_service(&server.configuration, lb);
    proxy_service.add_tcp(listen_addr);
    server.add_service(proxy_service);
    server.add_service(background);
    Ok(())
}

fn request_host(session: &Session) -> Option<String> {
    let host = session.get_header("host")?.to_str().ok()?;
    normalize_host(host).ok()
}

fn ensure_listen_addrs_available(
    public: SocketAddr,
    admin: SocketAddr,
) -> std::result::Result<(), StartupError> {
    let public_listener =
        TcpListener::bind(public).map_err(|source| StartupError::ListenAddrUnavailable {
            name: "public",
            addr: public,
            source,
        })?;
    let admin_listener =
        TcpListener::bind(admin).map_err(|source| StartupError::ListenAddrUnavailable {
            name: "admin",
            addr: admin,
            source,
        })?;
    drop((public_listener, admin_listener));
    Ok(())
}

fn run_healthcheck() -> std::io::Result<()> {
    let configured =
        std::env::var("PINGATE_ADMIN_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:6197".into());
    let mut addr: SocketAddr = configured.parse().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PINGATE_ADMIN_LISTEN_ADDR is invalid",
        )
    })?;
    if addr.ip().is_unspecified() {
        addr.set_ip(match addr.ip() {
            IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        });
    }

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut response = [0_u8; 128];
    let size = stream.read(&mut response)?;
    let response = std::str::from_utf8(&response[..size]).unwrap_or_default();
    if response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(std::io::Error::other("readiness endpoint returned non-200"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_occupied_public_listener() {
        let occupied = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("failed to create test listener: {err}"),
        };
        let public = occupied.local_addr().unwrap();
        let admin_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let admin = admin_listener.local_addr().unwrap();
        let error = ensure_listen_addrs_available(public, admin).unwrap_err();
        assert!(error.to_string().contains("public listener"));
    }

    #[test]
    fn detects_occupied_admin_listener() {
        let public_listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return,
            Err(err) => panic!("failed to create test listener: {err}"),
        };
        let public = public_listener.local_addr().unwrap();
        drop(public_listener);
        let admin_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let admin = admin_listener.local_addr().unwrap();
        let error = ensure_listen_addrs_available(public, admin).unwrap_err();
        assert!(error.to_string().contains("admin listener"));
    }
}
