# Scan-on-import dispatcher (orca)

**Fleet principle:** when a downloader finishes a file, the server that *hosts*
that media type is told to scan immediately — never left to a periodic rescan.
Media lives on the willow NFS/SMB share, so the content servers' filesystem
watchers do **not** fire; a push is required.

The **arr apps** have native connectors (Radarr/Sonarr → Plex/Jellyfin;
Lidarr → Navidrome via Custom Script — see [navidrome docs](../../navidrome/docs/navidrome.md)).
The **non-arr** managers (LazyLibrarian, Mylar3, Kapowarr) and Libation have no
usable per-import hook, so the push is driven from the **download clients**
instead, keyed by the download **category**.

## Mechanism

A single dispatcher script runs on download completion in each client:

- **SABnzbd** — `script_dir=/config/scripts`; the `books` category's `script` is
  set to `orca-scan-dispatch.py`. SAB passes the category as `argv[5]` / `$SAB_CAT`.
- **qBittorrent** — `[AutoRun]` enabled, program
  `python3 /config/scripts/orca-scan-dispatch.py "%L"` (`%L` = category).

The script is installed in each client container at
`/config/scripts/orca-scan-dispatch.py` with a sibling `orca-scan.env` (mode
`600`, server URLs + creds). Library IDs are not secret and live in the script.

### Category → server map

| Category | Servers scanned | Why |
|----------|-----------------|-----|
| `books` | Audiobookshelf (Audiobooks) + Kavita (Books) | SAB has one `books` category for e/audiobooks; over-scan is harmless |
| `audiobooks` | Audiobookshelf | explicit audiobook category, if used |
| `comics` | Komga (comics+manga) + Kavita (Comics) | |
| `manga` | Komga (manga) + Kavita (Comics) | |
| `music` | — (handled by Lidarr→Navidrome connector) | avoid double-trigger |
| `movies`/`tv`/`4k` | — (handled by Radarr/Sonarr→Plex/Jellyfin) | no-op |

Scans are idempotent, so over-scanning is safe. The script no-ops unmapped
categories, so it is safe to attach to every category / all torrents.

## Per-server scan APIs

- **Audiobookshelf** (`http://10.0.0.6:13378`): `POST /login` → `user.token`,
  then `POST /api/libraries/{id}/scan` (Bearer). Libs: Audiobooks
  `d1d6751d-cf80-4746-b301-c03b3ae1fe89`, podcasts `13ba8800-1315-4363-84e2-7956fa118d23`.
- **Komga** (`http://10.0.0.6:25600`): Basic auth, `POST /api/v1/libraries/{id}/scan`.
  Libs: comics `0PS084DCHJ1HH`, manga `0PS0877ZDJ0AZ`.
- **Kavita** (`http://10.0.0.6:5000`): `POST /api/Account/login` → JWT, then
  `POST /api/Library/scan?libraryId={id}`. Libs: Books `2`, Comics `1`.
- **Navidrome** (music): Subsonic `startScan` — see navidrome docs (wired via Lidarr, not this dispatcher).
- **Calibre-Web**: no scan API — it reads the Calibre DB live. New books appear
  when added to the Calibre library (LazyLibrarian → `calibredb`); no push needed.

## Credentials

Server creds are in the **orca 1Password vault**: `audiobookshelf (orca)`,
`komga (orca)`, `kavita (orca)`, `navidrome (orca)`. The only on-disk copies are
the per-client `orca-scan.env` files (`600`, app-uid owned).

## Known gaps / follow-ups

- **Category coverage on SAB**: only the `books` category has the script attached
  (the only book-ish SAB category present). If Mylar3/Kapowarr download comics via
  usenet, add a `comics` SAB category with `script=orca-scan-dispatch.py`.
  qBittorrent's AutoRun already fires for every torrent regardless of category.
- **Libation** (Audible audiobooks on baldur) bypasses both download clients —
  needs its own post-download hook to trigger an ABS scan (not yet wired).
- **Managers must tag** their downloads with the matching category for the map to fire.

## Future plugin capability

This dispatcher is the interim, hand-installed form of a first-class orca
capability: content-server plugins expose a `scan`/`configure` action, and the
manager/download-client plugins register a scan-on-import hook. See each server
plugin's `CAPABILITIES.md`. The category→server map and per-server scan calls
here are the spec for that code.
