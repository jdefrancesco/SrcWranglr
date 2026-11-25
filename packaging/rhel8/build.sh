#!/usr/bin/env bash
# Build SrcWranglr for an offline RHEL8 workstation, inside a Rocky Linux 8
# container (glibc 2.28, binary-compatible with RHEL8). Produces an x86_64
# stripped binary and the full install tarball.
#
# Uses Podman (the RHEL/Fedora default) or Docker — whichever is installed;
# override with CONTAINER_ENGINE=podman|docker. On a native x86_64 Linux host
# this is fast; on an arm64 host (e.g. Apple Silicon) it runs under emulation.
#
# Output lands in dist/rhel8/ at the repo root.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
image="srcwranglr-rhel8-build"
out="$repo_root/dist/rhel8"

engine="${CONTAINER_ENGINE:-}"
if [ -z "$engine" ]; then
  if command -v podman >/dev/null 2>&1; then
    engine=podman
  elif command -v docker >/dev/null 2>&1; then
    engine=docker
  else
    echo "error: need podman or docker on PATH (set CONTAINER_ENGINE to pick one)." >&2
    exit 1
  fi
fi
echo ">> Using container engine: $engine"

echo ">> Building image (x86_64 / glibc 2.28) — native on an x86_64 host, emulated on arm64…"
"$engine" build --platform=linux/amd64 \
  -f "$repo_root/packaging/rhel8/Dockerfile" \
  -t "$image" "$repo_root"

echo ">> Extracting artifacts to dist/rhel8/ …"
mkdir -p "$out"
cid="$("$engine" create "$image")"
"$engine" cp "$cid:/src/target/release/srcwranglr" "$out/srcwranglr"
# Best effort: the full .desktop/icon/license bundle, if it was produced.
"$engine" cp "$cid:/src/dist/." "$out/" 2>/dev/null || true
"$engine" rm "$cid" >/dev/null

echo ">> Done. Contents of dist/rhel8/:"
ls -lh "$out"
echo
echo ">> Binary details (want: ELF x86-64, dynamically linked, stripped):"
file "$out/srcwranglr" || true
