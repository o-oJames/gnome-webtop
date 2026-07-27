#!/bin/bash
set -e

# ─── Configuration ───────────────────────────────────────────────────
VNC_PASS="${VNC_PASSWORD:-password}"
RES="${VNC_RESOLUTION:-1920x1080}"
VNC_DEPTH="${VNC_DEPTH:-24}"
DISPLAY_NUM=":1"
VNC_PORT=5901
NOVNC_PORT=6901
export DISPLAY=${DISPLAY_NUM}

# ─── Mesa / GL — software rendering via llvmpipe ─────────────────────
export LIBGL_ALWAYS_SOFTWARE=1
export GALLIUM_DRIVER=llvmpipe
export MUTTER_ALLOW_SOFTWARE_RENDERING=1
export COGL_RENDERER=glx

# ─── Cursor theme (must be set before X clients start) ───────────────
export XCURSOR_THEME=DMZ-White
export XCURSOR_SIZE=24

# ─── XDG / GNOME Environment ─────────────────────────────────────────
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export XDG_SESSION_TYPE=x11
export XDG_CURRENT_DESKTOP=ubuntu:GNOME
export XDG_SESSION_DESKTOP=ubuntu
export XDG_CONFIG_DIRS=/etc/xdg/xdg-ubuntu:/etc/xdg
export GNOME_SHELL_SESSION_MODE=ubuntu

echo "═══════════════════════════════════════════════"
echo "  GNOME Webtop Container"
echo "  Resolution : ${RES}"
echo "  noVNC      : http://localhost:${NOVNC_PORT}/vnc.html"
echo "═══════════════════════════════════════════════"

# ─── 1. Prerequisites ────────────────────────────────────────────────
sudo mkdir -p "${XDG_RUNTIME_DIR}" /tmp/.ICE-unix /dev/dri
sudo chmod 700 "${XDG_RUNTIME_DIR}"
sudo chmod 1777 /tmp/.ICE-unix
sudo chown "$(id -u):$(id -g)" "${XDG_RUNTIME_DIR}"
mkdir -p ~/Desktop ~/Documents ~/Downloads ~/Music ~/Pictures ~/Videos ~/Templates ~/Public
sudo mknod /dev/dri/card0 c 226 0 2>/dev/null || true
sudo chmod 666 /dev/dri/card0 2>/dev/null || true

# Set default X cursor theme (fixes cross cursor in VNC)
mkdir -p ~/.icons/default
cat > ~/.icons/default/index.theme << 'CURSOR'
[Icon Theme]
Name=Default
Comment=Default Cursor Theme
Inherits=DMZ-White
CURSOR

# Bypass gnome-session GL acceleration check (llvmpipe works but the check fails)
sudo tee /usr/local/bin/gnome-session-check-accelerated > /dev/null << 'STUB'
#!/bin/sh
exit 0
STUB
sudo chmod +x /usr/local/bin/gnome-session-check-accelerated
export PATH="/usr/local/bin:${PATH}"

# ─── 2. System D-Bus ─────────────────────────────────────────────────
sudo mkdir -p /run/dbus
sudo rm -f /run/dbus/pid
sudo dbus-daemon --system --fork
echo "✅ System D-Bus"

# ─── 3. Fake logind (GNOME Shell requires org.freedesktop.login1) ───
python3 /usr/local/bin/fake-logind.py &
FAKE_LOGIND_PID=$!
sleep 1
echo "✅ Fake logind (PID ${FAKE_LOGIND_PID})"

# ─── 4. Session D-Bus ────────────────────────────────────────────────
eval "$(dbus-launch --sh-syntax)"
export DBUS_SESSION_BUS_ADDRESS
export DBUS_SESSION_BUS_PID
echo "✅ Session D-Bus"

# ─── 5. Start Xvfb ──────────────────────────────────────────────────
# Clean up stale files from previous run (container stop/start)
sudo rm -f /tmp/.X1-lock /tmp/.X11-unix/X1

echo "🖥️  Starting Xvfb on ${DISPLAY_NUM} (${RES})..."
Xvfb ${DISPLAY_NUM} \
    -screen 0 "${RES}x${VNC_DEPTH}" \
    +extension GLX \
    +extension RANDR \
    +extension RENDER \
    +extension XFIXES \
    -ac \
    -nolisten tcp &
XVFB_PID=$!

for i in $(seq 1 30); do
    xdpyinfo -display ${DISPLAY_NUM} >/dev/null 2>&1 && break
    sleep 0.5
done
if ! xdpyinfo -display ${DISPLAY_NUM} >/dev/null 2>&1; then
    echo "❌ Xvfb failed to start"; exit 1
fi
echo "✅ Xvfb running (PID ${XVFB_PID})"

# Set the root window cursor to a normal arrow (not cross)
xsetroot -cursor_name left_ptr -display ${DISPLAY_NUM} 2>/dev/null || true

# ─── 6. Verify GL ────────────────────────────────────────────────────
GL_RENDERER=$(glxinfo -display ${DISPLAY_NUM} 2>/dev/null | grep "OpenGL renderer" || echo "unknown")
echo "🔍 ${GL_RENDERER}"

# ─── 7. Start x11vnc ────────────────────────────────────────────────
x11vnc -display ${DISPLAY_NUM} \
    -rfbport ${VNC_PORT} \
    -passwd "${VNC_PASS}" \
    -forever -shared -noxdamage -repeat -xkb \
    -noxrecord -quiet \
    -cursor arrow -bg \
    -o /home/vncuser/.x11vnc.log
echo "✅ x11vnc on port ${VNC_PORT}"

# ─── 8. Start noVNC ─────────────────────────────────────────────────
websockify --web /usr/share/novnc "0.0.0.0:${NOVNC_PORT}" "localhost:${VNC_PORT}" &
sleep 1
echo "✅ noVNC on port ${NOVNC_PORT}"

# ─── 9. Start GNOME Desktop (direct, no gnome-session) ───────────────
echo "🚀 Starting GNOME desktop..."

# gnome-settings-daemon (themes, keyboard, power, etc.)
/usr/libexec/gnome-settings-daemon &>/dev/null &
sleep 2

# Start GNOME Shell directly (bypasses gnome-session acceleration check)
echo "   Starting GNOME Shell (direct)..."
gnome-shell --x11 > ~/.gnome-shell.log 2>&1 &
SHELL_PID=$!
sleep 5

if kill -0 ${SHELL_PID} 2>/dev/null && pgrep -x gnome-shell >/dev/null 2>&1; then
    echo "✅ GNOME Shell is running!"
else
    echo "⚠️  GNOME Shell failed. Trying with --replace..."
    gnome-shell --x11 --replace > ~/.gnome-shell.log 2>&1 &
    SHELL_PID=$!
    sleep 5

    if kill -0 ${SHELL_PID} 2>/dev/null; then
        echo "✅ GNOME Shell running (replace mode)"
    else
        echo "⚠️  GNOME Shell crashed. Falling back to GNOME Flashback..."
        export XDG_CURRENT_DESKTOP=GNOME-Flashback:GNOME
        metacity --display=${DISPLAY_NUM} &>/dev/null &
        sleep 1
        gnome-panel &>/dev/null &
        gnome-flashback &>/dev/null &
        nautilus -n &>/dev/null &
        echo "✅ GNOME Flashback (Metacity) running"
    fi
fi

# ─── 10. Desktop polish ──────────────────────────────────────────────
sleep 2

# Set icon theme, GTK theme, and cursor theme explicitly
gsettings set org.gnome.desktop.interface icon-theme 'Yaru' 2>/dev/null || true
gsettings set org.gnome.desktop.interface gtk-theme 'Yaru' 2>/dev/null || true
gsettings set org.gnome.desktop.interface cursor-theme 'DMZ-White' 2>/dev/null || true
gsettings set org.gnome.desktop.interface cursor-size 24 2>/dev/null || true
gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark' 2>/dev/null || true

# Set wallpaper
WALLPAPER=$(find /usr/share/backgrounds -name "*.jpg" -o -name "*.png" 2>/dev/null | head -1)
if [ -n "${WALLPAPER}" ]; then
    gsettings set org.gnome.desktop.background picture-uri "file://${WALLPAPER}" 2>/dev/null || true
    gsettings set org.gnome.desktop.background picture-uri-dark "file://${WALLPAPER}" 2>/dev/null || true
fi

# Set favorite apps in dock
gsettings set org.gnome.shell favorite-apps \
    "['org.gnome.Terminal.desktop', 'org.gnome.Nautilus.desktop', 'org.gnome.Settings.desktop']" 2>/dev/null || true

# Enable the Show Apps button at the bottom of the dock
gsettings set org.gnome.shell.extensions.dash-to-dock show-apps-at-top false 2>/dev/null || true
gsettings set org.gnome.shell.extensions.dash-to-dock show-show-apps-button true 2>/dev/null || true
gsettings set org.gnome.shell.extensions.dash-to-dock dock-fixed true 2>/dev/null || true
gsettings set org.gnome.shell.extensions.dash-to-dock extend-height true 2>/dev/null || true

# Open a terminal
gnome-terminal &>/dev/null &

echo ""
echo "═══════════════════════════════════════════════"
echo "  ✅ Desktop ready!"
echo "  👉 http://localhost:${NOVNC_PORT}/vnc.html"
echo "  Password: ${VNC_PASS}"
echo "═══════════════════════════════════════════════"

wait ${XVFB_PID}
