//! Centralized constants for timeouts, intervals, and limits.
//!
//! This module provides a single location for all tunable values used
//! throughout the applet, making them easy to discover and adjust.

/// D-Bus connection and signal handling constants.
pub mod dbus {
    /// Delay before retrying D-Bus connection after failure (seconds).
    pub const RETRY_DELAY_SECS: u64 = 5;

    /// Debounce interval for device refresh after D-Bus signals (seconds).
    /// Prevents rapid refreshes when multiple signals arrive in quick succession.
    pub const SIGNAL_REFRESH_DEBOUNCE_SECS: u64 = 3;

    /// Tick interval for checking whether a deferred refresh is due (seconds).
    /// When signals are dropped by the debounce window with no fetch in flight,
    /// the pending flag has no natural wake-up. This tick polls the flag and
    /// flushes it once the debounce window has cleared.
    pub const PENDING_REFRESH_TICK_SECS: u64 = 1;
    /// Backoff schedule for the daemon-activation race. Our proxies address the
    /// activatable name `org.kde.kdeconnect`, so a call made while the daemon is
    /// down starts it - but the bus calls activation done once the process owns
    /// the name, before the daemon has exported `/modules/kdeconnect`. That first
    /// call can come back `UnknownObject`/`UnknownInterface` on a healthy install.
    ///
    /// Recovery is sub-second, hence far tighter than `RETRY_DELAY_SECS`, which is
    /// tuned for SMS page loads. First entry is 0: attempt once immediately. Worst
    /// case before a genuine error reaches the UI is the sum, ~1.55s.
    pub const ACTIVATION_RETRY_DELAYS_MS: &[u64] = &[0, 150, 400, 1000];
}

/// SMS conversation and message loading constants.
pub mod sms {
    /// Refuse an MMS over this many bytes, attachment plus text. Above KDE Connect's own
    /// 600,000, below the ~1 MB caps carriers commonly enforce.
    pub const MMS_MAX_BYTES: u64 = 800_000;

    /// Maximum number of logical conversations kept in the derived list for display.
    pub const CONVERSATION_LIST_DISPLAY_LIMIT: usize = 40;
    /// Max height (logical px) of the conversation-list scroll area
    pub const CONVERSATION_LIST_MAX_HEIGHT: f32 = 520.0;
    /// SMS messages fetched per request (thread open + each older-message page).
    pub const MESSAGES_PER_PAGE: u32 = 10;

    /// Max contact suggestions shown in the new-message recipient dropdown
    pub const MAX_SUGGESTIONS_SHOWN: usize = 5;

    /// Sync-indicator window for a warm-start list (seconds): how long the list
    /// subscription shows the indicator after firing the bootstrap when the applet
    /// already holds a cached list. Dismissal only; the stream keeps ingesting after it.
    pub const CONVERSATION_TIMEOUT_CACHED_SECS: u64 = 3;

    /// No-signal net for a thread load (seconds). Restarted by every received
    /// message and disarmed by `conversationLoaded`, so it fires only when the
    /// daemon goes silent before answering: the phone never responds, or a
    /// >= 26.08.0 daemon has nothing cached for the thread and emits nothing.
    pub const MESSAGE_SUBSCRIPTION_TIMEOUT_SECS: u64 = 20;

    /// Window after `conversationLoaded` (milliseconds). The page normally follows
    /// that signal within a millisecond and completes the load on the received
    /// count; this fires only when the page never fills, and with at most one
    /// message received it triggers the one-shot retry instead.
    pub const PHONE_RESPONSE_TIMEOUT_MS: u64 = 8000;

    /// Settle window for the one-shot retried Conversations-interface read
    /// (milliseconds), fired when `PHONE_RESPONSE_TIMEOUT_MS` elapses with at most
    /// one message received. The daemon inserts the phone batch before it emits
    /// `conversationLoaded`, so the re-read pages the full thread; this only
    /// bounds how long the load waits for those signals.
    pub const CONVERSATION_RETRY_WAIT_MS: u64 = 4000;

    /// How long to show the sync indicator on cold start (milliseconds).
    /// This is the hard ceiling for a single bootstrap attempt while loading
    /// the conversation list on cold start.
    pub const CONVERSATION_LIST_PHONE_WAIT_MS: u64 = 8000;

    /// How long the post-send sync indicator stays up while waiting for the phone
    /// to sync a newly created conversation back. Advisory only: no send-outcome
    /// reporting exists at any layer (daemon or phone), so nothing guarantees the
    /// self-clearing instead of sticky.
    pub const NEW_MESSAGE_SYNC_INDICATOR_SECS: u64 = 15;

    /// After we have seen activity during bootstrap, treat the conversation
    /// list as settled once it stays quiet for this long.
    pub const CONVERSATION_LIST_QUIET_MS: u64 = 2000;

    /// If a cold bootstrap attempt settles with fewer than this many
    /// conversations, issue one more request before declaring sync complete.
    pub const CONVERSATION_LIST_RETRY_THRESHOLD: usize = 5;

    /// How long to wait for the retry bootstrap attempt.
    pub const CONVERSATION_LIST_RETRY_WAIT_MS: u64 = 6000;

    /// Bound retries for a transient `request_conversion` rejection (busy /
    /// briefly-unavailable daemon) before surfacing SmsError. Each waits RETRY_DELAY_SECS
    pub const MAX_MESSAGE_LOAD_RETRIES: u8 = 3;

    /// How long the reply caption reads "Sending..." before it relabels to
    /// "Not confirmed by your phone". A late echo still clears it. See docs/SMS.md.
    pub const SEND_ECHO_WINDOW_SECS: u64 = 60;
}

/// Popup geometry.
pub mod popup {
    /// libcosmic's `popup_container` height cap; the output cap can only lower it.
    pub const MAX_HEIGHT: f32 = 1000.0;
    /// Kept clear beyond the panel slot: panel margin, the 4 px popup offset, and slack.
    pub const SCREEN_MARGIN: f32 = 16.0;
}

/// Refresh and polling interval constants.
pub mod refresh {
    /// Interval for refreshing media player state (seconds).
    pub const MEDIA_INTERVAL_SECS: u64 = 2;
}

/// Notification display constants.
pub mod notifications {
    /// Requested expire_timeout (ms) for normal-urgency toasts (SMS / file / missed-call).
    /// Large on purpose: COSMIC clamps normal/low to `max_timeout_normal` (default 5s),
    /// so this defers the real duration to the system setting — honors a future raised
    /// cap automatically, and stays 5s today. (The daemon clamps; it does NOT ignore.)
    pub const NORMAL_NOTIFICATION_TIMEOUT_MS: u32 = 30_000;

    /// Display duration (ms) for the Critical incoming-call toast. COSMIC does NOT cap
    /// urgent notifications (`max_timeout_urgent = None`), so this is the LITERAL on-screen
    /// time — distinct mechanism from NORMAL_* above despite the equal number. ~30s ≈ the
    /// Android ring-to-voicemail window. NOT Timeout::Never: Connected has no active
    /// dismissal on call-end, so Never would leave a stale toast until manual dismiss.
    pub const CALL_RING_TIMEOUT_MS: u32 = 30_000;
}
