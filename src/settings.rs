use serde::Deserialize;
use std::{error::Error, fmt, net::SocketAddr};

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub server: ServerConfig,
    pub upstreams: UpstreamsConfig,
    pub proxy: ProxyConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
}

#[derive(Debug, Deserialize)]
pub struct UpstreamsConfig {
    pub addresses: Vec<String>,
    pub health_check_frequency: u64, // in seconds
}

#[derive(Debug, Default, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub upstream_tls: bool,
    #[serde(default, alias = "upstream_identifier")]
    pub upstream_sni: String,
}

#[derive(Debug)]
pub enum SettingsError {
    Config(config::ConfigError),
    Validation(ValidationError),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    EmptyUpstreams,
    InvalidListenAddr(String),
    InvalidUpstreamAddr(String),
    ZeroHealthCheckFrequency,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(err) => write!(f, "failed to read configuration: {err}"),
            Self::Validation(err) => write!(f, "invalid configuration: {err}"),
        }
    }
}

impl Error for SettingsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(err) => Some(err),
            Self::Validation(err) => Some(err),
        }
    }
}

impl From<config::ConfigError> for SettingsError {
    fn from(err: config::ConfigError) -> Self {
        Self::Config(err)
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyUpstreams => write!(f, "at least one upstream address is required"),
            Self::InvalidListenAddr(addr) => {
                write!(f, "server.listen_addr must be a socket address, got `{addr}`")
            }
            Self::InvalidUpstreamAddr(addr) => {
                write!(f, "upstreams.addresses contains an invalid socket address: `{addr}`")
            }
            Self::ZeroHealthCheckFrequency => {
                write!(f, "upstreams.health_check_frequency must be greater than zero")
            }
        }
    }
}

impl Error for ValidationError {}

impl Settings {
    pub fn new() -> Result<Self, SettingsError> {
        let cfg = config::Config::builder()
            .add_source(config::File::with_name("config/config"))
            .build()?;
        let settings = cfg.try_deserialize::<Self>()?;
        settings.validate().map_err(SettingsError::Validation)?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        parse_socket_addr(&self.server.listen_addr)
            .map_err(|_| ValidationError::InvalidListenAddr(self.server.listen_addr.clone()))?;

        if self.upstreams.addresses.is_empty() {
            return Err(ValidationError::EmptyUpstreams);
        }

        for addr in &self.upstreams.addresses {
            parse_socket_addr(addr)
                .map_err(|_| ValidationError::InvalidUpstreamAddr(addr.clone()))?;
        }

        if self.upstreams.health_check_frequency == 0 {
            return Err(ValidationError::ZeroHealthCheckFrequency);
        }

        Ok(())
    }
}

fn parse_socket_addr(addr: &str) -> Result<SocketAddr, std::net::AddrParseError> {
    addr.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_settings() -> Settings {
        Settings {
            server: ServerConfig {
                listen_addr: "127.0.0.1:6198".to_string(),
            },
            upstreams: UpstreamsConfig {
                addresses: vec![
                    "127.0.0.1:8000".to_string(),
                    "127.0.0.1:8001".to_string(),
                ],
                health_check_frequency: 1,
            },
            proxy: ProxyConfig {
                upstream_tls: false,
                upstream_sni: String::new(),
            },
        }
    }

    #[test]
    fn validates_demo_config() {
        assert_eq!(valid_settings().validate(), Ok(()));
    }

    #[test]
    fn rejects_empty_upstream_list() {
        let mut settings = valid_settings();
        settings.upstreams.addresses.clear();

        assert_eq!(settings.validate(), Err(ValidationError::EmptyUpstreams));
    }

    #[test]
    fn rejects_invalid_listen_address() {
        let mut settings = valid_settings();
        settings.server.listen_addr = "localhost".to_string();

        assert_eq!(
            settings.validate(),
            Err(ValidationError::InvalidListenAddr("localhost".to_string()))
        );
    }

    #[test]
    fn rejects_invalid_upstream_address() {
        let mut settings = valid_settings();
        settings.upstreams.addresses = vec!["not-an-address".to_string()];

        assert_eq!(
            settings.validate(),
            Err(ValidationError::InvalidUpstreamAddr(
                "not-an-address".to_string()
            ))
        );
    }

    #[test]
    fn rejects_zero_health_check_frequency() {
        let mut settings = valid_settings();
        settings.upstreams.health_check_frequency = 0;

        assert_eq!(
            settings.validate(),
            Err(ValidationError::ZeroHealthCheckFrequency)
        );
    }

    #[test]
    fn accepts_legacy_upstream_identifier_alias() {
        let cfg = config::Config::builder()
            .add_source(config::File::from_str(
                r#"
                [server]
                listen_addr = "127.0.0.1:6198"

                [upstreams]
                addresses = ["127.0.0.1:8000"]
                health_check_frequency = 1

                [proxy]
                upstream_identifier = "legacy.example"
                "#,
                config::FileFormat::Toml,
            ))
            .build()
            .unwrap();

        let settings = cfg.try_deserialize::<Settings>().unwrap();

        assert_eq!(settings.proxy.upstream_sni, "legacy.example");
        assert!(!settings.proxy.upstream_tls);
    }
}
