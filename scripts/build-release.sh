#!/usr/bin/env bash
# Build the clicord client binary for one or more targets into ./dist.
#
#   scripts/build-release.sh                       # host target only
#   scripts/build-release.sh x86_64-pc-windows-gnu # specific target(s)
#
# The host target builds with plain cargo. Other targets use `cross`
# (cargo install cross + Docker) when available; macOS targets need a macOS
# host, so for "all platforms at once" use the GitHub Actions release workflow.
set -euo pipefail
cd "$(dirname "$0")/.."

host="$(rustc -vV | sed -n 's/host: //p')"
targets=("$@")
[ ${#targets[@]} -eq 0 ] && targets=("$host")

mkdir -p dist
for t in "${targets[@]}"; do
  if [ "$t" = "$host" ]; then
    cargo build --release -p client --target "$t"
  elif command -v cross >/dev/null 2>&1; then
    cross build --release -p client --target "$t"
  else
    echo "skip $t — install 'cross' (cargo install cross) to cross-compile" >&2
    continue
  fi
  ext=""; [[ "$t" == *windows* ]] && ext=".exe"
  cp "target/$t/release/clicord$ext" "dist/clicord-$t$ext"
  echo "built dist/clicord-$t$ext"
done
