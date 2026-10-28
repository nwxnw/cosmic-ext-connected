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
list on a quiet window with one cold-start retry, threads on a full page received. Selecting a
device prefetches conversation heads with `activeConversations()` so the SMS view can open warm.

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

The applet's `ConversationStoreLoaded` message and the "store" wording in
`MESSAGE_SUBSCRIPTION_TIMEOUT_SECS`'s comment predate this being understood. They refer to the
daemon's in-memory cache, not to any persistent storage. Renaming them is cosmetic and has not
been done; do not read the names as evidence of a store that exists.


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

Thread loading uses a long-lived subscription:

1. Opening a thread starts `conversation_message_subscription`.
2. Match rules are installed before requests are fired.
3. One `requestConversation()` call is made, on the Conversations interface, for per-message UI signals.
4. If the daemon's cache holds a full page, the per-message signals arrive at once and the load completes on the tenth. If it has to fetch from the phone, `conversationLoaded` arrives first with the daemon's count and the page follows it within a millisecond; the load completes when `received >= min(page, count)`.
5. `PHONE_RESPONSE_TIMEOUT_MS` is a net for a page that never fills, not the normal completion.
6. The subscription then continues listening for new incoming messages and sent-message echoes until the thread closes.

Important details:

- The list is rendered oldest-first, so an unscrolled scrollable lands on the oldest message. Auto-scroll-to-bottom is dispatched on `ConversationStoreLoaded`, on `ConversationLoadComplete`, on each `ConversationMessageReceived` while `!initial_load_complete`, and on confirmed sent-message echoes. The per-message dispatch covers cached-store hits where the daemon's worker satisfies the request entirely from `m_conversations` and never emits `conversationLoaded` (see `requestconversationworker.cpp`'s `numHandled >= howMany` branch), which would otherwise leave the user pinned at the top of a long thread.
- `initial_load_complete` gates loading of older messages, and bounds the per-message auto-scroll so a new incoming SMS arriving while the user is reading older content doesn't yank them down.
- The daemon inserts a phone batch into `m_conversations` before it emits `conversationLoaded` and before it releases the worker (see "Where the daemon's data actually lives"), so a first-open page is never short because of write ordering. A page can still come up short when the thread is opened before the conversation-list bootstrap has delivered its head: the worker then has nothing to page from, and on a `26.08.0` or newer daemon it does not fetch either. The thread subscription does not match `conversationCreated`, so the head arriving later is invisible to it; the storm's `conversationLoaded(id, 1)` is what it sees. The daemon's `addMessages()` only emits `conversationUpdated` for the latest message in a thread, so historical backfill from the phone arrives silently, observable only through `conversationLoaded(count)`. Recovery is bounded to one re-issued `requestConversation` per thread open, with two triggers:
  - **Primary**: a second `conversationLoaded` arrives with `count > received_message_count` while the page is still under-filled (`received < messages_per_page`). The retry fires immediately. On the measured daemon the only producer of such a signal inside the load window is a list-bootstrap duplicate landing in the millisecond between the fetch's `conversationLoaded` and its page; the trigger is kept because it is harmless (pinned offset) and bounded.
  - **Fallback**: `phone_deadline` expires with `received <= 1`. This is the live case: the head-not-yet-cached open above. Since `480e69e` a one-message thread no longer reaches it, because its load completes on `conversationLoaded` itself.
- The retry re-reads the Conversations interface as `requestConversation(threadId, 0, received + page)`. **The offset is deliberately pinned to 0 and only the range end moves.** The daemon serves newest-first, so widening the range returns the same messages, and `crbegin() + 0` can never index past the end of its cache. A non-zero `start` against a cache holding fewer messages **segfaults kdeconnectd 23.08.5** (no bounds check between the D-Bus wire and the iterator in `requestconversationworker.cpp`) and silently returns nothing on newer daemons - so an out-of-range offset is an applet-side correctness bug against every daemon version, and the daemon does not restart itself. Clamping `start` against the last known store count is **not** an alternative: every count the applet holds is stale in exactly the crash scenario (a restarted daemon with a cold cache) and it cannot detect that, so the clamp is circular. The resulting per-message signals merge via `known_message_ids` dedup.

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

- `ConversationLoadComplete` is idempotent. Math (sort, `messages_has_more`, `last_seen_sms`) always runs; loading-state clear and scroll-to-bottom snap fire only on the first arrival. Late completions silently refresh stats so a slow-completing subscription can't yank the user back to bottom.
- `thread_has_more: HashMap<i64, bool>` tracks exhaustion per thread as `loaded_t < total_count`, counting only messages carrying that `thread_id` — so per-thread counts are directly comparable and merged-set size no longer forces a heuristic. That rule applies on a thread open; an older page sets it by the settle rule under "Older Message Loading", and a later `conversationLoaded` never raises it. The heuristic survives only for `total_count == 0`, which the daemon uses to mean *unknown*, not *empty*; there it falls back to `loaded_t >= MESSAGES_PER_PAGE`. `messages_has_more` is the OR across `thread_has_more` over `current_merged_thread_ids`.

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

Each request is `requestConversation(threadId, 0, loaded_count + page)` — **the offset is pinned to 0 here for the same reason as the truncation retry above**: a non-zero `start` segfaults kdeconnectd 23.08.5.

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
or conversation address is available, Connected shows a status and sends nothing.

On success:

- the conversation preview updates immediately with the latest body and timestamp
- an optimistic sent bubble is inserted into the open thread
- the long-lived message subscription reconciles that optimistic entry when the phone echoes back the real sent message

For merged conversations, optimistic-send reconciliation matches by `OPTIMISTIC_MESSAGE_UID` + body + 5-minute window 
with no thread-id filter, so an echo arriving on a sibling thread within `merged_thread_ids` still upgrades the optimistic 
bubble in place. Because the phone files a reply by its recipient set, the echo lands on the canonical thread, which is
in `merged_thread_ids` whenever merging is on.

### New Messages

New-message compose uses `sendWithoutConversation(addresses, message, attachments)` with explicit recipients. On success, the compose flow returns to the conversation list and keeps the conversation-list subscription active so the phone can sync back the resulting thread.

### Attachments

Both compose surfaces stage one file for the next send, passed through the `attachmentUrls` parameter of `sendSms` (replies) and `sendWithoutConversation` (new messages).

**The wire value is a plain absolute path**, not a `file://` URL and not base64, despite the name. The daemon opens it with `QFile`, base64-encodes the contents into the packet and sniffs the MIME type itself (`smsplugin.cpp`). Under Flatpak the chooser portal returns a path under the document portal mount, which the daemon reads on the host as the same user, so no `finish-args` entry is needed.

**The daemon validates nothing.** A path it cannot open becomes an empty attachment that is still dispatched, and the phone drops the whole message, text included, with no error anywhere. Every guard is ours: `stage_attachment` in `store.rs` stats the file at pick time, and `attachment_value` in `send.rs` stats it again before each send because the file can vanish in between. A failed check refuses the send rather than degrading to text-only; the compose text and chip survive for a retry.

**Size is a hard limit.** When the staged file plus the compose text is over `MMS_MAX_BYTES` (800,000 bytes), Send is disabled in both views, the update arms refuse it as well because Enter bypasses the button, and the chip carries a warning glyph and tooltip. The figure sits above KDE Connect SMS app cap of 600 KB and below the 1 MB caps carriers commonly enforce. 

Each compose surface owns its own staged file, `pending_attachment` for the thread view and `new_message_pending_attachment` for New message, cleared wherever its sibling compose state is cleared. A shared field let a file staged in one surface be sent from the other with no chip in sight.

No local preview is built. The optimistic entry carries an empty attachment list and reconciliation adopts the echo's list (`store.rs`, the uid-upgrade step), so the sent bubble renders through the received-message path. Without that adoption an attachment send reconciles cleanly and renders forever without its image.

The chooser is a portal round trip: the dialog takes focus and the panel tears the popup down, and the `SmsAttachmentSelected` and `NewMessageAttachmentSelected` arms in `app.rs` reopen it on every return path, cancel included, unless the panel left it standing. The double-click gap is in `docs/KNOWN_ISSUES.md`.

Send outcome is unknowable for attachments as for text (see "A send has no terminal state" below); the phone side has two literal `// TODO: Notify other end` in `SmsMmsUtils.kt`. Copy may say "sent" in the hand-off sense, never "delivered".

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
- **A send has no terminal state.** `Ok` from `sendSms` / `sendWithoutConversation` means the D-Bus call worked, nothing more (`docs/DBUS.md` → "A void method's `Ok`"); the only evidence of delivery is the phone's echo, so an unconfirmed send stays an optimistic bubble with no failed state. The conversation list is not updated optimistically. The row changes when the phone's echo arrives on the live list subscription.
- **History: a pre-`26.08.0` daemon silently discards a `replyToConversation` when the thread is not in its cache.** Connected no longer calls `replyToConversation` (see "Replies"), so this no longer affects it; kept for anyone driving the daemon directly. Upstream BUG 517659, fixed in `3bf16922` and first shipped in `v26.08.0`: `replyToConversation()` logged a warning and dropped the message outright when `m_conversations` did not hold the target thread, which is the state after a device reconnect, or before the list has loaded for the session. **This is the daemon's behaviour, not applet logic**. 
- **An attachment-only MMS can echo back without its attachment.** A text-less send sometimes come back with `eventField 0` and an empty attachment list, so the bubble renders as a bare timestamp. No pattern found; upstream. See `docs/KNOWN_ISSUES.md`.

## Reference

### Key Symbols

Messages (see `app.rs`):

- `ConversationReceived` — cached or newly discovered conversation summary
- `ConversationSyncStarted` / `ConversationSyncComplete` — spinner lifecycle for the list
- `ConversationMessageReceived` — individual message during thread load or live updates
- `ConversationStoreLoaded` — the daemon's cached read finished (triggers initial scroll). The `Store` in the name is historical and does not mean a persistent store; see "Where the daemon's data actually lives"
- `ConversationLoadComplete` - the page is in, `received >= min(page, daemon count)`, or a net expired (sets `initial_load_complete`)

Timeout constants (see `constants.rs`):

- `CONVERSATION_LIST_PHONE_WAIT_MS` — cold-start bootstrap ceiling
- `CONVERSATION_TIMEOUT_CACHED_SECS` — warm-start bootstrap window
- `CONVERSATION_LIST_QUIET_MS` — quiet-window settle after bootstrap activity
- `CONVERSATION_LIST_RETRY_THRESHOLD` / `CONVERSATION_LIST_RETRY_WAIT_MS` — cold-start retry gate and window
- `PHONE_RESPONSE_TIMEOUT_MS` - net after `conversationLoaded` for a page that never fills; its `<= 1` branch fires the one-shot re-read
- `CONVERSATION_RETRY_WAIT_MS` - settle window after that one-shot Conversations-interface re-read
- `MESSAGE_SUBSCRIPTION_TIMEOUT_SECS` - 20 s no-signal net, restarted by every received message
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
