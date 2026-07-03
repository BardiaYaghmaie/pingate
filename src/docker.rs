use crate::{
    routes::{normalize_host, Route, RouteError, RouteTable, Upstream},
    settings::{DockerConfig, Mode},
};

use bollard::{
    models::{
        ContainerSummary, ContainerSummaryHealthStatusEnum, ContainerSummaryStateEnum, Task,
        TaskState,
    },
    Docker, API_DEFAULT_VERSION,
};
use futures_util::StreamExt;
use log::{debug, info, warn};
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use tokio::{net::lookup_host, runtime::Builder, time};

pub const ENABLE_LABEL: &str = "pingate.enable";
pub const HOST_LABEL: &str = "pingate.host";
pub const PORT_LABEL: &str = "pingate.port";
pub const NETWORK_LABEL: &str = "pingate.network";

#[derive(Debug, PartialEq, Eq)]
pub enum DockerRouteError {
    Disabled,
    MissingHost,
    InvalidHost(RouteError),
    MissingPort,
    InvalidPort(String),
    MissingNetwork,
    UnknownNetwork(String),
    MissingIp,
    MissingServiceName,
}

impl fmt::Display for DockerRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "`{ENABLE_LABEL}` must be `true`"),
            Self::MissingHost => write!(f, "`{HOST_LABEL}` label is required"),
            Self::InvalidHost(err) => write!(f, "{err}"),
            Self::MissingPort => write!(f, "`{PORT_LABEL}` label is required"),
            Self::InvalidPort(port) => {
                write!(f, "`{PORT_LABEL}` must be a valid TCP port, got `{port}`")
            }
            Self::MissingNetwork => write!(
                f,
                "`{NETWORK_LABEL}` is required when a workload has multiple networks"
            ),
            Self::UnknownNetwork(network) => {
                write!(f, "network `{network}` is not attached to the workload")
            }
            Self::MissingIp => write!(f, "workload has no address on the selected network"),
            Self::MissingServiceName => write!(f, "Swarm service has no name"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DockerDiscovery {
    config: DockerConfig,
    mode: Mode,
    routes: RouteTable,
    ready: Arc<AtomicBool>,
}

impl DockerDiscovery {
    pub fn new(
        config: DockerConfig,
        mode: Mode,
        routes: RouteTable,
        ready: Arc<AtomicBool>,
    ) -> Self {
        debug_assert!(matches!(mode, Mode::Compose | Mode::Swarm));
        Self {
            config,
            mode,
            routes,
            ready,
        }
    }

    pub fn spawn(self) {
        thread::Builder::new()
            .name("pingate-docker-discovery".into())
            .spawn(move || {
                let runtime = Builder::new_current_thread().enable_all().build();
                match runtime {
                    Ok(runtime) => runtime.block_on(self.run()),
                    Err(err) => warn!("failed to start Docker discovery runtime: {err}"),
                }
            })
            .expect("failed to spawn Docker discovery thread");
    }

    async fn run(self) {
        loop {
            let docker = match self.connect().await {
                Ok(docker) => docker,
                Err(err) => {
                    warn!("failed to connect to Docker Engine: {err}");
                    time::sleep(Duration::from_secs(self.config.reconnect_interval_seconds)).await;
                    continue;
                }
            };

            if let Err(err) = self.refresh(&docker).await {
                warn!("failed to refresh Docker routes: {err}");
            }

            let mut events = Box::pin(docker.events(None));
            let mut resync =
                time::interval(Duration::from_secs(self.config.resync_interval_seconds));
            resync.tick().await;

            loop {
                tokio::select! {
                    _ = resync.tick() => {
                        if let Err(err) = self.refresh(&docker).await {
                            warn!("periodic Docker route refresh failed: {err}");
                            break;
                        }
                    }
                    event = events.next() => {
                        match event {
                            Some(Ok(event)) => {
                                debug!("Docker event received: {:?}", event.typ);
                                if let Err(err) = self.refresh(&docker).await {
                                    warn!("Docker route refresh after event failed: {err}");
                                    break;
                                }
                            }
                            Some(Err(err)) => {
                                warn!("Docker event stream stopped: {err}");
                                break;
                            }
                            None => {
                                warn!("Docker event stream closed");
                                break;
                            }
                        }
                    }
                }
            }

            time::sleep(Duration::from_secs(self.config.reconnect_interval_seconds)).await;
        }
    }

    async fn connect(&self) -> Result<Docker, bollard::errors::Error> {
        let docker = match (
            &self.config.tls_key_path,
            &self.config.tls_cert_path,
            &self.config.tls_ca_path,
        ) {
            (Some(key), Some(cert), Some(ca)) => Docker::connect_with_ssl(
                &self.config.host,
                key,
                cert,
                ca,
                120,
                API_DEFAULT_VERSION,
            )?,
            _ => Docker::connect_with_host(&self.config.host)?,
        };
        docker.negotiate_version().await
    }

    async fn refresh(&self, docker: &Docker) -> Result<(), bollard::errors::Error> {
        let routes = match self.mode {
            Mode::Compose => self.compose_routes(docker).await?,
            Mode::Swarm => self.swarm_routes(docker).await?,
            Mode::Static => unreachable!("static mode cannot use Docker discovery"),
        };

        match self.routes.replace(routes) {
            Ok(()) => {
                self.ready.store(true, Ordering::Release);
                info!("loaded {} Docker route(s)", self.routes.len());
            }
            Err(err) => warn!("discarding invalid Docker route snapshot: {err}"),
        }
        Ok(())
    }

    async fn compose_routes(&self, docker: &Docker) -> Result<Vec<Route>, bollard::errors::Error> {
        let containers = docker.list_containers(None).await?;
        Ok(routes_from_containers(
            &containers,
            self.config.default_network.as_deref(),
        ))
    }

    async fn swarm_routes(&self, docker: &Docker) -> Result<Vec<Route>, bollard::errors::Error> {
        let services = docker.list_services(None).await?;
        let tasks = docker.list_tasks(None).await?;
        let mut tasks_by_service: HashMap<&str, usize> = HashMap::new();
        for task in &tasks {
            let Some(service_id) = task.service_id.as_deref() else {
                continue;
            };
            if !is_running_task(task) {
                continue;
            }
            *tasks_by_service.entry(service_id).or_default() += 1;
        }

        let mut by_host: BTreeMap<String, Vec<Upstream>> = BTreeMap::new();
        for service in &services {
            let Some(labels) = service.spec.as_ref().and_then(|spec| spec.labels.as_ref()) else {
                continue;
            };
            if !label_enabled(labels) {
                continue;
            }

            let service_id = service.id.as_deref().unwrap_or_default();
            match route_metadata(labels, self.config.default_network.as_deref()) {
                Ok((host, port, Some(_network))) => {
                    let service_name =
                        match service.spec.as_ref().and_then(|spec| spec.name.as_deref()) {
                            Some(name) => name,
                            None => {
                                warn!(
                                    "skipping Swarm service {service_id}: {}",
                                    DockerRouteError::MissingServiceName
                                );
                                continue;
                            }
                        };
                    let running = tasks_by_service
                        .get(service_id)
                        .copied()
                        .unwrap_or_default();
                    let upstreams = if running == 0 {
                        Vec::new()
                    } else {
                        resolve_swarm_tasks(service_name, port).await
                    };
                    by_host.entry(host).or_default().extend(upstreams);
                }
                Ok((_host, _port, None)) => {
                    warn!(
                        "skipping Swarm service {service_id}: {}",
                        DockerRouteError::MissingNetwork
                    );
                }
                Err(err) => warn!("skipping Swarm service {service_id}: {err}"),
            }
        }

        Ok(grouped_routes(by_host))
    }
}

pub fn routes_from_containers(
    containers: &[ContainerSummary],
    default_network: Option<&str>,
) -> Vec<Route> {
    let mut by_host: BTreeMap<String, Vec<Upstream>> = BTreeMap::new();
    for container in containers {
        let Some(labels) = container.labels.as_ref() else {
            continue;
        };
        if !label_enabled(labels) {
            continue;
        }

        if container.state != Some(ContainerSummaryStateEnum::RUNNING)
            || !container_is_healthy(container)
        {
            if let Ok((host, _port, _network)) = route_metadata(labels, default_network) {
                by_host.entry(host).or_default();
            }
            continue;
        }

        match route_from_container(container, labels, default_network) {
            Ok((host, upstream)) => by_host.entry(host).or_default().push(upstream),
            Err(err) => warn!(
                "skipping Docker container {}: {err}",
                container_name(container)
            ),
        }
    }
    grouped_routes(by_host)
}

fn route_from_container(
    container: &ContainerSummary,
    labels: &HashMap<String, String>,
    default_network: Option<&str>,
) -> Result<(String, Upstream), DockerRouteError> {
    let (host, port, configured_network) = route_metadata(labels, default_network)?;
    let networks = container
        .network_settings
        .as_ref()
        .and_then(|settings| settings.networks.as_ref())
        .ok_or(DockerRouteError::MissingNetwork)?;

    let endpoint = match configured_network {
        Some(network) => networks
            .get(network)
            .ok_or_else(|| DockerRouteError::UnknownNetwork(network.to_string()))?,
        None if networks.len() == 1 => networks.values().next().expect("one network exists"),
        None => return Err(DockerRouteError::MissingNetwork),
    };
    let ip = endpoint
        .ip_address
        .as_deref()
        .filter(|ip| !ip.is_empty())
        .or_else(|| {
            endpoint
                .global_ipv6_address
                .as_deref()
                .filter(|ip| !ip.is_empty())
        })
        .and_then(|ip| ip.parse::<IpAddr>().ok())
        .ok_or(DockerRouteError::MissingIp)?;

    Ok((
        host,
        Upstream {
            addr: SocketAddr::new(ip, port).to_string(),
        },
    ))
}

fn route_metadata<'a>(
    labels: &'a HashMap<String, String>,
    default_network: Option<&'a str>,
) -> Result<(String, u16, Option<&'a str>), DockerRouteError> {
    if !label_enabled(labels) {
        return Err(DockerRouteError::Disabled);
    }
    let host = labels
        .get(HOST_LABEL)
        .ok_or(DockerRouteError::MissingHost)
        .and_then(|host| normalize_host(host).map_err(DockerRouteError::InvalidHost))?;
    let port_value = labels
        .get(PORT_LABEL)
        .ok_or(DockerRouteError::MissingPort)?;
    let port = port_value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| DockerRouteError::InvalidPort(port_value.clone()))?;
    let network = labels
        .get(NETWORK_LABEL)
        .map(String::as_str)
        .filter(|network| !network.trim().is_empty())
        .or(default_network);
    Ok((host, port, network))
}

fn label_enabled(labels: &HashMap<String, String>) -> bool {
    labels
        .get(ENABLE_LABEL)
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

fn container_is_healthy(container: &ContainerSummary) -> bool {
    !matches!(
        container.health.as_ref().and_then(|health| health.status),
        Some(ContainerSummaryHealthStatusEnum::STARTING)
            | Some(ContainerSummaryHealthStatusEnum::UNHEALTHY)
    )
}

fn is_running_task(task: &Task) -> bool {
    task.desired_state == Some(TaskState::RUNNING)
        && task.status.as_ref().and_then(|status| status.state) == Some(TaskState::RUNNING)
}

async fn resolve_swarm_tasks(service_name: &str, port: u16) -> Vec<Upstream> {
    let hostname = format!("tasks.{service_name}");
    let result = lookup_host((hostname.as_str(), port)).await;
    match result {
        Ok(addresses) => {
            let mut addresses = addresses.collect::<Vec<_>>();
            addresses.sort_unstable();
            addresses.dedup();
            addresses
                .into_iter()
                .map(|addr| Upstream {
                    addr: addr.to_string(),
                })
                .collect()
        }
        Err(err) => {
            warn!("failed to resolve Swarm tasks for `{service_name}`: {err}");
            Vec::new()
        }
    }
}

fn grouped_routes(by_host: BTreeMap<String, Vec<Upstream>>) -> Vec<Route> {
    by_host
        .into_iter()
        .map(|(host, mut upstreams)| {
            upstreams.sort_by(|left, right| left.addr.cmp(&right.addr));
            upstreams.dedup_by(|left, right| left.addr == right.addr);
            Route { host, upstreams }
        })
        .collect()
}

fn container_name(container: &ContainerSummary) -> String {
    container
        .names
        .as_ref()
        .and_then(|names| names.first())
        .map(|name| name.trim_start_matches('/').to_string())
        .or_else(|| {
            container
                .id
                .as_ref()
                .map(|id| id.chars().take(12).collect())
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::{
        ContainerSummaryHealth, ContainerSummaryNetworkSettings, EndpointSettings,
    };

    fn container(labels: &[(&str, &str)], network: &str, ip: &str) -> ContainerSummary {
        ContainerSummary {
            id: Some("abc123".into()),
            names: Some(vec!["/project-api-1".into()]),
            state: Some(ContainerSummaryStateEnum::RUNNING),
            labels: Some(
                labels
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
            ),
            network_settings: Some(ContainerSummaryNetworkSettings {
                networks: Some(HashMap::from([(
                    network.into(),
                    EndpointSettings {
                        ip_address: Some(ip.into()),
                        ..Default::default()
                    },
                )])),
            }),
            ..Default::default()
        }
    }

    fn labels() -> Vec<(&'static str, &'static str)> {
        vec![
            (ENABLE_LABEL, "true"),
            (HOST_LABEL, "api.localhost"),
            (PORT_LABEL, "8000"),
            (NETWORK_LABEL, "pingate-public"),
        ]
    }

    #[test]
    fn builds_compose_route_from_required_labels() {
        let routes = routes_from_containers(
            &[container(&labels(), "pingate-public", "172.18.0.2")],
            None,
        );
        assert_eq!(routes[0].host, "api.localhost");
        assert_eq!(routes[0].upstreams[0].addr, "172.18.0.2:8000");
    }

    #[test]
    fn supports_ipv6_endpoints() {
        let routes = routes_from_containers(
            &[container(&labels(), "pingate-public", "2001:db8::2")],
            None,
        );
        assert_eq!(routes[0].upstreams[0].addr, "[2001:db8::2]:8000");
    }

    #[test]
    fn ignores_disabled_and_marks_unhealthy_route_unavailable() {
        let disabled = container(
            &[(HOST_LABEL, "api.localhost"), (PORT_LABEL, "8000")],
            "pingate-public",
            "172.18.0.2",
        );
        let mut unhealthy = container(&labels(), "pingate-public", "172.18.0.3");
        unhealthy.health = Some(ContainerSummaryHealth {
            status: Some(ContainerSummaryHealthStatusEnum::UNHEALTHY),
            ..Default::default()
        });
        let routes = routes_from_containers(&[disabled, unhealthy], None);
        assert_eq!(routes.len(), 1);
        assert!(routes[0].upstreams.is_empty());
    }

    #[test]
    fn default_network_is_used_when_label_is_absent() {
        let labels = vec![
            (ENABLE_LABEL, "true"),
            (HOST_LABEL, "api.localhost"),
            (PORT_LABEL, "8000"),
        ];
        let routes = routes_from_containers(
            &[container(&labels, "pingate-public", "172.18.0.2")],
            Some("pingate-public"),
        );
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn missing_port_skips_container() {
        let labels = vec![(ENABLE_LABEL, "true"), (HOST_LABEL, "api.localhost")];
        assert!(routes_from_containers(
            &[container(&labels, "pingate-public", "172.18.0.2")],
            None
        )
        .is_empty());
    }

    #[test]
    fn replicas_are_grouped_and_deduplicated() {
        let routes = routes_from_containers(
            &[
                container(&labels(), "pingate-public", "172.18.0.2"),
                container(&labels(), "pingate-public", "172.18.0.3"),
                container(&labels(), "pingate-public", "172.18.0.3"),
            ],
            None,
        );
        assert_eq!(routes[0].upstreams.len(), 2);
    }

    #[test]
    fn only_running_swarm_tasks_are_eligible() {
        let running = Task {
            desired_state: Some(TaskState::RUNNING),
            status: Some(bollard::models::TaskStatus {
                state: Some(TaskState::RUNNING),
                ..Default::default()
            }),
            ..Default::default()
        };
        let starting = Task {
            desired_state: Some(TaskState::RUNNING),
            status: Some(bollard::models::TaskStatus {
                state: Some(TaskState::STARTING),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(is_running_task(&running));
        assert!(!is_running_task(&starting));
    }
}
