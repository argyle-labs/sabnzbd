<p align="center">
  <img src="assets/icon-256.png" width="120" alt="sabnzbd" />
</p>

# sabnzbd

SABnzbd is a web-based Usenet (NZB) binary downloader.

A first-party [orca](https://github.com/argyle-labs/orca) plugin (service-backend).

This repo is **self-contained** — the steps below run sabnzbd **by hand, without orca**. orca automates exactly this (same image, ports, and data) through one generic surface.

---

## Run it without orca

### Docker Compose

```yaml
# compose.yml
services:
  sabnzbd:
    image: lscr.io/linuxserver/sabnzbd:latest
    container_name: sabnzbd
    restart: unless-stopped
    ports:
      - "8080:8080/tcp"   # web UI
    volumes:
      - ./config:/config
      - /path/to/downloads:/downloads
```

```sh
docker compose up -d
```

### Other runtimes

**Podman** — the compose above works with `podman compose up -d`, or run it directly:

```sh
podman run -d --name sabnzbd --restart unless-stopped \
    -p 8080:8080/tcp \
    -v ./config:/config \
    -v /path/to/downloads:/downloads \
    lscr.io/linuxserver/sabnzbd:latest
```

**LXC** — on a container-capable LXC (e.g. a Proxmox LXC with nesting enabled) run the same image via Docker/Podman as above, or install sabnzbd from upstream directly on the guest: <https://sabnzbd.org/>.

**VM** — install sabnzbd from upstream (<https://sabnzbd.org/>) or run the same container image inside the VM; expose port `8080`.

**Unraid** — add via *Community Applications*, or *Docker → Add Container* with image `lscr.io/linuxserver/sabnzbd:latest`, port `8080`, and the volume paths above.

### Ports & data

| | |
|---|---|
| Default port | `8080` |
| Upstream | <https://sabnzbd.org/> |
| Operator notes | [sabnzbd.md](docs/sabnzbd.md) |


### Backup & restore

Back up the config/data volume(s) above — that's the whole service state (stop the container first for a clean copy). Restore by putting them back and starting it.

> With orca this is **`service.backup` / `service.restore`** — location-agnostic (docker / podman / lxc / vm), one command regardless of where sabnzbd runs. No per-service backup script.

## With orca

orca drives this plugin through the single generic `service.*` surface — no per-plugin tools:

```sh
orca service.deploy sabnzbd      # render + launch on any supported runtime
orca service.status sabnzbd      # health + rich diagnostics (typed payload)
orca service.backup sabnzbd      # location-agnostic backup (tar; PBS on Proxmox)
orca service.configure sabnzbd   # apply config via the upstream API
```

## Layout

- `src/` — the plugin (pure Rust): the `ServiceBackend` descriptor + `configure` / `status`.
- `docs/` — standalone operator notes.
- [CAPABILITIES.md](CAPABILITIES.md) — the service-backend contract checklist.
- `assets/` — plugin icon.
