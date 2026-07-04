# Todo

Prioritized by expected usefulness to a general-purpose reverse proxy. Items
within each section are ordered from most to least important.

## P0 — Core routing and proxy correctness

- [ ] Add host-and-path routing for Docker-backed modes, with exact,
  segment-aware prefix, and regular-expression path matchers (for example,
  `/metrics`, `/ws/`, and `/admin`).
- [ ] Define deterministic route precedence: exact paths before prefixes,
  longest matching prefix first, explicit priority for regular expressions,
  and a host-only fallback route.
- [ ] Add path transformations, including strip-prefix, add/replace-prefix,
  and parameterized URL rewrites while preserving query strings correctly.
- [ ] Add configurable upstream request and response header manipulation,
  equivalent to Nginx `proxy_set_header`, including safe defaults for
  `Host`, `X-Forwarded-For`, `X-Forwarded-Host`, and `X-Forwarded-Proto`.
- [ ] Add explicit WebSocket proxy support, including HTTP/1.1 upgrade and
  `Connection`/`Upgrade` header handling.
- [ ] Add configurable per-route upstream connect/read/write timeouts and
  bounded retry behavior.
- [ ] Add passive upstream failure detection so connection errors and selected
  response statuses temporarily remove unhealthy peers from rotation.
- [ ] Add a configurable maximum request body size equivalent to
  `client_max_body_size`.
- [ ] Allow one discovered workload to declare multiple routed hostnames.
- [ ] Add hostname-and-path routing for static mode, or document that advanced
  routing remains exclusive to Docker-backed modes.

## P1 — Routing policy and resilience

- [ ] Add composable route matching by HTTP method, request headers, and query
  parameters in addition to host and path.
- [ ] Add source IP access controls with allow/deny rules and CIDR support.
- [ ] Add per-route rate limiting.
- [ ] Add per-route authentication hooks, beginning with basic auth and an
  external forward-auth endpoint.
- [ ] Add configurable load-balancing policies beyond round-robin, including
  least-connections and sticky sessions.
- [ ] Add configurable HTTP redirects, including scheme, host, and path
  redirects with permanent or temporary status codes.
- [ ] Add reusable, ordered middleware chains for route-level request and
  response processing.

## P2 — Observability and operations

- [ ] Add configurable structured access logging, including request ID,
  selected route, upstream timing, address, and status fields.
- [ ] Add a proxy statistics/metrics endpoint comparable to Nginx
  `stub_status`, preferably with Prometheus-compatible metrics.
- [ ] Add per-route custom error responses and optional interception of
  selected upstream status codes.

## P3 — Content-serving conveniences

- [ ] Add response compression middleware.
- [ ] Add static-file serving or aliases for paths such as `/static/` and
  `/robots.txt`.
