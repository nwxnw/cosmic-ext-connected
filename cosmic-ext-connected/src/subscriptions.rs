//! D-Bus signal subscriptions for real-time updates from KDE Connect.

use crate::app::Message;
use crate::constants::dbus::RETRY_DELAY_SECS;
use crate::constants::sms::MAX_MESSAGE_LOAD_RETRIES;
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

/// State for the per-thread message subscription.
#[allow(clippy::large_enum_variant)]
enum ConversationMessageState {
    Init {
        thread_id: i64,
        device_id: String,
        messages_per_page: u32,
    },
    Listening {
        #[allow(dead_code)]
        conn: Connection,
        stream: zbus::MessageStream,
        thread_id: i64,
    },
    /// Terminal state after a connect or request failure
    Done,
}

/// Signal pump for one thread of the open conversation: install the match rules,
/// fire the first page's `requestConversation`, then forward every
/// `conversationUpdated` and `conversationLoaded` for the thread until iced drops
/// the subscription (`conversation_load_active` false, or a generation bump).
/// It decides nothing; the store settles the page (`FirstPage`). It reports only
/// a failure to connect or to fire the request.
///
/// The request is fired here, after the match rules and the stream exist, so no
/// signal it causes can be missed.
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

                    Some((
                        Message::ConversationLoadStarted { thread_id },
                        ConversationMessageState::Listening {
                            conn,
                            stream,
                            thread_id,
                        },
                    ))
                }
                ConversationMessageState::Listening {
                    conn,
                    mut stream,
                    thread_id,
                } => {
                    while let Some(item) = stream.next().await {
                        let msg = match item {
                            Ok(msg) => msg,
                            Err(e) => {
                                tracing::warn!("D-Bus conversation stream error: {}", e);
                                continue;
                            }
                        };
                        let header = msg.header();
                        if header.message_type() != zbus::message::Type::Signal
                            || header.interface().map(|i| i.as_str())
                                != Some("org.kde.kdeconnect.device.conversations")
                        {
                            continue;
                        }
                        let forward = match header.member().map(|m| m.as_str()) {
                            Some("conversationUpdated") => msg
                                .body()
                                .deserialize::<zbus::zvariant::OwnedValue>()
                                .ok()
                                .and_then(|value| parse_sms_message(&value))
                                .filter(|m| m.thread_id == thread_id)
                                .map(|message| Message::ConversationMessageReceived {
                                    thread_id,
                                    message,
                                }),
                            Some("conversationLoaded") => msg
                                .body()
                                .deserialize::<(i64, u64)>()
                                .ok()
                                .filter(|(t, _)| *t == thread_id)
                                .map(|(_, count)| Message::ConversationLoaded { thread_id, count }),
                            _ => None,
                        };
                        if let Some(message) = forward {
                            return Some((
                                message,
                                ConversationMessageState::Listening {
                                    conn,
                                    stream,
                                    thread_id,
                                },
                            ));
                        }
                    }
                    tracing::warn!("D-Bus message stream ended for thread {}", thread_id);
                    None
                }
                ConversationMessageState::Done => None,
            }
        },
    )
}
