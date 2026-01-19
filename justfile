# Justfile for cosmic-connected-applet
# Install just: cargo install just

# Installation paths
prefix := '/usr'
bindir := prefix / 'bin'
sharedir := prefix / 'share'

# Applet metadata
applet_name := 'cosmic-applet-connected'
desktop_file := 'com.github.cosmic-connected-applet.desktop'

# Default recipe - show available commands
default:
    @just --list

# Build debug version
build:
    cargo build

# Build release version
build-release:
    cargo build --release

# Run the applet (for testing)
run:
    cargo run -p cosmic-applet-connected

# Run in standalone window mode (for development)
run-standalone:
    cargo run -p cosmic-applet-connected -- --standalone

# Run standalone with debug logging
run-debug:
    RUST_LOG=cosmic_applet_connected=debug cargo run -p cosmic-applet-connected -- --standalone

# Install the applet to the system (builds first, requires sudo)
# Note: May fail under sudo if cargo not in PATH. Use install-only instead.
install: build-release install-only

# Install pre-built applet to system (requires sudo, no build)
# Usage: cargo build --release && sudo ~/.cargo/bin/just install-only
install-only:
    install -Dm755 target/release/{{applet_name}} {{bindir}}/{{applet_name}}
    install -Dm644 data/{{desktop_file}} {{sharedir}}/applications/{{desktop_file}}
    @echo "Installed {{applet_name}} to {{bindir}}"
    @echo "Installed {{desktop_file}} to {{sharedir}}/applications"
    @echo ""
    @echo "To add the applet to your panel:"
    @echo "  1. Open Settings > Desktop > Panel"
    @echo "  2. Click 'Add Widget' and find 'Connected'"
    @echo ""
    @echo "To reload after changes: killall cosmic-panel"

# Uninstall the applet from the system (requires sudo)
uninstall:
    rm -f {{bindir}}/{{applet_name}}
    rm -f {{sharedir}}/applications/{{desktop_file}}
    @echo "Uninstalled {{applet_name}}"
    @echo "Restart cosmic-panel to remove from panel: killall cosmic-panel"

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
