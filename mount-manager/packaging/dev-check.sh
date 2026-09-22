#!/usr/bin/env bash
# Development helper: run cargo inside the Linux build container.
#
#   packaging/dev-check.sh                      cargo check (core + GUI + tests)
#   packaging/dev-check.sh --core               cargo check --no-default-features
#   packaging/dev-check.sh test                 cargo test --lib
#   packaging/dev-check.sh clippy               cargo clippy
#   packaging/dev-check.sh fmt                  cargo fmt
#
# The cargo registry and target directory live in named volumes, so repeated
# runs are fast and the host tree stays clean.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
IMAGE="${MM_BUILDER_IMAGE:-mm-builder}"

CARGO_ARGS=(check --all-targets --color=always)
case "${1:-}" in
  --core)  CARGO_ARGS=(check --all-targets --no-default-features --color=always) ;;
  test)    CARGO_ARGS=(test --lib --color=always -- --test-threads=1) ;;
  clippy)  CARGO_ARGS=(clippy --all-targets --color=always -- -D warnings) ;;
  fmt)     CARGO_ARGS=(fmt -- --check) ;;
  build)   CARGO_ARGS=(build --release --color=always) ;;
  shell)   CARGO_ARGS=() ;;
  "")      ;;
  *)       CARGO_ARGS=("$@") ;;
esac

docker run --rm -it \
  -v "$PWD":/src -w /src \
  -v mm-cargo:/usr/local/cargo \
  -v mm-target:/src/target \
  -e CARGO_TERM_COLOR=always \
  "$IMAGE" ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}
