# Pingate

Pingate is an environment-configured HTTP reverse proxy and load balancer
built in Rust on [Pingora](https://github.com/cloudflare/pingora). It can route
to a fixed list of servers or discover labeled workloads from Docker Compose
and Docker Swarm automatically.

- No configuration files: settings come from `PINGATE_*` environment variables.
- Automatic route updates when containers, services, or replicas change.
- Round-robin balancing with readiness and liveness endpoints built in.
- One small container image for static, Compose, and Swarm deployments.

## Try it

Pingate is published at `ghcr.io/bardiayaghmaie/pingate`. Pull the image:

```shell
docker pull ghcr.io/bardiayaghmaie/pingate:latest
```

Then save the following as `compose.yaml`. It is a complete, copy-pasteable
demo: Pingate discovers two labeled nginx containers and balances requests
between them.

```yaml
services:
  pingate:
    image: ghcr.io/bardiayaghmaie/pingate:latest
    user: root
    environment:
      PINGATE_MODE: compose
      PINGATE_DOCKER_HOST: unix:///var/run/docker.sock
    ports:
      - "6198:6198"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    read_only: true
    tmpfs:
      - /tmp
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL

  api1:
    image: nginx:alpine
    command:
      - /bin/sh
      - -c
      - |
        printf 'api1\n' > /usr/share/nginx/html/index.html
        exec nginx -g 'daemon off;'
    labels:
      pingate.enable: "true"
      pingate.host: api.localhost
      pingate.port: "80"

  api2:
    image: nginx:alpine
    command:
      - /bin/sh
      - -c
      - |
        printf 'api2\n' > /usr/share/nginx/html/index.html
        exec nginx -g 'daemon off;'
    labels:
      pingate.enable: "true"
      pingate.host: api.localhost
      pingate.port: "80"
```

Start the demo and make several requests:

```shell
docker compose up --detach --wait

for request in 1 2 3 4; do
  curl -H 'Host: api.localhost' http://127.0.0.1:6198
done
```

The responses should alternate between `api1` and `api2`. Clean up with:

```shell
docker compose down
```

> [!WARNING]
> This quick start mounts the Docker socket directly into Pingate. That is
> convenient for local use but grants root-equivalent access to the host. For
> production, read [Docker API security](#docker-api-security) and use the
> socket-proxy pattern described there.

## Choose a mode

| Mode | Use it when | Upstreams come from |
|---|---|---|
| `static` | Your backend addresses are already known | `PINGATE_STATIC_UPSTREAMS` |
| `compose` | Services run in Docker Compose on one host | Labels on running containers |
| `swarm` | Services run in Docker Swarm | Labels on services and DNS for their running tasks |

Set exactly one mode with `PINGATE_MODE`. Docker is not used in `static` mode.

## Use Docker Compose discovery

Add these labels to every service Pingate should route:

```yaml
services:
  api:
    image: your-api:latest
    labels:
      pingate.enable: "true"
      pingate.host: api.example.com
      pingate.port: "8000"
      pingate.network: pingate-public
```

Then send requests to Pingate with the configured host:

```shell
curl -H 'Host: api.example.com' http://127.0.0.1
```

Pingate watches Docker events and periodically reconciles all routes, so
starting, stopping, or replacing a labeled container does not require a
Pingate restart. See [`docker-compose.yaml`](docker-compose.yaml) for a complete
working deployment.

### Workload labels

| Label | Required | Description |
|---|---|---|
| `pingate.enable` | yes | Must be exactly `"true"` (case-insensitive); absent or any other value disables routing |
| `pingate.host` | yes | Host header to route; it is normalized to lowercase and validated as a hostname |
| `pingate.port` | yes | Internal TCP port on the workload |
| `pingate.network` | sometimes | Docker network whose workload IP Pingate can reach; see [Docker networking](#docker-networking) |

Compose labels belong under `services.<name>.labels`. In Swarm they must be
under `services.<name>.deploy.labels`, because Pingate reads labels from the
service specification rather than its containers.

## Use static upstreams

Static mode balances every request across a fixed list of addresses and runs
periodic TCP health checks. Use the same published image in Compose:

```yaml
services:
  pingate:
    image: ghcr.io/bardiayaghmaie/pingate:latest
    environment:
      PINGATE_MODE: static
      PINGATE_STATIC_UPSTREAMS: 10.0.0.10:8000,10.0.0.11:8000
    ports:
      - "6198:6198"
```

Replace the example addresses with backend IPs reachable from the Pingate
container. The proxy is then available at `http://127.0.0.1:6198`. Set
`PINGATE_STATIC_UPSTREAM_TLS=true` and `PINGATE_STATIC_UPSTREAM_SNI` if the
upstream servers expect TLS.

## Use Docker Swarm discovery

Create the example's external overlay network, deploy the stack, and make a
request:

```shell
docker network create --driver overlay --attachable pingate-public
docker stack deploy --compose-file docker-stack.yaml pingate
curl -H 'Host: api.localhost' http://127.0.0.1:6198
```

Pingate must run on a manager, reach that manager's Docker Engine API, and
share the selected overlay network with routed services. It reads service
labels and running task state, then resolves `tasks.<service>` through Docker's
embedded DNS to balance directly across replicas. See
[`docker-stack.yaml`](docker-stack.yaml) for the complete example.

## Docker networking

A workload attached to multiple networks has multiple IP addresses. Pingate
chooses the routable network in this order:

1. Use the workload's `pingate.network` label.
2. Otherwise use `PINGATE_DOCKER_DEFAULT_NETWORK`.
3. Otherwise use the workload's only network, if it has exactly one.
4. Otherwise log a warning and skip that workload.

Usually, set `PINGATE_DOCKER_DEFAULT_NETWORK` once to the network shared by
Pingate and its backends. Add `pingate.network` only to workloads that need an
override.

## Configuration

Pingate deliberately has no TOML or YAML configuration file. All configuration
comes from environment variables, and invalid values fail fast at startup.

| Variable | Default | Description |
|---|---|---|
| `PINGATE_MODE` | required | One of `static`, `compose`, or `swarm` |
| `PINGATE_LISTEN_ADDR` | `0.0.0.0:6198` | Public proxy listener |
| `PINGATE_ADMIN_LISTEN_ADDR` | `0.0.0.0:6197` | Internal health listener; must differ from the public listener and should never be published externally |
| `PINGATE_STATIC_UPSTREAMS` | none | Comma-separated `host:port` list; required and used only in `static` mode |
| `PINGATE_STATIC_HEALTH_CHECK_INTERVAL_SECONDS` | `5` | TCP health-check cadence for static upstreams |
| `PINGATE_STATIC_UPSTREAM_TLS` | `false` | Use TLS when connecting to static upstreams |
| `PINGATE_STATIC_UPSTREAM_SNI` | empty | SNI value for static upstream TLS |
| `PINGATE_DOCKER_HOST` | `unix:///var/run/docker.sock` | Docker Engine endpoint; must start with `unix://`, `http://`, or `https://` |
| `PINGATE_DOCKER_RECONNECT_INTERVAL_SECONDS` | `2` | Delay before reconnecting a dropped Docker event stream |
| `PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS` | `30` | Full route reconciliation interval, independent of events |
| `PINGATE_DOCKER_DEFAULT_NETWORK` | none | Fallback routable Docker network when a workload has no `pingate.network` label |
| `PINGATE_DOCKER_TLS_CA_PATH` | none | CA certificate path for a remote Docker Engine |
| `PINGATE_DOCKER_TLS_CERT_PATH` | none | Client certificate path for a remote Docker Engine |
| `PINGATE_DOCKER_TLS_KEY_PATH` | none | Client key path for a remote Docker Engine |
| `RUST_LOG` | `info` | Standard `env_logger` filter, for example `debug` or `pingate=debug` |

The three Docker TLS paths must be supplied together; partial TLS
configuration is rejected.

## Request and health behavior

In Compose and Swarm modes, Pingate uses the request's `Host` header to choose
a route:

| Result | HTTP status |
|---|---|
| Missing or invalid `Host` header | `400 Bad Request` |
| No route is configured for the host | `404 Not Found` |
| A route exists but has no running upstreams | `503 Service Unavailable` |
| A healthy upstream is available | Proxied to the selected upstream |

The internal admin listener provides:

- `GET /healthz` for liveness. It returns `200` once the process is running.
- `GET /readyz` for readiness. It is immediate in static mode and returns
  `200` in Docker modes after the first successful discovery refresh.
- `pingate healthcheck`, the image's built-in `HEALTHCHECK` command, which
  requests `/readyz` and exits non-zero unless it receives `200`.

Do not expose the admin listener (port `6197` by default) to the public network.

## Docker API security

Access to the Docker socket is highly privileged: a process that can reach it
can do anything the Docker daemon can, including starting a privileged
container that mounts the host filesystem. There is no partial-permission mode
on the socket itself.

The examples in this repository mount the socket directly into Pingate and run
the container as root:

```yaml
user: root
environment:
  PINGATE_DOCKER_HOST: unix:///var/run/docker.sock
volumes:
  - /var/run/docker.sock:/var/run/docker.sock:ro
```

Running as root here is deliberate. The socket's owning group differs across
hosts: it may be a Linux `docker` group, gid 0 on Docker Desktop for Mac, or
another group on Docker Desktop for Windows/WSL. Matching it with `group_add`
is therefore fragile. Mounting the socket already grants root-equivalent host
power regardless of the container UID, so using a non-root UID in this setup
adds portability problems without creating a meaningful security boundary.

The examples still use `read_only: true`, `cap_drop: ALL`, and
`no-new-privileges`. Keep those protections, but note that `:ro` only prevents
replacement of the socket file; it does not restrict API operations made
through the socket.

### Recommended production setup: a socket proxy

For production—especially internet-facing, multi-tenant, or shared-host
deployments—put a restricted API proxy between Pingate and Docker. A proxy such
as [`lscr.io/linuxserver/socket-proxy`](https://github.com/linuxserver/docker-socket-proxy)
(based on [Tecnativa/docker-socket-proxy](https://github.com/Tecnativa/docker-socket-proxy))
is the only container that mounts the socket and exposes an explicit allowlist
of read-only endpoints:

```yaml
services:
  docker-proxy:
    image: lscr.io/linuxserver/socket-proxy:latest
    environment:
      CONTAINERS: 1
      EVENTS: 1
      INFO: 1
      NETWORKS: 1
      PING: 1
      VERSION: 1
      # SERVICES: 1   # Also required in Swarm mode
      # TASKS: 1      # Also required in Swarm mode
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks:
      - control
    read_only: true
    tmpfs:
      - /run

  pingate:
    image: ghcr.io/bardiayaghmaie/pingate:latest
    environment:
      PINGATE_MODE: compose
      PINGATE_DOCKER_HOST: http://docker-proxy:2375
    depends_on:
      - docker-proxy
    networks:
      - control
      - public
```

Pingate then needs neither the socket volume nor `user: root`; it can use the
image's default UID/GID `65532`. Keep the `control` network internal and do not
publish the socket proxy's port. The separate process is the security boundary,
so this allowlist should not be moved into Pingate itself.

## Development

### Prerequisites

- Rust `1.95.0` (pinned in [`rust-toolchain.toml`](rust-toolchain.toml))
- Docker with Compose for the end-to-end example

### Build and test

```shell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
docker build -t pingate:local .
```

Unit tests live beside the code in each Rust module. CI also runs Compose and
Swarm routing tests, audits Rust dependencies, verifies that the release image
runs as non-root, and scans the image for unfixed high and critical CVEs.

### Source map

| Path | Responsibility |
|---|---|
| [`src/main.rs`](src/main.rs) | Starts Pingora, selects the operating mode, wires the proxy and admin services, and implements `pingate healthcheck` |
| [`src/settings.rs`](src/settings.rs) | Parses and validates every environment variable |
| [`src/docker.rs`](src/docker.rs) | Discovers Docker containers or Swarm services and publishes route-table updates |
| [`src/routes.rs`](src/routes.rs) | Normalizes hosts and performs thread-safe round-robin route selection |
| [`src/admin.rs`](src/admin.rs) | Serves `/healthz` and `/readyz` |
| [`.github/workflows/ci.yml`](.github/workflows/ci.yml) | Runs Rust, Compose, image, and Swarm checks |

When changing settings or labels, update both the implementation and this
README so the configuration reference stays accurate.

## Releases

Pushing a semver git tag (`vX.Y.Z`) triggers the release workflow. It publishes
multi-architecture (`linux/amd64` and `linux/arm64`) images to
`ghcr.io/bardiayaghmaie/pingate` with version, minor-version, commit-SHA, and
`latest` tags, plus SBOM and provenance attestations.
