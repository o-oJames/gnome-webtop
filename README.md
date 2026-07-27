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

## Included Software

- GNOME Shell / GNOME Flashback
- GNOME Terminal, Nautilus, Settings, Tweaks
- Firefox can be added: `apt-get install -y firefox`
