# Pingate

Pingate is a small Pingora-based round-robin load balancer. It proxies
HTTP requests to upstream servers and uses TCP health checks to avoid unhealthy upstreams.

## Requirements

- Rust stable
- Python 3.14

## Run the demo

Install the Python demo dependencies:

```shell
python3.14 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

Start two upstream servers in separate terminals:

```shell
source .venv/bin/activate
PINGATE_INSTANCE=upstream-1 panther run --port 8000 --reload
```

```shell
source .venv/bin/activate
PINGATE_INSTANCE=upstream-2 panther run --port 8001 --reload
```

Run Pingate:

```shell
cargo run
```

Send a few requests through the load balancer:

```shell
curl 127.0.0.1:6198
curl 127.0.0.1:6198
curl 127.0.0.1:6198
```

The JSON response includes an `instance` field. Repeated requests should move
between `upstream-1` and `upstream-2`.

For request-level logs, run with:

```shell
RUST_LOG=pingate=debug cargo run
```

## Configuration

The default config lives at `config/config.toml`:

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

`upstream_tls` should stay `false` for the local Panther demo servers. Set it
to `true` and provide `upstream_sni` only when proxying to HTTPS upstreams.

## Checks

```shell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

The MkDocs site can be previewed with:

```shell
mkdocs serve -f docs/mkdocs.yml
```
