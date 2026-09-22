#!/bin/bash
# Verification helper used while developing: launches Mount Manager on the
# container's X display and saves a screenshot of the whole screen.
#
#   docker cp packaging/gui-verify.sh <container>:/tmp/
#   docker exec -u vncuser -e MM_SHOT=/tmp/shot.png <container> /tmp/gui-verify.sh [args...]
#
# Requires imagemagick (for `import`) inside the container.
set -u
export DISPLAY="${DISPLAY:-:1}"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  SHELL_PID=$(pgrep -u "$(id -u)" -o gnome-shell || true)
  if [ -n "$SHELL_PID" ]; then
    DBUS_SESSION_BUS_ADDRESS=$(tr '\0' '\n' < "/proc/$SHELL_PID/environ" \
      | sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p')
    export DBUS_SESSION_BUS_ADDRESS
  fi
fi
echo "display=$DISPLAY bus=${DBUS_SESSION_BUS_ADDRESS:-<none>}"

pkill -f 'bin/mount-manager|^mount-manager' 2>/dev/null
sleep 1
nohup mount-manager "$@" > /tmp/mm-gui.log 2>&1 &
sleep "${MM_WAIT:-9}"
import -window root "${MM_SHOT:-/tmp/mm-shot.png}"
echo "--- app log (first 20 lines) ---"
head -20 /tmp/mm-gui.log
echo "--- running? ---"
pgrep -a mount-manager | head -3
