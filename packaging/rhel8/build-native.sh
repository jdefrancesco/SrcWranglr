#!/usr/bin/env bash
# Build SrcWranglr natively on a RHEL8 / Rocky 8 host — NO Docker.
#
# Run this ON a RHEL8-family machine (the target itself, or a connected
# RHEL8/Rocky 8 build box). Output lands in dist/ at the repo root, same as the
# container build: a stripped x86_64 binary + the install bundle.
#
# One-time prerequisites (need root + network; skip any already present):
#
#   sudo dnf install -y gcc gcc-c++ make git
#   sudo dnf install -y dnf-plugins-core
#   sudo dnf config-manager --set-enabled powertools   # 'crb' on RHEL9/Rocky9
#   sudo dnf install -y libxkbcommon-devel wayland-devel libX11-devel \
#        libXcursor-devel libXrandr-devel libXi-devel \
#        mesa-libGL-devel mesa-libEGL-devel fontconfig-devel
#   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
#   source "$HOME/.cargo/env"
#
# If the build box is itself offline, vendor the crates on a connected machine
# first (`cargo vendor`) and carry them over — see packaging/rhel8/README.md.
set -euo pipefail
cd "$(dirname "$0")/../.."

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install rustup (see the prerequisites in this script's header)." >&2
  exit 1
fi

echo ">> Building release binary…"
cargo build --release --locked

echo ">> Packaging and stripping…"
make package-linux
bundle="$(ls dist/*-linux-x86_64.tar.gz)"
dir="$(basename "${bundle%.tar.gz}")"
# Strip AFTER packaging: make re-links via cargo, which would restore the
# unstripped binary. Strip the copy inside the bundle, then the bare one.
tar -C dist -xzf "$bundle"
strip "dist/$dir/srcwranglr"
tar -C dist -czf "$bundle" "$dir"
rm -rf "dist/$dir"
strip target/release/srcwranglr

echo ">> Done:"
echo "   $bundle"
echo "   target/release/srcwranglr"
file target/release/srcwranglr 2>/dev/null || true
