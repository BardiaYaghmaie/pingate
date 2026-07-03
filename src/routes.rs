use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub addr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub host: String,
    pub upstreams: Vec<Upstream>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    EmptyHost,
    InvalidHost(String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHost => write!(f, "host is required"),
            Self::InvalidHost(host) => write!(f, "invalid host `{host}`"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteTable {
    inner: Arc<RwLock<HashMap<String, RouteState>>>,
}

#[derive(Debug, Clone)]
struct RouteState {
    upstreams: Vec<Upstream>,
    next: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteSelection {
    Unknown,
    Unavailable,
    Available(Upstream),
}

impl Default for RouteTable {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl RouteTable {
    pub fn replace(&self, routes: Vec<Route>) -> Result<(), RouteError> {
        let mut next = HashMap::new();

        for route in routes {
            let host = normalize_host(&route.host)?;
            next.insert(
                host,
                RouteState {
                    upstreams: route.upstreams,
                    next: 0,
                },
            );
        }

        let mut routes = self.inner.write().expect("route table lock poisoned");
        *routes = next;
        Ok(())
    }

    pub fn select(&self, host: &str) -> RouteSelection {
        let Ok(host) = normalize_host(host) else {
            return RouteSelection::Unknown;
        };
        let mut routes = self.inner.write().expect("route table lock poisoned");
        let Some(route) = routes.get_mut(&host) else {
            return RouteSelection::Unknown;
        };

        if route.upstreams.is_empty() {
            return RouteSelection::Unavailable;
        }

        let upstream = route.upstreams[route.next % route.upstreams.len()].clone();
        route.next = route.next.wrapping_add(1);
        RouteSelection::Available(upstream)
    }

    pub fn len(&self) -> usize {
        self.inner.read().expect("route table lock poisoned").len()
    }
}

pub fn normalize_host(host: &str) -> Result<String, RouteError> {
    let host = strip_host_port(host.trim()).to_ascii_lowercase();

    if host.is_empty() {
        return Err(RouteError::EmptyHost);
    }

    if !is_valid_host(&host) {
        return Err(RouteError::InvalidHost(host));
    }

    Ok(host)
}

fn strip_host_port(host: &str) -> &str {
    if let Some(host_without_port) = host.strip_prefix('[') {
        if let Some(end) = host_without_port.find(']') {
            return &host_without_port[..end];
        }
    }

    match host.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => host,
        _ => host,
    }
}

fn is_valid_host(host: &str) -> bool {
    if host.contains("://")
        || host.contains('/')
        || host.contains('\\')
        || host.contains('*')
        || host.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return false;
    }

    host.split('.').all(is_valid_label)
}

fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hosts_for_lookup() {
        assert_eq!(
            normalize_host("API.localhost:8080"),
            Ok("api.localhost".into())
        );
    }

    #[test]
    fn rejects_invalid_hosts() {
        assert_eq!(
            normalize_host("http://api.localhost"),
            Err(RouteError::InvalidHost("http://api.localhost".into()))
        );
        assert_eq!(
            normalize_host("bad_host.localhost"),
            Err(RouteError::InvalidHost("bad_host.localhost".into()))
        );
    }

    #[test]
    fn selects_upstreams_round_robin() {
        let table = RouteTable::default();
        table
            .replace(vec![Route {
                host: "api.localhost".into(),
                upstreams: vec![
                    Upstream {
                        addr: "172.18.0.2:8000".into(),
                    },
                    Upstream {
                        addr: "172.18.0.3:8000".into(),
                    },
                ],
            }])
            .unwrap();

        let selected = |selection| match selection {
            RouteSelection::Available(upstream) => upstream.addr,
            other => panic!("expected available route, got {other:?}"),
        };
        assert_eq!(selected(table.select("api.localhost")), "172.18.0.2:8000");
        assert_eq!(selected(table.select("api.localhost")), "172.18.0.3:8000");
        assert_eq!(selected(table.select("api.localhost")), "172.18.0.2:8000");
    }

    #[test]
    fn distinguishes_unknown_and_temporarily_unavailable_routes() {
        let table = RouteTable::default();
        table
            .replace(vec![Route {
                host: "api.localhost".into(),
                upstreams: vec![],
            }])
            .unwrap();

        assert_eq!(table.select("api.localhost"), RouteSelection::Unavailable);
        assert_eq!(table.select("unknown.localhost"), RouteSelection::Unknown);
    }
}
