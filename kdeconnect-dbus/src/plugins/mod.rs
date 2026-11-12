//! D-Bus proxies for KDE Connect plugins.
//!
//! Each plugin provides specific functionality like battery status,
//! notifications, file sharing, etc. Plugins are only available on
//! devices that support them.

pub mod battery;
pub mod clipboard;
pub mod findmyphone;
pub mod mprisremote;
pub mod notifications;
pub mod ping;
pub mod share;
pub mod sms;
pub mod telephony;

pub use battery::BatteryProxy;
pub use clipboard::ClipboardProxy;
pub use findmyphone::FindMyPhoneProxy;
pub use mprisremote::MprisRemoteProxy;
pub use notifications::{NotificationInfo, NotificationProxy, NotificationsProxy};
pub use ping::PingProxy;
pub use share::ShareProxy;
pub use sms::{
    is_address_valid, parse_conversations, parse_sms_message, Attachment, ConversationSummary,
    ConversationsProxy, MessageType, SmsMessage, SmsProxy, OPTIMISTIC_MESSAGE_UID,
};
pub use telephony::TelephonyProxy;
