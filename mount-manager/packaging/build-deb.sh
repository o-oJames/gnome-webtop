#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Build the mount-manager Debian package.
#
#   packaging/build-deb.sh                 build target/release + dist/*.deb
#   packaging/build-deb.sh --stage DIR     only populate DIR (used by
#                                          debian/rules / dpkg-buildpackage)
#   packaging/build-deb.sh --no-build      reuse an existing target/release
#
# Two cargo builds are produced on purpose:
#   * mount-manager          (with the "gui" feature: GTK4 + libadwaita)
#   * mount-manager-helper   (--no-default-features: no GTK in a root helper)
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SRC_DIR"

STAGE_ONLY=""
DO_BUILD=1
OUT_DIR="$SRC_DIR/dist"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --stage)    STAGE_ONLY="$2"; shift 2 ;;
    --output)   OUT_DIR="$2"; shift 2 ;;
    --no-build) DO_BUILD=0; shift ;;
    -h|--help)  sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "build-deb.sh: unknown option $1" >&2; exit 2 ;;
  esac
done

log() { printf '  \033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '  \033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# ── helpers ─────────────────────────────────────────────────────────────────
# Resolve the shared library dependencies of the built binaries the same way
# dpkg-shlibdeps does, but without needing debhelper.
shlib_deps() {
  local deps="" tmpf
  if command -v dpkg-shlibdeps >/dev/null 2>&1; then
    tmpf="$(mktemp)"
    if dpkg-shlibdeps --ignore-missing-info -O"$tmpf" "$@" >/dev/null 2>&1; then
      deps="$(sed -n 's/^shlibs:Depends=//p' "$tmpf" | head -1)"
    fi
    rm -f "$tmpf"
  fi
  if [[ -z "$deps" ]]; then
    deps="$(
      for bin in "$@"; do
        ldd "$bin" 2>/dev/null | awk '{ for (i = 1; i <= NF; i++) if ($i ~ /^lib.*\.so/) { print $i; break } }'
      done | sort -u | while read -r soname; do
        dpkg -S "$soname" 2>/dev/null | head -1 | cut -d: -f1
      done | grep -v -- "-dev$" | sort -u | paste -sd, - | sed 's/,/, /g'
    )"
  fi
  if [[ -z "$deps" ]]; then
    # Last resort: the libraries a GTK4/libadwaita app always needs.
    deps="libc6, libgcc-s1, libglib2.0-0, libgtk-4-1, libadwaita-1-0"
  fi
  printf '%s' "$deps"
}

# ── metadata ────────────────────────────────────────────────────────────────
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[[ -n "$VERSION" ]] || die "cannot read version from Cargo.toml"
PKG="mount-manager"
APP_ID="io.github.mount_manager"

ARCH="$(dpkg-architecture -qDEB_HOST_ARCH 2>/dev/null || dpkg --print-architecture 2>/dev/null || uname -m)"
case "$ARCH" in
  x86_64) ARCH="amd64" ;; aarch64|arm64) ARCH="arm64" ;; armv7l) ARCH="armhf" ;;
  i686|i386) ARCH="i386" ;; riscv64) ARCH="riscv64" ;;
esac
MAINTAINER="$(sed -n 's/^Maintainer: //p' debian/control | head -1)"
MAINTAINER="${MAINTAINER:-Mount Manager maintainers <maintainer@example.com>}"

# ── build ───────────────────────────────────────────────────────────────────
CARGO="${CARGO:-cargo}"
if [[ "$DO_BUILD" == "1" ]]; then
  command -v "$CARGO" >/dev/null 2>&1 || die "cargo not found (install rustc/cargo or rustup)"
  log "cargo build --release (GUI)"
  RUSTFLAGS="${RUSTFLAGS:-}" $CARGO build --release --locked --bin "$PKG"
  log "cargo build --release --no-default-features (privileged helper)"
  $CARGO build --release --locked --no-default-features --bin "${PKG}-helper"
fi

GUI_BIN="target/release/$PKG"
HELPER_BIN="target/release/${PKG}-helper"
[[ -x "$GUI_BIN" ]]    || die "missing $GUI_BIN"
[[ -x "$HELPER_BIN" ]] || die "missing $HELPER_BIN"

# ── stage ───────────────────────────────────────────────────────────────────
if [[ -n "$STAGE_ONLY" ]]; then
  ROOT="$STAGE_ONLY"
else
  ROOT="$(mktemp -d "${TMPDIR:-/tmp}/${PKG}-deb.XXXXXX")"
  trap 'rm -rf "$ROOT"' EXIT
fi
log "staging into $ROOT"
rm -rf "$ROOT"
install -d -m 0755 "$ROOT/usr/bin" \
                   "$ROOT/usr/lib/$PKG" \
                   "$ROOT/usr/share/applications" \
                   "$ROOT/usr/share/icons/hicolor/scalable/apps" \
                   "$ROOT/usr/share/icons/hicolor/symbolic/apps" \
                   "$ROOT/usr/share/metainfo" \
                   "$ROOT/usr/share/polkit-1/actions" \
                   "$ROOT/usr/share/doc/$PKG" \
                   "$ROOT/usr/share/bash-completion/completions"

install -m 0755 "$GUI_BIN"    "$ROOT/usr/bin/$PKG"
# The helper is executed by pkexec: it must be root owned and not group/world
# writable, otherwise polkit refuses to run it.
install -m 0755 "$HELPER_BIN" "$ROOT/usr/lib/$PKG/${PKG}-helper"

install -m 0644 "data/${APP_ID}.desktop"        "$ROOT/usr/share/applications/${APP_ID}.desktop"
install -m 0644 "data/${APP_ID}.metainfo.xml"   "$ROOT/usr/share/metainfo/${APP_ID}.metainfo.xml"
install -m 0644 "data/io.github.mount_manager.policy" "$ROOT/usr/share/polkit-1/actions/${APP_ID}.policy"
install -m 0644 "data/$PKG.svg"                 "$ROOT/usr/share/icons/hicolor/scalable/apps/$PKG.svg"
install -m 0644 "data/$PKG-symbolic.svg"        "$ROOT/usr/share/icons/hicolor/symbolic/apps/$PKG-symbolic.svg"
install -m 0644 README.md                       "$ROOT/usr/share/doc/$PKG/README.md"
install -m 0644 LICENSE                         "$ROOT/usr/share/doc/$PKG/copyright"

# Compressed changelog, as Debian policy requires.
if command -v gzip >/dev/null 2>&1; then
  gzip -9nc debian/changelog > "$ROOT/usr/share/doc/$PKG/changelog.gz"
  chmod 0644 "$ROOT/usr/share/doc/$PKG/changelog.gz"
else
  install -m 0644 debian/changelog "$ROOT/usr/share/doc/$PKG/changelog"
fi

if [[ -f "packaging/${PKG}.bash" ]]; then
  install -m 0644 "packaging/${PKG}.bash" "$ROOT/usr/share/bash-completion/completions/$PKG"
fi

# ── DEBIAN/control (generated from the single source of truth) ───────────────
if [[ -z "$STAGE_ONLY" ]]; then
  install -d -m 0755 "$ROOT/DEBIAN"
  # Only the *binary* stanza belongs in DEBIAN/control, and the dpkg
  # substitution variables have to be resolved by hand because we are not
  # running debhelper here.
  SHLIB_DEPS="$(shlib_deps "$GUI_BIN" "$HELPER_BIN")"
  awk -v arch="$ARCH" -v shlibs="$SHLIB_DEPS" '
    /^Package:/        { inbinary = 1 }
    !inbinary          { next }
    /^ /               { print; next }        # Description continuation lines
    /^(Package|Architecture|Depends|Pre-Depends|Recommends|Suggests|Breaks|Conflicts|Provides|Section|Priority|Homepage|Description|Multi-Arch|Enhances|Built-Using):/ {
      line = $0
      sub(/^Architecture: any$/, "Architecture: " arch, line)
      gsub(/\$\{shlibs:Depends\}/, shlibs, line)
      gsub(/\$\{misc:Depends\}, /, "", line)
      gsub(/, \$\{misc:Depends\}/, "", line)
      gsub(/\$\{misc:Depends\}/, "", line)
      gsub(/, *,/, ", ", line)
      sub(/,[ \t]*$/, "", line)
      print line
    }
  ' debian/control > "$ROOT/DEBIAN/control"

  # Prepend the fields dpkg-deb needs and that are not in the source control.
  INSTALLED_SIZE="$(du -sk "$ROOT" | cut -f1)"
  {
    printf 'Version: %s\n' "$VERSION"
    printf 'Maintainer: %s\n' "$MAINTAINER"
    printf 'Installed-Size: %s\n' "$INSTALLED_SIZE"
    cat "$ROOT/DEBIAN/control"
  } > "$ROOT/DEBIAN/control.tmp"
  mv "$ROOT/DEBIAN/control.tmp" "$ROOT/DEBIAN/control"

  install -m 0755 debian/postinst "$ROOT/DEBIAN/postinst"
  install -m 0755 debian/prerm    "$ROOT/DEBIAN/prerm"
  install -m 0755 debian/postrm   "$ROOT/DEBIAN/postrm"

  # md5sums (relative paths, no ./ prefix)
  ( cd "$ROOT" && find usr -type f -print0 | LC_ALL=C sort -z | xargs -0 md5sum ) \
    > "$ROOT/DEBIAN/md5sums" 2>/dev/null || rm -f "$ROOT/DEBIAN/md5sums"

  mkdir -p "$OUT_DIR"
  DEB="$OUT_DIR/${PKG}_${VERSION}_${ARCH}.deb"
  log "building $DEB"
  dpkg-deb --build --root-owner-group "$ROOT" "$DEB" >/dev/null
  log "done: $DEB ($(du -h "$DEB" | cut -f1))"
  echo "$DEB"
else
  log "staged only (no DEBIAN directory)"
fi
