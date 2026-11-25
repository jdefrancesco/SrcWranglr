# Building SrcWranglr for an offline RHEL8 workstation

SrcWranglr is a GUI app (egui/eframe, OpenGL). A **fully static** Linux GUI
binary is not practical — winit/glow `dlopen` the X11/Wayland/OpenGL/xkbcommon
stack at runtime. The portable approach is a **dynamically linked binary built
against glibc 2.28** (RHEL8's version); it then runs on RHEL8 and any newer
glibc. The runtime graphics libraries ship with any RHEL8 desktop, so the
target needs no development packages.

## Build

On any machine with Docker (native x86_64 Linux is fastest; Apple Silicon works
under emulation):

```bash
./packaging/rhel8/build.sh
```

Artifacts land in `dist/rhel8/`:

- `srcwranglr` — the executable (ELF x86-64, dynamically linked, stripped; ~45M).
- `srcwranglr-<version>-linux-x86_64.tar.gz` — the full bundle (binary +
  `.desktop` + icons + licenses; ~17M).

The binary is stripped and its glibc floor is 2.28, so it runs on RHEL8 and any
newer glibc.

## Without Docker (native build)

The RHEL8 **target** never needs Docker — it only runs the finished binary.
Docker is just one way to produce a glibc-2.28 binary. If you have no Docker,
build natively on any RHEL8 / Rocky 8 host (the target itself, or a connected
RHEL8 build box):

```bash
./packaging/rhel8/build-native.sh
```

The script's header lists the one-time `dnf` + `rustup` prerequisites. If that
build host is itself offline, vendor the crates on a connected machine first:

```bash
cargo vendor vendor/                 # on a connected machine, in the repo
# commit/copy vendor/ + .cargo/config.toml across, then build with --offline
```

(A third option, from a non-Linux machine, is cross-compiling with
`cargo-zigbuild --target x86_64-unknown-linux-gnu.2.28` — no Docker, but a
blind build you must verify on the target.)

## Install on the air-gapped box

Copy the tarball across, then either run `make install-linux` from an extracted
source tree, or place files by hand:

```bash
tar xzf srcwranglr-*-linux-x86_64.tar.gz
cd srcwranglr-*-linux-x86_64
install -Dm755 srcwranglr          ~/.local/bin/srcwranglr
install -Dm644 srcwranglr.desktop  ~/.local/share/applications/srcwranglr.desktop
install -Dm644 srcwranglr.png      ~/.local/share/icons/hicolor/256x256/apps/srcwranglr.png
```

## Runtime libraries (already present on a RHEL8 desktop)

If the box is a minimal install, these are the runtime packages it needs:

```bash
dnf install mesa-libGL libxkbcommon libxkbcommon-x11 \
            libX11 libXcursor libXrandr libXi
```

Verify the glibc floor of a built binary with:

```bash
objdump -T dist/rhel8/srcwranglr | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail
```

The highest version shown must be ≤ the target's glibc (RHEL8 = 2.28).
