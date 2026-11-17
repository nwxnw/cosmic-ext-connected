//! D-Bus signal subscriptions for real-time updates from KDE Connect.

use crate::app::Message;
use crate::constants::dbus::RETRY_DELAY_SECS;
use crate::constants::sms::{
    CONVERSATION_RETRY_WAIT_MS, MAX_MESSAGE_LOAD_RETRIES, MESSAGE_SUBSCRIPTION_TIMEOUT_SECS,
    PHONE_RESPONSE_TIMEOUT_MS,
};
use crate::notifications::{
    should_show_call_notification, should_show_file_notification, should_show_sms_notification,
};
use futures_util::StreamExt;
use kdeconnect_dbus::plugins::{parse_sms_message, MessageType};
use zbus::Connection;

/// Retry a fallible D-Bus construction step `MAX_MESSAGE_LOAD_RETRIES` times,
/// `RETRY_DELAY_SECS` apart. Connection only: proxy construction does not round
/// tip, so wrapping it just repeats local validation errors for ten seconds.
///
/// On `Err` the caller must terminate with `Done`; re-entering `Init` hot-loops.
pub(crate) async fn with_dbus_retries<T, E, F, Fut>(label: &str, mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt: u8 = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt >= MAX_MESSAGE_LOAD_RETRIES {
                    tracing::warn!("{} failed after {} attempts: {}", label, attempt, e);
                    return Err(e);
                }
                tracing::warn!(
                    "{} failed (attempt {}/{}), retrying in {}s: {}",
                    label,
                    attempt,
                    MAX_MESSAGE_LOAD_RETRIES,
                    RETRY_DELAY_SECS,
                    e
                );
                tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
            }
        }
    }
}

/// Re-issue `requestConversation` on the Conversations interface as part of the
/// first-open truncation recovery path. The daemon worker treats `[start, end)`
/// as indices into its in-memory cache for the thread, sorted newest-first.
///
/// `start` is pinned to **0** and only `end` varies. Requesting `[numKnown, …)` -
/// the contract this used to follow - segfaults kdeconnectd 23.08.5 whenever the
/// daemon's cache holds fewer than `numKnown` messages, which is routine after a
/// daemon restart. Serving from 0 returns the same messages newest-first and can
/// never index past the end.
async fn fire_retry_request(
    conn: &Connection,
    device_id: &str,
    thread_id: i64,
    end: i32,
) -> Result<(), String> {
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);
    let proxy = kdeconnect_dbus::plugins::ConversationsProxy::builder(conn)
        .path(device_path.as_str())
        .map_err(|e| format!("path: {}", e))?
        .build()
        .await
        .map_err(|e| format!("build: {}", e))?;
    proxy
        .request_conversation(thread_id, 0, end)
        .await
        .map_err(|e| format!("call: {}", e))
}

/// State for D-Bus signal subscription.
#[allow(clippy::large_enum_variant)]
enum DbusSubscriptionState {
    Init,
    Listening {
        #[allow(dead_code)]
        conn: Connection,
        stream: zbus::MessageStream,
    },
}

/// Create a stream that listens for D-Bus signals from KDE Connect.
pub fn dbus_signal_subscription() -> impl futures_util::Stream<Item = Message> {
    futures_util::stream::unfold(DbusSubscriptionState::Init, |state| async move {
        match state {
            DbusSubscriptionState::Init => {
                // Connect to D-Bus
                let conn = match Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to connect to D-Bus for signals: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                        return Some((
                            Message::SubscriptionRetrying("D-Bus connection failed".to_string()),
                            DbusSubscriptionState::Init,
                        ));
                    }
                };

                // Add match rule to receive KDE Connect signals
                let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("Failed to create DBus proxy: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                        return Some((
                            Message::SubscriptionRetrying("D-Bus proxy failed".to_string()),
                            DbusSubscriptionState::Init,
                        ));
                    }
                };

                // Subscribe to all signals from KDE Connect daemon
                if let Ok(rule) = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .sender("org.kde.kdeconnect")
                    .map(|b| b.build())
                {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add match rule: {}", e);
                    } else {
                        tracing::debug!("Added match rule for kdeconnect daemon signals");
                    }
                }

                // Owner loss of the daemon's name. Emitted by the bus itself, so the
                // sender rule above does not cover it.
                if let Ok(rule) = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.DBus")
                    .and_then(|b| b.member("NameOwnerChanged"))
                    .and_then(|b| b.arg(0, "org.kde.kdeconnect"))
                    .map(|b| b.build())
                {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add NameOwnerChanged match rule: {}", e);
                    }
                }

                // Subscribe to share plugin signals for file notifications
                if let Ok(rule) = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.kde.kdeconnect.device.share")
                    .map(|b| b.build())
                {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add share match rule: {}", e);
                    } else {
                        tracing::debug!("Added match rule for share signals");
                    }
                } else {
                    tracing::warn!("Failed to build share match rule");
                }

                tracing::debug!("D-Bus signal subscription started");

                // Create message stream
                let stream = zbus::MessageStream::from(&conn);

                Some((
                    Message::DbusSignalReceived,
                    DbusSubscriptionState::Listening { conn, stream },
                ))
            }
            DbusSubscriptionState::Listening { conn, mut stream } => {
                // Wait for relevant signals - be selective to avoid excessive refreshes
                loop {
                    match stream.next().await {
                        Some(Ok(msg)) => {
                            if msg.header().message_type() == zbus::message::Type::Signal {
                                if let (Some(interface), Some(member)) =
                                    (msg.header().interface(), msg.header().member())
                                {
                                    let iface_str = interface.as_str();
                                    let member_str = member.as_str();

                                    // Handle share signals for file notifications
                                    if iface_str == "org.kde.kdeconnect.device.share"
                                        && member_str == "shareReceived"
                                    {
                                        // Extract device ID from path
                                        if let Some(path) = msg.header().path() {
                                            let path_str = path.as_str();
                                            if let Some(rest) = path_str
                                                .strip_prefix("/modules/kdeconnect/devices/")
                                            {
                                                let device_id = rest
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or(rest)
                                                    .to_string();

                                                // Parse the signal body
                                                let body = msg.body();
                                                if let Ok((file_url,)) =
                                                    body.deserialize::<(String,)>()
                                                {
                                                    // Cross-process deduplication via file lock
                                                    // KDE Connect sends 3 duplicate signals per file transfer
                                                    // and COSMIC spawns multiple applet processes
                                                    if !should_show_file_notification(&file_url) {
                                                        continue;
                                                    }

                                                    let file_name = file_url
                                                        .strip_prefix("file://")
                                                        .unwrap_or(&file_url)
                                                        .rsplit('/')
                                                        .next()
                                                        .unwrap_or("file")
                                                        .to_string();

                                                    return Some((
                                                        Message::FileReceived {
                                                            device_name: device_id,
                                                            file_url,
                                                            file_name,
                                                        },
                                                        DbusSubscriptionState::Listening {
                                                            conn,
                                                            stream,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    // Incoming SMS, for desktop notifications. The daemon also re-emits
                                    // conversationUpdated for every message of a page it serves; the
                                    // update arm's freshness check filters those.
                                    if iface_str == "org.kde.kdeconnect.device.conversations"
                                        && member_str == "conversationUpdated"
                                    {
                                        if let Some(path) = msg.header().path() {
                                            if let Some(rest) = path
                                                .as_str()
                                                .strip_prefix("/modules/kdeconnect/devices/")
                                            {
                                                let device_id = rest
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or(rest)
                                                    .to_string();
                                                let body = msg.body();
                                                if let Ok(value) =
                                                    body.deserialize::<zbus::zvariant::OwnedValue>()
                                                {
                                                    if let Some(sms_msg) = parse_sms_message(&value)
                                                    {
                                                        // Inbox (1) = received from others
                                                        if sms_msg.message_type
                                                            == MessageType::Inbox
                                                        {
                                                            // Cross-process deduplication via file lock
                                                            if !should_show_sms_notification(
                                                                sms_msg.thread_id,
                                                                sms_msg.date,
                                                            ) {
                                                                continue;
                                                            }
                                                            tracing::debug!(
                                                                "SMS received from {} on device {}",
                                                                sms_msg.primary_address(),
                                                                device_id
                                                            );
                                                            return Some((
                                                                Message::SmsNotificationReceived(
                                                                    device_id, sms_msg,
                                                                ),
                                                                DbusSubscriptionState::Listening {
                                                                    conn,
                                                                    stream,
                                                                },
                                                            ));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        continue;
                                    }
                                    // Incoming and missed calls, for desktop notifications.
                                    // Arguments: (event, phone_number, contact_name)
                                    if iface_str == "org.kde.kdeconnect.device.telephony"
                                        && member_str == "callReceived"
                                    {
                                        if let Some(path) = msg.header().path() {
                                            if let Some(rest) = path
                                                .as_str()
                                                .strip_prefix("/modules/kdeconnect/devices/")
                                            {
                                                let device_id = rest
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or(rest)
                                                    .to_string();
                                                let body = msg.body();
                                                if let Ok((event, phone_number, contact_name)) =
                                                    body.deserialize::<(String, String, String)>()
                                                {
                                                    // Cross-process deduplication via file lock
                                                    if !should_show_call_notification(
                                                        &event,
                                                        &phone_number,
                                                    ) {
                                                        continue;
                                                    }
                                                    tracing::debug!(
                                                        "Call signal: {} from {} ({}) on device {}",
                                                        event,
                                                        contact_name,
                                                        phone_number,
                                                        device_id
                                                    );
                                                    return Some((
                                                        Message::CallNotification {
                                                            device_id,
                                                            event,
                                                            phone_number,
                                                            contact_name,
                                                        },
                                                        DbusSubscriptionState::Listening {
                                                            conn,
                                                            stream,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                        continue;
                                    }
                                    // Pairing failure carries its reason as a string.
                                    // The daemon emits pairStateChanged ~20µs later
                                    // (core/device.cpp:257-261), so pair state already
                                    // corrects itself — this is only the reason.
                                    if iface_str == "org.kde.kdeconnect.device"
                                        && member_str == "pairingFailed"
                                    {
                                        if let Some(path) = msg.header().path() {
                                            if let Some(rest) = path
                                                .as_str()
                                                .strip_prefix("/modules/kdeconnect/devices/")
                                            {
                                                let device_id = rest
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or(rest)
                                                    .to_string();
                                                if let Ok((error,)) =
                                                    msg.body().deserialize::<(String,)>()
                                                {
                                                    return Some((
                                                        Message::PairingFailed { device_id, error },
                                                        DbusSubscriptionState::Listening {
                                                            conn,
                                                            stream,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    // The daemon exited: the bus's owner-loss signal for its name.
                                    if iface_str == "org.freedesktop.DBus"
                                        && member_str == "NameOwnerChanged"
                                    {
                                        if let Ok((name, old, new)) =
                                            msg.body().deserialize::<(String, String, String)>()
                                        {
                                            if name == "org.kde.kdeconnect" && new.is_empty() {
                                                tracing::info!(
                                                    "kdeconnectd lost its name (was {})",
                                                    old
                                                );
                                                return Some((
                                                    Message::DaemonGone,
                                                    DbusSubscriptionState::Listening {
                                                        conn,
                                                        stream,
                                                    },
                                                ));
                                            }
                                        }
                                        continue;
                                    }

                                    // Only trigger refresh on specific device-related signals.
                                    // Signal names match upstream KDE Connect
                                    // (see core/daemon.h and core/device.h). The previous
                                    // device-level names trustedChanged / pairingRequest /
                                    // hasPairingRequestsChanged do not exist upstream — pair
                                    // state is emitted as pairStateChanged(int).
                                    let is_relevant = match iface_str {
                                        // Daemon signals for device discovery and pair-state aggregate
                                        "org.kde.kdeconnect.daemon" => matches!(
                                            member_str,
                                            "deviceAdded"
                                                | "deviceRemoved"
                                                | "deviceVisibilityChanged"
                                                | "announcedNameChanged"
                                                | "pairingRequestsChanged"
                                        ),
                                        // Device signals for reachability and pair state
                                        "org.kde.kdeconnect.device" => matches!(
                                            member_str,
                                            "reachableChanged" | "pairStateChanged"
                                        ),
                                        // Battery and notification plugin signals
                                        "org.kde.kdeconnect.device.battery" => true,
                                        "org.kde.kdeconnect.device.notifications" => true,
                                        // PropertiesChanged from the daemon only: the sender rule
                                        // above is the only route in.
                                        "org.freedesktop.DBus.Properties" => {
                                            member_str == "PropertiesChanged"
                                        }
                                        _ => false,
                                    };

                                    if is_relevant {
                                        tracing::debug!("D-Bus signal: {}.{}", interface, member);
                                        return Some((
                                            Message::DbusSignalReceived,
                                            DbusSubscriptionState::Listening { conn, stream },
                                        ));
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!("D-Bus stream error: {}", e);
                        }
                        None => {
                            tracing::warn!("D-Bus stream ended, reconnecting...");
                            return Some((
                                Message::DbusSignalReceived,
                                DbusSubscriptionState::Init,
                            ));
                        }
                    }
                }
            }
        }
    })
}

/// State for conversation message subscription (incremental message loading).
#[allow(clippy::large_enum_variant)]
enum ConversationMessageState {
    Init {
        thread_id: i64,
        device_id: String,
        messages_per_page: u32,
    },
    Listening {
        conn: Connection,
        stream: zbus::MessageStream,
        thread_id: i64,
        device_id: String,
        messages_per_page: u32,
        /// Set once `conversationLoaded` has been seen (or the load completed on its
        /// page); disarms the no-signal net; `phone_deadline` takes over from here.
        local_store_done: bool,
        /// Total message count from `conversationLoaded` signal.
        total_message_count: Option<u64>,
        /// Armed by `conversationLoaded`. On a fetch the page follows that signal within a
        /// millisecond and completes the load on the count target. This fires only when the
        /// page never fills: `ConversationLoadComplete` once, or the one-shot retry when
        /// at most one message arrived. The subscription continues with a heartbeat sleep.
        phone_deadline: Option<tokio::time::Instant>,
        /// Whether `ConversationLoadComplete` has been emitted. Once true, the subscription
        /// continues silently with a heartbeat sleep until iced drops it.
        load_complete_emitted: bool,
        /// Count of `conversationUpdated` signals forwarded for this thread. Compared
        /// against the completion target on every pass, and against the one-shot retry's
        /// `<= 1` threshold at `phone_deadline` expiry.
        received_message_count: usize,
        /// Whether the one-shot truncation-recovery retry has fired. Bounds retries
        /// to one across both triggers (`docs/SMS.md`, "first-open truncation").
        retry_attempted: bool,
    },
    /// Terminal state - subscription is complete
    Done,
}

/// Heartbeat interval after initial load completes (seconds).
const MESSAGE_HEARTBEAT_SLEEP_SECS: u64 = 30;

/// Create a stream that listens for conversation messages during loading.
///
/// This subscription handles incremental message loading by:
/// 1. Setting up D-Bus match rules for signals
/// 2. Firing the request_conversation D-Bus call (AFTER rules are set up)
/// 3. Listening for `conversationUpdated` signals (individual messages)
/// 4. Emitting `ConversationLoadComplete` once `received >= min(page, conversationLoaded
///    count)` or when a clock fires with the page unfilled (`PHONE_RESPONSE_TIMEOUT_MS`
///    after `conversationLoaded`, `MESSAGE_SUBSCRIPTION_TIMEOUT_SECS` of silence before it)
/// 5. Continuing to listen for new messages until the conversation is closed
///
/// The subscription runs as long as the conversation is open. It is canceled by
/// iced dropping it when `conversation_load_active` becomes false (CloseConversation).
///
/// The request is fired from within the subscription to avoid race conditions
/// where signals arrive before we're ready to receive them.
pub fn conversation_message_subscription(
    thread_id: i64,
    device_id: String,
    messages_per_page: u32,
) -> impl futures_util::Stream<Item = Message> {
    futures_util::stream::unfold(
        ConversationMessageState::Init {
            thread_id,
            device_id,
            messages_per_page,
        },
        |state| async move {
            match state {
                ConversationMessageState::Init {
                    thread_id,
                    device_id,
                    messages_per_page,
                } => {
                    // Connect to D-Bus
                    let conn = match with_dbus_retries(
                        "thread D-Bus session connect",
                        Connection::session,
                    )
                    .await
                    {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!(
                                "Message subscription: D-Bus connect failed for conversation: {}",
                                e
                            );
                            return Some((
                                Message::SmsError(e.to_string()),
                                ConversationMessageState::Done,
                            ));
                        }
                    };

                    // Add match rule for conversationUpdated signals
                    let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!(
                                "Message subscription: D-Bus proxy failed for conversation: {}",
                                e
                            );
                            return Some((
                                Message::SmsError(e.to_string()),
                                ConversationMessageState::Done,
                            ));
                        }
                    };

                    // Subscribe to conversationUpdated signals (individual messages)
                    let updated_rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .interface("org.kde.kdeconnect.device.conversations")
                        .and_then(|b| b.member("conversationUpdated"))
                        .map(|b| b.build());

                    if let Ok(rule) = updated_rule {
                        if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                            tracing::warn!("Failed to add conversationUpdated match rule: {}", e);
                        } else {
                            tracing::debug!(
                                "Added match rule for conversation {} message signals",
                                thread_id
                            );
                        }
                    }

                    // Subscribe to conversationLoaded signals (completion marker)
                    let loaded_rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .interface("org.kde.kdeconnect.device.conversations")
                        .and_then(|b| b.member("conversationLoaded"))
                        .map(|b| b.build());

                    if let Ok(rule) = loaded_rule {
                        if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                            tracing::warn!("Failed to add conversationLoaded match rule: {}", e);
                        } else {
                            tracing::debug!(
                                "Added match rule for conversation {} loaded signal",
                                thread_id
                            );
                        }
                    }

                    // Create message stream BEFORE firing request
                    let stream = zbus::MessageStream::from(&conn);

                    // NOW fire the D-Bus request - after match rules are set up
                    // This ensures we don't miss any signals
                    let device_path =
                        format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

                    // One request on open: the Conversations interface's requestConversation.
                    // RequestConversationWorker pages the daemon's cache, fetching from the
                    // phone when the cache cannot fill the page (26.08.0+ skips the fetch on
                    // an empty cache); each message arrives as a conversationUpdated signal.

                    // Fire Conversations interface request (provides per-message signals)
                    let conversations_proxy = match async {
                        kdeconnect_dbus::plugins::ConversationsProxy::builder(&conn)
                            .path(device_path.as_str())?
                            .build()
                            .await
                    }
                    .await
                    {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!(
                                "Message subscription: Failed to create conversations proxy: {}",
                                e
                            );
                            return Some((
                                Message::SmsError(e.to_string()),
                                ConversationMessageState::Done,
                            ));
                        }
                    };
                    tracing::debug!(
                        "Firing request_conversation for thread {} (messages 0-{})",
                        thread_id,
                        messages_per_page
                    );
                    let mut attempt: u8 = 0;
                    loop {
                        match conversations_proxy
                            .request_conversation(thread_id, 0, messages_per_page as i32)
                            .await
                        {
                            Ok(()) => break,
                            Err(e) => {
                                attempt += 1;
                                if attempt >= MAX_MESSAGE_LOAD_RETRIES {
                                    tracing::warn!(
                                                    "request_conversation for thread {} failed after {} attempts: {}",
                                                    thread_id, attempt, e
                                                );
                                    return Some((
                                        Message::SmsError(e.to_string()),
                                        ConversationMessageState::Done, //give up gracefully
                                    ));
                                }
                                tracing::warn!(
                                                "request_conversation for thread {} failed (attempt {}/{}), retrying in  {}s: {}",
                                                thread_id, attempt, MAX_MESSAGE_LOAD_RETRIES, RETRY_DELAY_SECS, e
                                            );
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    RETRY_DELAY_SECS,
                                ))
                                .await;
                            }
                        }
                    }
                    tracing::info!(
                        "Conversation {} request sent, listening for signals",
                        thread_id
                    );

                    // Move to listening state, emit started message
                    Some((
                        Message::ConversationLoadStarted { thread_id },
                        ConversationMessageState::Listening {
                            conn,
                            stream,
                            thread_id,
                            device_id,
                            messages_per_page,
                            local_store_done: false,
                            total_message_count: None,
                            phone_deadline: None,
                            load_complete_emitted: false,
                            received_message_count: 0,
                            retry_attempted: false,
                        },
                    ))
                }
                ConversationMessageState::Listening {
                    conn,
                    mut stream,
                    thread_id,
                    device_id,
                    messages_per_page,
                    mut local_store_done,
                    mut total_message_count,
                    mut phone_deadline,
                    mut load_complete_emitted,
                    mut received_message_count,
                    mut retry_attempted,
                } => {
                    // Initial load, then long-lived listening. What the daemon does on a
                    // thread open:
                    //
                    // - Cache-served: the worker emits a full page of `conversationUpdated`
                    //   at once and no `conversationLoaded`. Completion is the page count.
                    // - Fetched: the worker emits what it holds and blocks until the phone
                    //   answers; `addMessages` then inserts the batch, emits
                    //   `conversationLoaded(count)` and releases the worker, whose page
                    //   signals are queued behind that emit. `conversationLoaded` therefore
                    //   arrives BEFORE the page's remaining messages. Completion is
                    //   `received >= min(page, count)`, checked at the top of every pass.
                    //
                    // After completion the subscription keeps listening with a heartbeat
                    // sleep, catching new messages (including sent echoes) until iced drops
                    // it on CloseConversation.
                    let local_store_timeout =
                        std::time::Duration::from_secs(MESSAGE_SUBSCRIPTION_TIMEOUT_SECS);
                    let phone_wait = std::time::Duration::from_millis(PHONE_RESPONSE_TIMEOUT_MS);
                    // No-signal net, armed until `conversationLoaded` is seen or the load
                    // completes. Recomputed on every re-entry to `Listening`, i.e. after
                    // every received message, so it is an inactivity timer, not a deadline
                    // measured from the open.
                    let local_store_deadline = if !local_store_done && !load_complete_emitted {
                        Some(tokio::time::Instant::now() + local_store_timeout)
                    } else {
                        None
                    };

                    loop {
                        let now = tokio::time::Instant::now();

                        // Completion target: a full page, or everything the daemon says it holds for
                        // this thread, whichever is smaller. `conversationLoaded` arrives before the
                        // worker's page when the daemon fetched (Model §2), so the target must not be
                        // gated on whether it has been seen; `load_complete_emitted` prevents re-firing.
                        let target = total_message_count
                            .filter(|n| *n > 0)
                            .map(|n| (n as usize).min(messages_per_page as usize))
                            .unwrap_or(messages_per_page as usize);
                        if !load_complete_emitted && received_message_count >= target {
                            tracing::info!(
                                "Subscription: thread {} received {} of target {} messages, signaling load complete",
                                thread_id,
                                received_message_count,
                                target
                            );
                            local_store_done = true;
                            load_complete_emitted = true;
                            return Some((
                                Message::ConversationLoadComplete {
                                    thread_id,
                                    total_count: total_message_count.unwrap_or(0),
                                },
                                ConversationMessageState::Listening {
                                    conn,
                                    stream,
                                    thread_id,
                                    device_id,
                                    messages_per_page,
                                    local_store_done,
                                    total_message_count,
                                    phone_deadline,
                                    load_complete_emitted,
                                    received_message_count,
                                    retry_attempted,
                                },
                            ));
                        }

                        // No-signal net: nothing arrived for MESSAGE_SUBSCRIPTION_TIMEOUT_SECS
                        // and `conversationLoaded` has not been seen. The phone never answered
                        // or a >= 26.08.0 daemon had nothing cached for the thread and
                        // emitted nothing (`0dc666ad9`).
                        if let Some(lsd) = local_store_deadline {
                            if !local_store_done && !load_complete_emitted && now >= lsd {
                                tracing::info!(
                                    "Subscription: no signal for thread {} within the inactivity \
                                     net (conversationLoaded never received), signaling load complete",
                                    thread_id
                                );
                                load_complete_emitted = true;
                                return Some((
                                    Message::ConversationLoadComplete {
                                        thread_id,
                                        total_count: total_message_count.unwrap_or(0),
                                    },
                                    ConversationMessageState::Listening {
                                        conn,
                                        stream,
                                        thread_id,
                                        device_id,
                                        messages_per_page,
                                        local_store_done,
                                        total_message_count,
                                        phone_deadline,
                                        load_complete_emitted,
                                        received_message_count,
                                        retry_attempted,
                                    },
                                ));
                            }
                        }

                        // `PHONE_RESPONSE_TIMEOUT_MS` elapsed after `conversationLoaded` with
                        // the page still unfilled. Normally the page follows that signal within
                        // a millisecond and completes the load on the count target above, so
                        // arriving here means the daemon had little or nothing to page from.
                        //
                        // One-shot retry (`docs/SMS.md`, "first-open truncation"): at most one
                        // message received means the thread was opened before the list
                        // bootstrap cached its head (the worker paged from an empty or
                        // one-message cache), or the thread has a single message and the
                        // fetch returned it as a duplicate. The daemon inserts the phone batch
                        // before it emits `conversationLoaded`, so a re-issued Conversations
                        // interface read pages the full thread. Bound to one attempt; a short
                        // page with more than one message completes as is.
                        if !load_complete_emitted {
                            if let Some(pd) = phone_deadline {
                                if now >= pd {
                                    if !retry_attempted && received_message_count <= 1 {
                                        tracing::info!(
                                            "Subscription: thread {} truncation suspected \
                                             ({} message(s) after the phone window), retrying \
                                             Conversations interface read (fallback path)",
                                            thread_id,
                                            received_message_count
                                        );
                                        let end = (received_message_count
                                            + messages_per_page as usize)
                                            as i32;
                                        match fire_retry_request(&conn, &device_id, thread_id, end)
                                            .await
                                        {
                                            Ok(()) => {
                                                tracing::info!(
                                                    "Retry request_conversation fired for thread {}",
                                                    thread_id
                                                );
                                            }
                                            Err(e) => {
                                                tracing::warn!(
                                                    "Retry request_conversation failed for thread {}: {}",
                                                    thread_id,
                                                    e
                                                );
                                            }
                                        }
                                        retry_attempted = true;
                                        phone_deadline = Some(
                                            tokio::time::Instant::now()
                                                + std::time::Duration::from_millis(
                                                    CONVERSATION_RETRY_WAIT_MS,
                                                ),
                                        );
                                        // Fall through to the select! loop so retry
                                        // signals are received as they arrive.
                                    } else {
                                        if retry_attempted {
                                            tracing::info!(
                                                "Subscription: thread {} retry settled with {} \
                                                 message(s); signaling load complete",
                                                thread_id,
                                                received_message_count
                                            );
                                        } else {
                                            tracing::info!(
                                                "Subscription: phone deadline reached for thread {}, \
                                                 signaling load complete (subscription continues)",
                                                thread_id
                                            );
                                        }
                                        load_complete_emitted = true;
                                        return Some((
                                            Message::ConversationLoadComplete {
                                                thread_id,
                                                total_count: total_message_count.unwrap_or(0),
                                            },
                                            ConversationMessageState::Listening {
                                                conn,
                                                stream,
                                                thread_id,
                                                device_id,
                                                messages_per_page,
                                                local_store_done,
                                                total_message_count,
                                                phone_deadline,
                                                load_complete_emitted,
                                                received_message_count,
                                                retry_attempted,
                                            },
                                        ));
                                    }
                                }
                            }
                        }

                        // Sleep until the next clock that can act
                        let wait_duration = if load_complete_emitted {
                            // Load complete: long heartbeat, just keeping unfold alive
                            tokio::time::Duration::from_secs(MESSAGE_HEARTBEAT_SLEEP_SECS)
                        } else if let Some(pd) = phone_deadline {
                            // `conversationLoaded` seen: sleep until the phone window elapses
                            pd.saturating_duration_since(now)
                        } else if let Some(lsd) = local_store_deadline {
                            // Nothing seen yet: sleep until the no-signal net fires
                            lsd.saturating_duration_since(now)
                        } else {
                            // Fallback heartbeat
                            tokio::time::Duration::from_secs(MESSAGE_HEARTBEAT_SLEEP_SECS)
                        };

                        tokio::select! {
                            biased;

                            // Priority: D-Bus signals
                            msg_option = stream.next() => {
                                match msg_option {
                                    Some(Ok(msg)) => {
                                        if msg.header().message_type() == zbus::message::Type::Signal {
                                            if let (Some(interface), Some(member)) =
                                                (msg.header().interface(), msg.header().member())
                                            {
                                                let iface_str = interface.as_str();
                                                let member_str = member.as_str();

                                                // Handle conversationUpdated signals (individual messages)
                                                if iface_str == "org.kde.kdeconnect.device.conversations"
                                                    && member_str == "conversationUpdated"
                                                {
                                                    let body = msg.body();
                                                    if let Ok(value) =
                                                        body.deserialize::<zbus::zvariant::OwnedValue>()
                                                    {
                                                        if let Some(sms_msg) = parse_sms_message(&value) {
                                                            if sms_msg.thread_id == thread_id {
                                                                tracing::debug!(
                                                                    "Subscription: received message uid={} for thread {}",
                                                                    sms_msg.uid,
                                                                    thread_id
                                                                );
                                                                received_message_count =
                                                                    received_message_count
                                                                        .saturating_add(1);
                                                                return Some((
                                                                    Message::ConversationMessageReceived {
                                                                        thread_id,
                                                                        message: sms_msg,
                                                                    },
                                                                    ConversationMessageState::Listening {
                                                                        conn,
                                                                        stream,
                                                                        thread_id,
                                                                        device_id,
                                                                        messages_per_page,
                                                                        local_store_done,
                                                                        total_message_count,
                                                                        phone_deadline,
                                                                        load_complete_emitted,
                                                                        received_message_count,
                                                                        retry_attempted,
                                                                    },
                                                                ));
                                                            }
                                                        }
                                                    }
                                                }

                                                // conversationLoaded: the phone answered and the daemon
                                                // now holds `message_count` for this thread. On a fetch
                                                // the worker's page follows within a millisecond.
                                                if iface_str == "org.kde.kdeconnect.device.conversations"
                                                    && member_str == "conversationLoaded"
                                                {
                                                    let body = msg.body();
                                                    if let Ok((conv_id, message_count)) =
                                                        body.deserialize::<(i64, u64)>()
                                                    {
                                                        if conv_id == thread_id {
                                                            let was_already_done =
                                                                local_store_done;
                                                            local_store_done = true;
                                                            total_message_count =
                                                                Some(message_count);
                                                            if retry_attempted {
                                                                // Retry's own conversationLoaded.
                                                                // Don't extend phone_deadline — its
                                                                // CONVERSATION_RETRY_WAIT_MS deadline
                                                                // is already running.
                                                                tracing::info!(
                                                                    "Subscription: retry conversationLoaded for thread {}, \
                                                                     daemon holds {}",
                                                                    thread_id,
                                                                    message_count
                                                                );
                                                            } else if was_already_done
                                                                && (message_count as usize)
                                                                    > received_message_count
                                                                && received_message_count
                                                                    < messages_per_page as usize
                                                            {
                                                                // Second `conversationLoaded` before the
                                                                // load completed, reporting more than we
                                                                // have received while the page is still
                                                                // unfilled. The measured daemon has no
                                                                // producer for this on a single open
                                                                // (each batch emits once and its page
                                                                // follows it); the remaining case is a
                                                                // list-storm duplicate landing in the
                                                                // millisecond between the emit and the
                                                                // page. Kept as a harmless one-shot
                                                                // re-read (offset pinned to 0); the
                                                                // page-size guard keeps it off natural
                                                                // pagination boundaries
                                                                tracing::info!(
                                                                    "Subscription: thread {} duplicate conversationLoaded \
                                                                     (cached={}, received={}), retrying Conversations \
                                                                     interface read (second conversationLoaded)",
                                                                    thread_id,
                                                                    message_count,
                                                                    received_message_count
                                                                );
                                                                let end = (received_message_count
                                                                    + messages_per_page as usize)
                                                                    as i32;
                                                                if let Err(e) = fire_retry_request(
                                                                    &conn, &device_id, thread_id,
                                                                    end,
                                                                )
                                                                .await
                                                                {
                                                                    tracing::warn!(
                                                                        "Retry request_conversation failed for thread {}: {}",
                                                                        thread_id,
                                                                        e
                                                                    );
                                                                }
                                                                retry_attempted = true;
                                                                phone_deadline = Some(
                                                                    tokio::time::Instant::now()
                                                                        + std::time::Duration::from_millis(
                                                                            CONVERSATION_RETRY_WAIT_MS,
                                                                        ),
                                                                );
                                                            } else {
                                                                tracing::debug!(
                                                                    "Subscription: conversationLoaded for thread {}, \
                                                                     daemon holds {}; page expected to follow, window {:?}",
                                                                    thread_id,
                                                                    message_count,
                                                                    phone_wait
                                                                );
                                                                phone_deadline = Some(
                                                                    tokio::time::Instant::now()
                                                                        + phone_wait,
                                                                );
                                                            }
                                                            return Some((
                                                                Message::ConversationLoaded {
                                                                    thread_id,
                                                                    count: message_count,
                                                                },
                                                                ConversationMessageState::Listening {
                                                                    conn,
                                                                    stream,
                                                                    thread_id,
                                                                    device_id,
                                                                    messages_per_page,
                                                                    local_store_done,
                                                                    total_message_count,
                                                                    phone_deadline,
                                                                    load_complete_emitted,
                                                                    received_message_count,
                                                                    retry_attempted,
                                                                },
                                                            ));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Some(Err(e)) => {
                                        tracing::warn!("D-Bus conversation stream error: {}", e);
                                    }
                                    None => {
                                        // Stream ended (D-Bus connection dropped)
                                        tracing::warn!(
                                            "D-Bus message stream ended for thread {}",
                                            thread_id
                                        );
                                        if !load_complete_emitted {
                                            return Some((
                                                Message::ConversationLoadComplete {
                                                    thread_id,
                                                    total_count: total_message_count.unwrap_or(0),
                                                },
                                                ConversationMessageState::Done,
                                            ));
                                        }
                                        return None;
                                    }
                                }
                            }

                            // Sleep until next deadline or heartbeat
                            _ = tokio::time::sleep(wait_duration) => {
                                // Loop back — deadline checks at top will handle expiry
                            }
                        }
                    }
                }
                ConversationMessageState::Done => {
                    // Terminal state - subscription is complete
                    None
                }
            }
        },
    )
}
