# Notification Systems

Desktop notification implementations for SMS, calls, and file transfers.

## SMS Notifications

Shows desktop notifications when new SMS messages are received.

### Implementation

1. **D-Bus Signal**: `conversationUpdated` from `org.kde.kdeconnect.device.conversations`, handled in the main `dbus_signal_subscription` alongside call and file signals

2. **Message Filtering**: Only incoming messages notified (MessageType::Inbox)

3. **Deduplication**: a cross-process file lock (the same message within 2 s), a 30 s freshness check, and `last_seen_sms` per device and thread, set when a thread is opened and after a notification is shown, so the messages a thread re-sends on open or Load older do not notify again in the process that opened the thread or showed the notification


4. **Contact Resolution**: Sender names resolved via `ContactLookup` using synced vCards

5. **Privacy Settings**:
   - `sms_notifications` - Master toggle
   - `sms_notification_show_sender` - Show/hide sender name
   - `sms_notification_show_content` - Show/hide message preview

### Display

```rust
notify_rust::Notification::new()
    .summary(&summary)  // "New SMS" or "New SMS from {name}"
    .body(&body)        // Message content or "Message received"
    .icon("phone-symbolic")
    .appname("Connected")
    .show()
```

### Gating

There is no separate subscription. The main D-Bus subscription yields the signal and the `update()` arm drops it unless `config.sms_notifications` is on. Calls (`config.call_notifications`) and files (`config.file_notifications`) work the same way.

## Call Notifications

Shows notifications for incoming and missed phone calls.

### D-Bus Signal

The telephony plugin emits `callReceived` signal with:
- `event` - "callReceived" or "missedCall"
- `phone_number` - Caller's phone number
- `contact_name` - Contact name if available

### Privacy Settings

- `call_notifications` - Master toggle
- `call_notification_show_name` - Show/hide contact name
- `call_notification_show_number` - Show/hide phone number

### Display

```rust
notify_rust::Notification::new()
    .summary(&summary)  // "Incoming Call" or "Incoming call from {name}"
    .body(&device_name) // Which device received the call
    .icon("call-start-symbolic")  // or "call-missed-symbolic"
    .appname("Connected")
    .urgency(notify_rust::Urgency::Critical)
    .show()
```

### Limitation: Mute Ringer

KDE Connect handles ringer muting internally via KNotification. No D-Bus method exposed for external muting - would require upstream changes.

## File Receive Notifications

Shows notifications when files are received from connected devices.

### D-Bus Signal

Share plugin emits `shareReceived` signal with:
- `file_url` - file:// URL of received file

### Privacy Settings

- `file_notifications` - Master toggle

### Display

```rust
let mut notification = notify_rust::Notification::new();
notification
    .summary(&fl!("file-received-from", device = device_name))
    .body(&file_name)
    .icon("folder-download-symbolic")
    .appname("Connected")
    .timeout(notify_rust::Timeout::Milliseconds(NORMAL_NOTIFICATION_TIMEOUT_MS));
// .show() blocks (zbus), so run it on a blocking thread; log the result
// rather than discarding it (a bare `let _ = …show()` silently swallows
// errors/panics — the cause of a long "missing SMS toast" hunt in v0.6.0).
match tokio::task::spawn_blocking(move || notification.show()).await {
    Ok(Ok(_handle)) => tracing::debug!("File notification shown"),
    Ok(Err(e)) => tracing::warn!("Failed to show file notification: {}", e),
    Err(e) => tracing::warn!("File notification task panicked: {}", e),
}
```

### Timeout Handling

COSMIC's notification daemon **clamps** the freedesktop `expire_timeout` hint — it does **not** ignore it. Displayed duration = `min(requested, daemon cap)`, where the cap is `max_timeout_normal` (default 5000 ms) for normal/low urgency; urgent notifications are uncapped (`max_timeout_urgent = None`).

Connected requests a large bounded timeout and **defers the real duration to COSMIC** (so a future raised cap is honored automatically, and the default is 5 s today). There is no in-app duration setting and no manual close:

- **Normal toasts** (SMS / file / missed-call) request `NORMAL_NOTIFICATION_TIMEOUT_MS` (30 s) → clamped to the daemon's normal cap (5 s by default).
- **The critical incoming-call toast** requests `CALL_RING_TIMEOUT_MS` (30 s), which is the *literal* on-screen time since urgent notifications are uncapped — a distinct mechanism from the normal path despite the equal number. (Not `Timeout::Never`: Connected has no active dismissal on call-end, so `Never` would leave a stale toast.)

Both constants live in `constants.rs::notifications`. The prior model — a user-configured `notification_timeout_secs` slider plus a `Timeout::Never` + `show_and_auto_close()` manual-close workaround built on the mistaken belief that the daemon ignored `expire_timeout` — was **removed in v0.6.0**.

> Note: every notification site logs the `.show()` result (a `match`, not `let _ = …`). The success arm is `debug!` (suppressed in release, which defaults to the `warn` filter — see `main.rs`); failure/panic arms are `warn!` so they surface in release builds.

## Cross-Process Deduplication

COSMIC spawns multiple applet processes. Each independently receives D-Bus signals, causing duplicate desktop notifications. Traditional in-process deduplication doesn't work across processes.

### Solution: File-Based Locking

Each notification type uses a dedicated file under `$XDG_RUNTIME_DIR` with POSIX file locking:

| Type | Dedup file | Key format |
|------|-----------|------------|
| File | `/$XDG_RUNTIME_DIR/cosmic-connected-file-dedup` | `{file_url}` |
| SMS | `/$XDG_RUNTIME_DIR/cosmic-connected-sms-dedup` | `{thread_id}:{message_date}` |
| Call | `/$XDG_RUNTIME_DIR/cosmic-connected-call-dedup` | `{event}:{phone_number}` |

The directory is resolved once per process (`DEDUP_DIR` in `notifications.rs`). `$XDG_RUNTIME_DIR` is user-private (mode 0700, tmpfs) and is the right place for per-session coordination files; `/tmp`, the location before 0.9.0, is world-writable with predictable names. When the variable is unset or empty the files fall back to `std::env::temp_dir()`, which is `/tmp` unless `TMPDIR` says otherwise, so a session without a runtime dir keeps the old behaviour.

Under Flatpak, both `/tmp` and `$XDG_RUNTIME_DIR` inside the sandbox are private to the app ID but shared by every instance of it (host paths `$XDG_RUNTIME_DIR/.flatpak/<app-id>/tmp` and `.../xdg-run`), so the dedup works across panel instances in either location and needs no `finish-args` entry. The host's own `/tmp` is never visible to the sandbox.

All use the same generic `should_show_notification()` function in `notifications.rs`:

```rust
use std::path::Path;

fn should_show_notification(dedup_file: &Path, key: &str) -> bool {
    // Open dedup file
    // Acquire exclusive lock with flock(fd, LOCK_EX)
    // Check if same key within 2 second window
    // Update file with new key and timestamp
    // Release lock with flock(fd, LOCK_UN)
}
```

Key points:
- Uses `libc::flock()` for atomic locking across processes
- 2-second deduplication window
- Static variables NOT shared between applet instances
- Call dedup key includes event type so `callReceived` and `missedCall` for the same number are treated as distinct notifications
- **No key carries a device id.** The SMS key is `{thread_id}:{message_date}`, and thread ids are assigned by the phone, so two paired phones can collide within the 2-second window and one notification is silently dropped. Latent today — it needs two paired devices sending at the same instant on colliding thread ids — but any key change should add the device id rather than assume uniqueness.

## Duplicate Notifications from KDE Connect

Connected and KDE Connect can notify for the **same** event independently, so a
user may see each incoming SMS or call announced twice. This is a cross-app
interaction, not a Connected bug — Connected's notifications are correct; KDE
Connect simply also raises its own. (File transfers don't duplicate — only
Connected notifies for those.)

### Mechanism per type

- **SMS** — KDE Connect's **"Receive notifications"** plugin
  (`kdeconnect_notifications`) mirrors the phone's own notifications to the
  desktop, including incoming SMS. Connected's SMS toast is generated
  independently from the `conversationUpdated` signal, in the
  `Message::SmsNotificationReceived` arm of `SmsConversationStore::update`
  (`sms/store.rs`), so both fire for one message.
- **Calls** — The duplicate is KDE Connect's **telephony** plugin. Connected
  reads call events from that same plugin's `callReceived` signal, so the source
  Connected would need to silence is the source it depends on.

### Why it isn't a one-line fix

The obvious workaround — disable the offending KDE Connect plugin — has a real
cost because that plugin's data feeds Connected's UI:

- The "Receive notifications" plugin's `activeNotifications()`
  (`kdeconnect-dbus/src/plugins/notifications.rs`, called from
  `fetch_notifications` in `device/fetch.rs`) is the **sole** source of
  `device.notifications`.
- That field drives the device-page notification list and its badge
  (`build_notifications_section` in `device_page.rs`) and the unread count badge
  on the device list (`device_row` in `device_list.rs`).

So disabling the plugin to stop the duplicate SMS toast also **empties the
device-page notification list and both badges** — all-or-nothing. Disabling the
telephony plugin likewise removes Connected's own call notifications. Connected's
SMS toast itself is independent and survives either way.

### Intermittent drop (related, observed v0.6.0)

COSMIC's notification daemon occasionally **drops Connected's own SMS toast**
when KDE Connect's mirrored notification for the same SMS lands shortly after
(~360 ms) — a same-event race in the daemon, reproducible on release builds.
Disabling the "Receive notifications" plugin removes the competing toast and
makes Connected's reliable, at the cost above. This is the daemon's behavior, not
a Connected `.show()` failure (the `.show()` succeeds; see the logged-`match`
note under [Timeout Handling](#timeout-handling)).

### Clean fix (not yet available)

The correct fix is to mute KDE Connect at the COSMIC notification daemon on a
per-application basis, which keeps the plugin's D-Bus data flowing (so the
device-page list and badges survive) while suppressing only KDE Connect's toasts.
COSMIC does not expose per-application notification controls yet. User-facing
guidance lives in the README under the stable anchor
`#duplicate-notifications-with-kde-connect` (linked from the in-app Notifications
page "Learn more"); the in-app hint is intentionally short.

## Unsupported: Incoming Ping Notifications

KDE Connect's ping plugin (`kdeconnect_ping`) does not emit D-Bus signals for incoming pings. When a ping is received, `kdeconnectd` handles it internally and sends a desktop notification directly via `KNotification`, bypassing any D-Bus signal mechanism. The applet cannot detect or replace incoming ping notifications. The ping plugin only exposes `sendPing()` methods (outgoing), not incoming signals.
