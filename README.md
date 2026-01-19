# COSMIC Connected

A native COSMIC desktop applet for phone-to-desktop connectivity, powered by KDE Connect.

COSMIC Connected connects your Android phone to your COSMIC desktop, enabling SMS messaging, file sharing, notifications, media control, and more—all through a native libcosmic interface.

Personal note: This project relied on Claude Code as a senior developer. My programming experience was at one time competent, but is outdated by decades. This project is much more than an afternoon vibe code - but the coding was done by Claude. It allowed me to create the app I needed, and I thought others might want it as well.

## Features

- **Device Management** - Pair, unpair, and monitor connected devices
- **SMS Messaging** - View conversations, read messages, reply, and compose new messages with contact lookup
- **File Sharing** - Send files and URLs to your phone
- **File Receive Notifications** - Get notified when files are received from your phone
- **Clipboard Sync** - Send clipboard content to your device
- **Notifications** - View and dismiss phone notifications from your desktop
- **Battery Status** - Monitor phone battery level and charging state
- **Media Controls** - Control music playback on your phone (play/pause, next/previous, volume)
- **Find My Phone** - Ring your phone to locate it
- **SMS Desktop Notifications** - Get notified when new SMS messages arrive (with privacy controls)
- **Call Notifications** - Get notified of incoming and missed calls (with privacy controls)
- **Ping** - Send a ping to locate your phone

## Requirements

- [COSMIC Desktop Environment](https://github.com/pop-os/cosmic-epoch)
- [KDE Connect](https://kdeconnect.kde.org/) daemon (`kdeconnectd`)
- [KDE Connect Android app](https://play.google.com/store/apps/details?id=org.kde.kdeconnect_tp) (or from [F-Droid](https://f-droid.org/packages/org.kde.kdeconnect_tp/))
- Rust toolchain (for building from source)

## Installation

### From Source

1. **Install KDE Connect daemon:**
   ```bash
   # Debian/Ubuntu/Pop!_OS
   sudo apt install kdeconnect

   # Fedora
   sudo dnf install kdeconnect

   # Arch
   sudo pacman -S kdeconnect
   ```

2. **Clone and build:**
   ```bash
   git clone https://github.com/nwxnw/cosmic-connect-applet.git
   cd cosmic-connect-applet
   cargo build --release
   ```

3. **Install to system:**
   ```bash
   sudo just install
   ```

4. **Add to panel:**
   - Open: Settings → Desktop → Panel → Add Widget
   - Find "Connected" and add it to your panel

### Uninstall

```bash
sudo just uninstall
```

## Usage

1. Install the KDE Connect app on your Android phone
2. Ensure both devices are on the same network
3. Click the Connected applet in your panel
4. Your phone should appear in the device list
5. Click on your phone and select "Pair" to establish a connection
6. Accept the pairing request on your phone

## Configuration

Settings are accessible through the applet's settings menu (gear icon). Options include:

- **Show battery percentage** - Display battery level in device list
- **Show offline devices** - Show paired devices that aren't currently connected
- **File notifications** - Enable desktop notifications for received files
- **SMS notifications** - Enable desktop notifications for incoming SMS
  - Show sender name (privacy option)
  - Show message content (privacy option)
- **Call notifications** - Enable desktop notifications for incoming/missed calls
  - Show contact name (privacy option)
  - Show phone number (privacy option)

Configuration is stored in `~/.config/cosmic/com.github.cosmic-connected-applet/v4/`

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  COSMIC Connected Applet (Rust/libcosmic)                      │
└──────────────────────┬──────────────────────────────────────────┘
                       │ D-Bus
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  kdeconnectd (KDE Connect daemon)                               │
└──────────────────────┬──────────────────────────────────────────┘
                       │ Network (TCP/UDP)
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  Android Phone (KDE Connect app)                                │
└─────────────────────────────────────────────────────────────────┘
```

COSMIC Connected acts as a native UI frontend to the KDE Connect daemon, communicating via D-Bus. The daemon handles all network connectivity, encryption, and protocol details.

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Check formatting
cargo fmt --check

# Run clippy lints
cargo clippy
```

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

### Development Tips

- Debug logging: `RUST_LOG=cosmic_applet_connected=debug cargo run`
- See `CLAUDE.md` for detailed development documentation

## License

This project is licensed under the GNU General Public License v3.0 - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [KDE Connect](https://kdeconnect.kde.org/) - The powerful daemon that makes this possible
- [System76](https://system76.com/) - For creating the COSMIC desktop and libcosmic
- [libcosmic](https://github.com/pop-os/libcosmic) - The COSMIC UI toolkit
