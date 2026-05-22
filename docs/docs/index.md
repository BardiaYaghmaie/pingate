# Pingate

Pingate is a compact Rust load balancer demo built with Pingora. It accepts
HTTP traffic on `0.0.0.0:6198`, selects from two local upstreams with
round-robin balancing, and runs TCP health checks in the background.

## Demo Flow

Create and activate a Python environment:

```shell
python3.12 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Start the first upstream:

```shell
source .venv/bin/activate
PINGATE_INSTANCE=upstream-1 panther run --port 8000 --reload
```

Start the second upstream in another terminal:

```shell
source .venv/bin/activate
PINGATE_INSTANCE=upstream-2 panther run --port 8001 --reload
```

Run Pingate:

```shell
cargo run
```

Send repeated requests through the proxy:

```shell
curl 127.0.0.1:6198
curl 127.0.0.1:6198
```

Each response includes an `instance` value from the selected upstream, so the
round-robin behavior is easy to see.

## Configuration

The demo configuration is stored in `config/config.toml`:

```toml
[server]
listen_addr = "0.0.0.0:6198"

[upstreams]
addresses = ["127.0.0.1:8000", "127.0.0.1:8001"]
health_check_frequency = 1

[proxy]
upstream_tls = false
upstream_sni = ""
```

`upstream_tls = false` matches the local Panther servers. For HTTPS upstreams,
set `upstream_tls = true` and provide the expected SNI hostname in
`upstream_sni`.

## Development Checks

```shell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Preview these docs locally:

```shell
mkdocs serve -f docs/mkdocs.yml
```
