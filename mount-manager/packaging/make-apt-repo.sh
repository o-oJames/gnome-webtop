#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Turn the built .deb files into a local APT repository, so the package can be
# installed with `apt-get update && apt-get install mount-manager`.
#
#   packaging/make-apt-repo.sh [DEB_DIR] [REPO_DIR] [SUITE] [COMPONENT]
#
# Result:
#   REPO/pool/main/m/mount-manager/mount-manager_<ver>_<arch>.deb
#   REPO/dists/<suite>/Release
#   REPO/dists/<suite>/main/binary-<arch>/Packages{,.gz}
#
# The repository is unsigned (a local, read-only repo inside the image); use
# `deb [trusted=yes] file:...` in sources.list.d — see install-apt-repo.sh.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEB_DIR="${1:-$SRC_DIR/dist}"
REPO_DIR="${2:-$SRC_DIR/dist/repo}"
SUITE="${3:-stable}"
COMPONENT="${4:-main}"
ORIGIN="${ORIGIN:-Mount Manager}"

log() { printf '  \033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '  \033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

shopt -s nullglob
DEBS=("$DEB_DIR"/*.deb)
[[ ${#DEBS[@]} -gt 0 ]] || die "no .deb files in $DEB_DIR (run packaging/build-deb.sh first)"

# pool/<section-first-letter>/... is the Debian convention.
POOL="$REPO_DIR/pool/main/m/mount-manager"
install -d -m 0755 "$POOL"
for deb in "${DEBS[@]}"; do
  install -m 0644 "$deb" "$POOL/"
  log "pooled $(basename "$deb")"
done

# Every architecture present in the pool gets its own binary-<arch> directory.
ARCHES=()
for deb in "$POOL"/*.deb; do
  arch="$(dpkg-deb -f "$deb" Architecture 2>/dev/null || echo "")"
  [[ -n "$arch" ]] || continue
  case " ${ARCHES[*]-} " in *" $arch "*) ;; *) ARCHES+=("$arch") ;; esac
done
[[ ${#ARCHES[@]} -gt 0 ]] || die "could not determine the architecture of the packages"

for arch in "${ARCHES[@]}"; do
  bin_dir="$REPO_DIR/dists/$SUITE/$COMPONENT/binary-$arch"
  install -d -m 0755 "$bin_dir"

  if command -v apt-ftparchive >/dev/null 2>&1; then
    ( cd "$REPO_DIR" && apt-ftparchive packages "pool/main" \
        | awk -v arch="$arch" 'BEGIN{RS="";ORS="\n\n"} $0 ~ ("\nArchitecture: " arch "\n") || $0 ~ ("\nArchitecture: all\n")' \
        > "$bin_dir/Packages" )
  else
    python3 "$SRC_DIR/packaging/make_apt_repo.py" packages "$REPO_DIR" "$bin_dir/Packages" "$arch"
  fi
  # apt-ftparchive may not be picky about architectures; regenerate cleanly when
  # the awk filter produced nothing.
  if [[ ! -s "$bin_dir/Packages" ]]; then
    python3 "$SRC_DIR/packaging/make_apt_repo.py" packages "$REPO_DIR" "$bin_dir/Packages" "$arch"
  fi
  gzip -9c "$bin_dir/Packages" > "$bin_dir/Packages.gz"
  log "index for $arch: $(grep -c '^Package: ' "$bin_dir/Packages" || true) package(s)"
done

release_dir="$REPO_DIR/dists/$SUITE"
# Remove a previous Release file first: apt-ftparchive hashes everything in the
# directory and must not include its own (stale) output.
rm -f "$release_dir/Release" "$release_dir/Release.gpg" "$release_dir/InRelease"
if command -v apt-ftparchive >/dev/null 2>&1; then
  ( cd "$REPO_DIR" && apt-ftparchive release \
      -o APT::FTPArchive::Release::Origin="$ORIGIN" \
      -o APT::FTPArchive::Release::Label="$ORIGIN" \
      -o APT::FTPArchive::Release::Suite="$SUITE" \
      -o APT::FTPArchive::Release::Codename="$SUITE" \
      -o APT::FTPArchive::Release::Components="$COMPONENT" \
      -o APT::FTPArchive::Release::Architectures="${ARCHES[*]}" \
      -o APT::FTPArchive::Release::Description="Local repository for $ORIGIN" \
      "dists/$SUITE" > "$release_dir/Release" )
else
  python3 "$SRC_DIR/packaging/make_apt_repo.py" release "$release_dir" \
      "$ORIGIN" "$SUITE" "$COMPONENT" "${ARCHES[*]}" > "$release_dir/Release"
fi
log "wrote $release_dir/Release"

cat <<INFO

Repository ready:
  $REPO_DIR

Install it with:
  sudo packaging/install-apt-repo.sh "$REPO_DIR"
  sudo apt-get update && sudo apt-get install -y mount-manager
INFO
