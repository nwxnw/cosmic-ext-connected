# Changelog

## [0.9.0] - Unreleased
### Added
- Send photos and files with your messages. The reply box and New message both have an attach button, the chosen file shows as a chip you can remove, and the popup comes back on its own after the file picker closes. Large files are flagged but still allowed.

### Changed
- The Flatpak sandbox no longer has unrestricted access to the session bus. It now talks only to KDE Connect and the notification service.

## [0.8.0] - 2026-08-30
Connected's popup now blurs like the rest of COSMIC, and it starts KDE Connect for you instead of failing with a cryptic error, so first run works the same on Fedora, Arch and Debian as it does on Pop!_OS.
### Added
- A Retry button when Connected can't reach KDE Connect or finds no devices, so you can try again without restarting the panel.

### Changed
- Connected now starts KDE Connect for you. If the daemon isn't running when the applet opens, the session bus launches it on demand instead of the applet showing an error.
- The daemon error card now says what actually went wrong - KDE Connect not installed, still starting, failed to start, or not responding - instead of one generic message, and those messages are translated.
- Updated COSMIC dependencies and refreshed the dependency tree, clearing three security advisories.
- Swedish and Czech translations updated. Polish is new this release.

### Fixed
- The popup renders with the proper compositor blur instead of a flat translucent panel.
- A brief hiccup while KDE Connect is starting no longer flashes an error card. Connected waits out the startup window and only reports a problem if the daemon really doesn't come up.
- A subscription that fails to start no longer blanks the device list; it retries quietly in the background.
- Setup and troubleshooting now cover Fedora and Arch: the right package name, the firewall ports KDE Connect needs, and what to check when the daemon won't start.

## [0.7.0] - 2026-08-04
### Added
- Pairing failures now say why. A pairing attempt that is rejected, times out, or can't reach the device shows the reason, instead of quietly reverting to the Pair button.

### Changed
- `just install` no longer requires sudo. Building from source now installs to `~/.local` by default. 
- The conversation list shows more conversations and scrolls, instead of paging.
- The conversation list keeps a consistent height. Filling the list no longer stretches the popup taller and taller.
- The Refresh button now asks KDE Connect to re-scan the network for your devices, instead of only re-reading what it already knew.

### Fixed
- SMS now recovers on its own after KDE Connect restarts or your phone reconnects. The conversation list refreshes, and an open conversation reloads instead of sitting on stale messages until you back out and reopen it.
- Reaction-over-SMS conversations stay merged as they age, and scrolling back through older messages works in merged conversations.
- The conversation list loads faster and more reliably.
- Opening a conversation while KDE Connect is briefly busy now retries instead of showing an empty thread.
- Scrolling back through older messages after KDE Connect restarted could crash KDE Connect's background service. Connected now requests older messages in a way the service can always satisfy.
- A single SMS error no longer shuts down the whole SMS view.
- Sending a new message no longer leaves a sync indicator stuck on the conversation list.
- The conversation list now returns to the newest thread after you view a conversation, instead of jumping to the oldest.

## [0.6.0] - 2026-06-24

### Added
- **Collapsible "Offline" device group.** The device list now orders reachable devices first and tucks paired-but-offline devices into a collapsible "Offline" group.
- **Unpair offline devices without reconnecting.** A paired device that's currently offline can now be unpaired straight from its device page.
- **About page.** Shows the app version and links to the project homepage and issue tracker.
- **Notifications page explains KDE Connect duplicate toasts.** A short hint and "Learn more" link explain why an incoming SMS or call can appear twice when KDE Connect's notifications are also enabled.
- **The message box grows as you type.** The reply and new-message compose fields now expand vertically and wrap as you type. Enter sends; Shift+Enter inserts a newline.

### Changed
- **Settings reworked into a focused "Notifications" page.** View-state settings moved to the main UI; notification settings provide per-type (SMS, calls, file transfers) on/off toggles and privacy options.
- **Removed the "Show offline devices" and "Show non-mobile devices" toggles.** Offline devices are handled by the new collapsible group; non-mobile devices are shown by default.
- **Battery percentage and the notification-count badge now always show.**
- **Notification duration now follows your COSMIC system settings** instead of a separate in-app slider.
- **New-message recipient picker polished.** Recipients now show as compact wrapping pills, the contact-suggestion list is cleaner (deduplicated, capped, and shown below the recipients), and the missing text cursor in the recipient field is fixed.

### Fixed
- **Recipient error-icon misfire.** Error glyph no longer shows on every name query.

## [0.5.2] - 2026-05-27

### Added
- **Czech translation.** Full Czech (`cs`) localization of the UI strings, plus translated AppStream metadata (summary, description, keywords) and the desktop-entry comment. Contributed by lorduskordus (#35).

### Changed
- **Updated COSMIC dependencies** (libcosmic, cosmic-protocols, cosmic-panel, winit) to current revisions, including the `selected_fill` field now required by `cosmic::iced::widget::text::Style`. No user-facing behavior change.
- **Store listing now states the KDE Connect requirement.** The AppStream description now makes explicit that KDE Connect must be installed on the computer and the KDE Connect companion app on the phone.

## [0.5.1] - 2026-05-15

### Changed
- **Internal:** Declared `com.system76.CosmicApplet` as a provides ID in `metainfo.xml` so the COSMIC Store categorizes under Applets. No behavior change.

## [0.5.0] - 2026-05-12

### Added
- **SMS: merged-conversation indicators and SMS-view toggle.** Conversations that Connected has merged from multiple phone-side threads (the iOS-reaction-over-SMS case) now show a small marker on the conversation list. A new toggle in the SMS view header switches between the merged and split views in one click — useful if the heuristic misfires for a particular conversation, or just to see what the phone's underlying thread structure looks like. The toggle shares state with the new SMS settings option.
- **SMS: split-thread indicators when merging is off.** When the merge toggle is off, conversations whose underlying threads are reaction-bucket siblings on the phone now show a passive marker on the conversation list. At-a-glance visibility into which conversations would merge if the feature were enabled.

### Fixed
- **SMS: iOS reaction-over-SMS no longer splits a conversation across multiple threads.** Threads with identical participants on the same SIM are merged into one logical conversation, and replies route through the merged set. **As a side effect, replying on these conversations now delivers a single copy to recipients — previously, AOSP canonicalization across the split could produce duplicate delivery.** A toggle in SMS settings lets you disable merging if it misfires on your carrier/device combo.
- **Notification dismiss button no longer clips past the popup edge** when the message text contains long unbreakable content (e.g. tracking URLs in shipment SMS).
- **Conversation list previews render at uniform single-line height** — multi-line message bodies are normalized to a single line in the list. The full body still renders in the thread view.
- **SMS: switching between paired phones no longer mixes the prior device's threads into the new device's conversation list.** The raw conversation cache is now cleared on device switch and seeded from prefetched data on open, so toggling the merge setting right after opening a prefetched SMS view doesn't collapse the list either.
- **SMS notification dedup is now scoped per-device.** Thread IDs are device-local, so phone A's thread 1234 and phone B's thread 1234 are unrelated conversations; previously, a numeric thread-id collision across phones could swallow a real notification in a narrow timing window.

### Changed
- **Internal refactor:** SMS conversation state extracted from the main applet module into a dedicated `SmsConversationStore`. No user-visible change on its own; enables the reaction-thread merging feature above and unlocks targeted SMS tests in v0.6.0.
- **UI:** "New message", "Add recipient", and "Pair device" `+` icons now use the COSMIC accent color so the primary action stands out from neutral row content.
- **Internal:** Release builds emit only `warn`/`error` log levels by default — set `RUST_LOG=cosmic_ext_connected=info` (or `=debug`) for verbose output during diagnostics. Debug builds still default to debug-level. `APP_ID` constant deduplicated to a single definition site.

## [0.4.0] - 2026-05-01

### Added
- Desktop / laptop peer support; "Show non-mobile devices" setting; inline share primitives on non-mobile device page; Enter-to-send on share-text inputs.
- Hover tooltips on the refresh, settings, and notification-dismiss icon buttons

### Changed
- Accent color restricted to actionable UI; tightened device page layout; Pair/Unpair moved into the actions list; notification count rendered as a badge.

### Fixed
- Pair-state updates more reliable: subscribe to the correct upstream D-Bus signal names (`pairStateChanged` and daemon-level `pairingRequestsChanged`), replacing three names that did not exist in upstream KDE Connect. Pair-state was previously riding only on the `PropertiesChanged` catch-all
- Pair-state updates no longer silently dropped during signal bursts: a follow-up refresh now fires after each signal-triggered fetch, picking up settled state even when trailing signals fall inside the 3 s debounce window
- Pair acceptance picked up promptly when accepting on the phone right after sending the request from Connected: a 1 s tick now flushes any deferred refresh once the debounce window clears, so the UI no longer hangs on "Waiting for device to accept" until the next ambient signal
- SMS thread truncation on first open: re-issues requestConversation when the daemon re-emits conversationLoaded with more messages than we received, and as a fallback on phone-response timeout.
- Message thread auto-scrolls to the newest message on open even for cached threads where the daemon doesn't emit conversationLoaded.

## [0.3.0] - 2026-04-14

### Changed
- Replaced rfd/gtk3 file dialog with libcosmic's native xdg-portal file chooser
- Renamed symbolic panel icons with app ID prefix so Flatpak exports them to the host

### Fixed
- SMS message handling: optimistic send with body-based reconciliation
- Conversation list and message thread subscriptions converted to long-lived (fixes premature termination during bursty phone signals)
- Conversation list bootstrap: settles on a quiet window, retries once on small cold-start results, and re-reads daemon cache to merge late-arriving data
- Cold start false "no conversations" display (spinner shown until data arrives)
- Missing applet icon in COSMIC Panel settings for Flatpak installs
- Conversation list: long message previews no longer push the date and chevron off the right edge

## [0.2.1] - 2026-04-12

### Changed
- Updated for libcosmic API changes (removal of top-level `cosmic::iced_*` re-exports)
- Track `Cargo.lock` to pin dependency versions

### Added
- Swedish translations for attachment-related strings

## [0.2.0] - 2026-04-03

### Added
- MMS attachment viewing (thumbnails in message bubbles, click to open)
- Multi-recipient support for new message compose (group messaging)
- Optimistic SMS sending indicator
- Conversation prefetch on device selection

### Changed
- Switched SMS replies to `replyToConversation` for better thread context
- Replaced optimistic SMS bubbles with simpler sending indicator

### Fixed
- APP_ID mismatch preventing applet from appearing on panel
- Empty conversation list on cold start (increased timeout with fallback)
- Compose row disappearing due to iced Shrink height compression
- Conversation list not updating after sending a new message
- MMS thumbnail display (whitespace in base64 data)

## [0.1.0] - 2026-02-10

### Added
- Native COSMIC desktop applet for phone connectivity via KDE Connect D-Bus
- Device discovery, pairing/unpairing, and management
- SMS messaging with conversation list, message threads, and compose
- Contact name resolution from synced vCards
- Group message sender name display
- Long-press to copy SMS message text
- Scroll-based lazy loading for SMS messages
- Subscription-based incremental conversation loading
- Media controls (play/pause, next/previous, volume, player selection)
- File receive notifications with cross-process deduplication
- Call notifications for incoming and missed calls (with privacy controls)
- SMS desktop notifications (with privacy controls)
- Find My Phone feature to ring connected devices
- Battery status display
- Clipboard sync (send to device)
- File and URL sharing
- Ping functionality
- Settings panel with notification privacy options and configurable timeout
- Custom panel icons (connected/disconnected states)
- Accent color theming
- Swedish translations (contributed by Luna Jernberg / bittin)
- Flatpak packaging support

### Changed
- Renamed applet from "COSMIC Connect" to "Connected"
- Renamed package from `cosmic-applet-connect` to `cosmic-ext-connected`
- SMS compose sends message on Enter key press
