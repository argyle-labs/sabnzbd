# SABnzbd

Web-based Usenet (NZB) client.

- **Port**: `8080` (web UI)
- **Image**: `lscr.io/linuxserver/sabnzbd:latest`
- **Compose**: [../compose.yml](../compose.yml)
- **Upstream**: <https://sabnzbd.org/>

## Run

See the [README](../README.md) for Docker Compose, Podman, LXC, VM, and Unraid
instructions. In short:

```sh
docker compose up -d
```

The web UI is then available on port `8080`.

## Volumes

| Container Path | Description |
|----------------|-------------|
| `/config` | SABnzbd configuration and state |
| `/downloads` | Working directory for in-progress and completed NZB content |

Map both to persistent host paths (or named volumes). The `/config` volume holds
the entire service state — back it up.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TZ` | `Etc/UTC` | Timezone |
| `PUID` / `PGID` | `1000` | User/group ID the container runs as |

Consult the [linuxserver.io image docs](https://docs.linuxserver.io/images/docker-sabnzbd/)
for the full set of supported variables and tags.

## Backup & restore

Back up the `/config` volume — that is the whole service state. Stop the
container first for a clean copy, then restore by putting the volume back and
starting the container again.

## Troubleshooting

```sh
# Container logs
docker logs sabnzbd

# Confirm the web UI is reachable
curl -fsS http://localhost:8080/ >/dev/null && echo ok
```
