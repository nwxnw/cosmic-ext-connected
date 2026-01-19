//! Standalone window mode for development testing.
//!
//! This mode runs the applet as a regular desktop window instead of a panel applet,
//! making it easier to develop and test without embedding in the COSMIC panel.

use crate::config::Config;
use crate::fl;

/// Maximum width for message bubbles in pixels.
/// Same value as applet mode for consistent appearance.
const MESSAGE_BUBBLE_MAX_WIDTH: u16 = 340;
use cosmic::app::{Core, Settings};
use cosmic::iced::widget::{column, row, scrollable, text};
use cosmic::iced::{Alignment, Length, Subscription};
use cosmic::widget;
use cosmic::{Application, Element};
use futures_util::StreamExt;
use kdeconnect_dbus::{
    plugins::{
        is_address_valid, parse_conversations, parse_messages, BatteryProxy, ClipboardProxy,
        ConversationSummary, ConversationsProxy, MessageType, MprisRemoteProxy, NotificationInfo,
        NotificationProxy, NotificationsProxy, PingProxy, ShareProxy, SmsMessage, SmsProxy,
    },
    Contact, ContactLookup, DaemonProxy, DeviceProxy,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Messages for the standalone window.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // NewMessage variants refer to SMS, not the enum
pub enum Message {
    /// Refresh device list
    RefreshDevices,
    /// Device list was updated
    DevicesUpdated(Vec<DeviceInfo>),
    /// D-Bus connection established
    DbusConnected(Arc<Mutex<Connection>>),
    /// D-Bus connection failed
    DbusConnectionFailed(String),
    /// Error occurred
    Error(String),
    /// Request pairing with a device
    RequestPair(String),
    /// Unpair from a device
    Unpair(String),
    /// Accept incoming pairing request
    AcceptPairing(String),
    /// Reject incoming pairing request
    RejectPairing(String),
    /// Pairing operation completed (success or failure message)
    PairingResult(Result<String, String>),
    /// D-Bus signal received indicating device state changed
    DbusSignalReceived,
    /// Send a ping to a device
    SendPing(String),
    /// Ping operation completed
    PingResult(Result<String, String>),
    /// Share file to device (opens file picker)
    ShareFile(String),
    /// File selected from picker
    FileSelected(Option<PathBuf>),
    /// Share text to device
    ShareText(String, String),
    /// Share operation completed
    ShareResult(Result<String, String>),
    /// Update share text input
    ShareTextInput(String),
    /// Dismiss a notification
    DismissNotification(String, String),
    /// Notification dismissed result
    DismissResult(Result<String, String>),
    /// Toggle settings panel visibility
    ToggleSettings,
    /// Toggle a boolean setting
    ToggleSetting(SettingKey),
    /// Open SMS view for a device
    OpenSmsView(String),
    /// Close SMS view and return to device list
    CloseSmsView,
    /// Open the "Send to device" submenu
    OpenSendToView(String, String), // device_id, device_type
    /// Return from SendTo view to device list
    BackFromSendTo,
    /// Open a specific conversation thread
    OpenConversation(i64),
    /// Close conversation and return to conversation list
    CloseConversation,
    /// Conversations loaded from device
    ConversationsLoaded(Vec<ConversationSummary>),
    /// Messages loaded for a specific thread
    MessagesLoaded(i64, Vec<SmsMessage>),
    /// SMS-related error occurred
    SmsError(String),
    /// Update SMS compose text input
    SmsComposeInput(String),
    /// Send SMS in current thread
    SendSms,
    /// SMS send operation completed
    SmsSendResult(Result<String, String>),
    /// Send current clipboard to device
    SendClipboard(String),
    /// Clipboard operation completed
    ClipboardResult(Result<String, String>),
    /// Open new message compose view
    OpenNewMessage,
    /// Close new message view
    CloseNewMessage,
    /// Update new message recipient input
    NewMessageRecipientInput(String),
    /// Update new message body input
    NewMessageBodyInput(String),
    /// Send the new message
    SendNewMessage,
    /// New message send completed
    NewMessageSendResult(Result<String, String>),
    /// Select a contact from suggestions (name, phone_number)
    SelectContact(String, String),
    /// Open media controls for a device
    OpenMediaView(String),
    /// Close media view
    CloseMediaView,
    /// Media info loaded from device
    MediaInfoLoaded(Option<MediaInfo>),
    /// Toggle play/pause
    MediaPlayPause,
    /// Skip to next track
    MediaNext,
    /// Go to previous track
    MediaPrevious,
    /// Set volume
    MediaSetVolume(i32),
    /// Select a different player
    MediaSelectPlayer(String),
    /// Media control action completed
    MediaActionResult(Result<String, String>),
    /// Refresh media info (for auto-refresh)
    MediaRefresh,
    /// Configuration changed (from file watcher)
    ConfigChanged(Config),

    // SMS Notifications
    /// New SMS received via D-Bus signal (device_id, message)
    SmsNotificationReceived(String, SmsMessage),
}

/// Information about current media playback.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    /// List of available players on the device.
    pub players: Vec<String>,
    /// Currently selected player name.
    pub current_player: String,
    /// Track title.
    pub title: String,
    /// Track artist.
    pub artist: String,
    /// Track album.
    pub album: String,
    /// Whether playback is active.
    pub is_playing: bool,
    /// Current volume (0-100).
    pub volume: i32,
    /// Current position in milliseconds.
    pub position: i64,
    /// Track length in milliseconds.
    pub length: i64,
    /// Can go to next track.
    pub can_next: bool,
    /// Can go to previous track.
    pub can_previous: bool,
}

/// Keys for boolean settings that can be toggled.
#[derive(Debug, Clone)]
pub enum SettingKey {
    ShowBatteryPercentage,
    ShowOfflineDevices,
    ForwardNotifications,
    SmsNotifications,
    SmsShowContent,
    SmsShowSender,
}

/// Current view mode for navigation.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ViewMode {
    /// Main device list view.
    #[default]
    DeviceList,
    /// Send to device submenu (file, clipboard, ping, text).
    SendTo,
    /// SMS conversation list for a device.
    ConversationList,
    /// Individual message thread view.
    MessageThread,
    /// Compose a new message to a new recipient.
    NewMessage,
    /// Media player controls for a device.
    MediaControls,
}

/// Basic device information for display.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_reachable: bool,
    pub is_paired: bool,
    pub is_pair_requested: bool,
    pub is_pair_requested_by_peer: bool,
    pub battery_level: Option<i32>,
    pub battery_charging: Option<bool>,
    pub notifications: Vec<NotificationInfo>,
}

/// Standalone window application state.
pub struct StandaloneApp {
    core: Core,
    config: Config,
    devices: Vec<DeviceInfo>,
    error: Option<String>,
    status_message: Option<String>,
    dbus_connection: Option<Arc<Mutex<Connection>>>,
    loading: bool,
    show_settings: bool,
    /// Text input for sharing
    share_text_input: String,
    /// Device awaiting file selection
    pending_share_device: Option<String>,
    /// Current view mode for navigation
    view_mode: ViewMode,
    /// Device ID currently viewing SMS for
    sms_device_id: Option<String>,
    /// Device name for SMS view header
    sms_device_name: Option<String>,
    /// List of conversations for current device
    conversations: Vec<ConversationSummary>,
    /// Current conversation thread ID being viewed
    current_thread_id: Option<i64>,
    /// Current conversation addresses (all recipients for group messages)
    current_thread_addresses: Option<Vec<String>>,
    /// Messages in the current thread
    messages: Vec<SmsMessage>,
    /// Whether SMS data is currently loading
    sms_loading: bool,
    /// Contact lookup for resolving phone numbers to names
    contacts: ContactLookup,
    /// Key to reset conversation list scroll position (incremented to force scroll to top)
    conversation_list_key: u32,
    /// Text input for composing SMS reply
    sms_compose_text: String,
    /// Whether SMS is currently being sent
    sms_sending: bool,
    /// Timestamp of last D-Bus signal refresh (for debouncing)
    last_signal_refresh: std::time::Instant,
    /// Cache of messages by thread_id for faster loading
    message_cache: HashMap<i64, Vec<SmsMessage>>,
    /// Phone number input for new message compose
    new_message_recipient: String,
    /// Message body for new message compose
    new_message_body: String,
    /// Whether the recipient is valid
    new_message_recipient_valid: bool,
    /// Whether a new message is being sent
    new_message_sending: bool,
    /// Contact suggestions for new message recipient
    contact_suggestions: Vec<Contact>,
    /// Device ID for media controls view
    media_device_id: Option<String>,
    /// Device name for media controls header
    media_device_name: Option<String>,
    /// Current media playback info
    media_info: Option<MediaInfo>,
    /// Whether media info is loading
    media_loading: bool,
    /// Device ID for SendTo view
    sendto_device_id: Option<String>,
    /// Device type for SendTo view header (e.g., "phone", "tablet")
    sendto_device_type: Option<String>,

    // SMS notification deduplication
    /// Last seen SMS timestamp per thread_id to avoid duplicate notifications
    last_seen_sms: HashMap<i64, i64>,
}

impl Application for StandaloneApp {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.cosmic-connect-applet.standalone";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, cosmic::app::Task<Self::Message>) {
        // Load config from disk or use defaults
        let config = Config::load();

        let app = StandaloneApp {
            core,
            config,
            devices: Vec::new(),
            error: None,
            status_message: None,
            dbus_connection: None,
            loading: true,
            show_settings: false,
            share_text_input: String::new(),
            pending_share_device: None,
            view_mode: ViewMode::default(),
            sms_device_id: None,
            sms_device_name: None,
            conversations: Vec::new(),
            current_thread_id: None,
            current_thread_addresses: None,
            messages: Vec::new(),
            sms_loading: false,
            contacts: ContactLookup::new(),
            conversation_list_key: 0,
            sms_compose_text: String::new(),
            sms_sending: false,
            last_signal_refresh: std::time::Instant::now(),
            message_cache: HashMap::new(),
            new_message_recipient: String::new(),
            new_message_body: String::new(),
            new_message_recipient_valid: false,
            new_message_sending: false,
            contact_suggestions: Vec::new(),
            media_device_id: None,
            media_device_name: None,
            media_info: None,
            media_loading: false,
            sendto_device_id: None,
            sendto_device_type: None,
            // SMS notification deduplication
            last_seen_sms: HashMap::new(),
        };

        // Connect to D-Bus on startup
        let task = cosmic::app::Task::perform(async { Connection::session().await }, |result| {
            cosmic::Action::App(match result {
                Ok(conn) => Message::DbusConnected(Arc::new(Mutex::new(conn))),
                Err(e) => Message::DbusConnectionFailed(e.to_string()),
            })
        });

        (app, task)
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        match message {
            Message::DbusConnected(conn) => {
                tracing::info!("D-Bus connection established");
                self.dbus_connection = Some(conn.clone());
                self.error = None;
                return cosmic::app::Task::perform(fetch_devices_async(conn), cosmic::Action::App);
            }
            Message::DbusConnectionFailed(err) => {
                tracing::error!("D-Bus connection failed: {}", err);
                self.error = Some(format!("Cannot connect to KDE Connect: {}", err));
                self.loading = false;
            }
            Message::RefreshDevices => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::debug!("Refreshing device list");
                    self.loading = true;
                    self.status_message = None;
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::DevicesUpdated(devices) => {
                tracing::debug!("Devices updated: {} devices", devices.len());
                self.devices = devices;
                self.error = None;
                self.loading = false;
                self.status_message = None; // Clear status after refresh
            }
            Message::Error(err) => {
                tracing::error!("Error: {}", err);
                self.error = Some(err);
                self.loading = false;
            }
            Message::RequestPair(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Requesting pairing with device: {}", device_id);
                    self.status_message = Some("Pairing request sent...".to_string());
                    return cosmic::app::Task::perform(
                        request_pair_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::Unpair(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Unpairing from device: {}", device_id);
                    self.status_message = Some("Unpairing...".to_string());
                    return cosmic::app::Task::perform(
                        unpair_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::AcceptPairing(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Accepting pairing from device: {}", device_id);
                    self.status_message = Some("Accepting pairing...".to_string());
                    return cosmic::app::Task::perform(
                        accept_pairing_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::RejectPairing(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Rejecting pairing from device: {}", device_id);
                    self.status_message = Some("Rejecting pairing...".to_string());
                    return cosmic::app::Task::perform(
                        reject_pairing_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::PairingResult(result) => {
                match result {
                    Ok(msg) => {
                        tracing::info!("Pairing operation succeeded: {}", msg);
                        self.status_message = Some(msg);
                    }
                    Err(err) => {
                        tracing::error!("Pairing operation failed: {}", err);
                        self.status_message = Some(format!("Error: {}", err));
                    }
                }
                // Refresh device list after pairing operation
                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::DbusSignalReceived => {
                // D-Bus signal received - only auto-refresh SMS views, not device list
                // The device list refreshes on explicit actions (pairing, etc.)

                // Skip if already loading to avoid interfering with signal-based message fetching
                if self.sms_loading {
                    return cosmic::app::Task::none();
                }

                // Debounce: require at least 3 seconds between signal-triggered refreshes
                let now = std::time::Instant::now();
                if now.duration_since(self.last_signal_refresh) < std::time::Duration::from_secs(3)
                {
                    return cosmic::app::Task::none();
                }

                if let Some(conn) = &self.dbus_connection {
                    match self.view_mode {
                        ViewMode::MessageThread => {
                            // Refresh the current message thread to show new messages
                            if let (Some(device_id), Some(thread_id)) =
                                (&self.sms_device_id, self.current_thread_id)
                            {
                                tracing::debug!(
                                    "Refreshing message thread {} for device {}",
                                    thread_id,
                                    device_id
                                );
                                self.last_signal_refresh = now;
                                // Don't show loading if we already have messages (background refresh)
                                if self.messages.is_empty() {
                                    self.sms_loading = true;
                                }
                                return cosmic::app::Task::perform(
                                    fetch_messages_async(
                                        conn.clone(),
                                        device_id.clone(),
                                        thread_id,
                                        self.config.messages_per_page,
                                    ),
                                    cosmic::Action::App,
                                );
                            }
                        }
                        ViewMode::ConversationList => {
                            // Refresh conversation list to show new messages
                            if let Some(device_id) = &self.sms_device_id {
                                tracing::debug!(
                                    "Refreshing conversation list for device {}",
                                    device_id
                                );
                                self.last_signal_refresh = now;
                                // Don't show loading if we already have conversations (background refresh)
                                if self.conversations.is_empty() {
                                    self.sms_loading = true;
                                }
                                return cosmic::app::Task::perform(
                                    fetch_conversations_async(conn.clone(), device_id.clone()),
                                    cosmic::Action::App,
                                );
                            }
                        }
                        _ => {
                            // Don't auto-refresh device list - too noisy
                            // Device list refreshes on explicit user actions
                        }
                    }
                }
            }
            Message::SendPing(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Sending ping to device: {}", device_id);
                    self.status_message = Some("Sending ping...".to_string());
                    return cosmic::app::Task::perform(
                        send_ping_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::PingResult(result) => match result {
                Ok(msg) => {
                    tracing::info!("Ping succeeded: {}", msg);
                    self.status_message = Some(msg);
                }
                Err(err) => {
                    tracing::error!("Ping failed: {}", err);
                    self.status_message = Some(format!("Ping failed: {}", err));
                }
            },
            Message::ShareFile(device_id) => {
                self.pending_share_device = Some(device_id);
                return cosmic::app::Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    |path| cosmic::Action::App(Message::FileSelected(path)),
                );
            }
            Message::FileSelected(path) => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, self.pending_share_device.take())
                {
                    if let Some(path) = path {
                        self.status_message = Some("Sharing file...".to_string());
                        return cosmic::app::Task::perform(
                            share_file_async(conn.clone(), device_id, path),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::ShareTextInput(text) => {
                self.share_text_input = text;
            }
            Message::ShareText(device_id, text) => {
                if let Some(conn) = &self.dbus_connection {
                    self.share_text_input.clear();
                    self.status_message = Some("Sharing text...".to_string());
                    return cosmic::app::Task::perform(
                        share_text_async(conn.clone(), device_id, text),
                        cosmic::Action::App,
                    );
                }
            }
            Message::ShareResult(result) => match result {
                Ok(msg) => {
                    tracing::info!("Share succeeded: {}", msg);
                    self.status_message = Some(msg);
                }
                Err(err) => {
                    tracing::error!("Share failed: {}", err);
                    self.status_message = Some(format!("Share failed: {}", err));
                }
            },
            Message::DismissNotification(device_id, notification_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!(
                        "Dismissing notification {} on device {}",
                        notification_id,
                        device_id
                    );
                    return cosmic::app::Task::perform(
                        dismiss_notification_async(conn.clone(), device_id, notification_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::DismissResult(result) => {
                match result {
                    Ok(msg) => {
                        tracing::info!("Dismiss succeeded: {}", msg);
                    }
                    Err(err) => {
                        tracing::error!("Dismiss failed: {}", err);
                        self.status_message = Some(format!("Failed to dismiss: {}", err));
                    }
                }
                // Refresh devices to update notification list
                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
            }
            Message::ToggleSetting(key) => {
                match key {
                    SettingKey::ShowBatteryPercentage => {
                        self.config.show_battery_percentage = !self.config.show_battery_percentage;
                    }
                    SettingKey::ShowOfflineDevices => {
                        self.config.show_offline_devices = !self.config.show_offline_devices;
                    }
                    SettingKey::ForwardNotifications => {
                        self.config.forward_notifications = !self.config.forward_notifications;
                    }
                    SettingKey::SmsNotifications => {
                        self.config.sms_notifications = !self.config.sms_notifications;
                    }
                    SettingKey::SmsShowContent => {
                        self.config.sms_notification_show_content =
                            !self.config.sms_notification_show_content;
                    }
                    SettingKey::SmsShowSender => {
                        self.config.sms_notification_show_sender =
                            !self.config.sms_notification_show_sender;
                    }
                }
                tracing::debug!("Settings updated: {:?}", self.config);
                // Save config to disk
                if let Err(err) = self.config.save() {
                    tracing::error!(?err, "Failed to save config");
                }
            }
            Message::ConfigChanged(config) => {
                tracing::info!("Config changed externally: {:?}", config);
                self.config = config;
            }
            Message::OpenSmsView(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    // Find device name for header
                    let device_name = self
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .map(|d| d.name.clone());

                    // Load contacts for this device
                    self.contacts = ContactLookup::load_for_device(&device_id);
                    tracing::info!(
                        "Loaded {} contacts for device {}",
                        self.contacts.len(),
                        device_id
                    );

                    self.view_mode = ViewMode::ConversationList;
                    self.sms_device_id = Some(device_id.clone());
                    self.sms_device_name = device_name;
                    self.sms_loading = true;
                    self.conversations.clear();
                    tracing::info!("Opening SMS view for device: {}", device_id);
                    return cosmic::app::Task::perform(
                        fetch_conversations_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::CloseSmsView => {
                self.view_mode = ViewMode::DeviceList;
                self.sms_device_id = None;
                self.sms_device_name = None;
                self.conversations.clear();
                self.messages.clear();
                self.current_thread_id = None;
                self.current_thread_addresses = None;
                self.sms_loading = false;
                self.sms_compose_text.clear();
                self.sms_sending = false;
                // Clear message cache when leaving SMS view (switching devices)
                self.message_cache.clear();
            }
            Message::OpenSendToView(device_id, device_type) => {
                self.sendto_device_id = Some(device_id);
                self.sendto_device_type = Some(device_type);
                self.view_mode = ViewMode::SendTo;
            }
            Message::BackFromSendTo => {
                self.view_mode = ViewMode::DeviceList;
                self.sendto_device_id = None;
                self.sendto_device_type = None;
            }
            Message::OpenConversation(thread_id) => {
                if let Some(conn) = &self.dbus_connection {
                    if let Some(device_id) = &self.sms_device_id {
                        // Find the addresses for this thread (all recipients for group messages)
                        let addresses = self
                            .conversations
                            .iter()
                            .find(|c| c.thread_id == thread_id)
                            .map(|c| c.addresses.clone());

                        self.view_mode = ViewMode::MessageThread;
                        self.current_thread_id = Some(thread_id);
                        self.current_thread_addresses = addresses;

                        // Check cache first - show cached messages immediately
                        let has_cache = if let Some(cached) = self.message_cache.get(&thread_id) {
                            tracing::info!(
                                "Using {} cached messages for thread {}",
                                cached.len(),
                                thread_id
                            );
                            self.messages = cached.clone();
                            self.sms_loading = false; // Don't show loading if we have cache
                            true
                        } else {
                            self.messages.clear();
                            self.sms_loading = true;
                            false
                        };

                        tracing::info!("Opening conversation thread: {}", thread_id);

                        // Fetch messages in background
                        let fetch_task = cosmic::app::Task::perform(
                            fetch_messages_async(
                                conn.clone(),
                                device_id.clone(),
                                thread_id,
                                self.config.messages_per_page,
                            ),
                            cosmic::Action::App,
                        );

                        // If we have cached messages, also scroll to bottom
                        if has_cache {
                            return cosmic::app::Task::batch(vec![
                                fetch_task,
                                scrollable::snap_to(
                                    widget::Id::new("message-thread"),
                                    scrollable::RelativeOffset::END,
                                ),
                            ]);
                        }
                        return fetch_task;
                    }
                }
            }
            Message::CloseConversation => {
                self.view_mode = ViewMode::ConversationList;
                self.current_thread_id = None;
                self.current_thread_addresses = None;
                self.messages.clear();
                self.sms_compose_text.clear();
                self.sms_sending = false;
                // Increment key for scrollable
                self.conversation_list_key = self.conversation_list_key.wrapping_add(1);

                // Refresh conversations in background to show any new messages
                // Don't show loading if we already have conversations (instant display)
                if let (Some(conn), Some(device_id)) = (&self.dbus_connection, &self.sms_device_id)
                {
                    if self.conversations.is_empty() {
                        self.sms_loading = true;
                    }
                    return cosmic::app::Task::perform(
                        fetch_conversations_async(conn.clone(), device_id.clone()),
                        cosmic::Action::App,
                    );
                }
                self.sms_loading = false;
            }
            Message::ConversationsLoaded(convs) => {
                tracing::info!(
                    "Loaded {} conversations (had {} cached)",
                    convs.len(),
                    self.conversations.len()
                );
                // Only update if we got actual conversations back
                // Don't overwrite cached conversations with empty results
                if !convs.is_empty() {
                    self.conversations = convs;
                    // Increment key to ensure scrollable starts at top
                    self.conversation_list_key = self.conversation_list_key.wrapping_add(1);
                }
                self.sms_loading = false;
            }
            Message::MessagesLoaded(thread_id, msgs) => {
                if self.current_thread_id == Some(thread_id) {
                    let had_messages = !self.messages.is_empty();
                    let new_count = msgs.len();

                    tracing::info!(
                        "Loaded {} messages for thread {} (had {} cached)",
                        new_count,
                        thread_id,
                        self.messages.len()
                    );

                    // Only update if we got actual messages back
                    // Don't overwrite cached messages with empty results
                    if !msgs.is_empty() {
                        self.message_cache.insert(thread_id, msgs.clone());
                        self.messages = msgs;
                    }

                    self.sms_loading = false;

                    // Only scroll to bottom if we didn't have cached messages
                    // (avoid jarring scroll when refreshing)
                    if !had_messages && !self.messages.is_empty() {
                        return scrollable::snap_to(
                            widget::Id::new("message-thread"),
                            scrollable::RelativeOffset::END,
                        );
                    }
                }
            }
            Message::SmsComposeInput(text) => {
                self.sms_compose_text = text;
            }
            Message::SendSms => {
                if let (Some(conn), Some(device_id), Some(thread_id), Some(addresses)) = (
                    &self.dbus_connection,
                    &self.sms_device_id,
                    self.current_thread_id,
                    &self.current_thread_addresses,
                ) {
                    if !self.sms_compose_text.is_empty() && !self.sms_sending {
                        let message_text = self.sms_compose_text.clone();
                        let recipients = addresses.clone();
                        self.sms_sending = true;
                        return cosmic::app::Task::perform(
                            send_sms_async(
                                conn.clone(),
                                device_id.clone(),
                                thread_id,
                                recipients,
                                message_text,
                            ),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::SmsSendResult(result) => {
                self.sms_sending = false;
                match result {
                    Ok(msg) => {
                        self.sms_compose_text.clear();
                        self.status_message = Some(msg);
                        // Refresh messages to show the sent message
                        if let (Some(conn), Some(device_id), Some(thread_id)) = (
                            &self.dbus_connection,
                            &self.sms_device_id,
                            self.current_thread_id,
                        ) {
                            return cosmic::app::Task::perform(
                                fetch_messages_async(
                                    conn.clone(),
                                    device_id.clone(),
                                    thread_id,
                                    self.config.messages_per_page,
                                ),
                                cosmic::Action::App,
                            );
                        }
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Failed to send: {}", err));
                    }
                }
            }
            Message::SmsError(err) => {
                tracing::error!("SMS error: {}", err);
                self.status_message = Some(format!("SMS error: {}", err));
                self.sms_loading = false;
            }
            Message::SendClipboard(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    self.status_message = Some("Sending clipboard...".to_string());
                    return cosmic::app::Task::perform(
                        send_clipboard_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::ClipboardResult(result) => match result {
                Ok(msg) => {
                    self.status_message = Some(msg);
                }
                Err(err) => {
                    self.status_message = Some(format!("Clipboard error: {}", err));
                }
            },
            Message::OpenNewMessage => {
                self.view_mode = ViewMode::NewMessage;
                self.new_message_recipient.clear();
                self.new_message_body.clear();
                self.new_message_recipient_valid = false;
                self.new_message_sending = false;
                self.contact_suggestions.clear();
            }
            Message::CloseNewMessage => {
                self.view_mode = ViewMode::ConversationList;
                self.new_message_recipient.clear();
                self.new_message_body.clear();
                self.new_message_recipient_valid = false;
                self.new_message_sending = false;
                self.contact_suggestions.clear();
            }
            Message::NewMessageRecipientInput(text) => {
                self.new_message_recipient_valid = is_address_valid(&text);
                // Search for matching contacts by name (limit to 5 suggestions)
                self.contact_suggestions = self
                    .contacts
                    .search_by_name(&text, 5)
                    .into_iter()
                    .cloned()
                    .collect();
                self.new_message_recipient = text;
            }
            Message::SelectContact(name, phone) => {
                // User selected a contact - fill in the phone number
                self.new_message_recipient = phone;
                self.new_message_recipient_valid = true;
                self.contact_suggestions.clear();
                self.status_message = Some(format!("Selected: {}", name));
            }
            Message::NewMessageBodyInput(text) => {
                self.new_message_body = text;
            }
            Message::SendNewMessage => {
                if let (Some(conn), Some(device_id)) = (&self.dbus_connection, &self.sms_device_id)
                {
                    if self.new_message_recipient_valid
                        && !self.new_message_body.is_empty()
                        && !self.new_message_sending
                    {
                        let recipient = self.new_message_recipient.clone();
                        let message = self.new_message_body.clone();
                        self.new_message_sending = true;
                        return cosmic::app::Task::perform(
                            send_new_sms_async(conn.clone(), device_id.clone(), recipient, message),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::NewMessageSendResult(result) => {
                self.new_message_sending = false;
                match result {
                    Ok(msg) => {
                        self.status_message = Some(msg);
                        // Clear fields and return to conversation list
                        self.new_message_recipient.clear();
                        self.new_message_body.clear();
                        self.new_message_recipient_valid = false;
                        self.view_mode = ViewMode::ConversationList;
                        // Refresh conversations to show the new thread
                        if let (Some(conn), Some(device_id)) =
                            (&self.dbus_connection, &self.sms_device_id)
                        {
                            return cosmic::app::Task::perform(
                                fetch_conversations_async(conn.clone(), device_id.clone()),
                                cosmic::Action::App,
                            );
                        }
                    }
                    Err(err) => {
                        self.status_message = Some(format!("Failed to send: {}", err));
                    }
                }
            }
            Message::OpenMediaView(device_id) => {
                // Find device name
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.name.clone());
                self.media_device_id = Some(device_id.clone());
                self.media_device_name = device_name;
                self.media_info = None;
                self.media_loading = true;
                self.view_mode = ViewMode::MediaControls;
                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        fetch_media_info_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::CloseMediaView => {
                self.view_mode = ViewMode::DeviceList;
                self.media_device_id = None;
                self.media_device_name = None;
                self.media_info = None;
                self.media_loading = false;
            }
            Message::MediaInfoLoaded(info) => {
                self.media_loading = false;
                self.media_info = info;
            }
            Message::MediaPlayPause => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    return cosmic::app::Task::perform(
                        media_action_async(conn.clone(), device_id.clone(), MediaAction::PlayPause),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaNext => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    return cosmic::app::Task::perform(
                        media_action_async(conn.clone(), device_id.clone(), MediaAction::Next),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaPrevious => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    return cosmic::app::Task::perform(
                        media_action_async(conn.clone(), device_id.clone(), MediaAction::Previous),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaSetVolume(volume) => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    // Update local state immediately for responsive UI
                    if let Some(ref mut info) = self.media_info {
                        info.volume = volume;
                    }
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::SetVolume(volume),
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaSelectPlayer(player) => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    // Update local state immediately
                    if let Some(ref mut info) = self.media_info {
                        info.current_player = player.clone();
                    }
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::SelectPlayer(player),
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaActionResult(result) => {
                if let Err(err) = result {
                    self.status_message = Some(format!("Media error: {}", err));
                }
                // Refresh media info after action
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    return cosmic::app::Task::perform(
                        fetch_media_info_async(conn.clone(), device_id.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaRefresh => {
                // Auto-refresh when in media view
                if self.view_mode == ViewMode::MediaControls {
                    if let (Some(conn), Some(device_id)) =
                        (&self.dbus_connection, &self.media_device_id)
                    {
                        return cosmic::app::Task::perform(
                            fetch_media_info_async(conn.clone(), device_id.clone()),
                            cosmic::Action::App,
                        );
                    }
                }
            }

            // SMS Notifications
            Message::SmsNotificationReceived(device_id, message) => {
                // Check if we've already seen this message (deduplication)
                let last_seen = self.last_seen_sms.get(&message.thread_id).copied();
                if last_seen.is_some() && last_seen >= Some(message.date) {
                    // Already seen this message or an older one
                    return cosmic::app::Task::none();
                }

                // Update last seen timestamp for this thread
                self.last_seen_sms.insert(message.thread_id, message.date);

                // Load contacts to resolve sender name
                let contacts = ContactLookup::load_for_device(&device_id);
                let sender_name = contacts.get_name_or_number(message.primary_address());

                // Build notification based on privacy settings
                let summary = if self.config.sms_notification_show_sender {
                    fl!("sms-notification-title-from", sender = sender_name.clone())
                } else {
                    fl!("sms-notification-title")
                };

                let body = if self.config.sms_notification_show_content {
                    message.body.clone()
                } else {
                    fl!("sms-notification-body-hidden")
                };

                // Show notification (non-blocking, fire-and-forget)
                // Note: Click-to-open functionality could be added in the future using
                // channels to communicate between the notification callback and the main app
                return cosmic::app::Task::perform(
                    async move {
                        // Show the notification
                        if let Err(e) = notify_rust::Notification::new()
                            .summary(&summary)
                            .body(&body)
                            .icon("phone-symbolic")
                            .appname("COSMIC Connected")
                            .show()
                        {
                            tracing::warn!("Failed to show SMS notification: {}", e);
                        }
                    },
                    |_| cosmic::Action::App(Message::RefreshDevices),
                );
            }
        }

        cosmic::app::Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        // Handle SMS, media, and SendTo views with their own headers
        match self.view_mode {
            ViewMode::ConversationList => return self.view_conversation_list(),
            ViewMode::MessageThread => return self.view_message_thread(),
            ViewMode::NewMessage => return self.view_new_message(),
            ViewMode::MediaControls => return self.view_media_controls(),
            ViewMode::SendTo => return self.view_send_to(),
            _ => {}
        }

        let settings_icon = if self.show_settings {
            "go-previous-symbolic"
        } else {
            "emblem-system-symbolic"
        };

        let header = row![
            text(fl!("app-title")).size(20),
            widget::horizontal_space(),
            widget::button::icon(widget::icon::from_name(settings_icon))
                .on_press(Message::ToggleSettings),
            widget::button::standard(fl!("refresh")).on_press(Message::RefreshDevices),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(16);

        // Status bar for pairing messages
        let status_bar: Option<Element<Message>> = self.status_message.as_ref().map(|msg| {
            widget::container(text(msg).size(12))
                .padding([4, 16])
                .width(Length::Fill)
                .into()
        });

        let content: Element<Message> = if self.show_settings {
            self.view_settings()
        } else if let Some(err) = &self.error {
            widget::container(
                column![
                    widget::icon::from_name("dialog-error-symbolic").size(48),
                    text(fl!("error")).size(18),
                    text(err.clone()).size(14),
                    widget::button::standard(fl!("retry")).on_press(Message::RefreshDevices),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.loading {
            widget::container(
                column![text(fl!("loading-devices")).size(16),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.devices.is_empty() {
            widget::container(
                column![
                    widget::icon::from_name("phone-disconnect-symbolic").size(48),
                    text(fl!("no-devices-found")).size(18),
                    text(fl!("no-devices-hint")).size(14),
                    text(fl!("no-devices-hint-extended")).size(14),
                    widget::button::standard(fl!("refresh")).on_press(Message::RefreshDevices),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            self.view_device_list()
        };

        let mut main_column = column![header].spacing(0);

        if let Some(status) = status_bar {
            main_column = main_column.push(status);
        }

        main_column
            .push(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subscriptions = vec![
            // Subscribe to D-Bus signals for device state changes
            Subscription::run(dbus_signal_subscription),
            // Watch for config changes from external sources
            self.core
                .watch_config::<Config>(crate::config::APP_ID)
                .map(|update| {
                    for err in update.errors {
                        tracing::error!(?err, "Error watching config");
                    }
                    Message::ConfigChanged(update.config)
                }),
        ];

        // Add media refresh timer when in media view
        if self.view_mode == ViewMode::MediaControls {
            subscriptions.push(
                cosmic::iced::time::every(std::time::Duration::from_secs(2))
                    .map(|_| Message::MediaRefresh),
            );
        }

        // Add SMS notification subscription when enabled and devices are connected
        if self.config.sms_notifications
            && self.devices.iter().any(|d| d.is_reachable && d.is_paired)
        {
            subscriptions.push(Subscription::run(sms_notification_subscription));
        }

        Subscription::batch(subscriptions)
    }
}

/// State for D-Bus signal subscription
#[allow(clippy::large_enum_variant)]
enum DbusSubscriptionState {
    Init,
    Listening {
        conn: Connection,
        stream: zbus::MessageStream,
    },
}

/// Create a stream that listens for D-Bus signals from KDE Connect
fn dbus_signal_subscription() -> impl futures_util::Stream<Item = Message> {
    futures_util::stream::unfold(DbusSubscriptionState::Init, |state| async move {
        match state {
            DbusSubscriptionState::Init => {
                // Connect to D-Bus
                let conn = match Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to connect to D-Bus for signals: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        return Some((
                            Message::Error("D-Bus connection failed".to_string()),
                            DbusSubscriptionState::Init,
                        ));
                    }
                };

                // Add match rule to receive KDE Connect signals
                let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("Failed to create DBus proxy: {}", e);
                        return Some((
                            Message::Error("D-Bus proxy failed".to_string()),
                            DbusSubscriptionState::Init,
                        ));
                    }
                };

                // Subscribe to all signals from KDE Connect
                // Build match rule for signals from kdeconnect daemon
                if let Ok(rule) = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .sender("org.kde.kdeconnect.daemon")
                    .map(|b| b.build())
                {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add match rule: {}", e);
                    } else {
                        tracing::debug!("Added match rule for kdeconnect daemon signals");
                    }
                }

                // Also subscribe to property changes
                if let Ok(rule) = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.freedesktop.DBus.Properties")
                    .map(|b| b.build())
                {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add properties match rule: {}", e);
                    } else {
                        tracing::debug!("Added match rule for property change signals");
                    }
                }

                tracing::debug!("D-Bus signal subscription started with match rules");

                // Create message stream
                let stream = zbus::MessageStream::from(&conn);

                Some((
                    Message::DbusSignalReceived,
                    DbusSubscriptionState::Listening { conn, stream },
                ))
            }
            DbusSubscriptionState::Listening { conn, mut stream } => {
                // Wait for signals - only trigger on SMS-related signals
                loop {
                    match stream.next().await {
                        Some(Ok(msg)) => {
                            if msg.header().message_type() == zbus::message::Type::Signal {
                                if let (Some(interface), Some(member)) =
                                    (msg.header().interface(), msg.header().member())
                                {
                                    let iface_str = interface.as_str();
                                    let member_str = member.as_str();

                                    // Only trigger refresh on SMS-related signals
                                    let is_sms_signal = iface_str
                                        == "org.kde.kdeconnect.device.conversations"
                                        && (member_str == "conversationUpdated"
                                            || member_str == "conversationCreated");

                                    if is_sms_signal {
                                        tracing::debug!(
                                            "SMS D-Bus signal: {} {}",
                                            interface,
                                            member
                                        );
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

/// State for SMS notification subscription.
#[allow(clippy::large_enum_variant)]
enum SmsSubscriptionState {
    Init,
    Listening {
        conn: Connection,
        stream: zbus::MessageStream,
    },
}

/// Create a stream that listens for incoming SMS messages via D-Bus signals.
fn sms_notification_subscription() -> impl futures_util::Stream<Item = Message> {
    use kdeconnect_dbus::plugins::parse_sms_message;

    futures_util::stream::unfold(SmsSubscriptionState::Init, |state| async move {
        match state {
            SmsSubscriptionState::Init => {
                // Connect to D-Bus
                let conn = match Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to connect to D-Bus for SMS signals: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        return Some((
                            Message::Error("D-Bus connection failed for SMS".to_string()),
                            SmsSubscriptionState::Init,
                        ));
                    }
                };

                // Add match rule for conversationUpdated signals
                let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("Failed to create DBus proxy for SMS: {}", e);
                        return Some((
                            Message::Error("D-Bus proxy failed for SMS".to_string()),
                            SmsSubscriptionState::Init,
                        ));
                    }
                };

                // Subscribe to conversation signals from KDE Connect
                // Note: interface() returns Result, so we chain with and_then for member()
                let rule_result = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .interface("org.kde.kdeconnect.device.conversations")
                    .and_then(|b| b.member("conversationUpdated"))
                    .map(|b| b.build());

                if let Ok(rule) = rule_result {
                    if let Err(e) = dbus_proxy.add_match_rule(rule).await {
                        tracing::warn!("Failed to add SMS match rule: {}", e);
                    } else {
                        tracing::debug!("Added match rule for SMS conversationUpdated signals");
                    }
                }

                tracing::debug!("SMS notification subscription started");

                // Create message stream
                let stream = zbus::MessageStream::from(&conn);

                // Don't emit a message on init, just move to listening state
                Some((
                    Message::RefreshDevices, // Trigger a refresh to pick up any pending state
                    SmsSubscriptionState::Listening { conn, stream },
                ))
            }
            SmsSubscriptionState::Listening { conn, mut stream } => {
                // Wait for conversationUpdated signals
                loop {
                    match stream.next().await {
                        Some(Ok(msg)) => {
                            if msg.header().message_type() == zbus::message::Type::Signal {
                                if let (Some(interface), Some(member)) =
                                    (msg.header().interface(), msg.header().member())
                                {
                                    let iface_str = interface.as_str();
                                    let member_str = member.as_str();

                                    // Only process conversationUpdated signals
                                    if iface_str == "org.kde.kdeconnect.device.conversations"
                                        && member_str == "conversationUpdated"
                                    {
                                        // Extract device ID from the path
                                        // Path format: /modules/kdeconnect/devices/{device_id}
                                        if let Some(path) = msg.header().path() {
                                            let path_str = path.as_str();
                                            if let Some(device_id) = path_str
                                                .strip_prefix("/modules/kdeconnect/devices/")
                                            {
                                                // Extract the device_id (may contain more path components)
                                                let device_id = device_id
                                                    .split('/')
                                                    .next()
                                                    .unwrap_or(device_id);

                                                // Parse the message body to get SMS data
                                                let body = msg.body();
                                                if let Ok(value) =
                                                    body.deserialize::<zbus::zvariant::OwnedValue>()
                                                {
                                                    if let Some(sms_msg) = parse_sms_message(&value)
                                                    {
                                                        // Only notify for received messages (MessageType::Sent due to inversion)
                                                        // See CLAUDE.md for explanation of message type inversion
                                                        if sms_msg.message_type
                                                            == kdeconnect_dbus::plugins::MessageType::Sent
                                                        {
                                                            tracing::debug!(
                                                                "SMS received from {} on device {}: {}",
                                                                sms_msg.primary_address(),
                                                                device_id,
                                                                &sms_msg.body[..sms_msg.body.len().min(30)]
                                                            );
                                                            return Some((
                                                                Message::SmsNotificationReceived(
                                                                    device_id.to_string(),
                                                                    sms_msg,
                                                                ),
                                                                SmsSubscriptionState::Listening {
                                                                    conn,
                                                                    stream,
                                                                },
                                                            ));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!("D-Bus SMS stream error: {}", e);
                        }
                        None => {
                            tracing::warn!("D-Bus SMS stream ended, reconnecting...");
                            return Some((Message::RefreshDevices, SmsSubscriptionState::Init));
                        }
                    }
                }
            }
        }
    })
}

impl StandaloneApp {
    fn view_settings(&self) -> Element<'_, Message> {
        let mut settings_col = column![
            self.view_setting_toggle(
                fl!("settings-battery"),
                fl!("settings-battery-desc"),
                self.config.show_battery_percentage,
                SettingKey::ShowBatteryPercentage,
            ),
            self.view_setting_toggle(
                fl!("settings-offline"),
                fl!("settings-offline-desc"),
                self.config.show_offline_devices,
                SettingKey::ShowOfflineDevices,
            ),
            self.view_setting_toggle(
                fl!("settings-notifications"),
                fl!("settings-notifications-desc"),
                self.config.forward_notifications,
                SettingKey::ForwardNotifications,
            ),
            widget::divider::horizontal::default(),
            self.view_setting_toggle(
                fl!("settings-sms-notifications"),
                fl!("settings-sms-notifications-desc"),
                self.config.sms_notifications,
                SettingKey::SmsNotifications,
            ),
        ]
        .spacing(8)
        .padding(16);

        // Show sub-settings only when SMS notifications are enabled
        if self.config.sms_notifications {
            settings_col = settings_col
                .push(self.view_setting_toggle(
                    fl!("settings-sms-show-sender"),
                    fl!("settings-sms-show-sender-desc"),
                    self.config.sms_notification_show_sender,
                    SettingKey::SmsShowSender,
                ))
                .push(self.view_setting_toggle(
                    fl!("settings-sms-show-content"),
                    fl!("settings-sms-show-content-desc"),
                    self.config.sms_notification_show_content,
                    SettingKey::SmsShowContent,
                ));
        }

        widget::container(widget::scrollable(settings_col).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_setting_toggle(
        &self,
        title: String,
        description: String,
        enabled: bool,
        key: SettingKey,
    ) -> Element<'_, Message> {
        let toggle =
            widget::toggler(enabled).on_toggle(move |_| Message::ToggleSetting(key.clone()));

        let setting_row = row![
            column![text(title).size(14), text(description).size(11),].spacing(2),
            widget::horizontal_space(),
            toggle,
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        widget::container(setting_row)
            .padding(12)
            .width(Length::Fill)
            .into()
    }

    fn view_device_list(&self) -> Element<'_, Message> {
        let mut device_widgets: Vec<Element<Message>> = Vec::new();

        for device in &self.devices {
            // Filter out offline devices if setting is disabled
            if !device.is_reachable && device.is_paired && !self.config.show_offline_devices {
                continue;
            }

            let status_icon = if device.is_reachable && device.is_paired {
                "phone-symbolic"
            } else if device.is_paired {
                "phone-disconnect-symbolic"
            } else {
                "phone-symbolic"
            };

            let status_text = match (
                device.is_reachable,
                device.is_paired,
                device.is_pair_requested,
                device.is_pair_requested_by_peer,
            ) {
                (_, _, _, true) => fl!("pairing-request"),
                (_, _, true, _) => fl!("pairing"),
                (true, true, _, _) => fl!("connected"),
                (true, false, _, _) => fl!("available"),
                (false, true, _, _) => fl!("paired-offline"),
                (false, false, _, _) => fl!("discovered"),
            };

            // Only show battery if setting is enabled
            // KDE Connect returns -1 when battery level is unknown, so filter those out
            let battery_text = if self.config.show_battery_percentage {
                match (device.battery_level, device.battery_charging) {
                    (Some(level), Some(true)) if level >= 0 => {
                        format!("{}% {}", level, fl!("charging"))
                    }
                    (Some(level), Some(false)) if level >= 0 => format!("{}%", level),
                    (Some(level), None) if level >= 0 => format!("{}%", level),
                    _ => String::new(),
                }
            } else {
                String::new()
            };

            let device_info = column![
                text(&device.name).size(16),
                text(format!("{} - {}", device.device_type, status_text)).size(12),
            ]
            .spacing(4);

            // Create action buttons based on device state
            let action_buttons = self.create_device_actions(device);

            let mut device_row = row![
                widget::icon::from_name(status_icon).size(32),
                device_info,
                widget::horizontal_space(),
            ]
            .spacing(12)
            .align_y(Alignment::Center);

            // Add battery info if available
            if !battery_text.is_empty() {
                device_row = device_row.push(
                    row![
                        widget::icon::from_name("battery-good-symbolic").size(16),
                        text(battery_text).size(12),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );
            }

            // Add action buttons
            device_row = device_row.push(action_buttons);

            // Build device section with notifications
            let mut device_section = column![device_row].spacing(8);

            // Add notifications if setting is enabled and there are notifications
            if self.config.forward_notifications && !device.notifications.is_empty() {
                let notif_header = text(format!(
                    "{} ({})",
                    fl!("notifications"),
                    device.notifications.len()
                ))
                .size(12);
                device_section = device_section.push(notif_header);

                for notif in &device.notifications {
                    let notif_widget = self.view_notification(device, notif);
                    device_section = device_section.push(notif_widget);
                }
            }

            device_widgets.push(
                widget::container(device_section)
                    .padding(12)
                    .width(Length::Fill)
                    .into(),
            );
        }

        widget::container(
            widget::scrollable(
                column(device_widgets)
                    .spacing(8)
                    .padding(16)
                    .width(Length::Fill),
            )
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn view_notification<'a>(
        &'a self,
        device: &'a DeviceInfo,
        notif: &'a NotificationInfo,
    ) -> Element<'a, Message> {
        let notif_title = if notif.title.is_empty() {
            notif.app_name.clone()
        } else {
            format!("{}: {}", notif.app_name, notif.title)
        };

        let notif_content =
            column![text(notif_title).size(13), text(&notif.text).size(11),].spacing(2);

        let mut notif_row = row![
            widget::icon::from_name("notification-symbolic").size(20),
            notif_content,
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        // Add dismiss button if notification is dismissable
        if notif.dismissable {
            let device_id = device.id.clone();
            let notif_id = notif.id.clone();
            notif_row = notif_row.push(
                widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                    .on_press(Message::DismissNotification(device_id, notif_id)),
            );
        }

        widget::container(notif_row)
            .padding([4, 8])
            .width(Length::Fill)
            .into()
    }

    fn create_device_actions(&self, device: &DeviceInfo) -> Element<'_, Message> {
        let device_id = device.id.clone();

        // If peer requested pairing, show accept/reject buttons
        if device.is_pair_requested_by_peer {
            let accept_id = device_id.clone();
            let reject_id = device_id;
            return row![
                widget::button::suggested(fl!("accept"))
                    .on_press(Message::AcceptPairing(accept_id)),
                widget::button::destructive(fl!("reject"))
                    .on_press(Message::RejectPairing(reject_id)),
            ]
            .spacing(8)
            .into();
        }

        // If we requested pairing, show cancel button (uses reject/cancel)
        if device.is_pair_requested {
            return widget::button::standard(fl!("cancel"))
                .on_press(Message::RejectPairing(device_id))
                .into();
        }

        // If paired and connected, show action buttons
        if device.is_paired && device.is_reachable {
            let sms_id = device_id.clone();
            let sendto_id = device_id.clone();
            let sendto_type = device.device_type.clone();
            let media_id = device_id.clone();
            let unpair_id = device_id;
            return row![
                widget::button::standard(fl!("sms")).on_press(Message::OpenSmsView(sms_id)),
                widget::button::standard(fl!("send-to", device = sendto_type.as_str()))
                    .on_press(Message::OpenSendToView(sendto_id, sendto_type)),
                widget::button::standard(fl!("media")).on_press(Message::OpenMediaView(media_id)),
                widget::button::destructive(fl!("unpair")).on_press(Message::Unpair(unpair_id)),
            ]
            .spacing(8)
            .into();
        }

        // If paired but offline, show only unpair button
        if device.is_paired {
            return widget::button::destructive(fl!("unpair"))
                .on_press(Message::Unpair(device_id))
                .into();
        }

        // If reachable but not paired, show pair button
        if device.is_reachable {
            return widget::button::suggested(fl!("pair"))
                .on_press(Message::RequestPair(device_id))
                .into();
        }

        // Device not reachable and not paired - no actions available
        widget::text("").into()
    }

    /// View for the SMS conversation list.
    fn view_conversation_list(&self) -> Element<'_, Message> {
        let default_device = fl!("device");
        let device_name = self.sms_device_name.as_deref().unwrap_or(&default_device);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseSmsView),
            text(fl!("messages-title", device = device_name)).size(18),
            widget::horizontal_space(),
            widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                .on_press(Message::OpenNewMessage),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(16);

        let content: Element<Message> = if self.sms_loading {
            widget::container(
                column![text(fl!("loading-conversations")).size(14),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.conversations.is_empty() {
            widget::container(
                column![
                    widget::icon::from_name("mail-unread-symbolic").size(48),
                    text(fl!("no-conversations")).size(16),
                    text(fl!("sms-will-appear")).size(12),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            let mut conv_widgets: Vec<Element<Message>> = Vec::new();

            for conv in &self.conversations {
                conv_widgets.push(self.view_conversation_row(conv));
            }

            widget::container(
                widget::scrollable(
                    column(conv_widgets)
                        .spacing(4)
                        .padding(16)
                        .width(Length::Fill),
                )
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().anchor(scrollable::Anchor::Start),
                ))
                .id(widget::Id::new(format!(
                    "conversation-list-{}",
                    self.conversation_list_key
                )))
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        column![header, content]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// View for a single conversation row in the list.
    fn view_conversation_row<'a>(&'a self, conv: &'a ConversationSummary) -> Element<'a, Message> {
        let thread_id = conv.thread_id;

        // Use contact name if available, otherwise show phone number
        let display_name = self.contacts.get_name_or_number(conv.primary_address());

        // Truncate the message preview if too long (respecting char boundaries)
        let preview = if conv.last_message.chars().count() > 50 {
            let truncated: String = conv.last_message.chars().take(47).collect();
            format!("{}...", truncated)
        } else {
            conv.last_message.clone()
        };

        let time_str = format_relative_time(conv.timestamp);

        let unread_indicator: Element<'_, Message> = if conv.unread {
            widget::icon::from_name("mail-unread-symbolic")
                .size(16)
                .into()
        } else {
            widget::horizontal_space().width(16).into()
        };

        let conv_row = row![
            unread_indicator,
            column![
                row![
                    text(display_name).size(14),
                    widget::horizontal_space(),
                    text(time_str).size(11),
                ]
                .align_y(Alignment::Center),
                text(preview).size(12),
            ]
            .spacing(4)
            .width(Length::Fill),
            widget::icon::from_name("go-next-symbolic").size(16),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        widget::mouse_area(widget::container(conv_row).padding(12).width(Length::Fill))
            .on_press(Message::OpenConversation(thread_id))
            .into()
    }

    /// View for a message thread.
    fn view_message_thread(&self) -> Element<'_, Message> {
        // Pre-compute translated strings to extend their lifetimes
        let default_unknown = fl!("unknown");
        // Get the primary address from the addresses vector
        let address = self
            .current_thread_addresses
            .as_ref()
            .and_then(|addrs| addrs.first())
            .map(|s| s.as_str())
            .unwrap_or(&default_unknown);
        // Use contact name if available for the header
        let display_name = self.contacts.get_name_or_number(address);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseConversation),
            text(display_name).size(18),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(16);

        let content: Element<Message> = if self.sms_loading {
            widget::container(
                column![text(fl!("loading-messages")).size(14),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.messages.is_empty() {
            widget::container(
                column![text(fl!("no-messages-conversation")).size(14),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            let mut msg_widgets: Vec<Element<Message>> = Vec::new();

            for msg in &self.messages {
                msg_widgets.push(self.view_message_bubble(msg));
            }

            widget::container(
                widget::scrollable(
                    column(msg_widgets)
                        .spacing(12)
                        .padding(16)
                        .width(Length::Fill),
                )
                .id(widget::Id::new("message-thread"))
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        // Compose bar at bottom
        let can_send = !self.sms_compose_text.is_empty() && !self.sms_sending;
        let send_button = if self.sms_sending {
            widget::button::standard(fl!("sending"))
        } else {
            widget::button::suggested(fl!("send")).on_press_maybe(if can_send {
                Some(Message::SendSms)
            } else {
                None
            })
        };

        let compose_bar = row![
            widget::text_input(fl!("type-message"), &self.sms_compose_text)
                .on_input(Message::SmsComposeInput)
                .on_submit(|_| Message::SendSms)
                .width(Length::Fill),
            send_button,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(12);

        column![header, content, compose_bar]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// View for a single message bubble.
    fn view_message_bubble<'a>(&'a self, msg: &'a SmsMessage) -> Element<'a, Message> {
        let is_sent = msg.message_type == MessageType::Sent;
        let time_str = format_relative_time(msg.date);

        let bubble_content = column![text(&msg.body).size(14), text(time_str).size(10),].spacing(4);

        // Use different styling for sent vs received messages
        // Note: is_sent logic appears inverted, so we swap the styling too
        let bubble = if is_sent {
            // Actually received - Secondary styling
            widget::container(bubble_content)
                .padding(12)
                .max_width(MESSAGE_BUBBLE_MAX_WIDTH)
                .class(cosmic::theme::Container::Secondary)
        } else {
            // Actually sent - Primary styling
            widget::container(bubble_content)
                .padding(12)
                .max_width(MESSAGE_BUBBLE_MAX_WIDTH)
                .class(cosmic::theme::Container::Primary)
        };

        // Align sent messages to the right, received to the left
        // Note: is_sent logic appears inverted, so we swap the branches
        if is_sent {
            // Actually receiving - bubble on left, with sender name above
            let sender_name = self.contacts.get_name_or_number(msg.primary_address());
            column![
                text(sender_name).size(11),
                row![bubble, widget::horizontal_space()].width(Length::Fill),
            ]
            .spacing(4)
            .width(Length::Fill)
            .into()
        } else {
            // Actually sent - spacer pushes bubble to right
            row![widget::horizontal_space(), bubble]
                .width(Length::Fill)
                .into()
        }
    }

    /// View for composing a new message to a new recipient.
    fn view_new_message(&self) -> Element<'_, Message> {
        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseNewMessage),
            text(fl!("new-message")).size(18),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(16);

        // Recipient input with validation indicator
        let recipient_valid_icon: Element<Message> = if self.new_message_recipient.is_empty() {
            widget::horizontal_space().width(24).into()
        } else if self.new_message_recipient_valid {
            widget::icon::from_name("emblem-ok-symbolic")
                .size(20)
                .into()
        } else if !self.contact_suggestions.is_empty() {
            // Show search icon when there are suggestions
            widget::icon::from_name("edit-find-symbolic")
                .size(20)
                .into()
        } else {
            widget::icon::from_name("dialog-error-symbolic")
                .size(20)
                .into()
        };

        let recipient_row = row![
            text(fl!("to")).size(14).width(Length::Fixed(40.0)),
            widget::text_input(fl!("recipient-placeholder"), &self.new_message_recipient)
                .on_input(Message::NewMessageRecipientInput)
                .width(Length::Fill),
            recipient_valid_icon,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 16]);

        // Contact suggestions (shown when there are matches)
        let suggestions_section: Element<Message> = if !self.contact_suggestions.is_empty() {
            let mut suggestion_widgets: Vec<Element<Message>> = Vec::new();

            for contact in &self.contact_suggestions {
                // Use the first phone number from the contact
                if let Some(phone) = contact.phone_numbers.first() {
                    let name = contact.name.clone();
                    let phone_clone = phone.clone();

                    let suggestion_row = widget::button::custom(
                        row![
                            widget::icon::from_name("avatar-default-symbolic").size(24),
                            column![
                                text(name.clone()).size(13),
                                text(phone_clone.clone()).size(11),
                            ]
                            .spacing(2),
                        ]
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .padding([8, 12])
                        .width(Length::Fill),
                    )
                    .on_press(Message::SelectContact(name, phone_clone))
                    .width(Length::Fill)
                    .class(cosmic::theme::Button::MenuItem);

                    suggestion_widgets.push(suggestion_row.into());
                }
            }

            widget::container(column(suggestion_widgets).spacing(2).width(Length::Fill))
                .padding([0, 16])
                .into()
        } else {
            // Help text when no suggestions
            widget::container(
                text(fl!("recipient-placeholder"))
                    .size(11)
                    .width(Length::Fill),
            )
            .padding([0, 16])
            .into()
        };

        // Message body input
        let message_input = widget::container(
            widget::text_input(fl!("type-message"), &self.new_message_body)
                .on_input(Message::NewMessageBodyInput)
                .on_submit(|_| Message::SendNewMessage)
                .width(Length::Fill),
        )
        .padding([8, 16]);

        // Send button
        let can_send = self.new_message_recipient_valid
            && !self.new_message_body.is_empty()
            && !self.new_message_sending;

        let send_button = if self.new_message_sending {
            widget::button::standard(fl!("sending"))
        } else {
            widget::button::suggested(fl!("send")).on_press_maybe(if can_send {
                Some(Message::SendNewMessage)
            } else {
                None
            })
        };

        let send_row = widget::container(
            row![widget::horizontal_space(), send_button]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([8, 16]);

        // Main content area
        let content = column![
            recipient_row,
            suggestions_section,
            widget::vertical_space().height(Length::Fixed(16.0)),
            message_input,
            send_row,
            widget::vertical_space(),
        ]
        .spacing(8)
        .width(Length::Fill);

        column![header, content]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// View for the "Send to device" submenu.
    fn view_send_to(&self) -> Element<'_, Message> {
        let device_type = self.sendto_device_type.as_deref().unwrap_or("device");
        let device_id = self.sendto_device_id.clone().unwrap_or_default();

        // Header with back button
        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::BackFromSendTo),
            text(fl!("send-to-title", device = device_type)).size(18),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 16]);

        // Action buttons
        let device_id_for_file = device_id.clone();
        let device_id_for_clipboard = device_id.clone();
        let device_id_for_ping = device_id.clone();
        let device_id_for_text = device_id.clone();
        let text_to_share = self.share_text_input.clone();

        let share_file_btn = widget::button::standard(fl!("share-file"))
            .leading_icon(widget::icon::from_name("document-send-symbolic").size(16))
            .on_press(Message::ShareFile(device_id_for_file));

        let send_clipboard_btn = widget::button::standard(fl!("share-clipboard"))
            .leading_icon(widget::icon::from_name("edit-copy-symbolic").size(16))
            .on_press(Message::SendClipboard(device_id_for_clipboard));

        let send_ping_btn = widget::button::standard(fl!("send-ping"))
            .leading_icon(widget::icon::from_name("emblem-ok-symbolic").size(16))
            .on_press(Message::SendPing(device_id_for_ping));

        // Share text section
        let share_text_heading = text(fl!("share-text")).size(14);

        let share_text_input =
            widget::text_input(fl!("share-text-placeholder"), &self.share_text_input)
                .on_input(Message::ShareTextInput)
                .width(Length::Fill);

        let send_text_btn = widget::button::standard(fl!("send-text"))
            .leading_icon(widget::icon::from_name("edit-paste-symbolic").size(16))
            .on_press_maybe(if self.share_text_input.is_empty() {
                None
            } else {
                Some(Message::ShareText(device_id_for_text, text_to_share))
            });

        // Status message if present
        let status_bar: Element<Message> = if let Some(msg) = &self.status_message {
            widget::container(text(msg).size(12))
                .padding([4, 16])
                .width(Length::Fill)
                .into()
        } else {
            widget::Space::new(Length::Shrink, Length::Shrink).into()
        };

        let content = column![
            share_file_btn,
            send_clipboard_btn,
            send_ping_btn,
            widget::divider::horizontal::default(),
            share_text_heading,
            share_text_input,
            send_text_btn,
        ]
        .spacing(12)
        .padding([0, 16]);

        column![
            header,
            status_bar,
            widget::divider::horizontal::default(),
            content,
        ]
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// View for media player controls.
    fn view_media_controls(&self) -> Element<'_, Message> {
        let default_device = fl!("device");
        let device_name = self.media_device_name.as_deref().unwrap_or(&default_device);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseMediaView),
            text(format!("{} - {}", fl!("media"), device_name)).size(18),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(16);

        let content: Element<Message> = if self.media_loading {
            widget::container(
                column![text(fl!("loading-media")).size(14),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if let Some(ref info) = self.media_info {
            if info.players.is_empty() {
                // No active media players
                widget::container(
                    column![
                        widget::icon::from_name("multimedia-player-symbolic").size(48),
                        text(fl!("no-media-players")).size(16),
                        text(fl!("start-playing")).size(12),
                    ]
                    .spacing(12)
                    .align_x(Alignment::Center),
                )
                .center(Length::Fill)
                .into()
            } else {
                // Show media controls
                self.view_media_player(info)
            }
        } else {
            // Error or no media plugin
            widget::container(
                column![
                    widget::icon::from_name("dialog-error-symbolic").size(48),
                    text(fl!("media-not-available")).size(16),
                    text(fl!("enable-mpris")).size(12),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        };

        column![header, content]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// View for the media player with controls.
    fn view_media_player(&self, info: &MediaInfo) -> Element<'_, Message> {
        // Player selector (if multiple players)
        let player_selector: Element<Message> = if info.players.len() > 1 {
            // Clone player list for use in widget
            let players: Vec<String> = info.players.clone();
            let selected_idx = players.iter().position(|p| p == &info.current_player);

            // Store player names for the closure
            let players_for_dropdown: Vec<String> = players.clone();

            widget::container(
                row![
                    text(fl!("player")).size(12),
                    widget::dropdown(players, selected_idx, move |idx| {
                        Message::MediaSelectPlayer(players_for_dropdown[idx].clone())
                    })
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([0, 16])
            .into()
        } else {
            widget::container(text(info.current_player.clone()).size(12))
                .padding([0, 16])
                .into()
        };

        // Track info - use owned strings
        let title_text = if info.title.is_empty() {
            fl!("no-track-playing")
        } else {
            info.title.clone()
        };
        let artist_text = if info.artist.is_empty() {
            "-".to_string()
        } else {
            info.artist.clone()
        };
        let album_text = if info.album.is_empty() {
            String::new()
        } else {
            info.album.clone()
        };

        let track_info = column![
            text(title_text).size(18),
            text(artist_text).size(14),
            text(album_text).size(12),
        ]
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fill);

        // Position display - use owned strings
        let position_str = format_duration(info.position);
        let length_str = format_duration(info.length);
        let position_display = row![
            text(position_str).size(11),
            widget::horizontal_space(),
            text(length_str).size(11),
        ]
        .padding([0, 16]);

        // Playback controls
        let play_icon = if info.is_playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        };

        let prev_button =
            widget::button::icon(widget::icon::from_name("media-skip-backward-symbolic"))
                .on_press_maybe(if info.can_previous {
                    Some(Message::MediaPrevious)
                } else {
                    None
                });

        let play_button = widget::button::icon(widget::icon::from_name(play_icon))
            .on_press(Message::MediaPlayPause);

        let next_button =
            widget::button::icon(widget::icon::from_name("media-skip-forward-symbolic"))
                .on_press_maybe(if info.can_next {
                    Some(Message::MediaNext)
                } else {
                    None
                });

        let playback_controls = row![prev_button, play_button, next_button,]
            .spacing(16)
            .align_y(Alignment::Center);

        let controls_container = widget::container(playback_controls)
            .width(Length::Fill)
            .align_x(Alignment::Center);

        // Volume control
        let volume_icon = if info.volume == 0 {
            "audio-volume-muted-symbolic"
        } else if info.volume < 33 {
            "audio-volume-low-symbolic"
        } else if info.volume < 66 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };

        let volume_slider = widget::slider(0..=100, info.volume, Message::MediaSetVolume);

        let volume_row = row![
            widget::icon::from_name(volume_icon).size(20),
            volume_slider,
            text(format!("{}%", info.volume))
                .size(11)
                .width(Length::Fixed(40.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 16]);

        // Assemble the view
        column![
            player_selector,
            widget::vertical_space().height(Length::Fixed(24.0)),
            widget::container(widget::icon::from_name("multimedia-player-symbolic").size(64))
                .width(Length::Fill)
                .align_x(Alignment::Center),
            widget::vertical_space().height(Length::Fixed(16.0)),
            widget::container(track_info).padding([0, 16]),
            widget::vertical_space().height(Length::Fixed(24.0)),
            position_display,
            widget::vertical_space().height(Length::Fixed(16.0)),
            controls_container,
            widget::vertical_space().height(Length::Fixed(24.0)),
            volume_row,
            widget::vertical_space(),
        ]
        .spacing(8)
        .width(Length::Fill)
        .into()
    }
}

/// Run the standalone window application.
pub fn run() -> cosmic::iced::Result {
    let settings = Settings::default()
        .size(cosmic::iced::Size::new(700.0, 650.0))
        .debug(false);

    cosmic::app::run::<StandaloneApp>(settings, ())
}

/// Fetch all devices from the KDE Connect daemon via D-Bus.
async fn fetch_devices_async(conn: Arc<Mutex<Connection>>) -> Message {
    let conn = conn.lock().await;

    let daemon = match DaemonProxy::new(&conn).await {
        Ok(d) => d,
        Err(e) => {
            return Message::Error(format!("Failed to connect to KDE Connect daemon: {}", e));
        }
    };

    let device_ids = match daemon.devices().await {
        Ok(ids) => ids,
        Err(e) => {
            return Message::Error(format!("Failed to get device list: {}", e));
        }
    };

    tracing::debug!("Found {} device(s)", device_ids.len());

    let mut devices = Vec::new();
    for device_id in device_ids {
        match fetch_device_info(&conn, &device_id).await {
            Ok(info) => devices.push(info),
            Err(e) => {
                tracing::warn!("Failed to get info for device {}: {}", device_id, e);
            }
        }
    }

    Message::DevicesUpdated(devices)
}

/// Fetch information for a single device.
async fn fetch_device_info(conn: &Connection, device_id: &str) -> Result<DeviceInfo, String> {
    let device = DeviceProxy::for_device(conn, device_id)
        .await
        .map_err(|e| e.to_string())?;

    let id = device_id.to_string();
    let name = device.name().await.map_err(|e| e.to_string())?;
    let device_type = device
        .device_type()
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    let is_reachable = device.is_reachable().await.unwrap_or(false);
    let is_paired = device.is_trusted().await.unwrap_or(false);
    let is_pair_requested = device.is_pair_requested().await.unwrap_or(false);
    let is_pair_requested_by_peer = device.is_pair_requested_by_peer().await.unwrap_or(false);

    let (battery_level, battery_charging) = if is_reachable && is_paired {
        fetch_battery_info(conn, device_id).await
    } else {
        (None, None)
    };

    let notifications = if is_reachable && is_paired {
        fetch_notifications(conn, device_id).await
    } else {
        Vec::new()
    };

    Ok(DeviceInfo {
        id,
        name,
        device_type,
        is_reachable,
        is_paired,
        is_pair_requested,
        is_pair_requested_by_peer,
        battery_level,
        battery_charging,
        notifications,
    })
}

/// Fetch battery information for a device.
async fn fetch_battery_info(conn: &Connection, device_id: &str) -> (Option<i32>, Option<bool>) {
    let path = format!(
        "{}/devices/{}/battery",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    tracing::debug!("Fetching battery info from path: {}", path);

    let builder = match BatteryProxy::builder(conn).path(path.as_str()) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to create battery proxy builder: {}", e);
            return (None, None);
        }
    };

    let battery = match builder.build().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to build battery proxy: {}", e);
            return (None, None);
        }
    };

    let charge = match battery.charge().await {
        Ok(c) => {
            tracing::debug!("Battery charge: {}", c);
            Some(c)
        }
        Err(e) => {
            tracing::warn!("Failed to get battery charge: {}", e);
            None
        }
    };

    let is_charging = match battery.is_charging().await {
        Ok(c) => {
            tracing::debug!("Battery is_charging: {}", c);
            Some(c)
        }
        Err(e) => {
            tracing::warn!("Failed to get is_charging: {}", e);
            None
        }
    };

    (charge, is_charging)
}

/// Request pairing with a device.
async fn request_pair_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.request_pair().await {
        Ok(()) => Message::PairingResult(Ok(
            "Pairing request sent. Please accept on your device.".to_string()
        )),
        Err(e) => Message::PairingResult(Err(format!("Failed to request pairing: {}", e))),
    }
}

/// Unpair from a device.
async fn unpair_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.unpair().await {
        Ok(()) => Message::PairingResult(Ok("Device unpaired successfully.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to unpair: {}", e))),
    }
}

/// Accept an incoming pairing request.
async fn accept_pairing_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.accept_pairing().await {
        Ok(()) => Message::PairingResult(Ok("Pairing accepted.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to accept pairing: {}", e))),
    }
}

/// Reject an incoming pairing request or cancel outgoing request.
async fn reject_pairing_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.reject_pairing().await {
        Ok(()) => Message::PairingResult(Ok("Pairing rejected/cancelled.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to reject pairing: {}", e))),
    }
}

/// Send a ping to a device.
async fn send_ping_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let path = format!("{}/devices/{}/ping", kdeconnect_dbus::BASE_PATH, device_id);

    let ping = match PingProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::PingResult(Err(format!("Failed to create ping proxy: {}", e)));
            }
        },
        None => {
            return Message::PingResult(Err("Failed to build ping proxy path".to_string()));
        }
    };

    match ping.send_ping().await {
        Ok(()) => Message::PingResult(Ok("Ping sent!".to_string())),
        Err(e) => Message::PingResult(Err(format!("Failed to send ping: {}", e))),
    }
}

/// Fetch notifications for a device.
async fn fetch_notifications(conn: &Connection, device_id: &str) -> Vec<NotificationInfo> {
    let notifications_path = format!(
        "{}/devices/{}/notifications",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    // Get the notifications proxy
    let notifications_proxy = match NotificationsProxy::builder(conn)
        .path(notifications_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to create notifications proxy: {}", e);
                return Vec::new();
            }
        },
        None => {
            tracing::warn!("Failed to build notifications proxy path");
            return Vec::new();
        }
    };

    // Get list of active notification IDs
    let notification_ids = match notifications_proxy.active_notifications().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("Failed to get active notifications: {}", e);
            return Vec::new();
        }
    };

    tracing::debug!(
        "Found {} notifications for device {}",
        notification_ids.len(),
        device_id
    );

    // Fetch info for each notification
    let mut notifications = Vec::new();
    for notif_id in notification_ids {
        let notif_path = format!(
            "{}/devices/{}/notifications/{}",
            kdeconnect_dbus::BASE_PATH,
            device_id,
            notif_id
        );

        let notif_proxy = match NotificationProxy::builder(conn)
            .path(notif_path.as_str())
            .ok()
            .map(|b| b.build())
        {
            Some(fut) => match fut.await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "Failed to create notification proxy for {}: {}",
                        notif_id,
                        e
                    );
                    continue;
                }
            },
            None => continue,
        };

        let app_name = notif_proxy.app_name().await.unwrap_or_default();
        let title = notif_proxy.title().await.unwrap_or_default();
        let text = notif_proxy.text().await.unwrap_or_default();
        let dismissable = notif_proxy.dismissable().await.unwrap_or(false);
        let reply_id = notif_proxy.reply_id().await.unwrap_or_default();

        notifications.push(NotificationInfo {
            id: notif_id,
            app_name,
            title,
            text,
            dismissable,
            repliable: !reply_id.is_empty(),
        });
    }

    notifications
}

/// Dismiss a notification on a device.
async fn dismiss_notification_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    notification_id: String,
) -> Message {
    let conn = conn.lock().await;

    let notif_path = format!(
        "{}/devices/{}/notifications/{}",
        kdeconnect_dbus::BASE_PATH,
        device_id,
        notification_id
    );

    let notif_proxy = match NotificationProxy::builder(&conn)
        .path(notif_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::DismissResult(Err(format!(
                    "Failed to create notification proxy: {}",
                    e
                )));
            }
        },
        None => {
            return Message::DismissResult(Err(
                "Failed to build notification proxy path".to_string()
            ));
        }
    };

    match notif_proxy.dismiss().await {
        Ok(()) => Message::DismissResult(Ok("Notification dismissed".to_string())),
        Err(e) => Message::DismissResult(Err(format!("Failed to dismiss: {}", e))),
    }
}

/// Share a file to a device.
async fn share_file_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    path: PathBuf,
) -> Message {
    let conn = conn.lock().await;

    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = match ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::ShareResult(Err(format!("Failed to create share proxy: {}", e)));
            }
        },
        None => {
            return Message::ShareResult(Err("Failed to build share proxy path".to_string()));
        }
    };

    let url = format!("file://{}", path.display());
    match share.share_url(&url).await {
        Ok(()) => Message::ShareResult(Ok(format!(
            "File sent: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))),
        Err(e) => Message::ShareResult(Err(format!("Failed to share file: {}", e))),
    }
}

/// Share text to a device.
async fn share_text_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    text: String,
) -> Message {
    let conn = conn.lock().await;

    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = match ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::ShareResult(Err(format!("Failed to create share proxy: {}", e)));
            }
        },
        None => {
            return Message::ShareResult(Err("Failed to build share proxy path".to_string()));
        }
    };

    match share.share_text(&text).await {
        Ok(()) => Message::ShareResult(Ok("Text sent to clipboard!".to_string())),
        Err(e) => Message::ShareResult(Err(format!("Failed to share text: {}", e))),
    }
}

/// Fetch SMS conversations from a device.
async fn fetch_conversations_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    // The conversations interface is on the device path, not /sms
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

    // Request the phone to send all conversation threads
    if let Err(e) = conversations_proxy.request_all_conversation_threads().await {
        tracing::warn!("Failed to request conversation threads: {}", e);
        // Continue anyway, conversations may already be cached
    }

    // Brief delay to allow the phone to respond
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    match conversations_proxy.active_conversations().await {
        Ok(values) => {
            tracing::info!("Received {} conversation values from D-Bus", values.len());
            let conversations = parse_conversations(values);
            tracing::info!("Parsed {} conversations", conversations.len());
            Message::ConversationsLoaded(conversations)
        }
        Err(e) => Message::SmsError(format!("Failed to get conversations: {}", e)),
    }
}

/// Fetch messages for a specific conversation thread using D-Bus signals.
async fn fetch_messages_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    messages_per_page: u32,
) -> Message {
    use kdeconnect_dbus::plugins::parse_sms_message;
    use std::collections::HashMap;

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

    // Request the specific conversation (start=0, end=messages_per_page to get recent messages)
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

    // Collect messages from signals until conversationLoaded or timeout
    let mut messages_map: HashMap<i64, SmsMessage> = HashMap::new();
    let timeout = tokio::time::Duration::from_secs(10);
    let start_time = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Check for conversationUpdated signals
            Some(signal) = updated_stream.next() => {
                match signal.args() {
                    Ok(args) => {
                        if let Some(msg) = parse_sms_message(&args.msg) {
                            if msg.thread_id == thread_id {
                                // Use date as key to deduplicate
                                messages_map.insert(msg.date, msg);
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
                            tracing::info!(
                                "Conversation {} loaded, expected {} messages, got {}",
                                thread_id,
                                args.message_count,
                                messages_map.len()
                            );
                            // Drain any remaining buffered conversationUpdated signals
                            // These may have arrived but not yet been processed
                            'drain: loop {
                                tokio::select! {
                                    biased;
                                    Some(signal) = updated_stream.next() => {
                                        if let Ok(args) = signal.args() {
                                            if let Some(msg) = parse_sms_message(&args.msg) {
                                                if msg.thread_id == thread_id {
                                                    messages_map.insert(msg.date, msg);
                                                    tracing::debug!(
                                                        "Drained message, total: {}",
                                                        messages_map.len()
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(5)) => {
                                        // No more signals available, done draining
                                        break 'drain;
                                    }
                                }
                            }
                            tracing::info!(
                                "After drain: {} messages for thread {}",
                                messages_map.len(),
                                thread_id
                            );
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
                    "Timeout waiting for messages, got {} messages",
                    messages_map.len()
                );
                break;
            }
        }
    }

    // Convert map to sorted vector
    let mut messages: Vec<SmsMessage> = messages_map.into_values().collect();
    messages.sort_by(|a, b| a.date.cmp(&b.date));

    tracing::info!(
        "Final: Loaded {} messages for thread {}",
        messages.len(),
        thread_id
    );
    Message::MessagesLoaded(thread_id, messages)
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

    // Simple polling fallback
    let mut messages = Vec::new();
    for attempt in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        match conversations_proxy.active_conversations().await {
            Ok(values) => {
                messages = parse_messages(values, thread_id);
                tracing::info!(
                    "Fallback attempt {}: Found {} messages for thread {}",
                    attempt + 1,
                    messages.len(),
                    thread_id
                );
                if messages.len() > 1 {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get messages on attempt {}: {}", attempt + 1, e);
            }
        }
    }

    Message::MessagesLoaded(thread_id, messages)
}

/// Send an SMS reply to a conversation thread using the SMS plugin directly.
/// This bypasses the daemon's conversation cache and sends to all recipients
/// for proper group message support.
async fn send_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    _thread_id: i64,
    recipients: Vec<String>,
    message: String,
) -> Message {
    use zbus::zvariant::{Structure, Value};

    let conn = conn.lock().await;
    let sms_path = format!("{}/devices/{}/sms", kdeconnect_dbus::BASE_PATH, device_id);

    let sms_proxy = match SmsProxy::builder(&conn)
        .path(sms_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::SmsSendResult(Err(format!("Failed to create SMS proxy: {}", e)));
            }
        },
        None => {
            return Message::SmsSendResult(Err("Failed to build SMS proxy path".to_string()));
        }
    };

    // Format ALL addresses as D-Bus structs for group message support
    // KDE Connect expects addresses as array of structs: a(s)
    let addresses: Vec<Value<'_>> = recipients
        .iter()
        .map(|addr| Value::Structure(Structure::from((addr.as_str(),))))
        .collect();

    // Send using the SMS plugin directly with subID=-1 (default SIM)
    let empty_attachments: Vec<Value<'_>> = vec![];
    match sms_proxy
        .send_sms(addresses, &message, empty_attachments, -1)
        .await
    {
        Ok(()) => Message::SmsSendResult(Ok("Message sent".to_string())),
        Err(e) => Message::SmsSendResult(Err(format!("Send failed: {}", e))),
    }
}

/// Send an SMS to a new recipient (creates or adds to existing conversation).
async fn send_new_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    recipient: String,
    message: String,
) -> Message {
    use zbus::zvariant::{Structure, Value};

    let conn = conn.lock().await;
    let device_path = format!("{}/devices/{}", kdeconnect_dbus::BASE_PATH, device_id);

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

    // Format addresses as D-Bus struct containing the phone number
    // KDE Connect expects addresses as array of structs: a(s)
    let address_struct = Structure::from((recipient.as_str(),));
    let addresses: Vec<Value<'_>> = vec![Value::Structure(address_struct)];
    let empty_attachments: Vec<Value<'_>> = vec![];

    match conversations_proxy
        .send_without_conversation(addresses, &message, empty_attachments)
        .await
    {
        Ok(()) => Message::NewMessageSendResult(Ok("Message sent".to_string())),
        Err(e) => Message::NewMessageSendResult(Err(format!("Send failed: {}", e))),
    }
}

/// Send the current local clipboard content to a device.
async fn send_clipboard_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/clipboard",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let clipboard = match ClipboardProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(c) => c,
            Err(e) => {
                return Message::ClipboardResult(Err(format!("Failed to create proxy: {}", e)))
            }
        },
        None => return Message::ClipboardResult(Err("Failed to build proxy path".to_string())),
    };

    match clipboard.send_clipboard().await {
        Ok(()) => Message::ClipboardResult(Ok("Clipboard sent to device".to_string())),
        Err(e) => Message::ClipboardResult(Err(format!("Failed to send clipboard: {}", e))),
    }
}

/// Format a Unix timestamp (in milliseconds) as a relative time string.
fn format_relative_time(timestamp_ms: i64) -> String {
    use chrono::{DateTime, Local, Utc};

    // Convert milliseconds to seconds for chrono
    let timestamp_secs = timestamp_ms / 1000;
    let datetime = DateTime::<Utc>::from_timestamp(timestamp_secs, 0);

    let Some(datetime) = datetime else {
        return "Unknown".to_string();
    };

    let now = Utc::now();
    let diff = now.signed_duration_since(datetime);

    let diff_mins = diff.num_minutes();
    let diff_hours = diff.num_hours();
    let diff_days = diff.num_days();

    if diff_mins < 1 {
        "Just now".to_string()
    } else if diff_mins < 60 {
        format!("{}m ago", diff_mins)
    } else if diff_hours < 24 {
        format!("{}h ago", diff_hours)
    } else if diff_days == 1 {
        "Yesterday".to_string()
    } else if diff_days < 7 {
        format!("{}d ago", diff_days)
    } else {
        // Format as date for older messages using local timezone
        let local: DateTime<Local> = datetime.into();
        local.format("%-m/%-d/%Y").to_string()
    }
}

/// Media control action types.
enum MediaAction {
    PlayPause,
    Next,
    Previous,
    SetVolume(i32),
    SelectPlayer(String),
}

/// Fetch media information from a device.
async fn fetch_media_info_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/mprisremote",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let proxy = match MprisRemoteProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("Failed to create mpris proxy: {}", e);
                return Message::MediaInfoLoaded(None);
            }
        },
        None => {
            tracing::debug!("Failed to build mpris proxy path");
            return Message::MediaInfoLoaded(None);
        }
    };

    // Fetch all properties
    let players = proxy.player_list().await.unwrap_or_default();
    let current_player = proxy.player().await.unwrap_or_default();
    let title = proxy.title().await.unwrap_or_default();
    let artist = proxy.artist().await.unwrap_or_default();
    let album = proxy.album().await.unwrap_or_default();
    let is_playing = proxy.is_playing().await.unwrap_or(false);
    let volume = proxy.volume().await.unwrap_or(0);
    let position = proxy.position().await.unwrap_or(0);
    let length = proxy.length().await.unwrap_or(0);
    // Note: canGoNext/canGoPrevious are per-player properties not exposed on the main interface.
    // We default to true to allow actions; the phone will handle if unsupported.
    let can_next = true;
    let can_previous = true;

    Message::MediaInfoLoaded(Some(MediaInfo {
        players,
        current_player,
        title,
        artist,
        album,
        is_playing,
        volume,
        position,
        length,
        can_next,
        can_previous,
    }))
}

/// Execute a media control action on a device.
async fn media_action_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    action: MediaAction,
) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/mprisremote",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let proxy = match MprisRemoteProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::MediaActionResult(Err(format!("Failed to create proxy: {}", e)));
            }
        },
        None => {
            return Message::MediaActionResult(Err("Failed to build proxy path".to_string()));
        }
    };

    let result = match action {
        MediaAction::PlayPause => proxy.send_action("PlayPause").await,
        MediaAction::Next => proxy.send_action("Next").await,
        MediaAction::Previous => proxy.send_action("Previous").await,
        MediaAction::SetVolume(vol) => proxy.set_volume(vol).await,
        MediaAction::SelectPlayer(player) => proxy.set_player(&player).await,
    };

    match result {
        Ok(()) => Message::MediaActionResult(Ok("OK".to_string())),
        Err(e) => Message::MediaActionResult(Err(format!("Action failed: {}", e))),
    }
}

/// Format milliseconds as mm:ss time string.
fn format_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0:00".to_string();
    }
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{}:{:02}", minutes, seconds)
}
