# Pingate

Pingate is a Docker-native HTTP reverse proxy built with Rust and Pingora. It
discovers labeled Docker Compose containers or Docker Swarm services and
balances requests across their replicas.

## Quick start with Docker Compose

```shell
docker compose up --build --detach --wait
curl -H 'Host: api.localhost' http://127.0.0.1:6198
curl -H 'Host: api.localhost' http://127.0.0.1:6198
```

The example mounts the Docker socket directly into Pingate (read-only) and
runs the container as root, since reaching the socket already implies
root-equivalent access to the host regardless of the container's UID. Only
TCP port `6198` is published. The health listener on TCP `6197` remains
internal. See "Docker API security" below for when to swap this for a
restricted, non-root socket-proxy sidecar instead.

## Workload labels

Every routed workload needs these labels:

```yaml
labels:
  pingate.enable: "true"
  pingate.host: api.example.com
  pingate.port: "8000"
  pingate.network: pingate-public
```

- `pingate.enable` must be `true`.
- `pingate.host` is the HTTP Host routed to the workload.
- `pingate.port` is the workload's internal TCP port.
- `pingate.network` selects the network address Pingate can reach. It may be
  omitted when `PINGATE_DOCKER_DEFAULT_NETWORK` is configured or the container
  has exactly one network.

Compose labels belong under `services.<name>.labels`. Swarm labels belong under
`services.<name>.deploy.labels`.

## Environment variables

Pingate has no configuration file. It is configured entirely through the
environment.

| Variable | Default | Description |
|---|---|---|
| `PINGATE_MODE` | required | `static`, `compose`, or `swarm` |
| `PINGATE_LISTEN_ADDR` | `0.0.0.0:6198` | Public proxy listener |
| `PINGATE_ADMIN_LISTEN_ADDR` | `0.0.0.0:6197` | Internal health listener |
| `PINGATE_STATIC_UPSTREAMS` | none | Comma-separated addresses for static mode |
| `PINGATE_STATIC_HEALTH_CHECK_INTERVAL_SECONDS` | `5` | Static TCP health interval |
| `PINGATE_STATIC_UPSTREAM_TLS` | `false` | Use TLS for static upstreams |
| `PINGATE_STATIC_UPSTREAM_SNI` | empty | Static upstream SNI |
| `PINGATE_DOCKER_HOST` | `unix:///var/run/docker.sock` | Docker Engine Unix or HTTP(S) endpoint |
| `PINGATE_DOCKER_RECONNECT_INTERVAL_SECONDS` | `2` | Event reconnect delay |
| `PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS` | `30` | Full reconciliation interval |
| `PINGATE_DOCKER_DEFAULT_NETWORK` | none | Default routable Docker network |
| `PINGATE_DOCKER_TLS_CA_PATH` | none | Engine TLS CA path |
| `PINGATE_DOCKER_TLS_CERT_PATH` | none | Engine client certificate path |
| `PINGATE_DOCKER_TLS_KEY_PATH` | none | Engine client key path |
| `RUST_LOG` | `info` | Rust log filter |

The three Docker TLS paths must be supplied together. Invalid configuration or
occupied listener addresses cause a clear startup failure.

## Docker Swarm

Create the external overlay network, then deploy the example stack:

```shell
docker network create --driver overlay --attachable pingate-public
docker stack deploy --compose-file docker-stack.yaml pingate
curl -H 'Host: api.localhost' http://127.0.0.1:6198
```

Pingate must reach a manager's Engine API and share the selected overlay
network with routed services. It reads service labels and running task state,
then resolves `tasks.<service>` through Docker's embedded DNS to balance
directly across replicas.

## Health checks

- `GET /healthz` on port `6197` reports process liveness.
- `GET /readyz` reports readiness after the first successful discovery sync.
- `pingate healthcheck` queries `/readyz` and is used by the image
  `HEALTHCHECK`.

Do not publish port `6197` to the public network.

## Docker API security

Access to the Docker socket is highly privileged: a process that can reach it
can do anything the Docker daemon can, including starting a privileged
container that mounts the host filesystem. There is no partial-permission
mode on the socket itself — you either can reach it or you can't.

The examples in this repo mount the socket directly into Pingate, read-only
on the bind mount, and run the container as root:

```yaml
user: root
environment:
  PINGATE_DOCKER_HOST: unix:///var/run/docker.sock
volumes:
  - /var/run/docker.sock:/var/run/docker.sock:ro
```

Running as root here is deliberate, not an oversight: the socket's owning
group differs across hosts (a Linux `docker` group GID, `root` (gid 0) on
Docker Desktop for Mac, something else again on Docker Desktop for
Windows/WSL), so chasing it with `group_add` is fragile and breaks silently
between environments. Mounting the socket at all already grants
root-equivalent power over the host regardless of which UID the container
runs as, so gating that behind a non-root UID buys no real security — it
just adds a portability problem. `read_only: true`, `cap_drop: ALL`, and
`no-new-privileges` still apply and are worth keeping.

`:ro` on the mount only stops Pingate from replacing the socket file itself —
it does not restrict which Docker API calls Pingate can make once connected.
If Pingate is ever compromised (a bug, a malicious dependency, a container
escape elsewhere on the host), the attacker gets full, unrestricted Docker
Engine API access, which is roughly equivalent to root on the host.

### When to use a socket-proxy sidecar instead

For production deployments — especially anything internet-facing,
multi-tenant, or where Pingate shares a host with workloads you don't fully
trust — put a restricted, read-only API proxy between Pingate and the daemon
instead of mounting the socket directly. A proxy such as
[`lscr.io/linuxserver/socket-proxy`](https://github.com/linuxserver/docker-socket-proxy)
(based on [Tecnativa/docker-socket-proxy](https://github.com/Tecnativa/docker-socket-proxy))
sits in its own container, is the only thing that touches the real socket,
and forwards only an explicit allowlist of read-only endpoints (list
containers/services, inspect networks, watch events) — nothing else. That
puts a real process boundary between an attacker who compromises Pingate and
the Docker daemon, which a same-process allowlist inside Pingate itself could
never provide.

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
      # SERVICES: 1   # add these two in Swarm mode
      # TASKS: 1
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks:
      - control
    read_only: true
    tmpfs:
      - /run

  pingate:
    # ...
    environment:
      PINGATE_DOCKER_HOST: http://docker-proxy:2375
    depends_on:
      - docker-proxy
    networks:
      - control
      - public
```

With this setup Pingate no longer needs the socket volume or `root` at all —
it can run as its image's default non-root UID/GID `65532` and only needs
network access to `docker-proxy` on the internal `control` network, which is
never published externally. This is the setup to prefer once you're running
in production.

## Development

```shell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
docker build -t pingate:local .
```

Pushing a semver git tag (`vX.Y.Z`) triggers the release workflow, which
publishes `linux/amd64` and `linux/arm64` images to
`ghcr.io/bardiayaghmaie/pingate` tagged `X.Y.Z`, `X.Y`, the commit SHA, and
`latest`.
