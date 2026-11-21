# UI Patterns

libcosmic patterns and UI conventions used in Connected.

## ViewMode Enum

The applet tracks current view:

```rust
pub enum ViewMode {
    DeviceList,       // Main device list
    DevicePage,       // Individual device details
    SendTo,           // "Send to device" submenu - mobile peers
    ShareText,        // Focused Share Text compose - non-mobile peers
    ConversationList, // SMS conversations
    MessageThread,    // SMS message thread
    NewMessage,       // Compose new SMS
    Settings,         // Settings panel
    About,            // About sub-page
    MediaControls,    // Media player controls
}
```

`SendTo` and `ShareText` are the two arms of the peer-class split: a mobile peer gets the full
submenu, a non-mobile peer gets the single compose view. The predicate is
`DeviceClass::is_mobile` (`device/class.rs`).

## Status Messages

Write `status_message` only through `set_status`, `set_transient_status` (clears after 3s) or
`set_view` (clears on navigation). Each bumps a generation, so a stale timer cannot clear a newer
status, and a device refresh never touches it.

The only other writers are the generation-checked `ClearStatusMessage` and a store-requested clear (`SmsReply::SetStatus(None)`), which leaves the generation alone. The SMS reply-box send note is not a status message: it is `ReplySend` in `sms/store.rs` and clears when the thread closes.

## Async Tasks

Use `Task::perform` with `cosmic::Action::App` wrapper:

```rust
cosmic::app::Task::perform(
    async { /* async work */ },
    |result| cosmic::Action::App(Message::ShareComplete(result)),
)
```

For an async fn that already returns a `Message`, pass `cosmic::Action::App` directly.

## Popup Windows

Popups **must** be created through `cosmic::surface::action`. That path registers the
surface in libcosmic's internal `surface_views` map; the raw iced command
(`iced::platform_specific::shell::wayland::commands::popup::get_popup`) creates a working
popup that is never registered.

An unregistered popup renders **with the frosted alpha but no compositor blur**. `Core::frosted()`
takes no surface argument, so the alpha applies regardless; `Core::blur()` is handed the surface
from the registry, and its untracked-applet branch returns false. The result composites the
desktop at the correct opacity with wallpaper edges intact behind the panel text. It looks like a
theming problem and is not one.

```rust
use cosmic::surface::action::{app_popup, destroy_popup};

fn open_popup_task() -> cosmic::app::Task<Message> {
    let action = app_popup::<ConnectApplet>(
        |_| Default::default(),
        |state: &mut ConnectApplet| {
            let new_id = window::Id::unique();
            state.popup = Some(new_id);
            state.core.applet.get_popup_settings(
                state.core.main_window_id().unwrap(),
                new_id,
                None, None, None,
            )
        },
        None,
    );
    cosmic::task::message(cosmic::Action::Surface(action))
}

Message::TogglePopup => {
    if let Some(popup_id) = self.popup.take() {
        return cosmic::task::message(cosmic::Action::Surface(destroy_popup(popup_id)));
    }
    return Self::open_popup_task();
}

Message::PopupClosed(id) => {
    if self.popup == Some(id) {
        self.popup = None;
    }
}
```

The settings closure takes `&mut App`, so the popup id is recorded there rather than before the
call. Passing `None` as the third argument means libcosmic renders through `view_window`.

`open_popup_task()` is the only popup-create path; the file chooser reopen uses it too (see File chooser).

**Do not use manual runtime actions.** Clicking the panel icon to close then leaves the popup
unable to reopen.

**Anchor rectangle.** `get_popup_settings(…, None, None, None)` supplies an origin-anchored rect
sized to the applet button, which is correct for a single-button applet. Passing the button's own
bounds instead requires `.on_press_with_rectangle()` on the panel button and a `Message::Surface`
variant that the view can emit — see `cosmic-ext-whether`. Only worth it if placement is actually
wrong; it changes positioning, so re-test both panel orientations if adopted.

**Popup height.** The cap is the autosize limit on `popup_container`, which libcosmic sets to 1000 px. `view_window` lowers it with `.max_height(self.popup_max_height())`, using the logical height of the panel's own output from the Wayland output events, minus the panel and a 16 px margin. A `Length::Fixed` wrapper inside the popup clamps to that limit rather than overriding it, which is why the thread view's `Fixed(10_000.0)` ends up exactly at the cap. The height arrives in `OutputEvent::Created` when the applet starts; a resolution or scale change while the panel runs doesn't send `InfoUpdate`, so the old cap holds until the panel restarts.

## File chooser

All pickers go through `pick_file()` in `app.rs`, which opens libcosmic's portal chooser and reads the result with `FileResponse::url().to_file_path()`; a non-file URL or a cancel arrives as `None`. The chooser takes focus and the panel tears the popup down, so every result arm (`FileSelected`, `SmsAttachmentSelected`, `NewMessageAttachmentSelected`) batches `reopen_after_chooser()`, which reopens through `open_popup_task()` on every return, cancel included, unless the panel left the popup standing. The chooser outlives the popup and the view that opened it, so the result message carries its target: `FileSelected` the device id, `SmsAttachmentSelected` the `(device_id, thread_id)`. Never read the target from current view state when the result arrives. The double-click gap is in `docs/KNOWN_ISSUES.md`.

## View Lifetimes

Use explicit lifetime annotations:

```rust
fn view(&self) -> Element<'_, Self::Message>
fn view_window(&self, _id: window::Id) -> Element<'_, Self::Message>
```

## Scrollable identity

**A scrollable's widget state is matched by tree position, never by name.** Two scrollables at
the same child index in different views share state.

`Scrollable::id()` returns `Internal::Set`, and every name-matching path in libcosmic's
vendored iced gates on `Internal::Custom` - the `NAMED` state harvest, the `NAMED` restore,
and the sibling `id_map` in `diff_children_custom`. A scrollable enters none of them, so it
falls through to positional matching, and the matched slot's id is then **force-copied onto
the new widget**.

Two consequences:

- **`.id()` on a scrollable is an operation target only, not a diff identity.** It makes
  `scrollable::snap_to` / `scroll_to` able to find the widget; it does not stop the widget
  from inheriting another scrollable's offset.
- **`set_id` overwrites what you set.** After a positional match, the widget answers to the
  id of the slot it landed in, not the one in your `view()`.

So a scroll-position bug between two views is fixed by resetting the *donor's* offset, not the
recipient's. That is why `Message::CloseConversation` snaps the **message thread's** scrollable to
`START` on the way out: the thread is anchored to the bottom, so its offset is measured from the
newest message and is non-zero whenever the user scrolled up; the top-anchored conversation list
is about to adopt that state and would read the same number as a distance from its top, and
targeting the list's own id cannot work. Verify against the libcosmic checkout matching `Cargo.lock`'s rev: `~/.cargo/git/checkouts/libcosmic-*/<short-sha>/iced/core/src/widget/tree.rs` and `…/iced/widget/src/scrollable.rs`. Other revs there are unrelated; `vendor/` records no rev and is current only right after `cargo vendor`.

**The message thread scrollable is `anchor_bottom()`.** A fresh widget tree starts at offset 0,
and under `Anchor::End` that is the newest message, so the thread lands on the newest message
after any rebuild (popup close and reopen destroys the tree) without a `snap_to` and without any
dependency on when the popup's tree exists. The anchor inverts the offset frame for that widget:
`RelativeOffset::START` is the bottom and `END` is the top, `Viewport::absolute_offset()` measures
from the bottom (use `absolute_offset_reversed()` for distance from the top), and content
prepended at the top does not move the view, so older-page loads need no position fix-up.

**No `on_scroll` on a scrollable in the popup.** A handler publishes a message whenever the scrollable's bounds change, and a publish while a newly created popup is redrawn before its first configure panics tiny_skia ([pop-os/libcosmic#1491](https://github.com/pop-os/libcosmic/issues/1491)). The message thread therefore loads older pages only from its Load older row (`docs/SMS.md`). Restore scroll-triggered loading once the upstream fix reaches the lock rev.

## Clickable List Item Pattern

Actions use clickable list item style:

```rust
let sp = cosmic::theme::spacing();
let row = row![
    icon::from_name("icon-name").size(24),
    text::body(fl!("label")),
    widget::space::horizontal(),
    icon::from_name("go-next-symbolic").size(16), // chevron: navigation items only
]
.spacing(sp.space_xs)
.align_y(Alignment::Center);

applet::menu_button(row).on_press(Message::SomeAction)
```

**When to use chevrons:** Items that navigate to another view (SMS Messages, Media Controls). Omit for immediate actions (Share file, Send Ping, Find Phone).

## Device Page Layout

1. **Header** - Back button, device icon, name, type, status, battery
2. **Actions** (list items) - suppressed while a pair request is in flight; the set depends on peer class and reachability:
   - *Mobile peer:* SMS Messages → ConversationList (chevron), Send to [device] → SendTo
     (chevron), Media Controls → MediaControls (chevron), Find Phone → rings device (no chevron)
   - *Non-mobile peer:* Share file, Share clipboard, Send Ping, Share Text → ShareText, Media
     Controls → MediaControls (chevron)
   - *Offline:* the action list is replaced by the `device-offline-actions-unavailable` caption

   A paired device always ends with a divider and Unpair, plus the `unpair-offline-note` caption when offline; an unpaired reachable device shows only Pair.
3. **Pairing section** - only while a pair request is in flight: Accept/Reject for a peer request, or a waiting caption with Cancel for ours. A `pairing-failed` caption follows the actions when the daemon reported one.
4. **Notifications section** - Device notifications list (`build_notifications_section`), hidden
   entirely when the device reports none

## Daemon-unavailable surface

`view_window` checks `self.error` **first**, before loading, the device list and the empty-list
card. While it is `Some`, nothing else renders.

```rust
pub enum ErrorState {
    SessionBus(String),      // the session bus itself is unreachable
    Daemon(DaemonUnavailable), // bus is fine, the daemon could not be reached
}
```

Each variant maps to one heading, with a caption only where there is a raw error worth showing:
`daemon-unreachable` (+ raw), `daemon-starting`, `daemon-not-started`, `daemon-not-found`,
`daemon-not-responding`, and `error` (+ raw) for `Other`. The variants themselves and what
produces each are in `docs/DBUS.md` → "Owning the name is not exporting the objects".

**One field, two writers, two clearers - and they are not symmetric.** `self.error` is written by
exactly `DbusConnectionFailed` and `DeviceFetchFailed`, and cleared by exactly `DbusConnected` and
`DevicesUpdated`. `RetryConnection` sets `self.loading = true` and re-fires the fetch but
**deliberately does not clear the error**, so the card stays up until something actually succeeds
rather than blinking away on a click that is about to fail again. Adding a third writer or a
convenience clear re-introduces the failure mode this shape exists to prevent.

**Subscription setup failures must not reach this surface.** `dbus_signal_subscription` is the only emitter of `Message::SubscriptionRetrying`: when it fails to build its session connection or bus proxy it emits that message, which only logs, and re-enters `Init`. The SMS subscriptions retry through `with_dbus_retries` and report a final failure as `SmsError`, which shows a transient status and never reaches `self.error`. Routing those through `Message::Error` blanked the device list
on a transient failure the subscription was already recovering from. `DeviceFetchFailed` is the
only path that should raise the card, because it is the only one that says something about device
state.

**The retry affordance appears on two cards.** The error card and the empty-device-list card both
carry a `retry` button dispatching `Message::RetryConnection`. On the error card the button swaps
to a non-pressable `process-working-symbolic` variant while `self.loading` is set, so a retry in
flight cannot be re-fired.

## fl!() Macro Lifetime Handling

`fl!()` returns owned `String`, not `&'static str`:

```rust
// Text widgets - pass directly
text(fl!("label"))
widget::button::standard(fl!("button-text"))

// text_input - pass directly without &
widget::text_input(fl!("placeholder"), &self.input_value)  // Correct
// widget::text_input(&fl!("placeholder"), &self.input_value)  // Won't compile!

// Fallback values - pre-compute
let default_name = fl!("unknown");
let name = self.device_name.as_deref().unwrap_or(&default_name);
```

## Configuration System

Uses `cosmic_config` for persistent settings:

```rust
// Location: ~/.config/cosmic/io.github.nwxnw.cosmic-ext-connected/v7/

// Load
let config = Config::load();

// Save
self.config.save()?;

// Watch for external changes
self.core.watch_config::<Config>(APP_ID)
    .map(|update| Message::ConfigChanged(update.config))
```
