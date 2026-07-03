# AGENTS.md

Guidance for AI coding agents (and humans) working in this repository. Keep
this file in sync with the code — whenever a change touches settings, labels,
modes, ports, or the deployment manifests, update the relevant section here in
the same commit.

## What this project is

Pingate is a Docker-native HTTP reverse proxy and load balancer written in
Rust on top of [Pingora](https://github.com/cloudflare/pingora). It has no
config file — everything is driven by `PINGATE_*` environment variables — and
runs in one of three mutually exclusive modes:

- **`static`** — load-balances across a fixed, comma-separated list of
  upstream addresses with periodic TCP health checks. No Docker involved.
- **`compose`** — discovers upstreams by polling the Docker Engine API for
  containers carrying `pingate.*` labels (single-host Compose deployments).
- **`swarm`** — discovers upstreams by polling Swarm services carrying
  `pingate.*` labels, then resolves running replicas via Docker's embedded
  DNS (`tasks.<service>`).

In `compose`/`swarm` modes, Pingate reconciles routes on a timer
(`PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS`) and on Docker events, so routing
follows container starts/stops/scaling without a restart.

## Source layout

| File | Responsibility |
|---|---|
| [src/main.rs](src/main.rs) | Entry point; builds the Pingora server, wires up the static or Docker-backed proxy service, starts the admin service, and implements the `pingate healthcheck` subcommand. |
| [src/settings.rs](src/settings.rs) | Parses and validates all `PINGATE_*` env vars into a `Settings` struct. This is the single source of truth for configuration — see its tests for exact validation rules. |
| [src/docker.rs](src/docker.rs) | `DockerDiscovery`: connects to the Docker Engine (via `bollard`), lists containers/services, filters by `pingate.*` labels, resolves upstream addresses, and republishes a `RouteTable`. Runs on its own dedicated OS thread with its own current-thread Tokio runtime (kept separate from Pingora's runtime). |
| [src/routes.rs](src/routes.rs) | `RouteTable`: thread-safe host → upstream-list map with round-robin selection. `normalize_host` validates/canonicalizes Host headers (rejects schemes, paths, wildcards, underscores, etc). |
| [src/admin.rs](src/admin.rs) | Internal admin HTTP app serving `/healthz` (liveness) and `/readyz` (readiness — true only after the first successful discovery sync in Docker modes, or immediately in static mode). |

There is no `config/` file consumed at runtime — the `config/` directory in
this repo is currently empty; do not add a TOML/YAML config loader without
discussing it, since "no config file, env-only" is a deliberate design choice
documented in the [Readme](Readme.md).

## Configuration reference

Keep this table in sync with [src/settings.rs](src/settings.rs) and the
[Readme.md](Readme.md) table — they must always agree.

| Variable | Default | Notes |
|---|---|---|
| `PINGATE_MODE` | *required* | `static`, `compose`, or `swarm`. |
| `PINGATE_LISTEN_ADDR` | `0.0.0.0:6198` | Public proxy listener. |
| `PINGATE_ADMIN_LISTEN_ADDR` | `0.0.0.0:6197` | Internal health/ready listener. Must differ from `PINGATE_LISTEN_ADDR`. Never publish this externally. |
| `PINGATE_STATIC_UPSTREAMS` | none | Comma-separated `host:port` list. Required (and only used) in `static` mode. |
| `PINGATE_STATIC_HEALTH_CHECK_INTERVAL_SECONDS` | `5` | TCP health check cadence for static upstreams. |
| `PINGATE_STATIC_UPSTREAM_TLS` | `false` | Speak TLS to static upstreams. |
| `PINGATE_STATIC_UPSTREAM_SNI` | empty | SNI for static upstream TLS. |
| `PINGATE_DOCKER_HOST` | `unix:///var/run/docker.sock` | Must start with `unix://`, `http://`, or `https://`. The example manifests point this at the raw socket by default; point it at a `docker-proxy` sidecar (`http://docker-proxy:2375`) for a hardened production setup — see "Docker API security model" below. |
| `PINGATE_DOCKER_RECONNECT_INTERVAL_SECONDS` | `2` | Delay before retrying a dropped Docker connection/event stream. |
| `PINGATE_DOCKER_RESYNC_INTERVAL_SECONDS` | `30` | Full reconciliation interval, independent of the event stream (belt-and-suspenders). |
| `PINGATE_DOCKER_DEFAULT_NETWORK` | none | Fallback Docker network used to pick a container/task's routable IP when the `pingate.network` label is absent. See "Networking" below. |
| `PINGATE_DOCKER_TLS_CA_PATH` / `_CERT_PATH` / `_KEY_PATH` | none | Client TLS for a remote Engine API. All three or none — partial sets are rejected at startup. |
| `RUST_LOG` | `info` | Standard `env_logger` filter. |

## Workload labels (`compose`/`swarm` modes)

Defined as constants in [src/docker.rs](src/docker.rs):
`ENABLE_LABEL`/`HOST_LABEL`/`PORT_LABEL`/`NETWORK_LABEL` →
`pingate.enable` / `pingate.host` / `pingate.port` / `pingate.network`.

```yaml
labels:
  pingate.enable: "true"
  pingate.host: api.example.com
  pingate.port: "8000"
  pingate.network: pingate-public
```

- `pingate.enable` must literally be `"true"` (case-insensitive); anything
  else (including absent) excludes the workload.
- `pingate.host` becomes the routable Host header, normalized/validated by
  `normalize_host` (lowercased, port stripped, must look like a hostname).
- `pingate.port` is the workload's internal TCP port Pingate connects to.
- `pingate.network` is optional and only matters for **networking** (below).
- Compose labels go under `services.<name>.labels`; Swarm labels must go
  under `services.<name>.deploy.labels` (Swarm ignores top-level labels for
  routing purposes since Pingate reads them off the *service spec*, not the
  container).

### Networking: `pingate.network` vs. `PINGATE_DOCKER_DEFAULT_NETWORK`

A container/service can be attached to multiple Docker networks, each giving
it a different IP — Pingate must know which one it can actually reach.
Resolution order (`route_metadata` in [src/docker.rs:339](src/docker.rs)):

1. Use the `pingate.network` label if set on that workload.
2. Otherwise fall back to `PINGATE_DOCKER_DEFAULT_NETWORK` if set.
3. Otherwise, if the workload has exactly one network, use it automatically.
4. Otherwise, fail with `MissingNetwork` and skip the workload (logged as a
   warning, not fatal to the process).

Practical rule: set `PINGATE_DOCKER_DEFAULT_NETWORK` once to your shared
routable network (e.g. `pingate-public`), and only add the per-workload
`pingate.network` label as an override when a specific service also joins a
second network (e.g. a private DB network) where the default wouldn't apply.

## Docker API security model

The example manifests ([docker-compose.yaml](docker-compose.yaml),
[docker-stack.yaml](docker-stack.yaml)) mount `/var/run/docker.sock`
**directly** into the Pingate container (read-only bind, `unix://` scheme).
This is the simple default; it is not the least-privileged option.

- The socket has no partial-permission mode — anything that can reach it can
  do anything the Docker daemon can do. `:ro` on the bind mount only stops
  Pingate from replacing the socket *file*; it does not restrict which
  Docker API calls Pingate makes once connected.
- Pingate's image runs as non-root UID/GID `65532` by default (see
  [Dockerfile](Dockerfile)), but the direct-socket-mount manifests override
  that with `user: root`. This is deliberate: the socket's owning group
  differs across hosts (a Linux `docker` group GID vs. `root`/gid 0 on Docker
  Desktop for Mac vs. something else on Docker Desktop for Windows/WSL), so
  matching it via `group_add` is fragile and breaks silently between
  environments — see the incident notes below. Since mounting the socket at
  all already grants root-equivalent host access regardless of the
  container's UID, gating that behind a non-root UID buys nothing real; it
  only trades a portability problem for the appearance of hardening.
- The Pingate container is otherwise still hardened: `read_only: true`,
  `cap_drop: ALL`, `no-new-privileges`.
- **Do not reintroduce `group_add`/`DOCKER_GID`-based non-root access for the
  direct-socket-mount path.** It was tried and reverted after failing on
  Docker Desktop for Mac: `getent` doesn't exist there, so `DOCKER_GID`
  silently resolved empty, fell back to a guessed default GID, and Pingate
  got `EACCES` connecting to the socket (surfaced as a generic hyper
  `client error (Connect)`, not an obviously-permissions error). The `root`
  socket-proxy sidecar path is the correct place to preserve a non-root UID,
  because there the container only ever gets a restricted read-only API
  surface — the hardening is real there, not cosmetic.

**Production alternative — socket-proxy sidecar.** For internet-facing or
multi-tenant deployments, put a restricted read-only API proxy (e.g.
`lscr.io/linuxserver/socket-proxy`, based on Tecnativa/docker-socket-proxy)
in its own container between Pingate and the daemon instead. It's the only
thing that mounts the real socket, exposes an HTTP allowlist on `:2375` gated
by env flags (`CONTAINERS`, `EVENTS`, `INFO`, `NETWORKS`, `PING`, `VERSION`,
plus `SERVICES`/`TASKS` for Swarm), and Pingate then talks to it over an
internal network via `PINGATE_DOCKER_HOST: http://docker-proxy:2375` with no
socket volume of its own. This is documented with a full example in the
Readme's "Docker API security" section — **do not re-inline the sidecar's
filtering logic into Pingate's own binary**: the security value comes from
enforcement happening in a separate process/container, so a Pingate
compromise can't just skip its own allowlist. If you fold that logic into the
same binary that holds the socket fd, you delete the actual security boundary
while keeping only its cosmetic effect.

## Health/readiness

- `GET /healthz` on `PINGATE_ADMIN_LISTEN_ADDR` (default `:6197`) — process
  liveness, always `200` once the process is up.
- `GET /readyz` — `200` once `ready` flips true. In `static` mode this is
  immediate; in `compose`/`swarm` mode it flips only after the first
  successful discovery refresh ([src/docker.rs:186-190](src/docker.rs)).
- `pingate healthcheck` (see `run_healthcheck` in
  [src/main.rs](src/main.rs)) is a small built-in HTTP client used as the
  Docker `HEALTHCHECK` command — it hits `/readyz` on
  `PINGATE_ADMIN_LISTEN_ADDR` and exits non-zero on anything but `200`.

## Development workflow

```shell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
docker build -t pingate:local .
```

Rust toolchain is pinned via [rust-toolchain.toml](rust-toolchain.toml)
(`1.95.0`); keep it and the Dockerfile's `rust:1.95.0-slim-bookworm` builder
image and the CI `dtolnay/rust-toolchain@1.95.0` step in lockstep when
bumping.

Tests live inline as `#[cfg(test)] mod tests` in each source file — prefer
adding cases there over new integration test files, following existing
patterns (e.g. `settings::from_lookup` takes an injectable env lookup closure
specifically so tests don't need real env vars; `docker.rs` tests build
`ContainerSummary`/`Task` fixtures directly rather than mocking the Docker
API).

## CI (`.github/workflows/ci.yml`)

Four jobs gate `master`/PRs — keep new functionality covered by the
appropriate one:

- **rust** — fmt, clippy (deny warnings), `cargo test --locked`, `cargo
  audit`.
- **compose** — builds the image, brings up [docker-compose.yaml](docker-compose.yaml),
  and asserts round-robin across `api1`/`api2`.
- **image** — builds the release image, checks it runs as non-root, and
  scans it with Trivy (fails on unfixed CRITICAL/HIGH CVEs).
- **swarm** — deploys [docker-stack.yaml](docker-stack.yaml) to a local
  single-node swarm, verifies routing, then verifies scaling
  (`docker service scale`) and a rolling update (`docker service update
  --force`) don't break routing.

## Release (`.github/workflows/release.yml`)

Tags matching `v*` build and push multi-arch (`linux/amd64`,
`linux/arm64`) images to `ghcr.io/bardiayaghmaie/pingate` with SBOM +
provenance attestation. Don't hand-push images to that registry outside this
workflow.

## Conventions for changes in this repo

- Any new `PINGATE_*` env var or `pingate.*` label must be added to **both**
  [Readme.md](Readme.md) and this file's tables in the same change.
- Settings validation errors should stay descriptive and mention the exact
  env var name (see the `SettingsError` messages in `settings.rs`) — Pingate
  fails fast and loudly at startup rather than silently defaulting.
- Docker discovery failures for a single workload (bad labels, missing
  network, unresolvable DNS) must be logged and skip only that workload —
  never let one misconfigured container take down the whole route table
  refresh (see the `warn!`-and-continue pattern throughout `docker.rs`).
- `RouteSelection` intentionally distinguishes `Unknown` (404 — no such host
  configured) from `Unavailable` (503 — host configured but zero healthy
  upstreams) from `Available`. Preserve this distinction in any routing
  changes; don't collapse it to a single error case.
