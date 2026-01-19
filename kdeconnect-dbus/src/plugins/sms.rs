//! D-Bus proxy for the SMS plugin.
//!
//! Provides access to SMS conversations and messages on the remote device.

use zbus::proxy;
use zbus::zvariant::{OwnedValue, Value};

/// Proxy for the SMS plugin D-Bus interface.
#[proxy(
    interface = "org.kde.kdeconnect.device.sms",
    default_service = "org.kde.kdeconnect.daemon"
)]
pub trait Sms {
    /// Request the phone to send all conversation threads.
    /// This triggers the phone to send conversation data which can then be
    /// retrieved via the Conversations interface.
    #[zbus(name = "requestAllConversations")]
    fn request_all_conversations(&self) -> zbus::Result<()>;

    /// Request messages from a specific conversation.
    ///
    /// # Arguments
    /// * `thread_id` - The conversation thread ID
    /// * `start_timestamp` - Timestamp to start from (0 for all)
    /// * `count` - Maximum number of messages to return
    #[zbus(name = "requestConversation")]
    fn request_conversation(
        &self,
        thread_id: i64,
        start_timestamp: i64,
        count: i64,
    ) -> zbus::Result<()>;

    /// Send an SMS message directly via the SMS plugin.
    /// This is more reliable than replyToConversation as it doesn't depend
    /// on the daemon's conversation cache.
    ///
    /// # Arguments
    /// * `addresses` - List of recipient addresses (phone numbers as D-Bus structs)
    /// * `text_message` - The text message to send
    /// * `attachment_urls` - URLs of attachments (empty for text-only messages)
    /// * `sub_id` - SIM subscription ID (-1 for default)
    #[zbus(name = "sendSms")]
    fn send_sms(
        &self,
        addresses: Vec<Value<'_>>,
        text_message: &str,
        attachment_urls: Vec<Value<'_>>,
        sub_id: i64,
    ) -> zbus::Result<()>;
}

/// Proxy for the conversations D-Bus interface.
/// Note: This interface is on the device path, not /sms
#[proxy(
    interface = "org.kde.kdeconnect.device.conversations",
    default_service = "org.kde.kdeconnect.daemon"
)]
pub trait Conversations {
    /// Get all active conversations.
    /// Returns a list of conversation data as variant values.
    #[zbus(name = "activeConversations")]
    fn active_conversations(&self) -> zbus::Result<Vec<OwnedValue>>;

    /// Request all conversation threads from the phone.
    #[zbus(name = "requestAllConversationThreads")]
    fn request_all_conversation_threads(&self) -> zbus::Result<()>;

    /// Request messages for a specific conversation.
    #[zbus(name = "requestConversation")]
    fn request_conversation(&self, conversation_id: i64, start: i32, end: i32) -> zbus::Result<()>;

    /// Signal emitted when a conversation message is updated/received.
    /// The msg parameter contains the message data as a variant.
    #[zbus(signal, name = "conversationUpdated")]
    fn conversation_updated(&self, msg: OwnedValue) -> zbus::Result<()>;

    /// Signal emitted when conversation loading is complete.
    /// `conversation_id` is the thread ID, `message_count` is total messages loaded.
    #[zbus(signal, name = "conversationLoaded")]
    fn conversation_loaded(&self, conversation_id: i64, message_count: u64) -> zbus::Result<()>;

    /// Signal emitted when a new conversation is created (first message received).
    /// The msg parameter contains the conversation data as a variant.
    #[zbus(signal, name = "conversationCreated")]
    fn conversation_created(&self, msg: OwnedValue) -> zbus::Result<()>;

    /// Reply to an existing conversation thread with a text message.
    ///
    /// # Arguments
    /// * `conversation_id` - The thread ID to reply to
    /// * `message` - The text message to send
    /// * `attachment_urls` - URLs of attachments (empty for text-only messages)
    #[zbus(name = "replyToConversation")]
    fn reply_to_conversation(
        &self,
        conversation_id: i64,
        message: &str,
        attachment_urls: Vec<Value<'_>>,
    ) -> zbus::Result<()>;

    /// Send a message to a recipient without specifying a conversation ID.
    ///
    /// The phone will automatically add the message to an existing conversation
    /// with the recipient if one exists, or create a new conversation if not.
    ///
    /// # Arguments
    /// * `addresses` - List of recipient addresses (phone numbers as D-Bus structs)
    /// * `message` - The text message to send
    /// * `attachment_urls` - URLs of attachments (empty for text-only messages)
    #[zbus(name = "sendWithoutConversation")]
    fn send_without_conversation(
        &self,
        addresses: Vec<Value<'_>>,
        message: &str,
        attachment_urls: Vec<Value<'_>>,
    ) -> zbus::Result<()>;
}

/// Message type indicating direction (sent vs received).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Message received from the contact (inbox).
    Inbox = 1,
    /// Message sent by the user.
    Sent = 2,
}

impl From<i32> for MessageType {
    fn from(value: i32) -> Self {
        // Android SMS type constants:
        // 1 = MESSAGE_TYPE_INBOX (received)
        // 2 = MESSAGE_TYPE_SENT
        // 3 = MESSAGE_TYPE_DRAFT
        // 4 = MESSAGE_TYPE_OUTBOX
        // 5 = MESSAGE_TYPE_FAILED
        // 6 = MESSAGE_TYPE_QUEUED
        // Only type 1 is truly a received message; all others are outgoing
        match value {
            1 => MessageType::Inbox,
            _ => MessageType::Sent,
        }
    }
}

/// A single SMS message.
#[derive(Debug, Clone)]
pub struct SmsMessage {
    /// The message text content.
    pub body: String,
    /// Phone numbers/addresses of all participants (for group messages).
    /// For 1-on-1 conversations this will have one element.
    pub addresses: Vec<String>,
    /// Unix timestamp in milliseconds.
    pub date: i64,
    /// Whether this is a sent or received message.
    pub message_type: MessageType,
    /// Whether the message has been read.
    pub read: bool,
    /// The conversation thread ID this message belongs to.
    pub thread_id: i64,
    /// SIM subscription ID (-1 for default).
    /// Required for MMS group messages to use the correct SIM.
    pub sub_id: i64,
}

impl SmsMessage {
    /// Get the primary address (first participant) for display purposes.
    pub fn primary_address(&self) -> &str {
        self.addresses.first().map(|s| s.as_str()).unwrap_or("Unknown")
    }
}

/// Summary of a conversation for the conversation list.
#[derive(Debug, Clone)]
pub struct ConversationSummary {
    /// The conversation thread ID.
    pub thread_id: i64,
    /// Phone numbers/addresses of all participants (for group messages).
    pub addresses: Vec<String>,
    /// Preview of the last message in the conversation.
    pub last_message: String,
    /// Timestamp of the last message in milliseconds.
    pub timestamp: i64,
    /// Whether there are unread messages.
    pub unread: bool,
}

impl ConversationSummary {
    /// Get the primary address (first participant) for display purposes.
    pub fn primary_address(&self) -> &str {
        self.addresses.first().map(|s| s.as_str()).unwrap_or("Unknown")
    }
}

/// Helper to extract a string from a Value.
fn get_string_from_value(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.as_str().to_string()),
        _ => None,
    }
}

/// Helper to extract an i64 from a Value.
fn get_i64_from_value(value: &Value<'_>) -> Option<i64> {
    match value {
        Value::I64(v) => Some(*v),
        Value::I32(v) => Some(*v as i64),
        Value::U64(v) => Some(*v as i64),
        Value::U32(v) => Some(*v as i64),
        Value::I16(v) => Some(*v as i64),
        Value::U16(v) => Some(*v as i64),
        _ => None,
    }
}

/// Helper to extract an i32 from a Value.
fn get_i32_from_value(value: &Value<'_>) -> Option<i32> {
    match value {
        Value::I32(v) => Some(*v),
        Value::I64(v) => Some(*v as i32),
        Value::I16(v) => Some(*v as i32),
        Value::U16(v) => Some(*v as i32),
        _ => None,
    }
}

/// Parse a D-Bus variant value into an SmsMessage.
///
/// KDE Connect returns messages as structs with fields in order:
/// (type: i32, body: s, addresses: a(s), date: i64, read: i32, ?, thread_id: i64, ?, ?, attachments: av)
pub fn parse_sms_message(value: &OwnedValue) -> Option<SmsMessage> {
    // Dereference OwnedValue to get Value
    let value_ref: &Value<'_> = value;

    // The value should be a struct
    let fields = match value_ref {
        Value::Structure(s) => s.fields(),
        _ => {
            tracing::debug!("SMS message is not a struct: {:?}", value_ref);
            return None;
        }
    };

    // Parse fields by position (from KDE Connect conversationmessage.h)
    // Field 0: eventField (i32) - event flags (e.g., 1 = EventTextMessage)
    // We don't currently use eventField, but it's documented here for reference

    // Field 1: body (string)
    let body = fields
        .get(1)
        .and_then(get_string_from_value)
        .unwrap_or_default();

    // Field 2: addresses (array of structs containing string)
    // Extract ALL addresses for group message support
    let addresses: Vec<String> = fields
        .get(2)
        .and_then(|v| {
            if let Value::Array(arr) = v {
                let addrs: Vec<String> = arr
                    .iter()
                    .filter_map(|addr_struct| {
                        // Each address is a struct with a single string field
                        if let Value::Structure(s) = addr_struct {
                            s.fields().first().and_then(get_string_from_value)
                        } else {
                            get_string_from_value(addr_struct)
                        }
                    })
                    .collect();
                if addrs.is_empty() {
                    None
                } else {
                    Some(addrs)
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| vec!["Unknown".to_string()]);

    // Field 3: date (i64)
    let date = fields.get(3).and_then(get_i64_from_value).unwrap_or(0);

    // Field 4: type (i32) - 1=Inbox (incoming), 2=Sent (outgoing)
    let msg_type_value = fields.get(4).and_then(get_i32_from_value).unwrap_or(1);
    let msg_type_parsed = MessageType::from(msg_type_value);

    // Field 5: read (i32, 0=unread, 1=read)
    let read = fields
        .get(5)
        .and_then(get_i32_from_value)
        .map(|v| v != 0)
        .unwrap_or(true);

    // Field 6: thread_id (i64)
    let thread_id = fields.get(6).and_then(get_i64_from_value).unwrap_or(0);

    // Field 7: uID (i32) - unique message ID
    // Field 8: subID (i64) - SIM subscription ID (which SIM card to use)
    let sub_id = fields.get(8).and_then(get_i64_from_value).unwrap_or(-1);
    // Field 9: attachments (array)

    Some(SmsMessage {
        body,
        addresses,
        date,
        message_type: msg_type_parsed,
        read,
        thread_id,
        sub_id,
    })
}

/// Maximum number of conversations to display in the list.
pub const MAX_CONVERSATIONS: usize = 20;

/// Parse a list of D-Bus variant values into conversation summaries.
///
/// Groups messages by thread_id and extracts the most recent message
/// from each thread to create summaries. Limited to MAX_CONVERSATIONS.
pub fn parse_conversations(values: Vec<OwnedValue>) -> Vec<ConversationSummary> {
    let mut messages: Vec<SmsMessage> = values.iter().filter_map(parse_sms_message).collect();

    // Sort by date descending to get most recent first
    messages.sort_by(|a, b| b.date.cmp(&a.date));

    // Group by thread_id and take the first (most recent) message per thread
    let mut seen_threads = std::collections::HashSet::new();
    let mut summaries = Vec::new();

    for msg in messages {
        if seen_threads.contains(&msg.thread_id) {
            continue;
        }
        seen_threads.insert(msg.thread_id);

        summaries.push(ConversationSummary {
            thread_id: msg.thread_id,
            addresses: msg.addresses,
            last_message: msg.body,
            timestamp: msg.date,
            unread: !msg.read,
        });

        // Limit to most recent conversations
        if summaries.len() >= MAX_CONVERSATIONS {
            break;
        }
    }

    summaries
}

/// Parse messages for a specific conversation thread.
pub fn parse_messages(values: Vec<OwnedValue>, thread_id: i64) -> Vec<SmsMessage> {
    let mut messages: Vec<SmsMessage> = values
        .iter()
        .filter_map(parse_sms_message)
        .filter(|msg| msg.thread_id == thread_id)
        .collect();

    // Sort by date ascending (oldest first for display)
    messages.sort_by(|a, b| a.date.cmp(&b.date));

    messages
}

/// Canonicalize a phone number by removing formatting characters.
///
/// Strips spaces, dashes, parentheses, and plus signs.
/// Leading zeros are preserved as they may be significant in some regions.
pub fn canonicalize_phone_number(phone: &str) -> String {
    phone
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '+'))
        .collect()
}

/// Validate if an address (phone number or email) is valid for SMS.
///
/// Returns true for:
/// - Phone numbers: 3-15 digits after removing formatting characters
/// - Email addresses: basic pattern with @ symbol
pub fn is_address_valid(address: &str) -> bool {
    let canonicalized = canonicalize_phone_number(address);

    // Check if it's a valid phone number (3-15 digits)
    if canonicalized.len() >= 3
        && canonicalized.len() <= 15
        && canonicalized.chars().all(|c| c.is_ascii_digit())
    {
        return true;
    }

    // Check if it's an email address (basic validation)
    if address.contains('@') {
        let parts: Vec<&str> = address.split('@').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return true;
        }
    }

    false
}
