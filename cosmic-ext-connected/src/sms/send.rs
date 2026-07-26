//! SMS sending functionality.

use crate::app::Message;
use kdeconnect_dbus::plugins::ConversationsProxy;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::zvariant::{Structure, Value};
use zbus::Connection;

/// Convert a picked file path into the daemon's expected `attachmentUrls` element
fn attachment_value(path: &std::path::Path) -> Result<Value<'static>, String> {
    if !path.is_file() {
        return Err(format!(
            "attachment is not a readable file: {}",
            path.display()
        ));
    }
    let s = path
        .to_str()
        .ok_or_else(|| "Attachment path is not valid UTF-8".to_string())?;
    Ok(Value::from(s.to_owned()))
}

/// Send an SMS reply to an existing conversation using replyToConversation.
///
/// Uses the Conversations D-Bus interface with a thread ID. The daemon looks up
/// addresses from its in-memory `m_conversations` cache, which is populated when
/// the user opens the conversation (our SMS plugin `requestConversation` call
/// primes it). This preserves thread context for group messages.
///
/// Note: `replyToConversation` silently no-ops if the cache is empty (no D-Bus
/// error). The cache is reliably primed by our conversation loading flow.
pub async fn send_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    message: String,
    attachment: Option<std::path::PathBuf>,
) -> Message {
    let conn = conn.lock().await;
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    let attachments: Vec<Value<'_>> = match attachment.as_deref().map(attachment_value) {
        Some(Ok(v)) => vec![v],
        Some(Err(e)) => return Message::SmsSendResult(Err(e)),
        None => vec![],
    };

    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::SmsSendResult(Err(format!("Failed to create proxy: {}", e)));
            }
        },
        None => {
            return Message::SmsSendResult(Err("Failed to build proxy path".to_string()));
        }
    };

    tracing::debug!(
        "Sending SMS via replyToConversation for thread_id={}",
        thread_id
    );

    match conversations_proxy
        .reply_to_conversation(thread_id, &message, attachments)
        .await
    {
        Ok(()) => {
            tracing::info!("SMS sent successfully via replyToConversation");
            Message::SmsSendResult(Ok(message))
        }
        Err(e) => {
            tracing::error!("SMS send failed: {}", e);
            Message::SmsSendResult(Err(format!("Send failed: {}", e)))
        }
    }
}

/// Send an SMS to one or more recipients (creates or adds to existing conversation).
pub async fn send_new_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    recipients: Vec<String>,
    message: String,
    attachment: Option<std::path::PathBuf>,
) -> Message {
    let conn = conn.lock().await;
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

    let attachments: Vec<Value<'_>> = match attachment.as_deref().map(attachment_value) {
        Some(Ok(v)) => vec![v],
        Some(Err(e)) => return Message::NewMessageSendResult(Err(e)),
        None => vec![],
    };
    let conversations_proxy = match ConversationsProxy::builder(&conn)
        .path(device_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::NewMessageSendResult(Err(format!(
                    "Failed to create proxy: {}",
                    e
                )));
            }
        },
        None => {
            return Message::NewMessageSendResult(Err("Failed to build proxy path".to_string()));
        }
    };

    // Format addresses as D-Bus structs for KDE Connect
    // KDE Connect's ConversationAddress is a struct containing a single string: (s)
    let addresses: Vec<Value<'_>> = recipients
        .iter()
        .map(|r| Value::Structure(Structure::from((r.clone(),))))
        .collect();

    match conversations_proxy
        .send_without_conversation(addresses, &message, attachments)
        .await
    {
        Ok(()) => Message::NewMessageSendResult(Ok("Message sent".to_string())),
        Err(e) => Message::NewMessageSendResult(Err(format!("Send failed: {}", e))),
    }
}
