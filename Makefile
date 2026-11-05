# Builds slurcetrail and packages it for the platform you run make on.
#
#   make              release binary (target/release/slurcetrail)
#   make package      this platform's package, in dist/:
#                       macOS    slurcetrail.app and slurcetrail-<version>-macos-<arch>.dmg
#                       Linux    slurcetrail-<version>-linux-<arch>.tar.gz
#                       Windows  slurcetrail-<version>-windows-<arch>.zip (Git Bash / MSYS2)
#   make app          macOS: just the .app bundle, with its icon
#   make install      macOS: copy the .app to $(APPLICATIONS)
#                     Linux: install binary, .desktop file and icons under $(PREFIX)
#   make uninstall    undo `make install`
#   make check        format check, clippy and tests
#   make icons        re-render assets/icon-*.png from assets/icon.svg (needs rsvg-convert)
#   make clean        remove dist/ and cargo's build output
#
# Variables: SIGN_IDENTITY (macOS codesign identity; default "-" = ad-hoc),
# BUNDLE_ID, APPLICATIONS (default /Applications), PREFIX (default /usr/local),
# DESTDIR (staging root for packagers).

APP           := slurcetrail
VERSION       := $(shell sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
ARCH          := $(shell uname -m)
CARGO         ?= cargo
RELEASE       := target/release
BUNDLE_ID     ?= io.github.jdefrancesco.slurcetrail
SIGN_IDENTITY ?= -
APPLICATIONS  ?= /Applications
PREFIX        ?= /usr/local
ICON_SVG      := assets/icon.svg
ICON_1024     := assets/icon-1024.png
ICON_256      := assets/icon-256.png

ifeq ($(OS),Windows_NT)
  PLATFORM := windows
else
  UNAME := $(shell uname -s)
  ifeq ($(UNAME),Darwin)
    PLATFORM := macos
  else ifeq ($(UNAME),Linux)
    PLATFORM := linux
  else
    PLATFORM := unsupported
  endif
endif

# Everything built here lands in dist/ (deleted only by name below).
APP_BUNDLE    := dist/$(APP).app
ICONSET       := dist/$(APP).iconset
ICNS          := dist/$(APP).icns
DMG           := dist/$(APP)-$(VERSION)-macos-$(ARCH).dmg
DMG_STAGING   := dist/dmg-staging
LINUX_NAME    := $(APP)-$(VERSION)-linux-$(ARCH)
WINDOWS_NAME  := $(APP)-$(VERSION)-windows-$(ARCH)

.PHONY: all build test check package package-macos package-linux package-windows \
	package-unsupported app dmg install install-macos install-linux uninstall \
	uninstall-macos uninstall-linux icons clean

all: build

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --locked

check:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets --locked -- -D warnings
	$(CARGO) test --locked

package: package-$(PLATFORM)

package-unsupported:
	@echo "No package format for $(UNAME); 'make build' gives you $(RELEASE)/$(APP)." >&2
	@exit 1

# --- macOS ------------------------------------------------------------------

package-macos: dmg

app: build $(ICNS)
	rm -rf "dist/$(APP).app"
	mkdir -p "$(APP_BUNDLE)/Contents/MacOS" "$(APP_BUNDLE)/Contents/Resources"
	cp "$(RELEASE)/$(APP)" "$(APP_BUNDLE)/Contents/MacOS/$(APP)"
	cp "$(ICNS)" "$(APP_BUNDLE)/Contents/Resources/$(APP).icns"
	sed -e 's/@VERSION@/$(VERSION)/g' -e 's/@BUNDLE_ID@/$(BUNDLE_ID)/g' \
		packaging/macos/Info.plist > "$(APP_BUNDLE)/Contents/Info.plist"
	plutil -lint "$(APP_BUNDLE)/Contents/Info.plist"
	codesign --force --sign "$(SIGN_IDENTITY)" "$(APP_BUNDLE)"
	@echo "Built $(APP_BUNDLE)"

# Every size macOS asks an .icns for, scaled down from the 1024px master.
$(ICNS): $(ICON_1024)
	rm -rf "dist/$(APP).iconset"
	mkdir -p "$(ICONSET)"
	for size in 16 32 128 256 512; do \
		sips -z $$size $$size "$<" --out "$(ICONSET)/icon_$${size}x$${size}.png" >/dev/null && \
		sips -z $$((size * 2)) $$((size * 2)) "$<" \
			--out "$(ICONSET)/icon_$${size}x$${size}@2x.png" >/dev/null || exit 1; \
	done
	iconutil -c icns "$(ICONSET)" -o "$@"
	rm -rf "dist/$(APP).iconset"

dmg: app
	rm -rf "dist/dmg-staging"
	rm -f "$(DMG)"
	mkdir -p "$(DMG_STAGING)"
	cp -R "$(APP_BUNDLE)" "$(DMG_STAGING)/"
	ln -s /Applications "$(DMG_STAGING)/Applications"
	hdiutil create -quiet -volname "$(APP) $(VERSION)" -srcfolder "$(DMG_STAGING)" \
		-ov -format UDZO "$(DMG)"
	rm -rf "dist/dmg-staging"
	@echo "Built $(DMG)"

install-macos: app
	@test -d "$(APPLICATIONS)" || { echo "No folder $(APPLICATIONS)" >&2; exit 1; }
	rm -rf "$(APPLICATIONS)/$(APP).app"
	cp -R "$(APP_BUNDLE)" "$(APPLICATIONS)/"
	@echo "Installed $(APPLICATIONS)/$(APP).app"

uninstall-macos:
	@test -d "$(APPLICATIONS)" || { echo "No folder $(APPLICATIONS)" >&2; exit 1; }
	rm -rf "$(APPLICATIONS)/$(APP).app"

# --- Linux ------------------------------------------------------------------

package-linux: build
	rm -rf "dist/$(LINUX_NAME)"
	mkdir -p "dist/$(LINUX_NAME)"
	cp "$(RELEASE)/$(APP)" LICENSE README.md "dist/$(LINUX_NAME)/"
	cp packaging/linux/$(APP).desktop "dist/$(LINUX_NAME)/"
	cp $(ICON_256) "dist/$(LINUX_NAME)/$(APP).png"
	cp $(ICON_SVG) "dist/$(LINUX_NAME)/$(APP).svg"
	tar -C dist -czf "dist/$(LINUX_NAME).tar.gz" "$(LINUX_NAME)"
	rm -rf "dist/$(LINUX_NAME)"
	@echo "Built dist/$(LINUX_NAME).tar.gz"

install-linux: build
	install -Dm755 "$(RELEASE)/$(APP)" "$(DESTDIR)$(PREFIX)/bin/$(APP)"
	install -Dm644 packaging/linux/$(APP).desktop \
		"$(DESTDIR)$(PREFIX)/share/applications/$(APP).desktop"
	install -Dm644 $(ICON_256) "$(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps/$(APP).png"
	install -Dm644 $(ICON_SVG) "$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(APP).svg"

uninstall-linux:
	rm -f "$(DESTDIR)$(PREFIX)/bin/$(APP)" \
		"$(DESTDIR)$(PREFIX)/share/applications/$(APP).desktop" \
		"$(DESTDIR)$(PREFIX)/share/icons/hicolor/256x256/apps/$(APP).png" \
		"$(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(APP).svg"

install: install-$(PLATFORM)

uninstall: uninstall-$(PLATFORM)

# --- Windows (Git Bash or MSYS2, with the Rust MSVC or GNU toolchain) -------

package-windows: build
	rm -rf "dist/$(WINDOWS_NAME)"
	rm -f "dist/$(WINDOWS_NAME).zip"
	mkdir -p "dist/$(WINDOWS_NAME)"
	cp "$(RELEASE)/$(APP).exe" LICENSE README.md "dist/$(WINDOWS_NAME)/"
	cd dist && tar -a -cf "$(WINDOWS_NAME).zip" "$(WINDOWS_NAME)"
	rm -rf "dist/$(WINDOWS_NAME)"
	@echo "Built dist/$(WINDOWS_NAME).zip"

# --- Shared -----------------------------------------------------------------

icons:
	rsvg-convert -w 1024 -h 1024 $(ICON_SVG) -o $(ICON_1024)
	rsvg-convert -w 256 -h 256 $(ICON_SVG) -o $(ICON_256)

clean:
	rm -rf dist
	$(CARGO) clean
