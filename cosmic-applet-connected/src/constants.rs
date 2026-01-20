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
}

/// SMS conversation and message loading constants.
pub mod sms {
    /// Timeout for conversation loading when cache exists (seconds).
    /// Shorter since we only need incremental updates.
    pub const CONVERSATION_TIMEOUT_CACHED_SECS: u64 = 3;

    /// Timeout for conversation loading on initial load (seconds).
    /// Longer to allow phone time to send all data.
    pub const CONVERSATION_TIMEOUT_INITIAL_SECS: u64 = 15;

    /// Activity timeout - stop collecting if no signals received (milliseconds).
    /// After receiving data, we stop waiting this long after the last signal.
    pub const SIGNAL_ACTIVITY_TIMEOUT_MS: u64 = 500;

    /// Interval for checking timeout conditions during signal collection (milliseconds).
    pub const TIMEOUT_CHECK_INTERVAL_MS: u64 = 50;

    /// Timeout for draining remaining buffered signals (milliseconds).
    pub const SIGNAL_DRAIN_TIMEOUT_MS: u64 = 5;

    /// Timeout for loading messages in a conversation thread (seconds).
    pub const MESSAGE_FETCH_TIMEOUT_SECS: u64 = 10;

    /// Interval for polling in fallback mode (milliseconds).
    pub const FALLBACK_POLLING_INTERVAL_MS: u64 = 500;

    /// Polling delays for fallback conversation loading (milliseconds).
    /// We poll multiple times with increasing delays to give the phone time to sync.
    pub const FALLBACK_POLLING_DELAYS_MS: &[u64] = &[500, 1000, 1500, 2000, 3000];

    /// Maximum number of conversation message threads to cache.
    /// When this limit is reached, the least recently accessed conversation is evicted.
    pub const MESSAGE_CACHE_MAX_CONVERSATIONS: usize = 10;
}

/// Refresh and polling interval constants.
pub mod refresh {
    /// Delay after sending SMS before refreshing the thread (seconds).
    pub const POST_SEND_DELAY_SECS: u64 = 2;

    /// Interval for refreshing media player state (seconds).
    pub const MEDIA_INTERVAL_SECS: u64 = 2;
}

/// Notification display constants.
pub mod notifications {
    /// Timeout for file received notifications (milliseconds).
    pub const FILE_TIMEOUT_MS: u32 = 5000;
}
