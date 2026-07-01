# SABnzbd

Usenet downloader.

- **Host**: <host> — see [Network Map](../network/network-map.md)
- **Port**: 8080
- **Image**: `lscr.io/linuxserver/sabnzbd`
- **Compose**: [compose/sabnzbd/docker-compose.yml](../../compose/sabnzbd/docker-compose.yml)
- **Network**: `media`
- **VPN**: All traffic routes through <vpn-provider> region-a via OPNsense. Killswitch blocks internet if VPN is down.

## Deploy

Deployed as a Portainer Git stack pointing at the `main` branch. Portainer polls GitHub every 5 minutes and redeploys on changes.

To manually redeploy: Portainer → Stacks → sabnzbd → Pull and redeploy.

## Volumes

| Host Path | Container Path | Description |
|-----------|---------------|-------------|
| `/opt/appdata/sabnzbd` | `/config` | SABnzbd config |
| `/mnt/<host>/downloads/incomplete` | `/downloads/incomplete` | In-progress downloads (NFS) |
| `/mnt/<host>/downloads/completed` | `/downloads/completed` | Finished downloads (NFS) |

NFS mounts on <host> are defined in `/etc/fstab` and managed by the `netmount` OpenRC service. See [nfs-alpine.md](../host-setup/nfs-alpine.md).

## Environment Variables

| Variable                    | Default          | Description               |
|-----------------------------|------------------|---------------------------|
| `TZ`                        | `Etc/UTC` | Timezone                  |
| `PUID` / `PGID`             | `1000`           | User/group ID             |
| `SABNZBD_IMAGE_TAG`         | `latest`         | Image tag                 |
| `SABNZBD_CONFIG_PATH`       | `./config`       | Config directory          |
| `DOWNLOADS_INCOMPLETE_PATH` | *(required)*     | Incomplete downloads path |
| `DOWNLOADS_COMPLETED_PATH`  | *(required)*     | Completed downloads path  |

## Troubleshooting

```bash
docker logs sabnzbd

# Check NFS mounts are active
mount | grep <host>

# Test write access
touch /mnt/<host>/downloads/completed/test && rm /mnt/<host>/downloads/completed/test
```
