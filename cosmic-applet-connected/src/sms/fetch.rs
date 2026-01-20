//! SMS conversation and message fetching from KDE Connect.
//!
//! This module provides two approaches for loading conversations:
//!
//! 1. **Fast cached loading** (`fetch_cached_conversations_async`): Returns immediately
//!    with whatever the daemon has cached. Used for instant UI display.
//!
//! 2. **Full signal-based sync** (`fetch_conversations_async`): Subscribes to D-Bus signals
//!    and waits for the phone to send fresh data. Used for background sync.
//!
//! The recommended pattern is to use both: show cached data immediately, then
//! refresh in the background for a responsive user experience.

use crate::app::Message;
use crate::constants::sms::{
    CONVERSATION_TIMEOUT_CACHED_SECS, CONVERSATION_TIMEOUT_INITIAL_SECS,
    FALLBACK_POLLING_DELAYS_MS, FALLBACK_POLLING_INTERVAL_MS, MESSAGE_FETCH_TIMEOUT_SECS,
    SIGNAL_ACTIVITY_TIMEOUT_MS, SIGNAL_DRAIN_TIMEOUT_MS, TIMEOUT_CHECK_INTERVAL_MS,
};
use futures_util::StreamExt;
use kdeconnect_dbus::plugins::{
    parse_conversations, parse_messages, parse_sms_message, ConversationSummary,
    ConversationsProxy, SmsMessage, MAX_CONVERSATIONS,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Fetch cached SMS conversations immediately (fast initial display).
///
/// This function returns whatever the daemon has cached without waiting for
/// the phone to sync. It's designed for instant UI display. Use
/// `fetch_conversations_async` afterwards for a full background sync.
pub async fn fetch_cached_conversations_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
) -> Message {
    let conn = conn.lock().await;

    // The conversations interface is on the device path
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    // Build conversations proxy on the device path
    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to create conversations proxy for cache: {}", e);
                return Message::ConversationsCached(Vec::new());
            }
        },
        None => {
            return Message::ConversationsCached(Vec::new());
        }
    };

    // Get cached conversations immediately (don't request from phone)
    match conversations_proxy.active_conversations().await {
        Ok(values) => {
            let conversations = parse_conversations(values);
            tracing::info!(
                "Loaded {} cached conversations for immediate display",
                conversations.len()
            );
            Message::ConversationsCached(conversations)
        }
        Err(e) => {
            tracing::warn!("Failed to get cached conversations: {}", e);
            Message::ConversationsCached(Vec::new())
        }
    }
}

/// Fetch SMS conversations for a device using signal-based loading.
pub async fn fetch_conversations_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    // The conversations interface is on the device path
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    // Build conversations proxy on the device path
    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::SmsError(format!("Failed to create conversations proxy: {}", e));
            }
        },
        None => {
            return Message::SmsError("Failed to build conversations proxy path".to_string());
        }
    };

    // Try signal-based loading first
    match fetch_conversations_via_signals(&conversations_proxy).await {
        Ok(conversations) => {
            tracing::info!(
                "Signal-based loading succeeded with {} conversations",
                conversations.len()
            );
            Message::ConversationsLoaded(conversations)
        }
        Err(e) => {
            tracing::warn!(
                "Signal-based conversation loading failed: {}, using fallback",
                e
            );
            fetch_conversations_fallback(&conversations_proxy).await
        }
    }
}

/// Fetch conversations using D-Bus signals for reliable loading.
async fn fetch_conversations_via_signals(
    conversations_proxy: &ConversationsProxy<'_>,
) -> Result<Vec<ConversationSummary>, String> {
    // Subscribe to signals BEFORE requesting data
    let mut created_stream = conversations_proxy
        .receive_conversation_created()
        .await
        .map_err(|e| format!("Failed to subscribe to conversationCreated: {}", e))?;

    let mut updated_stream = conversations_proxy
        .receive_conversation_updated()
        .await
        .map_err(|e| format!("Failed to subscribe to conversationUpdated: {}", e))?;

    let mut loaded_stream = conversations_proxy
        .receive_conversation_loaded()
        .await
        .map_err(|e| format!("Failed to subscribe to conversationLoaded: {}", e))?;

    // Get cached conversations first
    let cached = conversations_proxy.active_conversations().await.ok();
    let mut conversations_map: HashMap<i64, ConversationSummary> = HashMap::new();

    if let Some(values) = cached {
        tracing::info!("Loaded {} cached conversation values", values.len());
        for summary in parse_conversations(values) {
            conversations_map.insert(summary.thread_id, summary);
        }
        tracing::info!("Parsed {} cached conversations", conversations_map.len());
    }

    // Request fresh data from the phone
    if let Err(e) = conversations_proxy.request_all_conversation_threads().await {
        tracing::warn!("Failed to request conversation threads: {}", e);
        // If we have cached data, return it; otherwise propagate error
        if !conversations_map.is_empty() {
            let mut result: Vec<ConversationSummary> = conversations_map.into_values().collect();
            result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            result.truncate(MAX_CONVERSATIONS);
            return Ok(result);
        }
        return Err(format!("Failed to request conversations: {}", e));
    }

    // Activity-based timeout tracking
    // Use shorter timeout when we have cached data (just need incremental updates)
    // Use longer timeout when no cache (need to wait for phone to send data)
    let has_cache = !conversations_map.is_empty();
    let overall_timeout = if has_cache {
        tokio::time::Duration::from_secs(CONVERSATION_TIMEOUT_CACHED_SECS)
    } else {
        tokio::time::Duration::from_secs(CONVERSATION_TIMEOUT_INITIAL_SECS)
    };
    let activity_timeout = tokio::time::Duration::from_millis(SIGNAL_ACTIVITY_TIMEOUT_MS);
    let start_time = tokio::time::Instant::now();
    let mut last_activity = tokio::time::Instant::now();
    let mut loaded_signal_received = false;

    tracing::info!(
        "Starting signal collection: has_cache={}, timeout={}s",
        has_cache,
        overall_timeout.as_secs()
    );

    loop {
        tokio::select! {
            biased;

            // Check for conversationCreated signals (new conversations)
            Some(signal) = created_stream.next() => {
                last_activity = tokio::time::Instant::now();
                match signal.args() {
                    Ok(args) => {
                        if let Some(msg) = parse_sms_message(&args.msg) {
                            tracing::debug!("conversationCreated: thread {}", msg.thread_id);
                            // Only update if newer or not present
                            let should_update = conversations_map
                                .get(&msg.thread_id)
                                .map(|existing| msg.date > existing.timestamp)
                                .unwrap_or(true);
                            if should_update {
                                conversations_map.insert(msg.thread_id, ConversationSummary {
                                    thread_id: msg.thread_id,
                                    addresses: msg.addresses,
                                    last_message: msg.body,
                                    timestamp: msg.date,
                                    unread: !msg.read,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationCreated signal: {}", e);
                    }
                }
            }

            // Check for conversationUpdated signals (updated conversations)
            Some(signal) = updated_stream.next() => {
                last_activity = tokio::time::Instant::now();
                match signal.args() {
                    Ok(args) => {
                        if let Some(msg) = parse_sms_message(&args.msg) {
                            tracing::debug!("conversationUpdated: thread {}", msg.thread_id);
                            // Only update if newer or not present
                            let should_update = conversations_map
                                .get(&msg.thread_id)
                                .map(|existing| msg.date > existing.timestamp)
                                .unwrap_or(true);
                            if should_update {
                                conversations_map.insert(msg.thread_id, ConversationSummary {
                                    thread_id: msg.thread_id,
                                    addresses: msg.addresses,
                                    last_message: msg.body,
                                    timestamp: msg.date,
                                    unread: !msg.read,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationUpdated signal: {}", e);
                    }
                }
            }

            // Check for conversationLoaded signals (indicates activity)
            Some(_signal) = loaded_stream.next() => {
                last_activity = tokio::time::Instant::now();
                loaded_signal_received = true;
                tracing::debug!("conversationLoaded signal received");
            }

            // Check timeouts periodically
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(TIMEOUT_CHECK_INTERVAL_MS)) => {
                let elapsed = start_time.elapsed();
                let since_activity = last_activity.elapsed();

                // Overall timeout - hard limit
                if elapsed >= overall_timeout {
                    tracing::warn!(
                        "Overall timeout reached after {:?}, collected {} conversations",
                        elapsed,
                        conversations_map.len()
                    );
                    break;
                }

                // Activity timeout - stop if no signals for 500ms (but only after receiving data)
                if loaded_signal_received && since_activity >= activity_timeout {
                    tracing::info!(
                        "Activity timeout - no signals for {:?}, collected {} conversations",
                        since_activity,
                        conversations_map.len()
                    );
                    break;
                }
            }
        }
    }

    // Drain any remaining buffered signals
    'drain: loop {
        tokio::select! {
            biased;
            Some(signal) = created_stream.next() => {
                if let Ok(args) = signal.args() {
                    if let Some(msg) = parse_sms_message(&args.msg) {
                        let should_update = conversations_map
                            .get(&msg.thread_id)
                            .map(|existing| msg.date > existing.timestamp)
                            .unwrap_or(true);
                        if should_update {
                            conversations_map.insert(msg.thread_id, ConversationSummary {
                                thread_id: msg.thread_id,
                                addresses: msg.addresses,
                                last_message: msg.body,
                                timestamp: msg.date,
                                unread: !msg.read,
                            });
                        }
                    }
                }
            }
            Some(signal) = updated_stream.next() => {
                if let Ok(args) = signal.args() {
                    if let Some(msg) = parse_sms_message(&args.msg) {
                        let should_update = conversations_map
                            .get(&msg.thread_id)
                            .map(|existing| msg.date > existing.timestamp)
                            .unwrap_or(true);
                        if should_update {
                            conversations_map.insert(msg.thread_id, ConversationSummary {
                                thread_id: msg.thread_id,
                                addresses: msg.addresses,
                                last_message: msg.body,
                                timestamp: msg.date,
                                unread: !msg.read,
                            });
                        }
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(SIGNAL_DRAIN_TIMEOUT_MS)) => {
                break 'drain;
            }
        }
    }

    // Sort by timestamp descending (most recent first)
    let mut result: Vec<ConversationSummary> = conversations_map.into_values().collect();
    result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    result.truncate(MAX_CONVERSATIONS);

    tracing::info!("Final: {} conversations loaded via signals", result.len());
    Ok(result)
}

/// Fallback conversation fetching using polling when signal subscription fails.
async fn fetch_conversations_fallback(conversations_proxy: &ConversationsProxy<'_>) -> Message {
    // Request the phone to send data
    if let Err(e) = conversations_proxy.request_all_conversation_threads().await {
        tracing::warn!("Fallback: Failed to request conversation threads: {}", e);
    }

    // Poll with increasing delays, collecting all available conversations
    // Note: We don't stop early anymore - the phone may be slow to sync all conversations
    let mut best_result: Vec<ConversationSummary> = Vec::new();

    for (attempt, delay) in FALLBACK_POLLING_DELAYS_MS.iter().enumerate() {
        tokio::time::sleep(std::time::Duration::from_millis(*delay)).await;

        match conversations_proxy.active_conversations().await {
            Ok(values) => {
                let conversations = parse_conversations(values);
                tracing::info!(
                    "Fallback attempt {}: Found {} conversations",
                    attempt + 1,
                    conversations.len()
                );

                // Keep the best result
                if conversations.len() > best_result.len() {
                    best_result = conversations;
                }
            }
            Err(e) => {
                tracing::warn!("Fallback attempt {} failed: {}", attempt + 1, e);
            }
        }
    }

    tracing::info!(
        "Fallback complete: {} conversations loaded",
        best_result.len()
    );
    Message::ConversationsLoaded(best_result)
}

/// Fetch messages for a specific conversation thread using D-Bus signals.
pub async fn fetch_messages_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    messages_per_page: u32,
) -> Message {
    let conn = conn.lock().await;

    // The conversations interface is on the device path
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    // Build conversations proxy on the device path
    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::SmsError(format!("Failed to create conversations proxy: {}", e));
            }
        },
        None => {
            return Message::SmsError("Failed to build conversations proxy path".to_string());
        }
    };

    // Set up signal stream for conversationUpdated BEFORE requesting
    let mut updated_stream = match conversations_proxy.receive_conversation_updated().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!("Failed to subscribe to conversationUpdated: {}", e);
            // Fallback to simple polling
            return fetch_messages_fallback(&conversations_proxy, thread_id, messages_per_page)
                .await;
        }
    };

    // Set up signal stream for conversationLoaded
    let mut loaded_stream = match conversations_proxy.receive_conversation_loaded().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!("Failed to subscribe to conversationLoaded: {}", e);
            return fetch_messages_fallback(&conversations_proxy, thread_id, messages_per_page)
                .await;
        }
    };

    // Request the specific conversation
    tracing::debug!(
        "Requesting conversation {} (messages 0-{})",
        thread_id,
        messages_per_page
    );
    if let Err(e) = conversations_proxy
        .request_conversation(thread_id, 0, messages_per_page as i32)
        .await
    {
        tracing::warn!("Failed to request conversation: {}", e);
        return Message::SmsError(format!("Failed to request conversation: {}", e));
    }

    // Collect messages from signals until conversationLoaded, activity timeout, or hard timeout
    // Use uid (unique message ID) as key for reliable deduplication
    let mut messages_map: HashMap<i32, SmsMessage> = HashMap::new();
    let mut total_message_count: Option<u64> = None;
    let timeout = tokio::time::Duration::from_secs(MESSAGE_FETCH_TIMEOUT_SECS);
    let activity_timeout = tokio::time::Duration::from_millis(SIGNAL_ACTIVITY_TIMEOUT_MS);
    let check_interval = tokio::time::Duration::from_millis(TIMEOUT_CHECK_INTERVAL_MS);
    let start_time = tokio::time::Instant::now();
    let mut last_activity = tokio::time::Instant::now();
    let mut loaded_signal_received = false;

    loop {
        tokio::select! {
            biased;
            // Check for conversationUpdated signals (highest priority)
            Some(signal) = updated_stream.next() => {
                match signal.args() {
                    Ok(args) => {
                        if let Some(msg) = parse_sms_message(&args.msg) {
                            if msg.thread_id == thread_id {
                                // Use uid as key for reliable deduplication
                                messages_map.insert(msg.uid, msg);
                                last_activity = tokio::time::Instant::now();
                                tracing::debug!(
                                    "Received message for thread {}, total: {}",
                                    thread_id,
                                    messages_map.len()
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationUpdated signal: {}", e);
                    }
                }
            }
            // Check for conversationLoaded signal
            Some(signal) = loaded_stream.next() => {
                match signal.args() {
                    Ok(args) => {
                        if args.conversation_id == thread_id {
                            // Capture the total message count for pagination
                            total_message_count = Some(args.message_count);
                            loaded_signal_received = true;
                            last_activity = tokio::time::Instant::now();
                            tracing::info!(
                                "Conversation {} loaded signal received, total {} messages, got {}",
                                thread_id,
                                args.message_count,
                                messages_map.len()
                            );
                            // Don't break immediately - continue to drain buffered signals
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationLoaded signal: {}", e);
                    }
                }
            }
            // Periodic timeout check
            _ = tokio::time::sleep(check_interval) => {
                let elapsed = start_time.elapsed();
                let since_activity = last_activity.elapsed();

                // Hard timeout
                if elapsed >= timeout {
                    tracing::warn!(
                        "Hard timeout ({:?}) waiting for messages, got {} messages",
                        timeout,
                        messages_map.len()
                    );
                    break;
                }

                // Activity timeout: if we have messages and no activity for a while, we're done
                // This prevents waiting the full 10s when signals stop coming
                if !messages_map.is_empty() && since_activity >= activity_timeout {
                    tracing::info!(
                        "Activity timeout ({:?} since last signal), got {} messages",
                        since_activity,
                        messages_map.len()
                    );
                    break;
                }

                // If we received conversationLoaded and drained signals, we're done
                if loaded_signal_received && since_activity >= tokio::time::Duration::from_millis(SIGNAL_DRAIN_TIMEOUT_MS) {
                    tracing::info!(
                        "Loaded signal received and signals drained, got {} messages",
                        messages_map.len()
                    );
                    break;
                }
            }
        }
    }

    // Convert map to sorted vector
    let mut messages: Vec<SmsMessage> = messages_map.into_values().collect();
    messages.sort_by(|a, b| a.date.cmp(&b.date));

    tracing::info!(
        "Final: Loaded {} messages for thread {} (total in conversation: {:?})",
        messages.len(),
        thread_id,
        total_message_count
    );
    Message::MessagesLoaded(thread_id, messages, total_message_count)
}

/// Fallback message fetching using simple polling when signal subscription fails.
async fn fetch_messages_fallback(
    conversations_proxy: &ConversationsProxy<'_>,
    thread_id: i64,
    messages_per_page: u32,
) -> Message {
    // Request the conversation
    if let Err(e) = conversations_proxy
        .request_conversation(thread_id, 0, messages_per_page as i32)
        .await
    {
        tracing::warn!("Failed to request conversation: {}", e);
    }

    // Simple polling fallback - poll all attempts to give phone time to sync
    // Note: We don't stop early anymore - the phone may be slow to sync messages
    let mut best_messages = Vec::new();
    for attempt in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(
            FALLBACK_POLLING_INTERVAL_MS,
        ))
        .await;

        match conversations_proxy.active_conversations().await {
            Ok(values) => {
                let messages = parse_messages(values, thread_id);
                tracing::info!(
                    "Fallback attempt {}: Found {} messages for thread {}",
                    attempt + 1,
                    messages.len(),
                    thread_id
                );
                // Keep the best result
                if messages.len() > best_messages.len() {
                    best_messages = messages;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get messages on attempt {}: {}", attempt + 1, e);
            }
        }
    }

    tracing::info!(
        "Fallback complete: {} messages for thread {}",
        best_messages.len(),
        thread_id
    );
    // Fallback doesn't receive conversationLoaded signal, so total_count is unknown
    Message::MessagesLoaded(thread_id, best_messages, None)
}

/// Fetch older messages for pagination (starting from a given offset).
pub async fn fetch_older_messages_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    start_index: u32,
    count: u32,
) -> Message {
    let conn = conn.lock().await;

    // The conversations interface is on the device path
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    // Build conversations proxy on the device path
    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to create conversations proxy: {}", e);
                return Message::OlderMessagesLoaded(thread_id, Vec::new(), false, None);
            }
        },
        None => {
            return Message::OlderMessagesLoaded(thread_id, Vec::new(), false, None);
        }
    };

    // Set up signal stream for conversationUpdated BEFORE requesting
    let mut updated_stream = match conversations_proxy.receive_conversation_updated().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(
                "Failed to subscribe to conversationUpdated for older messages: {}",
                e
            );
            return Message::OlderMessagesLoaded(thread_id, Vec::new(), false, None);
        }
    };

    // Set up signal stream for conversationLoaded
    let mut loaded_stream = match conversations_proxy.receive_conversation_loaded().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(
                "Failed to subscribe to conversationLoaded for older messages: {}",
                e
            );
            return Message::OlderMessagesLoaded(thread_id, Vec::new(), false, None);
        }
    };

    // Request the specific conversation with pagination offset
    tracing::debug!(
        "Requesting older messages for thread {} (messages {}-{})",
        thread_id,
        start_index,
        start_index + count
    );
    if let Err(e) = conversations_proxy
        .request_conversation(thread_id, start_index as i32, count as i32)
        .await
    {
        tracing::warn!("Failed to request older messages: {}", e);
        return Message::OlderMessagesLoaded(thread_id, Vec::new(), false, None);
    }

    // Collect messages from signals until conversationLoaded or timeout
    // Use uid (unique message ID) as key for reliable deduplication
    let mut messages_map: HashMap<i32, SmsMessage> = HashMap::new();
    let mut total_message_count: Option<u64> = None;
    let timeout = tokio::time::Duration::from_secs(MESSAGE_FETCH_TIMEOUT_SECS);
    let start_time = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Check for conversationUpdated signals
            Some(signal) = updated_stream.next() => {
                match signal.args() {
                    Ok(args) => {
                        if let Some(msg) = parse_sms_message(&args.msg) {
                            if msg.thread_id == thread_id {
                                // Use uid as key for reliable deduplication
                                messages_map.insert(msg.uid, msg);
                                tracing::debug!(
                                    "Received older message for thread {}, total: {}",
                                    thread_id,
                                    messages_map.len()
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationUpdated signal: {}", e);
                    }
                }
            }
            // Check for conversationLoaded signal
            Some(signal) = loaded_stream.next() => {
                match signal.args() {
                    Ok(args) => {
                        if args.conversation_id == thread_id {
                            // Capture the total message count for pagination
                            total_message_count = Some(args.message_count);
                            tracing::info!(
                                "Older messages loaded for thread {}, total {} messages, got {}",
                                thread_id,
                                args.message_count,
                                messages_map.len()
                            );
                            // Drain any remaining buffered conversationUpdated signals
                            'drain: loop {
                                tokio::select! {
                                    biased;
                                    Some(signal) = updated_stream.next() => {
                                        if let Ok(args) = signal.args() {
                                            if let Some(msg) = parse_sms_message(&args.msg) {
                                                if msg.thread_id == thread_id {
                                                    messages_map.insert(msg.uid, msg);
                                                }
                                            }
                                        }
                                    }
                                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(SIGNAL_DRAIN_TIMEOUT_MS)) => {
                                        break 'drain;
                                    }
                                }
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse conversationLoaded signal: {}", e);
                    }
                }
            }
            // Timeout
            _ = tokio::time::sleep_until(start_time + timeout) => {
                tracing::warn!(
                    "Timeout waiting for older messages, got {} messages",
                    messages_map.len()
                );
                break;
            }
        }
    }

    // Convert map to sorted vector (oldest first)
    let mut messages: Vec<SmsMessage> = messages_map.into_values().collect();
    messages.sort_by(|a, b| a.date.cmp(&b.date));

    // Determine if there are more messages available using heuristic
    // (will be overridden by total_message_count if available)
    let has_more_heuristic = messages.len() >= count as usize;

    tracing::info!(
        "Loaded {} older messages for thread {}, has_more_heuristic: {}, total: {:?}",
        messages.len(),
        thread_id,
        has_more_heuristic,
        total_message_count
    );
    Message::OlderMessagesLoaded(thread_id, messages, has_more_heuristic, total_message_count)
}

