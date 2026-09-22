#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Publish a built repository to the local APT configuration so that
# `apt-get install mount-manager` works, exactly like any other package.
#
#   sudo packaging/install-apt-repo.sh [REPO_DIR] [DEST_DIR]
#
# The repository is copied (not symlinked) so the image keeps working even if
# the build tree goes away.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="${1:-$SRC_DIR/dist/repo}"
DEST_DIR="${2:-/opt/mount-manager/repo}"
SUITE="${SUITE:-stable}"
COMPONENT="${COMPONENT:-main}"
LIST_FILE="/etc/apt/sources.list.d/mount-manager.list"

log() { printf '  \033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '  \033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[[ -d "$REPO_DIR/dists/$SUITE" ]] || die "$REPO_DIR is not an APT repository (run make-apt-repo.sh first)"
[[ "$(id -u)" == "0" ]] || die "this script must run as root (use sudo)"

log "publishing $REPO_DIR → $DEST_DIR"
install -d -m 0755 "$(dirname "$DEST_DIR")"
rm -rf "$DEST_DIR"
cp -a "$REPO_DIR" "$DEST_DIR"
chmod -R a+rX "$DEST_DIR"

log "registering $LIST_FILE"
cat > "$LIST_FILE" <<LIST
# Local APT repository for Mount Manager (added by packaging/install-apt-repo.sh).
# Unsigned on purpose: it is a read-only repository built from local sources.
deb [trusted=yes] file:$DEST_DIR $SUITE $COMPONENT
LIST
chmod 0644 "$LIST_FILE"

log "apt-get update"
apt-get update -o Dir::Etc::sourcelist="$LIST_FILE" \
                -o Dir::Etc::sourceparts="-" \
                -o APT::Get::List-Cleanup="0" >/dev/null \
  || apt-get update

cat <<INFO

Mount Manager is now available through apt:

  sudo apt-get install -y mount-manager     # install
  sudo apt-get install --reinstall mount-manager
  sudo apt-get remove mount-manager         # remove

The app then appears in the GNOME app grid as "Mount Manager".
INFO
