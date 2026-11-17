# SMS Implementation Notes

How Connected's SMS surface works today, and the constraints a change must not break.

## What exists today

Connected reads SMS over `kdeconnectd`'s Conversations and SMS-plugin D-Bus interfaces. Two kinds
of long-lived subscription drive it: one for the conversation list, and one per underlying
threadId of the open conversation. Both are cache-first then signal-driven — a single cache read
per subscription for immediate display, then everything else arrives as broadcast signals.

Reaction-bucket threads — separate `threadId`s the phone shows as one conversation — are merged
into a `LogicalConversation` before anything renders. Most list, pagination and reply-recipient logic
therefore operates on a *set* of threadIds rather than one, which is the assumption most worth
checking before changing anything here.

The daemon offers no reliable "done" event, so both subscriptions derive settlement locally: the
list on a quiet window with one cold-start retry, threads on a full page received.

## Architecture Overview

### Where the daemon's data actually lives

`m_conversations` is an in-memory `QMap` on the daemon and **nothing else**. It is populated only
by `addMessages()` on the phone-response path, and it dies with the daemon process. There is no
persistent store anywhere in KDE Connect's SMS plugin - no sqlite, no `QSettings`, no `KConfig`.

Three consequences the rest of this document depends on:

- **A restarted daemon starts empty**, however long the phone has been paired. "Cold cache" is not
  an edge case; it is every first SMS open after a daemon restart or a login.
- **`activeConversations()` is a pure local read** of that map (newest message per thread). It
  sends nothing to the phone, so it cannot warm a cold cache - it can only report one. The only
  call that warms it is the SMS plugin's `requestAllConversations()` (the Conversations interface's
  `requestAllConversationThreads()` is a one-line delegate to it and is the call Connected makes),
  which sends `PACKET_TYPE_SMS_REQUEST_CONVERSATIONS`.
- **`conversationLoaded` is not a "finished reading storage" event.** It is emitted only from
  `addMessages()`, so it means "the phone delivered a batch", and its count is
  `m_known_messages[id].size()` - distinct UIDs held in memory right now.
- **`addMessages()` inserts before it signals.** A phone batch is written into `m_conversations`,
  then `conversationLoaded` is emitted, then the blocked `RequestConversationWorker` is released;
  its per-message signals are queued behind that emit. On a fetch, `conversationLoaded` therefore
  arrives *before* the page's remaining messages, never after them.
- **Since `26.08.0` the worker does not fetch for an empty cache** (`0dc666ad9`). It returns
  after its warning, so only a non-empty cache that cannot fill the request triggers a phone
  fetch. On `23.08.5` an empty cache fetches the newest 25.

### Conversation List Loading

Conversation-list loading is cache-first and long-lived:

1. Opening the SMS view starts `conversation_list_subscription`.
2. The subscription installs D-Bus match rules before firing bootstrap requests.
3. Bootstrap requests fire immediately, before anything is emitted, so the phone round trip is in flight as early as possible.
4. Cached conversations from `activeConversations()` are emitted as one batch; later data arrives as `conversationCreated` / `conversationUpdated` signals and merges into the list.
5. A quiet window or bootstrap deadline dismisses the sync indicator.
6. The subscription remains alive while the SMS view is open.

Important details:

- Warm starts use cached rows immediately and a shorter bootstrap window.
- Cold starts use a longer wait and one bounded retry.
- The conversation-list subscription reads the daemon cache exactly once, before its bootstrap requests fire. Everything after that arrives as signals, and there is no follow-up poll — a missed signal is not backfilled by a later cache read.
- An empty list after the cold-start deadline and retry shows the empty state. A device that advertises SMS but hold no threads (a tablet without service) answers the bootstrap with nothing at all, so this is the only way it can complete; the subscription keeps listening and a late phone's conversations replace it.

### Message Thread Loading

Thread loading uses one long-lived subscription per thread of the open conversation, as a signal pump:

1. Opening a thread starts `conversation_message_subscription` for each thread in `current_merged_thread_ids`.
2. It installs match rules, then fires one `requestConversation(thread, 0, page)` on the Conversations interface.
3. It forwards every `conversationUpdated` and `conversationLoaded` for its thread until the thread closes. It decides nothing.

The store settles each thread's first page (`FirstPage` in `store.rs`) on the same shape as an older page: a full page of new messages, or a `conversationLoaded` once the thread has served at least one signal. If the daemon's cache holds
a full page, the page arrives at once and settles on the tenth. If it has to fetch, `conversationLoaded` arrives first and the rest of the page follows within a millisecond, landing after the settle. The count is never compared with what
loaded: the daemon keys its cache by date, so before 26.12 it can include messages the cache overwrote, and a thread shorter than a page would never reach it.

A `conversationLoaded` that arrives before the thread has served anything means the request found an empty cache. On 23.08.5 the daemon fetched and the page follows; on 26.08.0+ (`0dc666ad9`) it served nothing, and the signal is the list
bootstrap delivering the thread's head. Either way the store re-reads once, `requestConversation(thread, 0, page)`. On a long thread the re-read is served from cache as duplicates. On a short thread it is a second phone fetch, and its
`conversationLoaded`, arriving after its duplicates, settles the page. `addMessages()` announces only a thread's latest message, so a backfill is visible only as a count; the re-read is how its messages arrive.

There is no clock. A thread whose phone never answers shows the loading text until Back or the reconnect regeneration.

- `has_more` for a thread settled on `conversationLoaded` is `count > page`; a full page is `true`. An overstated count can show Load older on a thread just over a page; one press settles it false.
- The initial load completes when every thread of the merged set has settled: `initial_load_complete` is set, the sync indicator clears and the view snaps to the newest message. Until then each arriving message also snaps, so the newest
stays in view.
- The re-read's offset is pinned to 0 and only the range end moves. The daemon serves newest-first, so widening the range returns the same messages, and `crbegin() + 0` can never index past the end of its cache. A non-zero `start` against
a cache holding fewer messages **segfaults kdeconnectd 23.08.5** (no bounds check between the D-Bus wire and the iterator in `requestconversationworker.cpp`) and silently returns nothing on newer daemons - so an out-of-range offset is an
applet-side correctness bug against every daemon version, and the daemon does not restart itself. Clamping `start` against the last known store count is **not** an alternative: every count the applet holds is stale in exactly the crash
scenario (a restarted daemon with a cold cache) and it cannot detect that, so the clamp is circular. The resulting per-message signals merge via `known_message_ids` dedup.

### Reaction-Thread Merging

iOS reactions over SMS arrive on slightly different address-sets and AOSP buckets them into a separate `threadId`. The phone re-merges visually; KDE Connect / Connected report them separately. Connected wraps each user-perceived conversation in a `LogicalConversation` (`sms/logical.rs`) that may collapse multiple underlying SMS threadIds.

Merging precondition — the grouping key in `merge_into_logical` (`sms/logical.rs`) is `(canonical address-set, conversation-level sub_id)`:

- both `sub_id`s are non-`-1` and equal, AND
- canonical address-sets are equal (digit-only normalize, leading-`1` stripped, deduplicated as a set).

Each `LogicalConversation` carries:

- `primary_thread_id` — the most-recently-active sibling within the merged set; the list row's identity.
- `merged_thread_ids` — all underlying threadIds composing this logical conversation. Always contains `primary_thread_id`. Single-element for non-merged.

Opening a merged conversation fans out the message subscription: one `conversation_message_subscription` per underlying threadId, each firing its own `requestConversation` and emitting `ConversationMessageReceived` for its own thread. Signal handlers accept any thread in the open `current_merged_thread_ids` set rather than only the primary.

Multi-subscription completion semantics:

- Each thread's first page settles on its own signals; the initial load completes when the last one does. `thread_has_more` is recorded per thread as it settles, by the rule above; an older page sets it by the settle rule under Older Message Loading, and a later `conversationLoaded` never raises it. `messages_has_more` is the OR across `thread_has_more` over `current_merged_thread_ids`.

### Recipient rule

A reply names its recipients, not a thread. The phone files an outgoing message by its recipient set, so no thread is passed
and there is no merged-set redirect to choose. Connected takes the recipients from the newest loaded message that carries
addresses, deduplicated (`dedupe_addresses`: last ten digits, a `0001` phantom dropped beside a real number), and falls 
back to the conversation's own address list. The deduplication also prevents the recipient-side duplicate delivery that 
the old symmetric-merge redirect existed to avoid.

With merging off (see "Per-entry markers and SMS-view toggle" below), a reply from the non-canonical sibling of a reaction-
bucket pair is still filed on the canonical thread, so its echo lands outside the open thread and the "Sending..." indicator
can stay pinned. This is documented behavior gated behind the user opt-out, not a regression.

### Per-entry markers and SMS-view toggle

The SMS conversation list shows a small marker on rows that participate in a reaction-bucket group. Two glyphs:

- **Merge marker** (visible when the merge toggle is on): rows whose `LogicalConversation.merged_thread_ids.len() > 1` show a converging-Y glyph next to the message preview. Indicates "this conversation merges multiple phone-side threads."
- **Split marker** (visible when the merge toggle is off): rows whose underlying thread has at least one reaction-bucket sibling in the conversation list show a parallel-arrows glyph next to the message preview. Indicates "this conversation has a sibling thread on the phone; turning the merge toggle on would combine them."

A header toggle in the SMS view (between the conversation-list title and the new-message button) switches between merged and split states. The toggle uses the same iconography as the per-entry markers — converging-Y when merging is on, parallel-arrows when off — and dispatches the same `Message::ToggleSetting(SettingKey::MergeReactionThreads)` as the settings-page option, so the two surfaces share state automatically. Toggling either one updates the other, and the toggle state persists across applet restarts.

The merge-off path uses the same grouping key as the merge-on path, so any pair the merge logic *would* combine also appears as split-marker entries when the user has merging off. When the v0.6.0+ subset clause returns to the heuristic, both surfaces pick it up automatically without further coordination.

### Older Message Loading

Older messages are loaded when the user presses Load older at the top of the thread, and are prepended as they arrive. The row shows only once the initial load has completed and more history exists, the same condition `request_older_page` checks. The thread is anchored to the bottom, so a prepend does not move the view. There is no scroll-to-top prefetch: an `on_scroll` handler publishes a message on every redraw whose bounds change, and that panics tiny_skia when a newly created popup is redrawn before its first configure ([pop-os/libcosmic#1491](https://github.com/pop-os/libcosmic/issues/1491)). Restore it once the upstream fix reaches the lock rev.

A thread's page settles on a full page of new messages, or on the first `conversationLoaded` for the thread after its cache burst. That signal is the last the daemon sends for the page: either its cache filled the request, or it fetched from the phone and emitted what arrived. More history exists if the cache filled the request (`served >= requested`) or the count grew past the last one seen (`last_count`). The count itself is never compared with the loaded count: the daemon keys its cache by date, so messages sent in the same second overwrite each other, and the count includes messages it can no longer serve. A `conversationLoaded` that arrives before the burst belongs to another request and is ignored. Because the range starts at 0, the daemon re-serves the messages already shown before the older ones; those duplicates count toward position in the range (`served`), not toward progress (`new`), so a page cannot settle full-page on them.

Outside a page, a `conversationLoaded` never raises a thread's `has_more`. A grown count there is usually a new incoming message, not older history, and raising it would hide messages again behind a thread that is already at its top.

For merged conversations, the fetch fans out: one request per threadId in `current_merged_thread_ids` that still has more to give (`thread_has_more`), each carrying its own already-loaded count. Each thread settles on its own counters, and the page is held open until every targeted thread has settled (`OlderPageLoad.pending`). `messages_has_more` is the OR across `thread_has_more`, so the thread stops paginating only when every merged thread is exhausted.

In a merged conversation one thread's page can cover hours while another's covers months, which would put the quiet threads' old messages directly above the busy thread's oldest loaded one, with its unloaded history out of reach. The view therefore hides every message older than the display floor (`display_floor`): the newest oldest-loaded date among threads that still have more, counting unknown as more. An older page targets only the threads at the floor. When one reaches the top of its history, the floor drops and the other threads' hidden messages appear in order. The request range still counts hidden messages, because the daemon pages by index, not by what is shown.

Each request is `requestConversation(threadId, 0, loaded_count + page)` — **the offset is pinned to 0 here for the same reason as the first page's re-read above**: a non-zero `start` segfaults kdeconnectd 23.08.5.

### Recovery after a restart or reconnect

A `kdeconnectd` restart does not close the applet's session-bus connection, so there is no error
to key on. Recovery keys instead on the SMS device going **unreachable → reachable** in
`Message::DevicesUpdated`, comparing the previous `self.devices` against the fetch that just
landed. ⚠ That edge exists only because a *failed* fetch marks every device unreachable
(`Message::DeviceFetchFailed`); leaving the last-known `true` in place makes the next success read
`true → true`, the edge never fires, and an open thread stays frozen. Do not "fix" that handler to
preserve reachability.

There is one lever: **a subscription's generation counter is part of its iced recipe key**, so
bumping it makes iced drop and re-`Init` the recipe, re-firing its requests from scratch. Two
counters exist — one for the conversation list, one shared by the per-thread message subscriptions
— bumped independently, each behind its own view-lifecycle flag. Nothing can command a running
subscription; this is the only way to restart one.

The message bump also resets the thread's load bookkeeping (`messages`, `known_message_ids`,
`thread_has_more`, `older_page`, `messages_has_more`, `initial_load_complete`), mirroring
`OpenConversation`. Without it the re-streamed page dedups against stale uids and `messages`
becomes the union of pre- and post-restart data — so a recovered thread is a fresh first page, not
a merge.

A daemon exit is also detected directly: `NameOwnerChanged` for `org.kde.kdeconnect` with an empty
new owner marks every device unreachable at once, so the edge is observed even when the daemon is
back inside one refresh window. Nothing re-activates the daemon on loss; the next call does.

## Sending Behavior

### Replies

Replies call `sendSms(addresses, message, attachments, subID)` on the SMS plugin (`/devices/{id}/sms`), with recipients 
Connected selects (`reply_recipients`, see "Recipient rule" above) and the reference message's `subID`. Connected does 
not use `replyToConversation`: it resolves recipients from the oldest message in the daemon's in-memory cache, which was 
the wrong set for 20 of 100 recent threads (D.63), and on a pre-`26.08.0` daemon it needs that cache primed. If no message
or conversation address is available, Connected shows a status and sends nothing. Android strips the user's own number
from MMS rows, so in a conversation with yourself an MMS carries no address and the recipient comes from the newest
SMS row on the page.

On `Ok` the draft and staged attachment clear, and a caption under the compose box reads "Sending..." until the phone's sent copy arrives on the thread's long-lived message subscription. Nothing is added to the thread locally: the echo is the only copy of a sent message, so the thread shows only what the daemon reports. Send is disabled only while the D-Bus call is in flight, so an unechoed MMS does not block the next send.

The echo is matched by type `Sent`, a body equal to the pending reply's, and being the newest message in the thread. The last test stops an older sent message with the same body, arriving from Load older or the initial load, from matching; every date in the thread is the phone's, so it has no clock skew, and a miss leaves the reply unconfirmed rather than wrongly confirmed. The match is scoped to the open merged set, so an echo on a sibling thread in `merged_thread_ids` still clears it. Because the phone files a reply by its recipient set, the echo lands on the canonical thread, which is in `merged_thread_ids` whenever merging is on.

If no echo arrives within `SEND_ECHO_WINDOW_SECS` (60 s), the caption changes to "Not confirmed by your phone", with an excerpt of the body when there is one. The clock only relabels: it never reports success or failure, and a late echo still clears it. The send state (`ReplySend` in `store.rs`) lives with the composer, so leaving the thread clears it. `SmsSendResult` carries the `(device_id, thread_id)` it was sent from, and a result for a thread that is no longer open changes no state, though a failure is still reported.

### New Messages

New-message compose uses `sendWithoutConversation(addresses, message, attachments)` with explicit recipients. On success, the compose flow returns to the conversation list and keeps the conversation-list subscription active so the phone can sync back the resulting thread.

### Attachments

Both compose surfaces stage one file for the next send, passed through the `attachmentUrls` parameter of `sendSms` (replies) and `sendWithoutConversation` (new messages).

**The wire value is a plain absolute path**, not a `file://` URL and not base64, despite the name. The daemon opens it with `QFile`, base64-encodes the contents into the packet and sniffs the MIME type itself (`smsplugin.cpp`). Under Flatpak the chooser portal returns a path under the document portal mount, which the daemon reads on the host as the same user, so no `finish-args` entry is needed.

**The daemon validates nothing.** A path it cannot open becomes an empty attachment that is still dispatched, and the phone drops the whole message, text included, with no error anywhere. Every guard is ours: `stage_attachment` in `store.rs` stats the file at pick time, and `attachment_value` in `send.rs` stats it again before each send because the file can vanish in between. A failed check refuses the send rather than degrading to text-only; the compose text and chip survive for a retry.

**Size is a hard limit.** When the staged file plus the compose text is over `MMS_MAX_BYTES` (800,000 bytes), Send is disabled in both views, the update arms refuse it as well because Enter bypasses the button, and the chip carries a warning glyph and tooltip. The figure sits above KDE Connect SMS app cap of 600 KB and below the 1 MB caps carriers commonly enforce. 

Each compose surface owns its own staged file, `pending_attachment` for the thread view and `new_message_pending_attachment` for New message, cleared wherever its sibling compose state is cleared. A shared field let a file staged in one surface be sent from the other with no chip in sight.

No local preview is built. The phone's echo is the only copy of a sent attachment, and it renders through the received-message path.

The chooser is a portal round trip: the dialog takes focus and the panel tears the popup down, and the `SmsAttachmentSelected` and `NewMessageAttachmentSelected` arms in `app.rs` reopen it on every return path, cancel included, unless the panel left it standing. The double-click gap is in `docs/KNOWN_ISSUES.md`.

Send outcome is unknowable for attachments as for text (see "A send is confirmed only by its echo" below); the phone side has two literal `// TODO: Notify other end` in `SmsMmsUtils.kt`. Copy may say "sent" in the hand-off sense, never "delivered".

### Sync indicator

The conversation list's sync indicator is the OR of **two independent flags** (`sync_active` in
`store.rs`): `conversation_sync_active`, owned by the list subscription's settle cycle, and
`new_message_sync_active`, owned by the post-send wait — set after a send and cleared by any list
change, or by `NEW_MESSAGE_SYNC_INDICATOR_SECS` if the phone never reports one. ⚠ **Do not merge
them.** One flag doing both jobs is a fixed bug: the two lifecycles clear on different events, so
a send left the indicator stuck. Merging also breaks the cold-start spinner, which depends on the
settle cycle owning its own flag.

## Caching

Caching behavior:

- Re-opening SMS for the same device reuses in-memory conversation data and refreshes in the background.
- Switching devices clears device-specific SMS state as needed.
- Contacts are loaded per device from KDE Connect's synced vCard directory and reused for same-device reopens. Loading once at SMS-view open and preserving across re-opens avoids a race where async contact loading completes after the conversation list has already rendered with phone numbers.

### Contact Name Resolution

`ContactLookup` parses vCards from `~/.local/share/kpeoplevcard/kdeconnect-{device-id}/`.

- `get_name_or_number(&address)` — resolves a single address. Used for per-message sender labels in thread view.
- `get_group_display_name(&addresses, limit)` — resolves multiple addresses into a comma-separated contact list (e.g. "Alice, Bob, Charlie, ..."). Used in the conversation list, thread header, and SMS notifications.

## MMS Attachments

The KDE Connect daemon sets its Qt application name to `"kdeconnect.daemon"` in `kdeconnectd.cpp`. Qt's `QStandardPaths::CacheLocation` resolves to `~/.cache/<applicationName>/`, so MMS attachments are cached at `~/.cache/kdeconnect.daemon/<device-name>/<uniqueIdentifier>` (e.g. `PART_1762553269778`). Files have no extension — the MIME type comes from the message's attachment metadata. The Flatpak manifest must include `--filesystem=xdg-cache/kdeconnect.daemon:ro` for the applet to read cached attachments.

## Known Constraints

- `conversationLoaded` reports `m_known_messages[id].size()` - the count of distinct message UIDs the daemon currently holds **in memory** for that thread - not the phone's authoritative total. It is emitted only from `addMessages()`, so it fires only when the phone has delivered a batch, never on a purely cached read.
- Notification correctness depends on careful `last_seen_sms` handling when opening threads and merging incoming data.
- **A send is confirmed only by its echo.** `Ok` from `sendSms` / `sendWithoutConversation` means the D-Bus call worked, nothing more (`docs/DBUS.md` → "A void method's `Ok`"). The phone's sent copy is the only evidence, and it means the phone recorded the message as sent, not that it was delivered. A reply with no echo ends at "Not confirmed by your phone", which means unknown, not failed. The conversation list is not updated optimistically. The row changes when the phone's echo arrives on the live list subscription.
- **History: a pre-`26.08.0` daemon silently discards a `replyToConversation` when the thread is not in its cache.** Connected no longer calls `replyToConversation` (see "Replies"), so this no longer affects it; kept for anyone driving the daemon directly. Upstream BUG 517659, fixed in `3bf16922` and first shipped in `v26.08.0`: `replyToConversation()` logged a warning and dropped the message outright when `m_conversations` did not hold the target thread, which is the state after a device reconnect, or before the list has loaded for the session. **This is the daemon's behaviour, not applet logic**. 
- **An attachment-only MMS can echo back without its attachment.** A text-less send sometimes come back with `eventField 0` and an empty attachment list, so the bubble renders as a bare timestamp. No pattern found; upstream. See `docs/KNOWN_ISSUES.md`.

## Reference

### Key Symbols

Messages (see `app.rs`):

- `ConversationReceived` — cached or newly discovered conversation summary
- `ConversationSyncStarted` / `ConversationSyncComplete` — spinner lifecycle for the list
- `ConversationMessageReceived` — individual message during thread load or live updates
- `ConversationLoaded` - a `conversationLoaded` signal for an open thread: the daemon's known count, an upper bound on what its cache serves

Timeout constants (see `constants.rs`):

- `CONVERSATION_LIST_PHONE_WAIT_MS` — cold-start bootstrap ceiling
- `CONVERSATION_TIMEOUT_CACHED_SECS` — warm-start bootstrap window
- `CONVERSATION_LIST_QUIET_MS` — quiet-window settle after bootstrap activity
- `CONVERSATION_LIST_RETRY_THRESHOLD` / `CONVERSATION_LIST_RETRY_WAIT_MS` — cold-start retry gate and window
- `NEW_MESSAGE_SYNC_INDICATOR_SECS` — post-send window the conversation-list sync indicator stays up while waiting for the phone to report the new conversation

D-Bus surface:

- Device base path: `/modules/kdeconnect/devices/{id}`
- Conversations interface: `org.kde.kdeconnect.device.conversations` (signals: `conversationCreated`, `conversationUpdated`, `conversationLoaded`)
- SMS plugin path: `/modules/kdeconnect/devices/{id}/sms` (`org.kde.kdeconnect.device.sms`) — used by `sendSms` (replies) and `getAttachment`; thread loading and the list bootstrap go through the Conversations interface.

### Message Types

- Message types: `1 = inbox`, `2 = sent`, `3 = draft`, `4 = outbox`, `5 = failed`, `6 = queued`
- Message fields relied on by the app: body, addresses, date, type, read, thread ID, UID, sub ID, attachments

## Related Docs

- `docs/KNOWN_ISSUES.md`
- `docs/NOTIFICATIONS.md`
- `docs/DBUS.md`
- `docs/UI_PATTERNS.md`
