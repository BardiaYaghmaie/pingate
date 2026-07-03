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

The example uses a restricted Docker socket proxy. Only TCP port `6198` is
published. The health listener on TCP `6197` remains internal.

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

Access to the Docker socket is highly privileged. The examples place a
read-only API proxy between Pingate and the daemon. Pingate also supports a
direct socket mount for controlled environments:

```yaml
environment:
  PINGATE_DOCKER_HOST: unix:///var/run/docker.sock
volumes:
  - /var/run/docker.sock:/var/run/docker.sock:ro
```

The image runs as UID/GID `65532`; direct socket deployments must grant that
process access to the socket without making the container privileged.

## Development

```shell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
docker build -t pingate:local .
```

Release tags publish `linux/amd64` and `linux/arm64` images to
`ghcr.io/bardiayaghmaie/pingate`.
