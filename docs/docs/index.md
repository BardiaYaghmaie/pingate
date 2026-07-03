# Pingate

Pingate is an environment-configured reverse proxy for Docker Compose and
Docker Swarm. It listens for public traffic on TCP `6198` and exposes internal
liveness and readiness checks on TCP `6197`.

## Compose

```shell
docker compose up --build --detach --wait
curl -H 'Host: api.localhost' http://127.0.0.1:6198
```

Routed containers require `pingate.enable`, `pingate.host`, `pingate.port`, and
normally `pingate.network` labels. Pingate refreshes routes after Docker events
and periodically, without requiring a restart.

## Swarm

```shell
docker network create --driver overlay --attachable pingate-public
docker stack deploy --compose-file docker-stack.yaml pingate
```

For Swarm, put routing labels under `deploy.labels`. Pingate runs with manager
API access and resolves running replicas over the shared overlay network.

## Health

- `/healthz` reports process liveness.
- `/readyz` becomes successful after the initial discovery reconciliation.
- `pingate healthcheck` is available for container health checks.

The admin port is intentionally not published by the supplied deployment
examples.

See the repository README for the complete environment and label reference.
