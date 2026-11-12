//! SMS sending functionality.

use crate::app::Message;
use crate::fl;
use kdeconnect_dbus::plugins::{ConversationsProxy, SmsProxy};
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::zvariant::{Structure, Value};
use zbus::Connection;

/// Convert a picked file path into the daemon's expected `attachmentUrls` element
fn attachment_value(path: &std::path::Path) -> Result<Value<'static>, String> {
    if !path.is_file() || std::fs::File::open(path).is_err() {
        tracing::warn!("attachment missing or unreadable at send");
        return Err(fl!("attachment-missing"));
    }
    let s = path.to_str().ok_or_else(|| fl!("attachment-missing"))?;
    Ok(Value::from(s.to_owned()))
}

/// Send a reply to an existing conversation via the SMS plugin's `sendSms`.
///
/// `sendSms` is the call both daemon reply paths end in (`SmsPlugin::sendSms`,
/// identical on 23.08.5 and 26.08.0). The caller chooses the recipients
/// (`SmsConversationStore::reply_recipients`) because `replyToConversation` resolves
/// them from the oldest cached message, which can be the wrong recipient set.
/// `Ok` is not delivery (`docs/DBUS.md`).
pub async fn send_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    addresses: Vec<String>,
    sub_id: i64,
    message: String,
    attachment: Option<std::path::PathBuf>,
) -> Message {
    let conn = conn.lock().await;
    let sms_path = format!("{}/devices/{}/sms", kdeconnect_dbus::BASE_PATH, device_id);

    let attachments: Vec<Value<'_>> = match attachment.as_deref().map(attachment_value) {
        Some(Ok(v)) => vec![v],
        Some(Err(e)) => return Message::SmsSendResult(Err(e)),
        None => vec![],
    };

    let sms_proxy = match SmsProxy::builder(&conn)
        .path(sms_path.as_str())
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

    let recipient_count = addresses.len();
    let addresses: Vec<Value<'_>> = addresses
        .into_iter()
        .map(|a| Value::Structure(Structure::from((a,))))
        .collect();

    match sms_proxy
        .send_sms(addresses, &message, attachments, sub_id)
        .await
    {
        Ok(()) => {
            tracing::info!("sendSms accepted for {} recipient(s)", recipient_count);
            Message::SmsSendResult(Ok(message))
        }
        Err(e) => {
            tracing::error!("sendSms failed: {}", e);
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
        Ok(()) => Message::NewMessageSendResult(Ok(fl!("sms-sent"))),
        Err(e) => Message::NewMessageSendResult(Err(format!("Send failed: {}", e))),
    }
}
