//! Subscription for incremental conversation list loading via D-Bus signals.
//!
//! This module provides a subscription that listens for conversationCreated and
//! conversationUpdated signals to provide real-time UI updates as conversations
//! are received from the phone.

use crate::app::Message;
use crate::constants::dbus::RETRY_DELAY_SECS;
use crate::constants::sms::{SIGNAL_ACTIVITY_TIMEOUT_MS, TIMEOUT_CHECK_INTERVAL_MS};
use futures_util::StreamExt;
use kdeconnect_dbus::plugins::{parse_sms_message, ConversationSummary, ConversationsProxy};
use zbus::Connection;

/// Overall timeout for conversation list sync (seconds).
const CONVERSATION_LIST_TIMEOUT_SECS: u64 = 20;

/// State for conversation list subscription.
#[allow(clippy::large_enum_variant)]
enum ConversationListState {
    Init {
        device_id: String,
    },
    /// Emitting cached conversations one at a time before listening for signals
    EmittingCached {
        conn: Connection,
        stream: zbus::MessageStream,
        device_id: String,
        pending_conversations: Vec<ConversationSummary>,
        start_time: tokio::time::Instant,
    },
    Listening {
        #[allow(dead_code)]
        conn: Connection,
        stream: zbus::MessageStream,
        device_id: String,
        start_time: tokio::time::Instant,
        last_activity: tokio::time::Instant,
        received_any_data: bool,
    },
}

/// Create a stream that listens for conversation list updates via D-Bus signals.
///
/// This subscription handles incremental conversation loading by:
/// 1. Setting up D-Bus match rules for signals
/// 2. Getting initial cached conversations via activeConversations()
/// 3. Firing requestAllConversationThreads() to trigger phone sync
/// 4. Listening for `conversationCreated`/`conversationUpdated` signals
/// 5. Emitting `Message::ConversationReceived` for each conversation (immediate UI update)
/// 6. Emitting `Message::ConversationSyncComplete` when activity stops or timeout
pub fn conversation_list_subscription(
    device_id: String,
) -> impl futures_util::Stream<Item = Message> {
    futures_util::stream::unfold(
        ConversationListState::Init { device_id },
        |state| async move {
            match state {
                ConversationListState::Init { device_id } => {
                    // Connect to D-Bus
                    let conn = match Connection::session().await {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!(
                                "Failed to connect to D-Bus for conversation list: {}",
                                e
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS))
                                .await;
                            return Some((
                                Message::SmsError(format!("D-Bus connection failed: {}", e)),
                                ConversationListState::Init { device_id },
                            ));
                        }
                    };

                    // Add match rules for conversation signals
                    let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!("Failed to create DBus proxy: {}", e);
                            return Some((
                                Message::SmsError(format!("D-Bus proxy failed: {}", e)),
                                ConversationListState::Init { device_id },
                            ));
                        }
                    };

                    // Subscribe to conversationCreated signals
                    let created_rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .interface("org.kde.kdeconnect.device.conversations")
                        .and_then(|b| b.member("conversationCreated"))
                        .map(|b| b.build());

                    if let Ok(rule) = created_rule {
                        if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                            tracing::warn!("Failed to add conversationCreated match rule: {}", e);
                        } else {
                            tracing::debug!("Added match rule for conversationCreated signals");
                        }
                    }

                    // Subscribe to conversationUpdated signals
                    let updated_rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .interface("org.kde.kdeconnect.device.conversations")
                        .and_then(|b| b.member("conversationUpdated"))
                        .map(|b| b.build());

                    if let Ok(rule) = updated_rule {
                        if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                            tracing::warn!("Failed to add conversationUpdated match rule: {}", e);
                        } else {
                            tracing::debug!("Added match rule for conversationUpdated signals");
                        }
                    }

                    // Subscribe to conversationLoaded signals
                    let loaded_rule = zbus::MatchRule::builder()
                        .msg_type(zbus::message::Type::Signal)
                        .interface("org.kde.kdeconnect.device.conversations")
                        .and_then(|b| b.member("conversationLoaded"))
                        .map(|b| b.build());

                    if let Ok(rule) = loaded_rule {
                        if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                            tracing::warn!("Failed to add conversationLoaded match rule: {}", e);
                        } else {
                            tracing::debug!("Added match rule for conversationLoaded signals");
                        }
                    }

                    // Create message stream BEFORE firing request
                    let stream = zbus::MessageStream::from(&conn);

                    // Build conversations proxy for the device
                    let device_path = format!(
                        "{}/devices/{}",
                        kdeconnect_dbus::BASE_PATH,
                        device_id
                    );

                    let conversations_proxy = match ConversationsProxy::builder(&conn)
                        .path(device_path.as_str())
                        .ok()
                        .map(|b| b.build())
                    {
                        Some(fut) => match fut.await {
                            Ok(p) => Some(p),
                            Err(e) => {
                                tracing::warn!("Failed to create conversations proxy: {}", e);
                                None
                            }
                        },
                        None => None,
                    };

                    // Get cached conversations first (for immediate display)
                    let mut initial_conversations: Vec<ConversationSummary> = Vec::new();
                    if let Some(ref proxy) = conversations_proxy {
                        if let Ok(cached) = proxy.active_conversations().await {
                            tracing::info!("Got {} cached conversation values", cached.len());
                            for value in &cached {
                                if let Some(sms_msg) = parse_sms_message(value) {
                                    initial_conversations.push(ConversationSummary {
                                        thread_id: sms_msg.thread_id,
                                        addresses: sms_msg.addresses,
                                        last_message: sms_msg.body,
                                        timestamp: sms_msg.date,
                                        unread: !sms_msg.read,
                                    });
                                }
                            }
                            // Sort by timestamp (newest first) and deduplicate
                            initial_conversations.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                            let mut seen = std::collections::HashSet::new();
                            initial_conversations.retain(|c| seen.insert(c.thread_id));
                            tracing::info!("Parsed {} unique cached conversations", initial_conversations.len());
                        }
                    }

                    // Fire request for conversation threads
                    if let Some(ref proxy) = conversations_proxy {
                        tracing::info!("Firing requestAllConversationThreads for device {}", device_id);
                        if let Err(e) = proxy.request_all_conversation_threads().await {
                            tracing::warn!("Failed to request conversation threads: {}", e);
                        }
                    }

                    let now = tokio::time::Instant::now();

                    // If we have cached data, transition to EmittingCached state
                    if !initial_conversations.is_empty() {
                        tracing::info!(
                            "Emitting {} cached conversations for device {}",
                            initial_conversations.len(),
                            device_id
                        );

                        // Emit the first one and store the rest
                        let first = initial_conversations.remove(0);
                        return Some((
                            Message::ConversationReceived {
                                device_id: device_id.clone(),
                                conversation: first,
                            },
                            ConversationListState::EmittingCached {
                                conn,
                                stream,
                                device_id,
                                pending_conversations: initial_conversations,
                                start_time: now,
                            },
                        ));
                    }

                    // No cached data - emit sync started and go to listening
                    Some((
                        Message::ConversationSyncStarted { device_id: device_id.clone() },
                        ConversationListState::Listening {
                            conn,
                            stream,
                            device_id,
                            start_time: now,
                            last_activity: now,
                            received_any_data: false,
                        },
                    ))
                }
                ConversationListState::EmittingCached {
                    conn,
                    stream,
                    device_id,
                    mut pending_conversations,
                    start_time,
                } => {
                    // Emit cached conversations one at a time
                    if !pending_conversations.is_empty() {
                        let conversation = pending_conversations.remove(0);
                        tracing::debug!(
                            "Emitting cached conversation: thread {} ({} remaining)",
                            conversation.thread_id,
                            pending_conversations.len()
                        );
                        return Some((
                            Message::ConversationReceived {
                                device_id: device_id.clone(),
                                conversation,
                            },
                            ConversationListState::EmittingCached {
                                conn,
                                stream,
                                device_id,
                                pending_conversations,
                                start_time,
                            },
                        ));
                    }

                    // All cached conversations emitted, transition to listening for signals
                    tracing::debug!(
                        "Finished emitting cached conversations, now listening for signals for device {}",
                        device_id
                    );
                    let now = tokio::time::Instant::now();
                    Some((
                        Message::ConversationSyncStarted { device_id: device_id.clone() },
                        ConversationListState::Listening {
                            conn,
                            stream,
                            device_id,
                            start_time,
                            last_activity: now,
                            received_any_data: true, // We received cached data
                        },
                    ))
                }
                ConversationListState::Listening {
                    conn,
                    mut stream,
                    device_id,
                    start_time,
                    mut last_activity,
                    mut received_any_data,
                } => {
                    let overall_timeout =
                        tokio::time::Duration::from_secs(CONVERSATION_LIST_TIMEOUT_SECS);
                    let activity_timeout =
                        tokio::time::Duration::from_millis(SIGNAL_ACTIVITY_TIMEOUT_MS);
                    let check_interval =
                        tokio::time::Duration::from_millis(TIMEOUT_CHECK_INTERVAL_MS);

                    loop {
                        tokio::select! {
                            biased;

                            // Wait for D-Bus signals
                            Some(msg_result) = stream.next() => {
                                match msg_result {
                                    Ok(msg) => {
                                        if msg.header().message_type() == zbus::message::Type::Signal {
                                            if let (Some(interface), Some(member)) =
                                                (msg.header().interface(), msg.header().member())
                                            {
                                                let iface_str = interface.as_str();
                                                let member_str = member.as_str();

                                                // Check if this signal is for our device
                                                let is_our_device = msg.header().path()
                                                    .map(|p| p.as_str().contains(&device_id))
                                                    .unwrap_or(false);

                                                if !is_our_device {
                                                    continue;
                                                }

                                                // Handle conversationCreated signals
                                                if iface_str == "org.kde.kdeconnect.device.conversations"
                                                    && member_str == "conversationCreated"
                                                {
                                                    last_activity = tokio::time::Instant::now();
                                                    received_any_data = true;
                                                    let body = msg.body();
                                                    if let Ok(value) = body.deserialize::<zbus::zvariant::OwnedValue>() {
                                                        if let Some(sms_msg) = parse_sms_message(&value) {
                                                            let conversation = ConversationSummary {
                                                                thread_id: sms_msg.thread_id,
                                                                addresses: sms_msg.addresses,
                                                                last_message: sms_msg.body,
                                                                timestamp: sms_msg.date,
                                                                unread: !sms_msg.read,
                                                            };
                                                            tracing::debug!(
                                                                "conversationCreated: thread {} for device {}",
                                                                conversation.thread_id,
                                                                device_id
                                                            );
                                                            return Some((
                                                                Message::ConversationReceived {
                                                                    device_id: device_id.clone(),
                                                                    conversation,
                                                                },
                                                                ConversationListState::Listening {
                                                                    conn,
                                                                    stream,
                                                                    device_id,
                                                                    start_time,
                                                                    last_activity,
                                                                    received_any_data,
                                                                },
                                                            ));
                                                        }
                                                    }
                                                }

                                                // Handle conversationUpdated signals
                                                if iface_str == "org.kde.kdeconnect.device.conversations"
                                                    && member_str == "conversationUpdated"
                                                {
                                                    last_activity = tokio::time::Instant::now();
                                                    received_any_data = true;
                                                    let body = msg.body();
                                                    if let Ok(value) = body.deserialize::<zbus::zvariant::OwnedValue>() {
                                                        if let Some(sms_msg) = parse_sms_message(&value) {
                                                            let conversation = ConversationSummary {
                                                                thread_id: sms_msg.thread_id,
                                                                addresses: sms_msg.addresses,
                                                                last_message: sms_msg.body,
                                                                timestamp: sms_msg.date,
                                                                unread: !sms_msg.read,
                                                            };
                                                            tracing::debug!(
                                                                "conversationUpdated: thread {} for device {}",
                                                                conversation.thread_id,
                                                                device_id
                                                            );
                                                            return Some((
                                                                Message::ConversationReceived {
                                                                    device_id: device_id.clone(),
                                                                    conversation,
                                                                },
                                                                ConversationListState::Listening {
                                                                    conn,
                                                                    stream,
                                                                    device_id,
                                                                    start_time,
                                                                    last_activity,
                                                                    received_any_data,
                                                                },
                                                            ));
                                                        }
                                                    }
                                                }

                                                // Handle conversationLoaded signals (progress marker)
                                                if iface_str == "org.kde.kdeconnect.device.conversations"
                                                    && member_str == "conversationLoaded"
                                                {
                                                    last_activity = tokio::time::Instant::now();
                                                    received_any_data = true;
                                                    tracing::debug!(
                                                        "conversationLoaded signal for device {}",
                                                        device_id
                                                    );
                                                    // Continue listening - more signals may come
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("D-Bus stream error: {}", e);
                                    }
                                }
                            }

                            // Periodic timeout check
                            _ = tokio::time::sleep(check_interval) => {
                                let elapsed = start_time.elapsed();
                                let since_activity = last_activity.elapsed();

                                // Hard timeout
                                if elapsed >= overall_timeout {
                                    tracing::info!(
                                        "Conversation list sync timeout after {:?} for device {}",
                                        elapsed,
                                        device_id
                                    );
                                    return Some((
                                        Message::ConversationSyncComplete { device_id },
                                        ConversationListState::Init { device_id: String::new() }, // Done
                                    ));
                                }

                                // Activity timeout - only if we received some data
                                if received_any_data && since_activity >= activity_timeout {
                                    tracing::info!(
                                        "Conversation list sync complete (activity timeout) for device {}, received data: {}",
                                        device_id,
                                        received_any_data
                                    );
                                    return Some((
                                        Message::ConversationSyncComplete { device_id },
                                        ConversationListState::Init { device_id: String::new() }, // Done
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}
