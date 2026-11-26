# Justfile for cosmic-ext-connected
# Install just: cargo install just

# Applet metadata
name := 'cosmic-ext-connected'
export APPID := 'io.github.nwxnw.cosmic-ext-connected'

# Installation paths (overridable via env vars for Flatpak builds)
# Default prefix is user-local, so `just install` needs no sudo.
# System-wide install remains available: `sudo just prefix=/usr install`
# Packagers: just rootdir="$pkgdir" prefix=/usr install
rootdir := ''
default-prefix := env_var('HOME') / '.local'
prefix := default-prefix
base-dir := absolute_path(clean(rootdir / prefix))
export INSTALL_DIR := base-dir / 'share'
# Runtime location of the binary (excludes rootdir, which is a staging dir only)
exec_dir := clean(prefix / 'bin')
staged := if rootdir != '' { 'yes' } else if prefix != default-prefix { 'yes' } else { '' }

bin_dir := env_var_or_default("BIN_DIR", base-dir / 'bin')
app_dir := env_var_or_default("APP_DIR", INSTALL_DIR / 'applications')
metainfo_dir := env_var_or_default("METAINFO_DIR", INSTALL_DIR / 'metainfo')
icon_dir := env_var_or_default("ICON_DIR", INSTALL_DIR / 'icons' / 'hicolor' / 'scalable' / 'apps')

# Default recipe - show available commands
default:
    @just --list

# Build debug version
build *args:
    cargo build {{args}}

# Build release version
build-release *args:
    cargo build --release {{args}}

# Run the applet (for testing)
run:
    cargo run -p {{name}}

# Install pre-built applet for current user (no sudo)
# Usage: cargo build --release && just install
install:
    @[ "$(id -u)" -ne 0 ] || [ -n "${BIN_DIR:-}" ] || [ -n "{{staged}}" ] || { echo "Run 'just install' WITHOUT sudo - this is a per-user install." >&2; exit 1; }
    install -Dm0755 "${CARGO_TARGET_DIR:-target}/release/{{name}}" {{bin_dir}}/{{name}}
    install -Dm0755 data/{{APPID}}.sh {{bin_dir}}/{{name}}.sh
    install -Dm0644 data/{{APPID}}.desktop {{app_dir}}/{{APPID}}.desktop
    @[ -n "${BIN_DIR:-}" ] || sed -i 's|^Exec=.*|Exec={{exec_dir}}/{{name}}|' {{app_dir}}/{{APPID}}.desktop
    install -Dm0644 data/{{APPID}}.metainfo.xml {{metainfo_dir}}/{{APPID}}.metainfo.xml
    install -Dm0644 data/icons/hicolor/scalable/apps/{{APPID}}.svg {{icon_dir}}/{{APPID}}.svg
    install -Dm0644 data/icons/hicolor/scalable/apps/{{APPID}}-symbolic.svg {{icon_dir}}/{{APPID}}-symbolic.svg
    install -Dm0644 data/icons/hicolor/scalable/apps/{{APPID}}-disconnected-symbolic.svg {{icon_dir}}/{{APPID}}-disconnected-symbolic.svg
    install -Dm0644 data/icons/hicolor/scalable/apps/{{APPID}}-merged-symbolic.svg {{icon_dir}}/{{APPID}}-merged-symbolic.svg
    install -Dm0644 data/icons/hicolor/scalable/apps/{{APPID}}-split-symbolic.svg {{icon_dir}}/{{APPID}}-split-symbolic.svg
    @echo "Installed {{name}} to {{bin_dir}}"
    @echo "Installed {{APPID}}.desktop to {{app_dir}}"
    @echo ""
    @echo "To add the applet to your panel:"
    @echo "  1. Open Settings > Desktop > Panel"
    @echo "  2. Click 'Add Widget' and find 'Connected'"
    @echo ""
    @echo "To reload after changes: killall cosmic-panel"
    @if [ -e /usr/bin/{{name}} ]; then \
        echo ""; \
        echo "NOTE: a system-wide install still exists at /usr/bin/{{name}}."; \
        echo "It is now shadowed but not removed. Clean it up with:"; \
        echo "    sudo just uninstall-system"; \
    fi

# Uninstall the per-user applet (no sudo)
uninstall:
    @[ "$(id -u)" -ne 0 ] || [ -n "${BIN_DIR:-}" ] || [ -n "{{staged}}" ] || { echo "Run 'just uninstall' WITHOUT sudo - this is a per-user install." >&2; exit 1; }
    rm -f {{bin_dir}}/{{name}}
    rm -f {{bin_dir}}/{{name}}.sh
    rm -f {{app_dir}}/{{APPID}}.desktop
    rm -f {{metainfo_dir}}/{{APPID}}.metainfo.xml
    rm -f {{icon_dir}}/{{APPID}}.svg
    rm -f {{icon_dir}}/{{APPID}}-symbolic.svg
    rm -f {{icon_dir}}/{{APPID}}-disconnected-symbolic.svg
    rm -f {{icon_dir}}/{{APPID}}-merged-symbolic.svg
    rm -f {{icon_dir}}/{{APPID}}-split-symbolic.svg
    @echo "Uninstalled {{name}}"
    @echo "Restart cosmic-panel to remove from panel: killall cosmic-panel"

# Remove a legacy system-wide install left by the old `sudo just install`
# Usage: sudo just uninstall-system
uninstall-system:
    rm -f /usr/bin/{{name}} /usr/bin/{{name}}.sh
    rm -f /usr/share/applications/{{APPID}}.desktop
    rm -f /usr/share/metainfo/{{APPID}}.metainfo.xml
    rm -f /usr/share/icons/hicolor/scalable/apps/{{APPID}}.svg
    rm -f /usr/share/icons/hicolor/scalable/apps/{{APPID}}-symbolic.svg
    rm -f /usr/share/icons/hicolor/scalable/apps/{{APPID}}-disconnected-symbolic.svg
    rm -f /usr/share/icons/hicolor/scalable/apps/{{APPID}}-merged-symbolic.svg
    rm -f /usr/share/icons/hicolor/scalable/apps/{{APPID}}-split-symbolic.svg
    @echo "Removed system-wide install from /usr"

# Run tests
test:
    cargo test

# Run tests with output
test-verbose:
    cargo test -- --nocapture

# Check code formatting
fmt-check:
    cargo fmt --check

# Format code
fmt:
    cargo fmt

# Run clippy lints
clippy:
    cargo clippy -- -D warnings

# Run all checks (format, clippy, test)
check: fmt-check clippy test

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update

# Check if kdeconnectd is running
check-daemon:
    @dbus-send --session --print-reply \
        --dest=org.kde.kdeconnect.daemon \
        /modules/kdeconnect \
        org.kde.kdeconnect.daemon.devices 2>/dev/null \
        && echo "✓ KDE Connect daemon is running" \
        || echo "✗ KDE Connect daemon is NOT running"

# List connected devices via D-Bus
list-devices:
    @dbus-send --session --print-reply \
        --dest=org.kde.kdeconnect.daemon \
        /modules/kdeconnect \
        org.kde.kdeconnect.daemon.devices

# Introspect KDE Connect D-Bus interface
introspect:
    @dbus-send --session --print-reply \
        --dest=org.kde.kdeconnect.daemon \
        /modules/kdeconnect \
        org.freedesktop.DBus.Introspectable.Introspect
