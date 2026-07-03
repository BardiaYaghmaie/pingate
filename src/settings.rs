use std::{env, error::Error, fmt, net::SocketAddr, path::PathBuf};

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:6198";
const DEFAULT_ADMIN_LISTEN_ADDR: &str = "0.0.0.0:6197";
const DEFAULT_DOCKER_HOST: &str = "unix:///var/run/docker.sock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Static,
    Compose,
    Swarm,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub mode: Mode,
    pub server: ServerConfig,
    pub static_upstreams: StaticConfig,
    pub docker: DockerConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub admin_listen_addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct StaticConfig {
    pub addresses: Vec<SocketAddr>,
    pub health_check_interval_seconds: u64,
    pub upstream_tls: bool,
    pub upstream_sni: String,
}

#[derive(Debug, Clone)]
pub struct DockerConfig {
    pub host: String,
    pub reconnect_interval_seconds: u64,
    pub resync_interval_seconds: u64,
    pub default_network: Option<String>,
    pub tls_ca_path: Option<PathBuf>,
    pub tls_cert_path: Option<PathBuf>,
    pub tls_key_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(String);

impl SettingsError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for SettingsError {}

impl Settings {
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    fn from_lookup<F>(lookup: F) -> Result<Self, SettingsError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let required = |key: &str| {
            lookup(key)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| SettingsError::new(format!("{key} is required")))
        };
        let value_or = |key: &str, default: &str| lookup(key).unwrap_or_else(|| default.into());

        let mode = match required("PINGATE_MODE")?.to_ascii_lowercase().as_str() {
            "static" => Mode::Static,
            "compose" => Mode::Compose,
            "swarm" => Mode::Swarm,
            value => {
                return Err(SettingsError::new(format!(
                    "PINGATE_MODE must be static, compose, or swarm; got `{value}`"
                )))
            }
        };

        let listen_addr = parse_socket_addr(
            "PINGATE_LISTEN_ADDR",
            &value_or("PINGATE_LISTEN_ADDR", DEFAULT_LISTEN_ADDR),
        )?;
        let admin_listen_addr = parse_socket_addr(
            "PINGATE_ADMIN_LISTEN_ADDR",
            &value_or("PINGATE_ADMIN_LISTEN_ADDR", DEFAULT_ADMIN_LISTEN_ADDR),
        )?;
        if listen_addr == admin_listen_addr {
            return Err(SettingsError::new(
                "PINGATE_LISTEN_ADDR and PINGATE_ADMIN_LISTEN_ADDR must differ",
            ));
        }

        let addresses = match lookup("PINGATE_STATIC_UPSTREAMS") {
            Some(value) => parse_upstreams(&value)?,
            None if mode == Mode::Static => {
                return Err(SettingsError::new(
                    "PINGATE_STATIC_UPSTREAMS is required in static mode",
                ))
            }
            None => Vec::new(),
        };

        let health_check_interval_seconds = parse_positive_u64(
            "PINGATE_STATIC_HEALTH_CHECK_INTERVAL_SECONDS",
            &value_or("PINGATE_STATIC_HEALTH_CHECK_INTERVAL_SECONDS", "5"),
        )?;
        let upstream_tls = parse_bool(
            "PINGATE_STATIC_UPSTREAM_TLS",
            &value_or("PINGATE_STATIC_UPSTREAM_TLS", "false"),
        )?;
        let upstream_sni = lookup("PINGATE_STATIC_UPSTREAM_SNI").unwrap_or_default();

        let host = value_or("PINGATE_DOCKER_HOST", DEFAULT_DOCKER_HOST);
        if !host.starts_with("unix://")
            && !host.starts_with("http://")
            && !host.starts_with("https://")
        {
            return Err(SettingsError::new(
                "PINGATE_DOCKER_HOST must use unix://, http://, or https://",
            ));
        }

        let reconnect_interval_seconds = parse_positive_u64(
            "PINGATE_DOCKER_RECONNECT_INTERVAL_SECONDS",
            &value_or("PINGATE_DOCKER_RECONNECT_INTERVAL_SECONDS", "2"),
        )?;
        let resync_interval_seconds = parse_positive_u64(
            "PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS",
            &value_or("PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS", "30"),
        )?;
        let default_network = non_empty(lookup("PINGATE_DOCKER_DEFAULT_NETWORK"));
        let tls_ca_path = path(lookup("PINGATE_DOCKER_TLS_CA_PATH"));
        let tls_cert_path = path(lookup("PINGATE_DOCKER_TLS_CERT_PATH"));
        let tls_key_path = path(lookup("PINGATE_DOCKER_TLS_KEY_PATH"));
        let custom_tls_count = [
            tls_ca_path.is_some(),
            tls_cert_path.is_some(),
            tls_key_path.is_some(),
        ]
        .into_iter()
        .filter(|configured| *configured)
        .count();
        if (1..3).contains(&custom_tls_count) {
            return Err(SettingsError::new(
                "PINGATE_DOCKER_TLS_CA_PATH, PINGATE_DOCKER_TLS_CERT_PATH, and PINGATE_DOCKER_TLS_KEY_PATH must be supplied together",
            ));
        }

        Ok(Self {
            mode,
            server: ServerConfig {
                listen_addr,
                admin_listen_addr,
            },
            static_upstreams: StaticConfig {
                addresses,
                health_check_interval_seconds,
                upstream_tls,
                upstream_sni,
            },
            docker: DockerConfig {
                host,
                reconnect_interval_seconds,
                resync_interval_seconds,
                default_network,
                tls_ca_path,
                tls_cert_path,
                tls_key_path,
            },
        })
    }
}

fn parse_socket_addr(key: &str, value: &str) -> Result<SocketAddr, SettingsError> {
    value
        .parse()
        .map_err(|_| SettingsError::new(format!("{key} must be a socket address, got `{value}`")))
}

fn parse_upstreams(value: &str) -> Result<Vec<SocketAddr>, SettingsError> {
    let values = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| parse_socket_addr("PINGATE_STATIC_UPSTREAMS", value))
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() {
        return Err(SettingsError::new(
            "PINGATE_STATIC_UPSTREAMS must contain at least one address",
        ));
    }
    Ok(values)
}

fn parse_positive_u64(key: &str, value: &str) -> Result<u64, SettingsError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| SettingsError::new(format!("{key} must be a positive integer")))
}

fn parse_bool(key: &str, value: &str) -> Result<bool, SettingsError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(SettingsError::new(format!(
            "{key} must be true or false, got `{value}`"
        ))),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn path(value: Option<String>) -> Option<PathBuf> {
    non_empty(value).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(values: &[(&str, &str)]) -> Result<Settings, SettingsError> {
        let values = values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<HashMap<_, _>>();
        Settings::from_lookup(|key| values.get(key).cloned())
    }

    #[test]
    fn requires_mode() {
        assert_eq!(
            settings(&[]).unwrap_err().to_string(),
            "PINGATE_MODE is required"
        );
    }

    #[test]
    fn loads_compose_defaults() {
        let settings = settings(&[("PINGATE_MODE", "compose")]).unwrap();
        assert_eq!(settings.mode, Mode::Compose);
        assert_eq!(settings.server.listen_addr.port(), 6198);
        assert_eq!(settings.server.admin_listen_addr.port(), 6197);
        assert_eq!(settings.docker.host, DEFAULT_DOCKER_HOST);
    }

    #[test]
    fn parses_static_upstream_list() {
        let settings = settings(&[
            ("PINGATE_MODE", "static"),
            ("PINGATE_STATIC_UPSTREAMS", "127.0.0.1:8000, 127.0.0.1:8001"),
        ])
        .unwrap();
        assert_eq!(settings.static_upstreams.addresses.len(), 2);
    }

    #[test]
    fn rejects_static_mode_without_upstreams() {
        assert_eq!(
            settings(&[("PINGATE_MODE", "static")])
                .unwrap_err()
                .to_string(),
            "PINGATE_STATIC_UPSTREAMS is required in static mode"
        );
    }

    #[test]
    fn rejects_identical_listeners() {
        let error = settings(&[
            ("PINGATE_MODE", "compose"),
            ("PINGATE_LISTEN_ADDR", "127.0.0.1:7000"),
            ("PINGATE_ADMIN_LISTEN_ADDR", "127.0.0.1:7000"),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("must differ"));
    }

    #[test]
    fn rejects_incomplete_docker_client_certificate() {
        let error = settings(&[
            ("PINGATE_MODE", "swarm"),
            ("PINGATE_DOCKER_TLS_CERT_PATH", "/cert.pem"),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("must be supplied together"));
    }

    #[test]
    fn rejects_unsupported_docker_host_scheme() {
        let error = settings(&[
            ("PINGATE_MODE", "compose"),
            ("PINGATE_DOCKER_HOST", "ssh://manager"),
        ])
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("must use unix://, http://, or https://"));
    }
}
