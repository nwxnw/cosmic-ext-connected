# CLAUDE.md

Guidance for Claude Code when working with cosmic-ext-connected.

## Project Overview

Connected is a panel applet for the COSMIC™ desktop environment providing phone-to-desktop connectivity. It uses KDE Connect's daemon (`kdeconnectd`) as a D-Bus backend while providing a native libcosmic UI.

**Key Principle:** This project does NOT modify KDE Connect. It consumes kdeconnectd as a D-Bus service.

## Release Management

- **`main`** — sole development branch (no stable release branches yet)
- **`v0.1.0`** tag exists as a historical marker for the first GitHub release
- All work (features and fixes) goes to `main` directly or via `feature/*` branches
- A release branch will be created when the project is published to a Flatpak repository or package archive

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Connected Applet (Rust)                                        │
│  ├── cosmic-ext-connected/     (UI layer - libcosmic)          │
│  └── kdeconnect-dbus/          (D-Bus client - zbus)           │
└──────────────────────┬──────────────────────────────────────────┘
                       │ D-Bus (org.kde.kdeconnect.*)
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  kdeconnectd (system package: apt install kdeconnect)          │
└──────────────────────┬──────────────────────────────────────────┘
                       │ TCP/UDP/Bluetooth
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  Android Phone (KDE Connect app)                                │
└─────────────────────────────────────────────────────────────────┘
```

## Build Commands

```bash
cargo build                              # Build all crates
cargo build --release                    # Build release
cargo test && cargo clippy               # Test and lint
just install                             # Install (per-user, to ~/.local)
just uninstall                           # Uninstall
```

**Development cycle:** `cargo build --release && just install && killall cosmic-panel`

**Applets cannot be run standalone.** `cargo run` floods with resize messages, because outside the panel the host container is the wrong size. Use the development cycle above and read the journal instead.

**Flatpak build:**
```bash
flatpak-builder --user --install --force-clean build-dir io.github.nwxnw.cosmic-ext-connected.json
killall cosmic-panel                     # Reload panel
```

**Flatpak sandbox permissions** (in `finish-args` of manifest):
- `--talk-name=org.kde.kdeconnect` - the daemon's activatable bus name; a talk grant also lets the bus start it on demand. There is no `--socket=session-bus`, so every D-Bus peer needs its own entry (portals and `org.freedesktop.DBus` are implicit)
- `--talk-name=org.freedesktop.Notifications` - desktop notifications via `notify-rust`
- `--filesystem=xdg-config/cosmic:rw` - read/write COSMIC config (file-based; `dbus-config` is not compiled in, so `com.system76.CosmicSettingsDaemon` is not needed)
- `--filesystem=xdg-data/kpeoplevcard:ro` — read contacts for SMS name resolution
- `--filesystem=xdg-cache/kdeconnect.daemon:ro` — read MMS attachment cache (daemon uses Qt app name `"kdeconnect.daemon"` → cache at `~/.cache/kdeconnect.daemon/`)

**Debug logs:** `journalctl --user SYSLOG_IDENTIFIER=cosmic-ext-connected -f` (logs land here directly via `tracing_journald`; see `docs/LOGGING.md` for routing details)

## Project Structure

```
cosmic-ext-connected/
├── cosmic-ext-connected/src/
│   ├── main.rs              # Entry point
│   ├── app.rs               # Core: ConnectApplet, Message enum, update()
│   ├── config.rs            # User preferences (cosmic_config)
│   ├── constants.rs         # Timeouts, page sizes, sentinel values
│   ├── i18n.rs              # Fluent localization boilerplate
│   ├── notifications.rs     # Cross-process notification deduplication
│   ├── subscriptions.rs     # D-Bus signal subscriptions
│   ├── device/              # mod / fetch / class / actions
│   ├── media/               # mod / fetch / views
│   ├── sms/
│   │   ├── conversation_subscription.rs  # List-loading state machine + cached emit
│   │   ├── fetch.rs                      # Conversation fetching
│   │   ├── logical.rs                    # LogicalConversation: reaction-bucket merging
│   │   ├── send.rs                       # replyToConversation / sendWithoutConversation
│   │   ├── store.rs                      # SmsConversationStore (cache + merge state)
│   │   └── views.rs                      # Conversation list and thread views
│   ├── ui/                  # Device list / device page / shared widgets
│   └── views/               # Settings, Send-To, view helpers
│
├── data/
│   ├── io.github.nwxnw.cosmic-ext-connected.desktop
│   ├── io.github.nwxnw.cosmic-ext-connected.metainfo.xml
│   └── icons/hicolor/scalable/apps/
│       ├── io.github.nwxnw.cosmic-ext-connected.svg                          # App icon (128x128, #BEBEBE fill)
│       ├── io.github.nwxnw.cosmic-ext-connected-symbolic.svg                  # Panel: connected state
│       └── io.github.nwxnw.cosmic-ext-connected-disconnected-symbolic.svg     # Panel: disconnected state
│
├── io.github.nwxnw.cosmic-ext-connected.json  # Flatpak manifest
│
├── kdeconnect-dbus/src/
│   ├── daemon.rs            # org.kde.kdeconnect.daemon proxy
│   ├── device.rs            # Device interface proxy
│   ├── contacts.rs          # ContactLookup: vCard parsing, name resolution, group display names
│   └── plugins/             # Per-plugin D-Bus proxies
│
└── docs/                    # Detailed implementation docs
    ├── DBUS.md              # D-Bus interface reference + pitfalls
    ├── SMS.md               # SMS loading, merging, optimistic send, MMS cache
    ├── NOTIFICATIONS.md     # SMS/call/file notifications, dedup, ping limitation
    ├── MEDIA.md             # Media controls
    ├── UI_PATTERNS.md       # libcosmic UI patterns
    ├── LOGGING.md           # Tracing/journald routing for diagnostics
    ├── KNOWN_ISSUES.md      # Known issues and workarounds
    └── CHANGELOG.md         # Version history
```

## Key Conventions

### Internationalization
All user-visible text must use `fl!()` macro:
```rust
text(fl!("devices"))
text(fl!("battery-level", level = battery_percent))
```

### D-Bus Property Naming
KDE Connect uses camelCase. Always specify explicit names in zbus:
```rust
#[zbus(property, name = "isCharging")]
fn is_charging(&self) -> zbus::Result<bool>;
```

### Module Organization
- `Message` enum stays in `app.rs` - all modules import from app
- `ConnectApplet` struct stays in `app.rs` - centralized state
- View functions take params structs, return `Element<Message>`
- Async functions are standalone, return `Message` variants

### Icons
- **Panel icons** use `fill="currentColor"` (symbolic) so COSMIC themes them automatically
- **App icon** uses hardcoded `fill="#BEBEBE"` because COSMIC Settings does not theme non-symbolic app icons
- **All icon filenames are prefixed with the app ID** (`io.github.nwxnw.cosmic-ext-connected*`) so Flatpak exports them to the host. Icons without the app-ID prefix are silently excluded from Flatpak export (non-allowed export filename), breaking icon rendering in COSMIC Settings for Flatpak installs.
- Device icon everywhere is `"io.github.nwxnw.cosmic-ext-connected-symbolic"` (our custom mobile phone icon)
- Panel connected state: `"io.github.nwxnw.cosmic-ext-connected-symbolic"`, disconnected: `"io.github.nwxnw.cosmic-ext-connected-disconnected-symbolic"`
- Disconnected icon follows Pop convention: main element at `opacity="0.35"`, X indicator at full opacity

### UI Theming
- Back navigation buttons use `cosmic::theme::Button::Link` (accent-colored icon and text, no background)
- Headings adjacent to back buttons use `cosmic::theme::Text::Accent`
- To accent-color an icon widget, convert `Named` to `Icon` first, then apply `Svg::custom`:
  ```rust
  icon::from_name("icon-name").size(48).icon()
      .class(cosmic::theme::Svg::custom(|theme| {
          cosmic::iced::widget::svg::Style {
              color: Some(theme.cosmic().accent_text_color().into()),
          }
      }))
  ```
  Note: `svg::Style` lives at `cosmic::iced::widget::svg::Style` (the consumer re-export). Reaching for `cosmic::iced_widget::svg::Style` — which is libcosmic's *internal* path — won't resolve from consumer crates.

### Code Style
- Follow rustfmt and clippy
- Prefer explicit error handling over `.unwrap()`
- Use `fl!()` for all UI strings

## Daemon Interaction — standing rules

Hard-won constraints on how this applet talks to `kdeconnectd`. Each is here because violating
it has cost real time. Mechanism lives in `docs/SMS.md` / `docs/DBUS.md` / `docs/UI_PATTERNS.md`.

- **Signals are broadcasts, not request/response.** No request ID, no correlation to the caller,
  no guarantee a call emits anything. **The tell is a timeout** — if completion is detected by
  waiting out a clock, that is the bug. Derive completion locally.
- **Capture before naming the culprit.** A mechanism read off a plausible code path has been
  wrong here repeatedly. Before proposing a fix: `dbus-monitor` unfiltered, or one
  `tracing::debug!`, or the journal already on disk. **`dbus-monitor` is the authority on signal
  order; the journal is not** — the subscription logs at stream-read time and `update()` logs at
  process time (`docs/LOGGING.md`).
- **Never send a `requestConversation` offset > 0.** An out-of-range `start` segfaults
  kdeconnectd 23.08.5 and silently returns nothing on newer daemons. Pin `start = 0` and widen
  the range end; clamping against a known count is circular (`docs/DBUS.md`).
- **Name the thing that clears any new state field.** A subscription may not clear a flag it did
  not set; it terminates itself with `Done`. Every instance of this bug so far has been the same
  shape — one field with two owners, typically a view-lifecycle flag written by an error path.
- **`Ok` is not an acknowledgement, and `isReachable` is not reachability.** A void D-Bus method
  cannot report delivery; `isReachable` reports that a link *object* exists (`docs/DBUS.md`).
- **In `app.rs` `update()` arms, check whether a new statement runs on every path or inside the
  guard above it.** Clippy stays silent; the tell is the indent. `cargo fmt` before committing.
- **Scrollable state is matched positionally, never by name.** `.id()` on a scrollable is an
  operation target only, and `set_id` overwrites it (`docs/UI_PATTERNS.md`).

## Detailed Documentation

Implementation details live under `docs/`. Read the relevant file before working in that area — most cross-cutting pitfalls (the two `requestConversation` methods, `conversationLoaded` unreliability, subscription phase machines, optimistic-send reconciliation, MMS cache path, COSMIC notification daemon's `expire_timeout` quirk, etc.) are documented there, not in this file.

- **[docs/DBUS.md](docs/DBUS.md)** — D-Bus interface reference, testing commands, signal subscription, surface-level pitfalls
- **[docs/SMS.md](docs/SMS.md)** — Conversation list and thread loading, reaction-bucket merging, sending and optimistic reconciliation, MMS attachment cache
- **[docs/NOTIFICATIONS.md](docs/NOTIFICATIONS.md)** — SMS/call/file notifications, cross-process deduplication, ping limitation
- **[docs/MEDIA.md](docs/MEDIA.md)** — Media player controls, `sendAction` pattern
- **[docs/UI_PATTERNS.md](docs/UI_PATTERNS.md)** — libcosmic patterns, ViewMode, popups
- **[docs/LOGGING.md](docs/LOGGING.md)** — Tracing events to systemd journal via `tracing_journald`
- **[docs/KNOWN_ISSUES.md](docs/KNOWN_ISSUES.md)** — Known issues and workarounds
- **[docs/CHANGELOG.md](docs/CHANGELOG.md)** — Version history

## External Resources

- [COSMIC Applets](https://github.com/pop-os/cosmic-applets) - Reference implementations
- [libcosmic](https://github.com/pop-os/libcosmic) - UI toolkit
- [zbus Book](https://dbus2.github.io/zbus/) - D-Bus client docs
- [KDE Connect](https://invent.kde.org/network/kdeconnect-kde) - Protocol reference
