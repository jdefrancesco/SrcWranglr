# Build SrcWranglr for RHEL8 / glibc 2.28.
#
# Rocky Linux 8 is binary-compatible with RHEL8 (same glibc 2.28), so a binary
# built here runs on an offline RHEL8 workstation and any newer glibc. The app
# is a GUI (egui/eframe, OpenGL) — it is NOT statically linked, because winit
# and glow dlopen X11/Wayland/GL/xkbcommon at runtime. Those runtime libraries
# ship with any RHEL8 desktop, so the target needs no dev packages.
FROM --platform=linux/amd64 rockylinux:8

# Install Rust first (its own layer) so adding a system lib below doesn't force
# a re-download. dnf's Rust is far too old for current crates.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN dnf -y install curl gcc gcc-c++ make git which \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
       | sh -s -- -y --profile minimal --default-toolchain stable

# GUI dev libraries — a link-time safety net (most are dlopen'd at runtime, but
# a few -sys crates probe them at build time). The X*-devel packages live in the
# PowerTools repo, disabled by default on RHEL8/Rocky8.
RUN dnf -y install dnf-plugins-core \
    && dnf config-manager --set-enabled powertools \
    && dnf -y install \
      libxkbcommon-devel wayland-devel \
      libX11-devel libXcursor-devel libXrandr-devel libXi-devel \
      mesa-libGL-devel mesa-libEGL-devel fontconfig-devel \
    && dnf clean all

WORKDIR /src
COPY . .
# Drop any host-built (arm64/macOS) artifacts that slipped in, then build the
# release binary. Its own layer, so re-running the strip/package step below does
# not trigger a full recompile.
RUN rm -rf target dist && cargo build --release --locked

# Package the Linux bundle (binary + .desktop + icons + licenses), then strip
# the binary — both inside the bundle and the bare copy. Strip runs AFTER
# packaging because `make` re-links via cargo, which would otherwise restore the
# unstripped binary. Result: ~45M binary, ~17M bundle.
RUN make package-linux \
    && bundle="$(ls dist/*-linux-x86_64.tar.gz)" \
    && dir="$(basename "${bundle%.tar.gz}")" \
    && tar -C dist -xzf "$bundle" \
    && strip "dist/$dir/srcwranglr" \
    && tar -C dist -czf "$bundle" "$dir" \
    && rm -rf "dist/$dir" \
    && strip target/release/srcwranglr
