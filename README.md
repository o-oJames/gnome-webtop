# GNOME Webtop

A full GNOME desktop environment accessible from any web browser via noVNC.

## Quick Start

```bash
# Build and start
docker compose up -d --build

# Open in browser
open http://localhost:6901/vnc.html
```

**Default VNC password:** `password`

## Configuration

Set via environment variables in `docker-compose.yml`:

| Variable           | Default       | Description              |
|--------------------|---------------|--------------------------|
| `VNC_PASSWORD`     | `password`    | VNC/noVNC login password |
| `VNC_RESOLUTION`   | `1920x1080`   | Desktop resolution       |
| `VNC_DEPTH`        | `24`          | Color depth (bits)       |
| `MOUNT_MANAGER_AUTOMOUNT` | `0`    | `1` = mount saved network shares at startup |

### Mounting inside the container

Docker blocks `mount(2)` by default, so `docker-compose.yml` adds the
capabilities Mount Manager needs for *system* mounts, exposes `/dev/fuse` for
SSHFS/FUSE mounts, and disables the seccomp/AppArmor profiles that would
otherwise refuse them:

```yaml
cap_add: [SYS_ADMIN, DAC_OVERRIDE, DAC_READ_SEARCH, SETUID, SETGID, CHOWN]
devices: [/dev/fuse]
security_opt: [seccomp=unconfined, apparmor=unconfined]
```

Remove that block if you would rather the container could not mount anything —
Mount Manager still works, but only with user-session (GVFS) mounts, which need
no kernel privileges.

## Architecture

```
Browser ──HTTP──▶ noVNC (:6901) ──WebSocket──▶ Xvnc (:5901) ──▶ GNOME Shell
                  (websockify)                  (TigerVNC)       (llvmpipe SW render)
```

- **Xvnc** — virtual X11 framebuffer (no physical display needed)
- **noVNC + websockify** — HTML5 VNC client served over HTTP
- **GNOME Shell** — full desktop, software-rendered via Mesa llvmpipe
- **Fallback** — automatically drops to GNOME Flashback (Metacity) if Shell fails

## GPU Acceleration (optional)

If you have an Intel/AMD GPU on the host, uncomment the `devices` block in
`docker-compose.yml` and set `LIBGL_ALWAYS_SOFTWARE=0` for hardware rendering.

## Troubleshooting

```bash
# View logs
docker compose logs -f

# Check GNOME Shell log inside container
docker exec -it gnome-webtop cat /home/vncuser/.gnome-shell.log

# Reset desktop (restart container)
docker compose restart

# Full rebuild
docker compose down && docker compose up -d --build
```

## Mount Manager (built from source in this repo)

[`mount-manager/`](mount-manager) is a Rust + GTK4/libadwaita application that
manages mounted external paths. The image build compiles it into a `.deb`,
publishes a **local APT repository** at `/opt/mount-manager/repo` and installs
it with `apt-get install mount-manager` — so it behaves like any other package:

```bash
docker compose exec gnome-webtop apt-cache policy mount-manager
docker compose exec gnome-webtop sudo apt-get install --reinstall mount-manager
```

In the desktop: press <kbd>Super</kbd> and type **Mount Manager** (it is also
pinned to the dash). It handles SMB/CIFS, NFS, SSHFS/SFTP, WebDAV, FTP, USB
drives and bind mounts, and it asks for the administrator password inside the
window when root rights are needed.

```bash
# …or drive it from a terminal
docker compose exec gnome-webtop mount-manager list --all
docker compose exec gnome-webtop mount-manager devices
docker compose exec gnome-webtop mount-manager mount smb://nas/data /mnt/data --user joe
docker compose exec gnome-webtop mount-manager info
```

See [`mount-manager/README.md`](mount-manager/README.md) for the full
documentation: features, protocol support, packaging, security model and the
complete CLI reference.

## Included Software

- GNOME Shell / GNOME Flashback
- GNOME Terminal, Nautilus, Settings, Tweaks
- **Mount Manager** — GUI + CLI for SMB/NFS/SSHFS/WebDAV/FTP/USB mounts
- Firefox (from the Mozilla APT repository)
