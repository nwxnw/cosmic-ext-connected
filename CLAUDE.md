# CLAUDE.md

This file provides guidance to Claude Code when working with the cosmic-connected-applet project.

## Project Overview

COSMIC Connected is a desktop applet for the COSMIC desktop environment (System76's Rust-based DE) that provides phone-to-desktop connectivity. It leverages KDE Connect's daemon as a backend service while providing a native COSMIC/libcosmic user interface.

**Key Principle:** This project does NOT modify KDE Connect. It consumes the KDE Connect daemon (`kdeconnectd`) as a D-Bus service and builds a completely new UI using libcosmic.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  COSMIC Connected Applet (Rust)                                   │
│  ├── cosmic-applet-connected/  (UI layer - libcosmic)            │
│  └── kdeconnect-dbus/        (D-Bus client - zbus)             │
└──────────────────────┬──────────────────────────────────────────┘
                       │ D-Bus (org.kde.kdeconnect.*)
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  kdeconnectd (KDE Connect daemon)                               │
│  - Installed via system package (apt install kdeconnect)        │
│  - Handles: device discovery, encryption, pairing, protocols    │
└──────────────────────┬──────────────────────────────────────────┘
                       │ TCP/UDP/Bluetooth
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│  Android Phone (KDE Connect app from Play Store/F-Droid)        │
└─────────────────────────────────────────────────────────────────┘
```

### Design Decisions

1. **KDE Connect as D-Bus Service**: Use the system-installed kdeconnectd daemon. Do not embed, fork, or modify KDE Connect source code.

2. **Complete UI Replacement**: Build all user-facing UI in libcosmic. The KDE Connect Qt/QML apps are not used.

3. **Separate D-Bus Crate**: Isolate D-Bus interface code in `kdeconnect-dbus/` crate for clean separation and potential reuse.

4. **libcosmic as Dependency**: Use libcosmic via Cargo dependency, not as a submodule.

## Project Structure

```
cosmic-connected-applet/
├── CLAUDE.md                     # This file
├── Cargo.toml                    # Workspace root
├── Cargo.lock
├── rust-toolchain.toml           # Pin Rust version
├── justfile                      # Build automation (includes install/uninstall)
│
├── data/                         # Desktop entry for applet registration
│   └── com.github.cosmic-connected-applet.desktop
│
├── cosmic-applet-connected/        # Main applet crate
│   ├── Cargo.toml
│   ├── i18n.toml                # Fluent localization config
│   ├── i18n/                    # Translation files
│   │   └── en/                  # English translations
│   │       └── cosmic-applet-connected.ftl
│   └── src/
│       ├── main.rs              # Entry point
│       ├── app.rs               # Panel applet state & logic
│       ├── config.rs            # User preferences
│       ├── i18n.rs              # Localization module with fl!() macro
│       └── ui/
│           ├── mod.rs
│           ├── device_list.rs   # Device listing view
│           ├── device_page.rs   # Individual device view
│           └── widgets/         # Reusable UI components
│
├── kdeconnect-dbus/              # D-Bus interface crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs               # Crate root, exports proxies
│       ├── error.rs             # Error types
│       ├── daemon.rs            # org.kde.kdeconnect.daemon proxy
│       ├── device.rs            # Device interface proxy
│       ├── contacts.rs          # Contact lookup from synced vCards
│       └── plugins/             # Per-plugin D-Bus proxies
│           ├── mod.rs
│           ├── battery.rs       # Battery status plugin
│           ├── clipboard.rs     # Clipboard sync plugin
│           ├── findmyphone.rs   # Find my phone plugin (ring device)
│           ├── mprisremote.rs   # Media player remote control plugin
│           ├── notifications.rs # Notifications plugin
│           ├── ping.rs          # Ping plugin
│           ├── share.rs         # File/URL sharing plugin
│           ├── sms.rs           # SMS/conversations plugin
│           └── telephony.rs     # Telephony plugin (call notifications)
│
├── docs/                         # Additional documentation
│   ├── CHANGELOG.md             # Development history
│   └── DBUS.md                  # D-Bus testing commands reference
│
└── reference/                    # Reference material (gitignored)
    └── kdeconnect-kde/          # KDE Connect source clone
```

## Technology Stack

- **Language**: Rust (edition 2021)
- **UI Framework**: libcosmic (System76's COSMIC toolkit, built on Iced)
- **D-Bus Client**: zbus with tokio async runtime
- **Backend**: kdeconnectd (system package)

## Build Commands

```bash
# Build all crates
cargo build

# Build release
cargo build --release

# Run as panel applet (requires COSMIC desktop)
cargo run -p cosmic-applet-connected

# Run tests
cargo test

# Check formatting
cargo fmt --check

# Run clippy lints
cargo clippy

# Install applet to system (for panel testing)
just install

# Uninstall applet
just uninstall
```

## Development & Testing Workflow

### Applet Registration

COSMIC discovers panel applets through `.desktop` files with special keys:

```ini
[Desktop Entry]
Name=Connected
Type=Application
Exec=cosmic-applet-connected
Icon=phone-symbolic
NoDisplay=true
X-CosmicApplet=true
X-CosmicHoverPopup=Auto
```

Key fields:
- `X-CosmicApplet=true` - Identifies this as a panel applet (required)
- `X-CosmicHoverPopup=Auto` - Controls popup behavior on hover
- `NoDisplay=true` - Hides from application launcher (applets aren't standalone apps)

### Installation Workflow

```bash
# 1. Build release version
cargo build --release

# 2. Install to system (copies binary and .desktop file)
sudo just install

# 3. Add applet to panel
#    Open: Settings > Desktop > Panel > Add Widget
#    Find "Connected" in the list and add it

# 4. After code changes, rebuild and reinstall
cargo build --release && sudo just install

# 5. Restart panel to load updated applet
#    Either: Log out and back in
#    Or: killall cosmic-panel (it auto-restarts)
```

### Development Tips

**Testing Changes:**
- Build release and install: `cargo build --release && sudo just install`
- Restart panel: `killall cosmic-panel`
- Panel auto-restarts and loads the updated applet

**Debug Logging:**
```bash
# View panel applet logs (applet runs as part of cosmic-panel)
journalctl --user -f | grep cosmic-applet-connected
```

### Uninstallation

```bash
# Remove from system
sudo just uninstall

# The applet will disappear from panel on next restart
```

## Dependencies

### System Requirements
- KDE Connect daemon: `sudo apt install kdeconnect`
- Rust toolchain: Install via rustup
- COSMIC desktop environment

### Key Cargo Dependencies
- `libcosmic` - COSMIC UI toolkit
- `zbus` - D-Bus client library
- `tokio` - Async runtime
- `serde` / `serde_json` - Serialization
- `chrono` - Date/time formatting
- `dirs` - Platform-specific directory paths
- `rfd` - Native file dialogs
- `i18n-embed` / `i18n-embed-fl` - Fluent localization system
- `rust-embed` - Embed translation files at compile time

## D-Bus Interface Reference

KDE Connect exposes these key D-Bus interfaces:

| Interface | Path | Purpose |
|-----------|------|---------|
| `org.kde.kdeconnect.daemon` | `/modules/kdeconnect` | Device discovery, announcements |
| `org.kde.kdeconnect.device` | `/modules/kdeconnect/devices/<id>` | Per-device operations, pairing |
| `org.kde.kdeconnect.device.battery` | (same + /battery) | Battery status (charge, isCharging) |
| `org.kde.kdeconnect.device.clipboard` | (same + /clipboard) | Clipboard sync |
| `org.kde.kdeconnect.device.findmyphone` | (same + /findmyphone) | Trigger phone to ring |
| `org.kde.kdeconnect.device.mprisremote` | (same + /mprisremote) | Media player control (play, pause, volume) |
| `org.kde.kdeconnect.device.ping` | (same + /ping) | Send ping to device |
| `org.kde.kdeconnect.device.notifications` | (same + /notifications) | List active notifications |
| `org.kde.kdeconnect.device.notifications.notification` | (same + /notifications/<id>) | Individual notification details |
| `org.kde.kdeconnect.device.share` | (same + /share) | File/URL sharing |
| `org.kde.kdeconnect.device.sms` | (same + /sms) | Request SMS conversations |
| `org.kde.kdeconnect.device.conversations` | `/modules/kdeconnect/devices/<id>` | SMS conversation data and signals |
| `org.kde.kdeconnect.device.telephony` | (same + /telephony) | Incoming/missed call notifications |

### D-Bus Property Naming

KDE Connect uses camelCase for D-Bus property names (e.g., `isCharging`, `isPairRequested`). When using zbus, explicitly specify property names with the `#[zbus(property, name = "...")]` attribute to avoid case mismatch issues:

```rust
#[zbus(property, name = "isCharging")]
fn is_charging(&self) -> zbus::Result<bool>;
```

For D-Bus testing commands, see `docs/DBUS.md`.

## Code Style

- Follow Rust standard conventions (rustfmt)
- Use `clippy` for linting
- Prefer explicit error handling over `.unwrap()` in production code
- Document public APIs with rustdoc comments

## Internationalization (i18n)

The applet uses the Fluent localization system following COSMIC app patterns. **All user-visible text must use the `fl!()` macro** instead of hardcoded strings.

### File Structure

```
cosmic-applet-connected/
├── i18n.toml                           # Fluent configuration
├── i18n/
│   └── en/
│       └── cosmic-applet-connected.ftl   # English translations
└── src/
    └── i18n.rs                         # fl!() macro and initialization
```

### Using the fl!() Macro

**Always use `fl!()` for UI text - never use hardcoded strings:**

```rust
use crate::fl;

// Simple translation
text(fl!("devices"))

// Translation with arguments
text(fl!("battery-level", level = battery_percent))

// Button labels
widget::button::standard(fl!("send-ping"))
```

### Translation File Format (Fluent)

Translations are defined in `.ftl` files using Fluent syntax:

```ftl
# Simple messages
devices = Devices
send-ping = Send Ping
loading = Loading...

# Messages with variables
battery-level = { $level }%
messages-title = Messages - { $device }
```

### Adding New Translations

1. Add the message key and English text to `i18n/en/cosmic-applet-connected.ftl`
2. Use `fl!("message-key")` in code
3. For new languages, create `i18n/<lang>/cosmic-applet-connected.ftl` (e.g., `i18n/de/`, `i18n/es/`)

### Important: Lifetime Handling with fl!()

The `fl!()` macro returns an owned `String`, not a `&'static str`. This affects how you use it with widgets:

**For text widgets and buttons** - pass directly (they accept owned strings):
```rust
text(fl!("label"))
widget::button::standard(fl!("button-text"))
```

**For text_input placeholders** - pass directly without `&` (converts to `Cow::Owned`):
```rust
// Correct - passes owned String
widget::text_input(fl!("placeholder"), &self.input_value)

// Incorrect - creates temporary reference that won't live long enough
widget::text_input(&fl!("placeholder"), &self.input_value)  // Won't compile!
```

**For fallback values with unwrap_or** - pre-compute the default:
```rust
// Correct - default_name lives for the scope
let default_name = fl!("unknown");
let name = self.device_name.as_deref().unwrap_or(&default_name);

// Incorrect - temporary is dropped immediately
let name = self.device_name.as_deref().unwrap_or(&fl!("unknown"));  // Won't compile!
```

### Locale Detection

The system automatically detects the user's locale on startup via `i18n_embed::DesktopLanguageRequester`. No manual configuration is needed.

## Configuration System

The applet uses COSMIC's configuration system (`cosmic_config`) for persistent settings.

### Config Location

Settings are stored in `~/.config/cosmic/com.github.cosmic-connected-applet/v4/`

### Config Struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize, CosmicConfigEntry, PartialEq, Eq)]
#[version = 3]
pub struct Config {
    pub show_battery_percentage: bool,       // Show battery % in device list
    pub show_offline_devices: bool,          // Show paired but offline devices
    pub forward_notifications: bool,         // Enable desktop notifications
    pub messages_per_page: u32,              // SMS messages to load per request
    pub sms_notifications: bool,             // Enable SMS desktop notifications
    pub sms_notification_show_content: bool, // Show message content (privacy)
    pub sms_notification_show_sender: bool,  // Show sender name (privacy)
    pub call_notifications: bool,            // Enable call desktop notifications
    pub call_notification_show_number: bool, // Show phone number (privacy)
    pub call_notification_show_name: bool,   // Show contact name (privacy)
}
```

### Usage Pattern

```rust
// Load on startup
let config = Config::load();

// Save when settings change
if let Err(err) = self.config.save() {
    tracing::error!(?err, "Failed to save config");
}

// Watch for external changes via subscription
self.core.watch_config::<Config>(crate::config::APP_ID)
    .map(|update| Message::ConfigChanged(update.config))
```

## libcosmic Patterns

### Async Tasks
Use `Task::perform` with `cosmic::Action::App` wrapper for async operations:

```rust
cosmic::app::Task::perform(
    async { /* async work */ },
    |result| cosmic::Action::App(Message::from(result)),
)
```

### Popup Windows

Use the standard COSMIC applet popup helpers for reliable toggle behavior:

```rust
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};

// In your Message enum
enum Message {
    TogglePopup,
    PopupClosed(window::Id),
    // ...
}

// In update()
Message::TogglePopup => {
    return if let Some(popup_id) = self.popup.take() {
        // Close existing popup
        destroy_popup(popup_id)
    } else {
        // Open new popup
        let new_id = window::Id::unique();
        self.popup.replace(new_id);

        let popup_settings = self.core.applet.get_popup_settings(
            self.core.main_window_id().unwrap(),
            new_id,
            None,
            None,
            None,
        );

        get_popup(popup_settings)
    };
}

Message::PopupClosed(id) => {
    if self.popup == Some(id) {
        self.popup = None;
    }
}
```

**Important:** Use `destroy_popup()` and `get_popup()` helpers instead of manual runtime actions. The manual approach can cause issues where clicking the panel icon to close the popup prevents reopening it.

### View Lifetimes
Use explicit lifetime annotations in view functions:

```rust
fn view(&self) -> Element<'_, Self::Message>
fn view_window(&self, _id: window::Id) -> Element<'_, Self::Message>
```

### Popup Width Management

The applet uses two popup widths depending on the view:

```rust
const DEFAULT_POPUP_WIDTH: f32 = 360.0;  // Standard libcosmic width
const WIDE_POPUP_WIDTH: f32 = 450.0;     // For SMS/media views
```

Views using **wide popup** (450px):
- `ConversationList` - SMS conversation list
- `MessageThread` - SMS message thread
- `NewMessage` - Compose new SMS
- `MediaControls` - Media player controls

Views using **default popup** (360px):
- `DeviceList` - Main device list
- `DevicePage` - Individual device view
- `Settings` - Settings panel
- `SendTo` - Send to device submenu

The `popup_container()` method takes a width parameter:

```rust
fn popup_container<'a>(&self, content: impl Into<Element<'a, Message>>, width: f32) -> Element<'a, Message>
```

## UI Navigation and View Modes

### ViewMode Enum

The applet uses a `ViewMode` enum to track the current view:

```rust
pub enum ViewMode {
    DeviceList,       // Main device list
    DevicePage,       // Individual device details
    SendTo,           // "Send to device" submenu
    ConversationList, // SMS conversations
    MessageThread,    // SMS message thread
    NewMessage,       // Compose new SMS
    Settings,         // Settings panel
    MediaControls,    // Media player controls
}
```

### Device Page Layout

When viewing a connected device, the page is organized as:

1. **Header** - Back button, device icon, name, type, status, battery
2. **Actions** (clickable list items):
   - SMS Messages → Opens ConversationList (with chevron)
   - Send to [device] → Opens SendTo submenu (with chevron)
   - Media Controls → Opens MediaControls (with chevron)
   - Find Phone → Triggers phone to ring (no chevron - immediate action)
3. **Pairing section** - Pair/unpair buttons based on state
4. **Notifications section** - List of device notifications (if any)

### SendTo Submenu

The "Send to [device]" submenu consolidates sending actions:

1. **Back button** - Returns to device page
2. **Share file** - Opens file picker (list item)
3. **Send Clipboard** - Sends current clipboard contents (list item)
4. **Send Ping** - Sends ping to device (list item)
5. **Divider**
6. **Share text** - Text input with Send button

Items 2-4 use the clickable list item pattern (without chevrons since they perform immediate actions rather than navigating to another view).

### Clickable List Item Pattern

Actions use a clickable list item style for consistent full-width layout:

```rust
// For navigation items (with chevron)
let row = row![
    icon::from_name("icon-name").size(24),
    text(fl!("label")).size(14),
    widget::horizontal_space(),
    icon::from_name("go-next-symbolic").size(16),  // Chevron for navigation
]
.spacing(12)
.align_y(Alignment::Center);

// For action items (no chevron)
let row = row![
    icon::from_name("icon-name").size(24),
    text(fl!("label")).size(14),
    widget::horizontal_space(),
]
.spacing(12)
.align_y(Alignment::Center);

// Both use the same button wrapper
widget::button::custom(
    widget::container(row).padding(8).width(Length::Fill),
)
.class(cosmic::theme::Button::Text)
.on_press(Message::SomeAction)
.width(Length::Fill)
```

**When to use chevrons:** Include the `go-next-symbolic` chevron icon for items that navigate to another view (e.g., SMS Messages, Media Controls). Omit it for items that perform immediate actions (e.g., Share file, Send Clipboard, Send Ping, Find Phone).

## D-Bus Signal Subscription

To receive real-time updates from KDE Connect (e.g., pairing state changes), subscribe to D-Bus signals using match rules:

```rust
use zbus::fdo::DBusProxy;

// Add match rule for KDE Connect signals
let dbus_proxy = DBusProxy::new(&conn).await?;
let rule = zbus::MatchRule::builder()
    .msg_type(zbus::message::Type::Signal)
    .sender("org.kde.kdeconnect.daemon")
    .map(|b| b.build())?;
dbus_proxy.add_match_rule(rule).await?;

// Create message stream and filter for relevant signals
let stream = zbus::MessageStream::from(&conn);
```

Without explicit match rules, D-Bus signals may not be delivered to the application.

## SMS Implementation Notes

### Signal-Based Data Fetching

Both conversation lists and individual messages are fetched using D-Bus signals rather than polling. This provides reliable loading regardless of phone response time.

#### Conversation List Loading

The conversation list is loaded via `fetch_conversations_async` which:

1. Subscribes to `conversationCreated`, `conversationUpdated`, and `conversationLoaded` signals
2. Loads cached conversations from `activeConversations()` first (instant display)
3. Calls `requestAllConversationThreads()` to trigger fresh data from the phone
4. Collects conversations from signals using activity-based timeout:
   - Stops 500ms after the last signal (once data starts arriving)
   - Hard timeout of 15 seconds maximum
5. Falls back to polling if signal subscription fails

```rust
// Signal-based loading with activity timeout
let activity_timeout = Duration::from_millis(500);
let overall_timeout = Duration::from_secs(15);

loop {
    tokio::select! {
        Some(signal) = created_stream.next() => {
            // New conversation, add to map
            last_activity = Instant::now();
        }
        Some(signal) = updated_stream.next() => {
            // Updated conversation, update if newer
            last_activity = Instant::now();
        }
        Some(_) = loaded_stream.next() => {
            // Activity indicator
            loaded_signal_received = true;
            last_activity = Instant::now();
        }
        _ = sleep(Duration::from_millis(50)) => {
            // Check timeouts
            if loaded_signal_received && last_activity.elapsed() >= activity_timeout {
                break; // Done - no signals for 500ms
            }
            if start_time.elapsed() >= overall_timeout {
                break; // Hard timeout
            }
        }
    }
}
```

#### Message Thread Loading

Individual message threads use a similar pattern:

1. Subscribe to `conversationUpdated` and `conversationLoaded` signals
2. Call `requestConversation(thread_id, start, count)` to request messages
3. Collect messages from `conversationUpdated` signals as they arrive
4. Stop collecting when `conversationLoaded` signal is received for that thread

### Conversation List Caching

The conversation list is cached in memory to provide instant display when returning to the SMS view.

**Caching behavior:**
- When navigating back from SMS to device page, conversations are preserved in memory
- When re-opening SMS for the **same device**, cached conversations display immediately
- A background refresh runs to fetch any new conversations
- When switching to a **different device**, cache is cleared and fresh data is loaded

**State preservation:**
```rust
// OpenSmsView checks for cached data
let same_device = self.sms_device_id.as_ref() == Some(&device_id);
let has_cache = same_device && !self.conversations.is_empty();

if has_cache {
    self.sms_loading = false;  // Show cached data immediately
    // Trigger background refresh
} else {
    self.sms_loading = true;   // Show loading spinner
    self.conversations.clear();
}

// CloseSmsView preserves cache
Message::CloseSmsView => {
    self.view_mode = ViewMode::DevicePage;
    // Keep: sms_device_id, conversations, contacts, message_cache
    // Clear: messages, current_thread_id, sms_compose_text
}
```

**Message cache:** Individual message threads are also cached in `message_cache: HashMap<i64, Vec<SmsMessage>>`. When opening a conversation, cached messages display immediately while a background refresh runs.

### Contact Name Resolution

KDE Connect syncs contacts as vCard files to `~/.local/share/kpeoplevcard/kdeconnect-{device-id}/`. The `ContactLookup` struct parses these files and provides phone number to name mapping:

```rust
let contacts = ContactLookup::load_for_device(&device_id);
let name = contacts.get_name_or_number("+15551234567"); // Returns "John Doe" or the number
```

### Message Type Constants

Android SMS type values (from `msg.message_type`):
- `1` = MESSAGE_TYPE_INBOX (received)
- `2` = MESSAGE_TYPE_SENT
- `3` = MESSAGE_TYPE_DRAFT
- `4` = MESSAGE_TYPE_OUTBOX
- `5` = MESSAGE_TYPE_FAILED
- `6` = MESSAGE_TYPE_QUEUED

**D-Bus Struct Field Order**

The message struct from KDE Connect has the following field order (from `conversationmessage.h`):
- Field 0: `eventField` (i32) - Event flags (e.g., 1 = text message)
- Field 1: `body` (string) - Message text
- Field 2: `addresses` (array) - List of phone numbers
- Field 3: `date` (i64) - Timestamp
- Field 4: `type` (i32) - **Message type** (1=Inbox/received, 2=Sent)
- Field 5: `read` (i32) - Read status
- Field 6: `threadID` (i64) - Conversation thread ID
- Field 7: `uID` (i32) - Unique message ID
- Field 8: `subID` (i64) - SIM ID
- Field 9: `attachments` (array) - Attachment list

The message direction is determined by the `type` field at position 4:
```rust
// MessageType::Inbox (1) = incoming/received, MessageType::Sent (2) = outgoing/sent
let is_received = msg.message_type == MessageType::Inbox;
```

## SMS Desktop Notifications

The applet shows desktop notifications when new SMS messages are received.

### Implementation

1. **D-Bus Signal Subscription**: A separate subscription (`sms_notification_subscription`) listens for `conversationUpdated` signals from `org.kde.kdeconnect.device.conversations`.

2. **Message Filtering**: Only incoming messages are notified (MessageType::Inbox).

3. **Deduplication**: A `last_seen_sms: HashMap<i64, i64>` tracks the latest seen timestamp per thread_id to prevent duplicate notifications.

4. **Contact Resolution**: Sender names are resolved via `ContactLookup` using synced vCard files.

5. **Privacy Settings**: Users can control notification content:
   - `sms_notifications` - Master toggle for SMS notifications
   - `sms_notification_show_sender` - Show/hide sender name
   - `sms_notification_show_content` - Show/hide message preview

### Notification Display

Notifications are shown using `notify-rust` (freedesktop notification protocol):

```rust
notify_rust::Notification::new()
    .summary(&summary)  // "New SMS" or "New SMS from {name}"
    .body(&body)        // Message content or "Message received"
    .icon("phone-symbolic")
    .appname("COSMIC Connected")
    .show()
```

### Subscription Lifecycle

The SMS notification subscription is active when:
- `config.sms_notifications` is enabled
- At least one device is both reachable AND paired

The subscription automatically reconnects on D-Bus disconnection.

## Call Notifications

The applet shows desktop notifications for incoming and missed phone calls.

### D-Bus Signal

The telephony plugin emits a `callReceived` signal with three string arguments:
- `event` - "callReceived" for incoming call, "missedCall" for missed call
- `phone_number` - The caller's phone number
- `contact_name` - Contact name if available, otherwise same as phone number

### Implementation

1. **D-Bus Signal Subscription**: A separate subscription (`call_notification_subscription`) listens for `callReceived` signals from `org.kde.kdeconnect.device.telephony`.

2. **Device Name Resolution**: The device name is fetched via `DeviceProxy` to show which phone received the call.

3. **Privacy Settings**: Users can control notification content:
   - `call_notifications` - Master toggle for call notifications
   - `call_notification_show_name` - Show/hide contact name
   - `call_notification_show_number` - Show/hide phone number

### Notification Display

```rust
notify_rust::Notification::new()
    .summary(&summary)  // "Incoming Call" or "Incoming call from {name}"
    .body(&device_name) // Which device received the call
    .icon("call-start-symbolic")  // or "call-missed-symbolic" for missed
    .appname("COSMIC Connected")
    .urgency(notify_rust::Urgency::Critical)  // Incoming calls are high priority
    .show()
```

### Limitation: Mute Ringer

The KDE Connect daemon handles ringer muting internally via KNotification actions. There's no D-Bus method exposed to mute the ringer programmatically from external applications. This would require upstream KDE Connect changes.

## File Receive Notifications

The applet shows desktop notifications when files are received from connected devices.

### D-Bus Signal

The share plugin emits a `shareReceived` signal with a single string argument:
- `file_url` - The file:// URL of the received file (e.g., `file:///home/user/Downloads/photo.jpg`)

### Implementation

1. **D-Bus Signal Subscription**: The main D-Bus subscription listens for `shareReceived` signals from `org.kde.kdeconnect.device.share`.

2. **Cross-Process Deduplication**: COSMIC spawns multiple applet processes, and KDE Connect sends 3 duplicate signals per file transfer. A file-based lock mechanism (`/tmp/cosmic-connected-file-dedup`) ensures only one notification is shown:
   - Uses `libc::flock()` for atomic file locking across processes
   - Stores last file URL and timestamp
   - Deduplication window of 2 seconds

3. **Privacy Settings**: Users can enable/disable file notifications:
   - `file_notifications` - Master toggle for file notifications

### Notification Display

```rust
notify_rust::Notification::new()
    .summary(&fl!("file-received-from", device = device_name))
    .body(&file_name)
    .icon("folder-download-symbolic")
    .appname("COSMIC Connected")
    .timeout(notify_rust::Timeout::Milliseconds(5000))
    .show()
```

### Cross-Process Deduplication Details

COSMIC panel spawns applets as separate processes (each with its own PID and memory space). This means:
- Static variables are NOT shared between applet instances
- Multiple processes receive the same D-Bus signals
- Traditional in-process deduplication doesn't work

The solution uses a temp file with POSIX file locking:
```rust
fn should_show_file_notification(file_url: &str) -> bool {
    // Open /tmp/cosmic-connected-file-dedup
    // Acquire exclusive lock with flock(fd, LOCK_EX)
    // Check if same file URL within 2 second window
    // Update file with new URL and timestamp
    // Release lock with flock(fd, LOCK_UN)
}
```

## Media Controls Implementation

### D-Bus Interface

The MPRIS Remote plugin uses a single `sendAction` method for all playback controls, not individual methods:

```rust
// Correct - use sendAction with action name
proxy.send_action("PlayPause").await?;
proxy.send_action("Next").await?;
proxy.send_action("Previous").await?;

// Incorrect - these methods don't exist on the D-Bus interface
// proxy.play_pause().await?;  // Won't work
// proxy.next().await?;        // Won't work
```

Valid action strings: `"Play"`, `"Pause"`, `"PlayPause"`, `"Stop"`, `"Next"`, `"Previous"`

### Available Properties

The mprisremote interface exposes these readable properties:
- `playerList` - List of available media players on the device
- `player` - Currently selected player name
- `isPlaying` - Whether playback is active
- `volume` - Current volume (0-100), type: `int32`
- `length` - Track length in milliseconds, type: `int32`
- `position` - Current playback position in milliseconds, type: `int32`
- `title`, `artist`, `album` - Current track metadata
- `canSeek` - Whether the player supports seeking

Writable properties (set via D-Bus property setters):
- `volume` - Set playback volume
- `position` - Seek to position
- `player` - Select active player

**Important:** All properties must have explicit `name = "..."` attributes in the zbus proxy definition to ensure correct D-Bus property name mapping:

```rust
#[zbus(property, name = "volume")]
fn volume(&self) -> zbus::Result<i32>;

#[zbus(property, name = "length")]
fn length(&self) -> zbus::Result<i32>;  // D-Bus returns int32, not int64
```

**Note:** `canGoNext`, `canGoPrevious`, `canPlay`, `canPause` are per-player properties not exposed on the main interface. The UI defaults these to `true` and lets the phone handle unsupported actions.

### Player Selection Persistence

When the user selects a player from the dropdown, the selection must be explicitly applied before each media action. The D-Bus `sendAction` method operates on whatever player the daemon considers "current", which may not reflect the user's recent selection due to timing/sync issues.

The solution is to track the user's selection locally (`media_selected_player`) and call `set_player()` before every action:

```rust
async fn media_action_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    action: MediaAction,
    ensure_player: Option<String>,  // User's selected player
) -> Message {
    // ...
    if let Some(ref player) = ensure_player {
        proxy.set_player(player).await.ok();  // Ensure correct player before action
    }
    // Then perform the action
    proxy.send_action("PlayPause").await?;
}
```

This ensures playback controls affect the player the user actually selected.

## Known Issues

### Group MMS Sending Not Supported

Sending messages to group MMS conversations (multiple recipients) does not work reliably with KDE Connect. This is a known upstream issue tracked at [KDE Bug 501835](https://bugs.kde.org/show_bug.cgi?id=501835).

**Symptoms:**
- Replying to a group message thread silently fails
- The D-Bus call to `sendSms` returns success but the message never appears on the phone
- This affects COSMIC Connected, the native KDE Connect SMS app, and kdeconnect-cli alike

**Technical details:**
- KDE Connect can receive and display group MMS messages
- The `sendSms` D-Bus method accepts multiple addresses but the Android app doesn't process them correctly for MMS groups on many devices
- The issue may be device/ROM-specific - some users report it works on certain Android configurations
- MMS group identity on Android is tied to internal thread IDs, not just the participant list

**Current handling:**
- The applet detects group conversations (multiple unique recipients) and shows "Group messaging not supported" when attempting to send
- This prevents the confusing behavior of showing an optimistic "sent" message that never actually delivers

**Workaround:**
- Use the phone directly to reply to group messages

### Conversation List Scroll Position

When returning from viewing a message thread to the conversation list, the scroll position defaults to the bottom (oldest conversations) instead of the top (most recent). Multiple approaches were attempted without success:

- `scrollable::snap_to` with `RelativeOffset { x: 0.0, y: 0.0 }`
- `scrollable::scroll_to` with `AbsoluteOffset { x: 0.0, y: 0.0 }`
- Changing scrollable ID via a key counter to force widget recreation
- Setting explicit `direction` with `Scrollbar::new().anchor(Anchor::Start)`

The issue appears to be related to how iced/libcosmic preserves scrollable widget state across view changes. The message thread scroll-to-bottom (using `RelativeOffset::END`) works correctly, suggesting the problem is specific to scroll-to-top behavior or timing of when the scroll command executes relative to view rendering.

Potential solutions to investigate:
- Use a subscription to trigger scroll after view renders
- Restructure the view to not reuse the scrollable widget
- Store and restore scroll position manually
- File an issue with libcosmic/iced if this is a bug

## Future Enhancements

Potential features to implement in future development:

### Media Controls Enhancements
- Album art display (KDE Connect supports sending album art as binary payload)
- Seek slider for playback position control
- Loop and shuffle toggle controls

### Additional KDE Connect Plugins
- **Run Commands** - Execute predefined commands on the phone
- **Mousepad/Keyboard Input** - Send mouse movements and keyboard input to phone
- **Presenter** - Control presentations remotely
- **Screen Sharing** - View phone screen (if supported)

### SMS Notification Enhancements
- Click-to-open conversation from notification (requires async channel communication with notify-rust callback)
- Quick reply action from notification (if COSMIC supports notification actions)
- Notification sound customization

### UI/UX Improvements
- Device icons based on device type (phone, tablet, laptop)
- Dark/light theme following system preference
- Keyboard shortcuts for common actions
- System tray integration when not using panel applet

### Technical Improvements
- Connection status indicator with reconnection handling
- Plugin availability detection (show/hide buttons based on device capabilities)
- Better error messages and recovery suggestions

## Reference Material

The `reference/kdeconnect-kde/` directory contains a clone of KDE Connect source for reference. Key files:
- `dbusinterfaces/*.xml` - D-Bus interface definitions
- `cli/kdeconnect-cli.cpp` - Example D-Bus usage patterns
- `plugins/*/` - Plugin implementations showing packet types

This directory is gitignored and not part of the build.

## External Resources

- [COSMIC Applets Repository](https://github.com/pop-os/cosmic-applets) - Reference implementations
- [libcosmic](https://github.com/pop-os/libcosmic) - UI toolkit
- [zbus Book](https://dbus2.github.io/zbus/) - D-Bus client documentation
- [KDE Connect Protocol](https://invent.kde.org/network/kdeconnect-kde) - Protocol reference
