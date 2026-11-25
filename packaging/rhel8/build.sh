#!/usr/bin/env bash
# Build SrcWranglr for an offline RHEL8 workstation, inside a Rocky Linux 8
# container (glibc 2.28, binary-compatible with RHEL8). Produces an x86_64
# binary and, when the bundle step succeeds, the full install tarball.
#
# Run this on any machine with Docker. On an Apple Silicon / arm64 host it runs
# under emulation (slower); on a native x86_64 Linux host it is fast.
#
# Output lands in dist/rhel8/ at the repo root.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
image="srcwranglr-rhel8-build"
out="$repo_root/dist/rhel8"

echo ">> Building image (x86_64 / glibc 2.28) — this can take a while under emulation…"
docker build --platform=linux/amd64 \
  -f "$repo_root/packaging/rhel8/Dockerfile" \
  -t "$image" "$repo_root"

echo ">> Extracting artifacts to dist/rhel8/ …"
mkdir -p "$out"
cid="$(docker create --platform=linux/amd64 "$image")"
docker cp "$cid:/src/target/release/srcwranglr" "$out/srcwranglr"
# Best effort: the full .desktop/icon/license bundle, if it was produced.
docker cp "$cid:/src/dist/." "$out/" 2>/dev/null || true
docker rm "$cid" >/dev/null

echo ">> Done. Contents of dist/rhel8/:"
ls -lh "$out"
echo
echo ">> Binary details (want: ELF x86-64, dynamically linked):"
file "$out/srcwranglr" || true
