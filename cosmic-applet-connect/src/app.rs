//! Main application state and logic for the COSMIC Connect applet.

use crate::config::Config;
use crate::fl;
use crate::ui;
use cosmic::app::Core;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::{column, row, scrollable, text};
use cosmic::iced::{Alignment, Subscription};
use cosmic::iced::{Color, Length};
use cosmic::iced_core::layout::Limits;
use cosmic::iced_core::Shadow;
use cosmic::iced_runtime::core::window;
use cosmic::widget;
use cosmic::widget::autosize::autosize;
use cosmic::widget::container::Container;
use cosmic::{Application, Element, Renderer};

/// Default popup width (matches libcosmic default).
const DEFAULT_POPUP_WIDTH: f32 = 360.0;

/// Wide popup width for SMS/media views that need more space.
const WIDE_POPUP_WIDTH: f32 = 450.0;

/// Maximum height of the popup window in pixels.
const POPUP_MAX_HEIGHT: f32 = 1000.0;
use futures_util::StreamExt;
use kdeconnect_dbus::{
    contacts::{Contact, ContactLookup},
    plugins::{
        is_address_valid, parse_conversations, parse_messages, BatteryProxy, ClipboardProxy,
        ConversationSummary, ConversationsProxy, MessageType, MprisRemoteProxy, NotificationInfo,
        NotificationProxy, NotificationsProxy, PingProxy, ShareProxy, SmsMessage,
    },
    DaemonProxy, DeviceProxy,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Messages that drive the applet's state changes.
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // NewMessage variants refer to SMS, not the enum
pub enum Message {
    /// Toggle the popup visibility
    TogglePopup,
    /// Popup was closed
    PopupClosed(window::Id),
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

    // Navigation
    /// Select a device to view its detail page
    SelectDevice(String),
    /// Return to the device list
    BackToList,
    /// Open the "Send to device" submenu
    OpenSendToView(String, String), // device_id, device_type
    /// Return from SendTo view to device page
    BackFromSendTo,

    // Ping actions
    /// Send a ping to a device
    SendPing(String),
    /// Ping operation completed
    PingComplete(Result<(), String>),

    // Share actions
    /// Initiate file sharing (opens file picker)
    ShareFile(String),
    /// File was selected from picker
    FileSelected(Option<PathBuf>),
    /// Initiate text sharing
    ShareText(String, String),
    /// Share operation completed
    ShareComplete(Result<(), String>),
    /// Update the text input for sharing
    ShareTextInput(String),
    /// Configuration changed (from file watcher or external source)
    ConfigChanged(Config),

    // Pairing actions
    /// Request pairing with a device
    RequestPair(String),
    /// Unpair from a device
    Unpair(String),
    /// Accept incoming pairing request
    AcceptPairing(String),
    /// Reject/cancel pairing request
    RejectPairing(String),
    /// Pairing operation completed
    PairingResult(Result<String, String>),
    /// D-Bus signal received indicating device state changed
    DbusSignalReceived,

    // Notification actions
    /// Dismiss a notification on a device
    DismissNotification(String, String), // device_id, notification_id
    /// Notification dismiss result
    DismissResult(Result<String, String>),

    // Clipboard actions
    /// Send current desktop clipboard to device
    SendClipboard(String), // device_id
    /// Clipboard operation completed
    ClipboardResult(Result<String, String>),

    // Settings
    /// Toggle the settings view
    ToggleSettings,
    /// Toggle a specific setting
    ToggleSetting(SettingKey),

    // SMS
    /// Open SMS view for a device
    OpenSmsView(String),
    /// Close SMS view and return to device list
    CloseSmsView,
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
    /// Open new message compose view
    OpenNewMessage,
    /// Close new message view
    CloseNewMessage,
    /// Update new message recipient input
    NewMessageRecipientInput(String),
    /// Update new message body input
    NewMessageBodyInput(String),
    /// Select a contact from suggestions
    SelectContact(String, String), // name, phone
    /// Send a new message
    SendNewMessage,
    /// New message send result
    NewMessageSendResult(Result<String, String>),

    // Media controls
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

    // SMS Notifications
    /// New SMS received via D-Bus signal (device_id, message)
    SmsNotificationReceived(String, SmsMessage),
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

/// View mode for the applet popup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ViewMode {
    /// Main device list view
    #[default]
    DeviceList,
    /// Individual device detail page
    DevicePage,
    /// Send to device submenu (file, clipboard, ping, text)
    SendTo,
    /// SMS conversation list for a device
    ConversationList,
    /// SMS message thread view
    MessageThread,
    /// New message compose view
    NewMessage,
    /// Settings view
    Settings,
    /// Media player controls
    MediaControls,
}

/// The main applet state.
pub struct ConnectApplet {
    core: Core,
    config: Config,
    popup: Option<window::Id>,
    devices: Vec<DeviceInfo>,
    error: Option<String>,
    /// Status message for user feedback (e.g., "Ping sent", "Pairing failed")
    status_message: Option<String>,
    /// D-Bus connection (shared for async operations)
    dbus_connection: Option<Arc<Mutex<Connection>>>,
    /// Whether we're currently fetching devices
    loading: bool,
    /// Current view mode
    view_mode: ViewMode,
    /// Currently selected device ID (for device page navigation)
    selected_device: Option<String>,
    /// Device ID awaiting file selection from file picker
    pending_share_device: Option<String>,
    /// Text input for sharing
    share_text_input: String,
    /// Timestamp of last D-Bus signal refresh (for debouncing)
    last_signal_refresh: std::time::Instant,

    // SMS state
    /// Device ID currently viewing SMS for
    sms_device_id: Option<String>,
    /// Device name for SMS view header
    sms_device_name: Option<String>,
    /// List of conversations for current device
    conversations: Vec<ConversationSummary>,
    /// Current conversation thread ID being viewed
    current_thread_id: Option<i64>,
    /// Current conversation address (for header)
    current_thread_address: Option<String>,
    /// Messages in the current thread
    messages: Vec<SmsMessage>,
    /// Whether SMS data is currently loading
    sms_loading: bool,
    /// Contact lookup for resolving phone numbers to names
    contacts: ContactLookup,
    /// Key to reset conversation list scroll position
    conversation_list_key: u32,
    /// Text input for composing SMS reply
    sms_compose_text: String,
    /// Whether SMS is currently being sent
    sms_sending: bool,
    /// Cache of messages by thread_id for faster loading
    message_cache: HashMap<i64, Vec<SmsMessage>>,

    // New message compose state
    /// Recipient input for new message
    new_message_recipient: String,
    /// Body input for new message
    new_message_body: String,
    /// Whether the recipient is valid
    new_message_recipient_valid: bool,
    /// Whether new message is being sent
    new_message_sending: bool,
    /// Contact suggestions for new message
    contact_suggestions: Vec<Contact>,

    // Media controls state
    /// Device ID for media controls view
    media_device_id: Option<String>,
    /// Device name for media controls header
    media_device_name: Option<String>,
    /// Current media playback info
    media_info: Option<MediaInfo>,
    /// Whether media info is loading
    media_loading: bool,
    /// User's explicit player selection (overrides D-Bus value until view is closed)
    media_selected_player: Option<String>,

    // SendTo submenu state
    /// Device ID for SendTo view
    sendto_device_id: Option<String>,
    /// Device type for SendTo view header (e.g., "phone", "tablet")
    sendto_device_type: Option<String>,

    // SMS notification deduplication
    /// Last seen SMS timestamp per thread_id to avoid duplicate notifications
    last_seen_sms: HashMap<i64, i64>,
}

impl Application for ConnectApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.cosmic-connect-applet";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, cosmic::app::Task<Self::Message>) {
        // Load config from disk or use defaults
        let config = Config::load();

        let app = ConnectApplet {
            core,
            config,
            popup: None,
            devices: Vec::new(),
            error: None,
            status_message: None,
            dbus_connection: None,
            loading: true,
            view_mode: ViewMode::DeviceList,
            selected_device: None,
            pending_share_device: None,
            share_text_input: String::new(),
            last_signal_refresh: std::time::Instant::now(),
            // SMS state
            sms_device_id: None,
            sms_device_name: None,
            conversations: Vec::new(),
            current_thread_id: None,
            current_thread_address: None,
            messages: Vec::new(),
            sms_loading: false,
            contacts: ContactLookup::default(),
            conversation_list_key: 0,
            sms_compose_text: String::new(),
            sms_sending: false,
            message_cache: HashMap::new(),
            // New message state
            new_message_recipient: String::new(),
            new_message_body: String::new(),
            new_message_recipient_valid: false,
            new_message_sending: false,
            contact_suggestions: Vec::new(),
            // Media controls state
            media_device_id: None,
            media_device_name: None,
            media_info: None,
            media_loading: false,
            media_selected_player: None,
            // SendTo state
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

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                return if let Some(popup_id) = self.popup.take() {
                    destroy_popup(popup_id)
                } else {
                    let new_id = window::Id::unique();
                    self.popup.replace(new_id);

                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    // Override size limits - use wide width as max to accommodate all views
                    popup_settings.positioner.size_limits = Limits::NONE
                        .min_height(1.0)
                        .min_width(1.0)
                        .max_width(WIDE_POPUP_WIDTH)
                        .max_height(POPUP_MAX_HEIGHT);

                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::DbusConnected(conn) => {
                tracing::info!("D-Bus connection established");
                self.dbus_connection = Some(conn.clone());
                self.error = None;
                // Immediately fetch devices
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

            // Navigation
            Message::SelectDevice(device_id) => {
                self.selected_device = Some(device_id);
                self.view_mode = ViewMode::DevicePage;
                self.share_text_input.clear();
            }
            Message::BackToList => {
                self.selected_device = None;
                self.view_mode = ViewMode::DeviceList;
                self.share_text_input.clear();
            }
            Message::OpenSendToView(device_id, device_type) => {
                self.sendto_device_id = Some(device_id);
                self.sendto_device_type = Some(device_type);
                self.view_mode = ViewMode::SendTo;
            }
            Message::BackFromSendTo => {
                self.view_mode = ViewMode::DevicePage;
                self.sendto_device_id = None;
                self.sendto_device_type = None;
            }

            // Ping
            Message::SendPing(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    self.status_message = Some("Sending ping...".to_string());
                    return cosmic::app::Task::perform(
                        send_ping_async(conn.clone(), device_id),
                        |result| cosmic::Action::App(Message::PingComplete(result)),
                    );
                }
            }
            Message::PingComplete(result) => match result {
                Ok(()) => {
                    tracing::info!("Ping sent successfully");
                    self.status_message = Some("Ping sent!".to_string());
                }
                Err(e) => {
                    tracing::error!("Ping failed: {}", e);
                    self.status_message = Some(format!("Ping failed: {}", e));
                }
            },

            // Share
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
                            |result| cosmic::Action::App(Message::ShareComplete(result)),
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
                        |result| cosmic::Action::App(Message::ShareComplete(result)),
                    );
                }
            }
            Message::ShareComplete(result) => match result {
                Ok(()) => {
                    tracing::info!("Share completed successfully");
                    self.status_message = Some("Shared successfully!".to_string());
                }
                Err(e) => {
                    tracing::error!("Share failed: {}", e);
                    self.status_message = Some(format!("Share failed: {}", e));
                }
            },
            Message::ConfigChanged(config) => {
                tracing::info!("Config changed: {:?}", config);
                self.config = config;
            }

            // Pairing
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
                    tracing::info!("Rejecting/cancelling pairing for device: {}", device_id);
                    self.status_message = Some("Rejecting pairing...".to_string());
                    return cosmic::app::Task::perform(
                        reject_pairing_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::PairingResult(result) => {
                match &result {
                    Ok(msg) => {
                        tracing::info!("Pairing result: {}", msg);
                        self.status_message = Some(msg.clone());
                    }
                    Err(err) => {
                        tracing::error!("Pairing error: {}", err);
                        self.status_message = Some(format!("Error: {}", err));
                    }
                }
                // Refresh devices to update pairing state
                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::DbusSignalReceived => {
                // D-Bus signal received - debounce to avoid excessive refreshes
                // Require at least 3 seconds between signal-triggered refreshes
                let now = std::time::Instant::now();
                if now.duration_since(self.last_signal_refresh) < std::time::Duration::from_secs(3)
                {
                    return cosmic::app::Task::none();
                }

                if let Some(conn) = &self.dbus_connection {
                    tracing::debug!("D-Bus signal received, refreshing devices");
                    self.last_signal_refresh = now;
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }

            // Notifications
            Message::DismissNotification(device_id, notification_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!(
                        "Dismissing notification {} on {}",
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
                match &result {
                    Ok(msg) => tracing::info!("Dismiss result: {}", msg),
                    Err(err) => {
                        tracing::error!("Dismiss error: {}", err);
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

            // Clipboard
            Message::SendClipboard(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Sending clipboard to device: {}", device_id);
                    self.status_message = Some("Sending clipboard...".to_string());
                    return cosmic::app::Task::perform(
                        send_clipboard_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::ClipboardResult(result) => match &result {
                Ok(msg) => {
                    tracing::info!("Clipboard result: {}", msg);
                    self.status_message = Some(msg.clone());
                }
                Err(err) => {
                    tracing::error!("Clipboard error: {}", err);
                    self.status_message = Some(format!("Clipboard error: {}", err));
                }
            },

            // Settings
            Message::ToggleSettings => {
                if self.view_mode == ViewMode::Settings {
                    self.view_mode = ViewMode::DeviceList;
                } else {
                    self.view_mode = ViewMode::Settings;
                }
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

            // SMS
            Message::OpenSmsView(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    // Find device name for header
                    let device_name = self
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .map(|d| d.name.clone());

                    // Check if we have cached conversations for this device
                    let same_device = self.sms_device_id.as_ref() == Some(&device_id);
                    let has_cache = same_device && !self.conversations.is_empty();

                    self.view_mode = ViewMode::ConversationList;
                    self.sms_device_id = Some(device_id.clone());
                    self.sms_device_name = device_name;

                    if has_cache {
                        // Use cached conversations, trigger background refresh
                        self.sms_loading = false;
                        tracing::info!(
                            "Using cached {} conversations for device: {}",
                            self.conversations.len(),
                            device_id
                        );
                        // Trigger background refresh to get any new conversations
                        return cosmic::app::Task::perform(
                            fetch_conversations_async(conn.clone(), device_id),
                            cosmic::Action::App,
                        );
                    } else {
                        // No cache or different device - clear and fetch
                        self.sms_loading = true;
                        self.conversations.clear();
                        self.message_cache.clear();
                        self.contacts = ContactLookup::load_for_device(&device_id);
                        tracing::info!("Opening SMS view for device: {}", device_id);
                        return cosmic::app::Task::perform(
                            fetch_conversations_async(conn.clone(), device_id),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::CloseSmsView => {
                self.view_mode = ViewMode::DevicePage;
                // Keep sms_device_id, sms_device_name, conversations, contacts, and
                // message_cache for when user returns to SMS view
                self.messages.clear();
                self.current_thread_id = None;
                self.current_thread_address = None;
                self.sms_loading = false;
                self.sms_compose_text.clear();
                self.sms_sending = false;
            }
            Message::OpenConversation(thread_id) => {
                if let Some(conn) = &self.dbus_connection {
                    if let Some(device_id) = &self.sms_device_id {
                        // Find the conversation address for the header
                        let address = self
                            .conversations
                            .iter()
                            .find(|c| c.thread_id == thread_id)
                            .map(|c| c.address.clone());

                        self.current_thread_id = Some(thread_id);
                        self.current_thread_address = address;
                        self.view_mode = ViewMode::MessageThread;

                        // Check if we have cached messages
                        let has_cache = if let Some(cached) = self.message_cache.get(&thread_id) {
                            self.messages = cached.clone();
                            tracing::debug!(
                                "Using cached {} messages for thread {}",
                                cached.len(),
                                thread_id
                            );
                            self.sms_loading = false;
                            true
                        } else {
                            self.sms_loading = true;
                            false
                        };

                        tracing::info!("Opening conversation thread: {}", thread_id);

                        // Fetch messages and scroll to bottom
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
                self.current_thread_address = None;
                self.messages.clear();
                self.sms_compose_text.clear();
                self.sms_sending = false;

                // Increment key to reset scroll position
                self.conversation_list_key = self.conversation_list_key.wrapping_add(1);

                // Refresh conversations in background
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
                // Only update if we got conversations back
                if !convs.is_empty() {
                    self.conversations = convs;
                    self.conversation_list_key = self.conversation_list_key.wrapping_add(1);
                }
                self.sms_loading = false;
            }
            Message::MessagesLoaded(thread_id, msgs) => {
                if self.current_thread_id == Some(thread_id) {
                    let had_messages = !self.messages.is_empty();
                    tracing::info!(
                        "Loaded {} messages for thread {} (had {} cached)",
                        msgs.len(),
                        thread_id,
                        self.messages.len()
                    );
                    // Only update if we got more messages than currently shown
                    if msgs.len() >= self.messages.len() {
                        // Update cache
                        self.message_cache.insert(thread_id, msgs.clone());
                        self.messages = msgs;
                    }
                    self.sms_loading = false;

                    // Scroll to bottom if we didn't have cached messages
                    // (avoid jarring scroll when refreshing)
                    if !had_messages && !self.messages.is_empty() {
                        return scrollable::snap_to(
                            widget::Id::new("message-thread"),
                            scrollable::RelativeOffset::END,
                        );
                    }
                }
            }
            Message::SmsError(err) => {
                tracing::error!("SMS error: {}", err);
                self.status_message = Some(format!("SMS error: {}", err));
                self.sms_loading = false;
            }
            Message::SmsComposeInput(text) => {
                self.sms_compose_text = text;
            }
            Message::SendSms => {
                if let (Some(conn), Some(device_id), Some(thread_id)) = (
                    &self.dbus_connection,
                    &self.sms_device_id,
                    self.current_thread_id,
                ) {
                    if !self.sms_compose_text.is_empty() && !self.sms_sending {
                        let message_text = self.sms_compose_text.clone();
                        self.sms_sending = true;
                        return cosmic::app::Task::perform(
                            send_sms_async(
                                conn.clone(),
                                device_id.clone(),
                                thread_id,
                                message_text,
                            ),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::SmsSendResult(result) => {
                self.sms_sending = false;
                match &result {
                    Ok(msg) => {
                        tracing::info!("SMS send result: {}", msg);
                        self.sms_compose_text.clear();
                        self.status_message = Some(msg.clone());
                        // Refresh messages to show sent message
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
                        tracing::error!("SMS send error: {}", err);
                        self.status_message = Some(format!("Send failed: {}", err));
                    }
                }
            }

            // New message
            Message::OpenNewMessage => {
                self.view_mode = ViewMode::NewMessage;
                self.new_message_recipient.clear();
                self.new_message_body.clear();
                self.new_message_recipient_valid = false;
                self.new_message_sending = false;
                // Clear any previous suggestions; they will be populated by search
                self.contact_suggestions.clear();
            }
            Message::CloseNewMessage => {
                self.view_mode = ViewMode::ConversationList;
                self.new_message_recipient.clear();
                self.new_message_body.clear();
                self.new_message_recipient_valid = false;
                self.new_message_sending = false;
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
            Message::NewMessageBodyInput(text) => {
                self.new_message_body = text;
            }
            Message::SelectContact(name, phone) => {
                // User selected a contact - fill in the phone number
                self.new_message_recipient = phone;
                self.new_message_recipient_valid = true;
                self.contact_suggestions.clear();
                tracing::debug!("Selected contact: {}", name);
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
                match &result {
                    Ok(msg) => {
                        tracing::info!("New message send result: {}", msg);
                        self.status_message = Some(msg.clone());
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
                        tracing::error!("New message send error: {}", err);
                        self.status_message = Some(format!("Send failed: {}", err));
                    }
                }
            }

            // Media control messages
            Message::OpenMediaView(device_id) => {
                // Find device name for header
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.name.clone());

                self.media_device_id = Some(device_id.clone());
                self.media_device_name = device_name;
                self.media_info = None;
                self.media_loading = true;
                self.media_selected_player = None;
                self.view_mode = ViewMode::MediaControls;

                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        fetch_media_info_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::CloseMediaView => {
                self.view_mode = ViewMode::DevicePage;
                self.media_device_id = None;
                self.media_device_name = None;
                self.media_info = None;
                self.media_loading = false;
                self.media_selected_player = None;
            }
            Message::MediaInfoLoaded(info) => {
                self.media_loading = false;
                // Preserve user's explicit player selection if set
                self.media_info = match (info, &self.media_selected_player) {
                    (Some(mut media_info), Some(selected)) => {
                        if media_info.players.contains(selected) {
                            media_info.current_player = selected.clone();
                        }
                        Some(media_info)
                    }
                    (info, _) => info,
                };
            }
            Message::MediaPlayPause => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    let ensure_player = self.media_selected_player.clone();
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::PlayPause,
                            ensure_player,
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaNext => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    let ensure_player = self.media_selected_player.clone();
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::Next,
                            ensure_player,
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaPrevious => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    let ensure_player = self.media_selected_player.clone();
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::Previous,
                            ensure_player,
                        ),
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
                    let ensure_player = self.media_selected_player.clone();
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::SetVolume(volume),
                            ensure_player,
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::MediaSelectPlayer(player) => {
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    // Track user's explicit selection (persists until view is closed)
                    self.media_selected_player = Some(player.clone());
                    // Update local state immediately
                    if let Some(ref mut info) = self.media_info {
                        info.current_player = player.clone();
                    }
                    return cosmic::app::Task::perform(
                        media_action_async(
                            conn.clone(),
                            device_id.clone(),
                            MediaAction::SelectPlayer(player),
                            None, // SelectPlayer doesn't need ensure_player
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
                let sender_name = contacts.get_name_or_number(&message.address);

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
                            .appname("COSMIC Connect")
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
        // Panel button with icon
        let icon_name = if self.devices.iter().any(|d| d.is_reachable && d.is_paired) {
            "phone-symbolic"
        } else {
            "phone-disconnect-symbolic"
        };

        self.core
            .applet
            .icon_button(icon_name)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Self::Message> {
        // Determine popup width based on view mode
        // SMS and media views need wider popup for message bubbles
        let popup_width = match self.view_mode {
            ViewMode::ConversationList
            | ViewMode::MessageThread
            | ViewMode::NewMessage
            | ViewMode::MediaControls => WIDE_POPUP_WIDTH,
            _ => DEFAULT_POPUP_WIDTH,
        };

        // Handle error state first
        if let Some(err) = &self.error {
            let content: Element<Message> = widget::container(
                column![text(fl!("error")).size(16), text(err.clone()).size(12),]
                    .spacing(8)
                    .align_x(Alignment::Center),
            )
            .padding(16)
            .into();
            return self.popup_container(content, popup_width);
        }

        // Handle loading state
        if self.loading && self.view_mode == ViewMode::DeviceList {
            let content: Element<Message> = widget::container(
                column![text(fl!("loading")).size(14),].align_x(Alignment::Center),
            )
            .padding(16)
            .into();
            return self.popup_container(content, popup_width);
        }

        // Route to appropriate view based on view mode
        let content: Element<Message> = match &self.view_mode {
            ViewMode::Settings => self.view_settings(),
            ViewMode::ConversationList => self.view_conversation_list(),
            ViewMode::MessageThread => self.view_message_thread(),
            ViewMode::NewMessage => self.view_new_message(),
            ViewMode::MediaControls => self.view_media_controls(),
            ViewMode::SendTo => self.view_send_to(),
            ViewMode::DevicePage => {
                if let Some(device_id) = &self.selected_device {
                    if let Some(device) = self.devices.iter().find(|d| &d.id == device_id) {
                        ui::device_page::view(device, self.status_message.as_deref())
                    } else {
                        ui::device_list::view(
                            &self.devices,
                            &self.config,
                            self.status_message.as_deref(),
                        )
                    }
                } else {
                    ui::device_list::view(
                        &self.devices,
                        &self.config,
                        self.status_message.as_deref(),
                    )
                }
            }
            ViewMode::DeviceList => {
                if self.devices.is_empty() {
                    widget::container(
                        column![
                            text(fl!("no-devices")).size(16),
                            text(fl!("no-devices-hint")).size(12),
                            widget::divider::horizontal::default(),
                            widget::button::icon(widget::icon::from_name("emblem-system-symbolic"))
                                .on_press(Message::ToggleSettings),
                        ]
                        .spacing(8)
                        .align_x(Alignment::Center),
                    )
                    .padding(16)
                    .into()
                } else {
                    ui::device_list::view(
                        &self.devices,
                        &self.config,
                        self.status_message.as_deref(),
                    )
                }
            }
        };

        self.popup_container(content, popup_width)
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

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}

/// ID for the autosize widget used in popup container.
static POPUP_AUTOSIZE_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("popup-autosize"));

impl ConnectApplet {
    /// Create a popup container with specified width.
    /// Based on libcosmic's popup_container but with configurable width limits.
    fn popup_container<'a>(
        &self,
        content: impl Into<Element<'a, Message>>,
        width: f32,
    ) -> Element<'a, Message> {
        use cosmic::iced::alignment::{Horizontal, Vertical};
        use cosmic_panel_config::PanelAnchor;

        let (vertical_align, horizontal_align) = match self.core.applet.anchor {
            PanelAnchor::Left => (Vertical::Center, Horizontal::Left),
            PanelAnchor::Right => (Vertical::Center, Horizontal::Right),
            PanelAnchor::Top => (Vertical::Top, Horizontal::Center),
            PanelAnchor::Bottom => (Vertical::Bottom, Horizontal::Center),
        };

        autosize(
            Container::<Message, cosmic::Theme, Renderer>::new(
                Container::<Message, cosmic::Theme, Renderer>::new(content).style(|theme| {
                    let cosmic = theme.cosmic();
                    let corners = cosmic.corner_radii;
                    cosmic::iced_widget::container::Style {
                        text_color: Some(cosmic.background.on.into()),
                        background: Some(Color::from(cosmic.background.base).into()),
                        border: cosmic::iced::Border {
                            radius: corners.radius_m.into(),
                            width: 1.0,
                            color: cosmic.background.divider.into(),
                        },
                        shadow: Shadow::default(),
                        icon_color: Some(cosmic.background.on.into()),
                    }
                }),
            )
            .width(Length::Shrink)
            .height(Length::Shrink)
            .align_x(horizontal_align)
            .align_y(vertical_align),
            POPUP_AUTOSIZE_ID.clone(),
        )
        .limits(
            Limits::NONE
                .min_height(1.0)
                .min_width(width)
                .max_width(width)
                .max_height(POPUP_MAX_HEIGHT),
        )
        .into()
    }

    /// Render the settings view.
    fn view_settings(&self) -> Element<'_, Message> {
        let back_btn = widget::button::text(fl!("back"))
            .leading_icon(widget::icon::from_name("go-previous-symbolic").size(16))
            .on_press(Message::ToggleSettings);

        let mut settings_col = column![
            back_btn,
            widget::divider::horizontal::default(),
            text(fl!("settings")).size(16),
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

        widget::container(settings_col).width(Length::Fill).into()
    }

    /// Render a single setting toggle row.
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

    /// Render the SMS conversation list view.
    fn view_conversation_list(&self) -> Element<'_, Message> {
        let default_device = fl!("device");
        let device_name = self.sms_device_name.as_deref().unwrap_or(&default_device);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseSmsView),
            text(fl!("messages-title", device = device_name)).size(16),
            widget::horizontal_space(),
            widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                .on_press(Message::OpenNewMessage),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        let content: Element<Message> = if self.sms_loading && self.conversations.is_empty() {
            widget::container(
                column![text(fl!("loading-conversations")).size(14),].align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.conversations.is_empty() {
            widget::container(
                column![
                    widget::icon::from_name("mail-message-new-symbolic").size(48),
                    text(fl!("no-conversations")).size(16),
                    text(fl!("start-new-message")).size(12),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            // Build conversation list
            let mut conv_column = column![].spacing(4);
            for conv in &self.conversations {
                let display_name = self.contacts.get_name_or_number(&conv.address);

                let snippet = conv.last_message.chars().take(50).collect::<String>();
                let date_str = format_timestamp(conv.timestamp);

                let conv_row = widget::button::custom(
                    widget::container(
                        row![
                            column![text(display_name).size(14), text(snippet).size(11),]
                                .spacing(2),
                            widget::horizontal_space(),
                            text(date_str).size(10),
                            widget::icon::from_name("go-next-symbolic").size(16),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(8)
                    .width(Length::Fill),
                )
                .class(cosmic::theme::Button::Text)
                .on_press(Message::OpenConversation(conv.thread_id))
                .width(Length::Fill);

                conv_column = conv_column.push(conv_row);
            }

            widget::scrollable(conv_column.padding([0, 8]))
                .width(Length::Fill)
                .into()
        };

        column![header, widget::divider::horizontal::default(), content,]
            .spacing(8)
            .width(Length::Fill)
            .into()
    }

    /// Render the SMS message thread view.
    fn view_message_thread(&self) -> Element<'_, Message> {
        let default_unknown = fl!("unknown");
        let address = self
            .current_thread_address
            .as_deref()
            .unwrap_or(&default_unknown);
        let display_name = self.contacts.get_name_or_number(address);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseConversation),
            text(display_name).size(16),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        let content: Element<Message> = if self.sms_loading && self.messages.is_empty() {
            widget::container(
                column![text(fl!("loading-messages")).size(14),].align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else if self.messages.is_empty() {
            widget::container(
                column![text(fl!("no-messages")).size(14),].align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .into()
        } else {
            // Build message list with improved styling
            // Max width for bubbles is ~75% of popup width for better readability
            let bubble_max_width = (WIDE_POPUP_WIDTH * 0.75) as u16;

            let mut msg_column = column![].spacing(12).padding([8, 12]);
            for msg in &self.messages {
                // Note: message_type logic appears inverted from KDE Connect data
                // MessageType::Sent actually means received, Inbox means sent
                let is_received = msg.message_type == MessageType::Sent;
                let time_str = format_timestamp(msg.date);

                let msg_text =
                    column![text(&msg.body).size(14), text(time_str).size(10),].spacing(4);

                let msg_container = widget::container(msg_text)
                    .padding(12)
                    .max_width(bubble_max_width)
                    .class(if is_received {
                        cosmic::theme::Container::Card
                    } else {
                        cosmic::theme::Container::Primary
                    });

                // Received messages: show sender name and align left
                // Sent messages: align right with clear visual separation
                let msg_row: Element<Message> = if is_received {
                    let sender_name = self.contacts.get_name_or_number(&msg.address);
                    column![
                        text(sender_name).size(11),
                        row![msg_container, widget::horizontal_space(),].width(Length::Fill),
                    ]
                    .spacing(4)
                    .width(Length::Fill)
                    .into()
                } else {
                    row![widget::horizontal_space(), msg_container,]
                        .width(Length::Fill)
                        .into()
                };

                msg_column = msg_column.push(msg_row);
            }

            widget::container(
                widget::scrollable(msg_column)
                    .id(widget::Id::new("message-thread"))
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        // Compose bar
        let send_enabled = !self.sms_compose_text.is_empty() && !self.sms_sending;
        let compose_bar = row![
            widget::text_input(fl!("type-message"), &self.sms_compose_text)
                .on_input(Message::SmsComposeInput)
                .width(Length::Fill),
            widget::button::icon(widget::icon::from_name("mail-send-symbolic")).on_press_maybe(
                if send_enabled {
                    Some(Message::SendSms)
                } else {
                    None
                }
            ),
        ]
        .spacing(8)
        .padding([8, 12]);

        column![
            header,
            widget::divider::horizontal::default(),
            content,
            widget::divider::horizontal::default(),
            compose_bar,
        ]
        .spacing(4)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Render the new message compose view.
    fn view_new_message(&self) -> Element<'_, Message> {
        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseNewMessage),
            text(fl!("new-message")).size(16),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        // Recipient input with validation indicator
        let recipient_valid_icon: Element<Message> = if self.new_message_recipient.is_empty() {
            widget::horizontal_space().width(20).into()
        } else if self.new_message_recipient_valid {
            widget::icon::from_name("emblem-ok-symbolic")
                .size(16)
                .into()
        } else if !self.contact_suggestions.is_empty() {
            // Show search icon when there are suggestions
            widget::icon::from_name("edit-find-symbolic")
                .size(16)
                .into()
        } else {
            widget::icon::from_name("dialog-error-symbolic")
                .size(16)
                .into()
        };

        let recipient_input =
            widget::text_input(fl!("recipient-placeholder"), &self.new_message_recipient)
                .on_input(Message::NewMessageRecipientInput)
                .width(Length::Fill);

        let recipient_row = row![
            text(fl!("to")).size(14),
            recipient_input,
            recipient_valid_icon,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        // Contact suggestions (shown when there are matches)
        let suggestions_section: Element<Message> = if !self.contact_suggestions.is_empty() {
            let mut suggestion_widgets: Vec<Element<Message>> = Vec::new();

            for contact in &self.contact_suggestions {
                if let Some(phone) = contact.phone_numbers.first() {
                    let name = contact.name.clone();
                    let phone_clone = phone.clone();

                    let suggestion_row = widget::button::custom(
                        row![
                            widget::icon::from_name("avatar-default-symbolic").size(20),
                            column![
                                text(name.clone()).size(13),
                                text(phone_clone.clone()).size(11),
                            ]
                            .spacing(2),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .padding([6, 8])
                        .width(Length::Fill),
                    )
                    .on_press(Message::SelectContact(name, phone_clone))
                    .width(Length::Fill)
                    .class(cosmic::theme::Button::MenuItem);

                    suggestion_widgets.push(suggestion_row.into());
                }
            }

            widget::container(column(suggestion_widgets).spacing(2).width(Length::Fill))
                .padding([0, 12])
                .into()
        } else if !self.new_message_recipient.is_empty() && !self.new_message_recipient_valid {
            // Help text when no matching contacts and input is not a valid number
            widget::container(
                text(fl!("recipient-placeholder"))
                    .size(11)
                    .width(Length::Fill),
            )
            .padding([4, 12])
            .into()
        } else {
            widget::Space::new(Length::Shrink, Length::Shrink).into()
        };

        // Message input
        let message_input = widget::text_input(fl!("type-message"), &self.new_message_body)
            .on_input(Message::NewMessageBodyInput)
            .on_submit(|_| Message::SendNewMessage)
            .width(Length::Fill);

        // Send button
        let send_enabled = self.new_message_recipient_valid
            && !self.new_message_body.is_empty()
            && !self.new_message_sending;

        let send_btn = if self.new_message_sending {
            widget::button::standard(fl!("sending"))
        } else {
            widget::button::suggested(fl!("send"))
                .leading_icon(widget::icon::from_name("mail-send-symbolic").size(16))
                .on_press_maybe(if send_enabled {
                    Some(Message::SendNewMessage)
                } else {
                    None
                })
        };

        let send_row = widget::container(
            row![widget::horizontal_space(), send_btn,]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([8, 12]);

        column![
            header,
            widget::divider::horizontal::default(),
            recipient_row,
            suggestions_section,
            widget::container(message_input).padding([8, 12]),
            send_row,
            widget::vertical_space(),
        ]
        .spacing(4)
        .width(Length::Fill)
        .into()
    }

    /// View for the "Send to device" submenu.
    fn view_send_to(&self) -> Element<'_, Message> {
        use cosmic::widget::icon;

        let device_type = self.sendto_device_type.as_deref().unwrap_or("device");
        let device_id = self.sendto_device_id.clone().unwrap_or_default();

        // Back button
        let back_btn = widget::button::text(fl!("back"))
            .leading_icon(icon::from_name("go-previous-symbolic").size(16))
            .on_press(Message::BackFromSendTo);

        // Header
        let header = text(fl!("send-to-title", device = device_type)).size(16);

        // Action list items (consistent with device page style)
        let device_id_for_file = device_id.clone();
        let device_id_for_clipboard = device_id.clone();
        let device_id_for_ping = device_id.clone();
        let device_id_for_text = device_id.clone();
        let text_to_share = self.share_text_input.clone();

        // Share file list item
        let share_file_row = row![
            icon::from_name("document-send-symbolic").size(24),
            text(fl!("share-file")).size(14),
            widget::horizontal_space(),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let share_file_item = widget::button::custom(
            widget::container(share_file_row)
                .padding(8)
                .width(Length::Fill),
        )
        .class(cosmic::theme::Button::Text)
        .on_press(Message::ShareFile(device_id_for_file))
        .width(Length::Fill);

        // Send clipboard list item
        let send_clipboard_row = row![
            icon::from_name("edit-copy-symbolic").size(24),
            text(fl!("share-clipboard")).size(14),
            widget::horizontal_space(),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let send_clipboard_item = widget::button::custom(
            widget::container(send_clipboard_row)
                .padding(8)
                .width(Length::Fill),
        )
        .class(cosmic::theme::Button::Text)
        .on_press(Message::SendClipboard(device_id_for_clipboard))
        .width(Length::Fill);

        // Send ping list item
        let send_ping_row = row![
            icon::from_name("emblem-synchronizing-symbolic").size(24),
            text(fl!("send-ping")).size(14),
            widget::horizontal_space(),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let send_ping_item = widget::button::custom(
            widget::container(send_ping_row)
                .padding(8)
                .width(Length::Fill),
        )
        .class(cosmic::theme::Button::Text)
        .on_press(Message::SendPing(device_id_for_ping))
        .width(Length::Fill);

        // Share text section
        let share_text_heading = text(fl!("share-text")).size(14);

        let share_text_input =
            widget::text_input(fl!("share-text-placeholder"), &self.share_text_input)
                .on_input(Message::ShareTextInput)
                .width(Length::Fill);

        let send_text_btn = widget::button::standard(fl!("send-text"))
            .leading_icon(icon::from_name("edit-paste-symbolic").size(16))
            .on_press_maybe(if self.share_text_input.is_empty() {
                None
            } else {
                Some(Message::ShareText(device_id_for_text, text_to_share))
            });

        // Status message if present
        let status_bar: Element<Message> = if let Some(msg) = &self.status_message {
            widget::container(text(msg).size(11))
                .padding([4, 8])
                .width(Length::Fill)
                .class(cosmic::theme::Container::Card)
                .into()
        } else {
            widget::Space::new(Length::Shrink, Length::Shrink).into()
        };

        widget::container(
            column![
                back_btn,
                status_bar,
                widget::divider::horizontal::default(),
                header,
                share_file_item,
                send_clipboard_item,
                send_ping_item,
                widget::divider::horizontal::default(),
                share_text_heading,
                share_text_input,
                send_text_btn,
            ]
            .spacing(12)
            .padding(16),
        )
        .into()
    }

    /// View for media player controls.
    fn view_media_controls(&self) -> Element<'_, Message> {
        let default_device = fl!("device");
        let device_name = self.media_device_name.as_deref().unwrap_or(&default_device);

        let header = row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .on_press(Message::CloseMediaView),
            text(format!("{} - {}", fl!("media"), device_name)).size(16),
            widget::horizontal_space(),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        let content: Element<Message> = if self.media_loading {
            widget::container(
                column![text(fl!("loading-media")).size(14),]
                    .spacing(12)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding(24)
            .into()
        } else if let Some(ref info) = self.media_info {
            if info.players.is_empty() {
                // No active media players
                widget::container(
                    column![
                        widget::icon::from_name("multimedia-player-symbolic").size(48),
                        text(fl!("no-media-players")).size(14),
                        text(fl!("start-playing")).size(12),
                    ]
                    .spacing(12)
                    .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .padding(24)
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
                    text(fl!("media-not-available")).size(14),
                    text(fl!("enable-mpris")).size(12),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding(24)
            .into()
        };

        column![header, widget::divider::horizontal::default(), content,]
            .spacing(8)
            .width(Length::Fill)
            .into()
    }

    /// View for the media player with controls.
    fn view_media_player(&self, info: &MediaInfo) -> Element<'_, Message> {
        // Player selector (if multiple players)
        let player_selector: Element<Message> = if info.players.len() > 1 {
            let players: Vec<String> = info.players.clone();
            // Find selected index, defaulting to first player if current_player is empty or not found
            let selected_idx = if info.current_player.is_empty() {
                Some(0)
            } else {
                players
                    .iter()
                    .position(|p| p == &info.current_player)
                    .or(Some(0))
            };
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
            .padding([0, 12])
            .into()
        } else {
            widget::container(text(info.current_player.clone()).size(12))
                .padding([0, 12])
                .into()
        };

        // Track info
        let title_text = if info.title.is_empty() {
            "No track playing".to_string()
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
            text(title_text).size(16),
            text(artist_text).size(13),
            text(album_text).size(11),
        ]
        .spacing(4)
        .align_x(Alignment::Center)
        .width(Length::Fill);

        // Position display
        let position_str = format_duration(info.position);
        let length_str = format_duration(info.length);
        let position_display = row![
            text(position_str).size(10),
            widget::horizontal_space(),
            text(length_str).size(10),
        ]
        .padding([0, 12]);

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
                .size(10)
                .width(Length::Fixed(36.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 12]);

        // Assemble the view
        column![
            player_selector,
            widget::vertical_space().height(Length::Fixed(16.0)),
            widget::container(widget::icon::from_name("multimedia-player-symbolic").size(48))
                .width(Length::Fill)
                .align_x(Alignment::Center),
            widget::vertical_space().height(Length::Fixed(12.0)),
            widget::container(track_info).padding([0, 12]),
            widget::vertical_space().height(Length::Fixed(16.0)),
            position_display,
            widget::vertical_space().height(Length::Fixed(12.0)),
            controls_container,
            widget::vertical_space().height(Length::Fixed(16.0)),
            volume_row,
        ]
        .spacing(4)
        .padding([0, 0, 16, 0]) // Add bottom padding
        .width(Length::Fill)
        .into()
    }
}

/// Format a Unix timestamp as a human-readable date/time string.
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{Local, TimeZone};
    let datetime = Local.timestamp_millis_opt(timestamp).single();
    match datetime {
        Some(dt) => {
            let now = Local::now();
            if dt.date_naive() == now.date_naive() {
                dt.format("%H:%M").to_string()
            } else {
                dt.format("%b %d").to_string()
            }
        }
        None => "Unknown".to_string(),
    }
}

/// Fetch all devices from the KDE Connect daemon via D-Bus.
async fn fetch_devices_async(conn: Arc<Mutex<Connection>>) -> Message {
    let conn = conn.lock().await;

    // Get the daemon proxy
    let daemon = match DaemonProxy::new(&conn).await {
        Ok(d) => d,
        Err(e) => {
            return Message::Error(format!("Failed to connect to KDE Connect daemon: {}", e));
        }
    };

    // Get list of all device IDs
    let device_ids = match daemon.devices().await {
        Ok(ids) => ids,
        Err(e) => {
            return Message::Error(format!("Failed to get device list: {}", e));
        }
    };

    tracing::debug!("Found {} device(s)", device_ids.len());

    // Fetch info for each device
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

    // Try to get battery info if available
    let (battery_level, battery_charging) = if is_reachable && is_paired {
        fetch_battery_info(conn, device_id).await
    } else {
        (None, None)
    };

    // Fetch notifications if device is connected and paired
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

/// Send a ping to a device.
async fn send_ping_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Result<(), String> {
    let conn = conn.lock().await;
    let path = format!("{}/devices/{}/ping", kdeconnect_dbus::BASE_PATH, device_id);

    let ping = PingProxy::builder(&conn)
        .path(path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    ping.send_ping().await.map_err(|e| e.to_string())
}

/// Share a file to a device.
async fn share_file_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    path: PathBuf,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    let url = format!("file://{}", path.display());
    share.share_url(&url).await.map_err(|e| e.to_string())
}

/// Share text to a device.
async fn share_text_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    text: String,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    share.share_text(&text).await.map_err(|e| e.to_string())
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

/// Accept incoming pairing request.
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

/// Reject or cancel a pairing request.
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

/// Send current desktop clipboard to a device.
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

                // Subscribe to all signals from KDE Connect daemon
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

                // Also subscribe to property changes (for battery, pairing state, etc.)
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

                                    // Only trigger refresh on specific device-related signals
                                    let is_relevant = match iface_str {
                                        // Daemon signals for device discovery
                                        "org.kde.kdeconnect.daemon" => matches!(
                                            member_str,
                                            "deviceAdded"
                                                | "deviceRemoved"
                                                | "deviceVisibilityChanged"
                                                | "announcedNameChanged"
                                        ),
                                        // Device signals for pairing state
                                        "org.kde.kdeconnect.device" => matches!(
                                            member_str,
                                            "reachableChanged"
                                                | "trustedChanged"
                                                | "pairingRequest"
                                                | "hasPairingRequestsChanged"
                                        ),
                                        // Battery and notification plugin signals
                                        "org.kde.kdeconnect.device.battery" => true,
                                        "org.kde.kdeconnect.device.notifications" => true,
                                        // Property changes for any kdeconnect interface
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
                                                                sms_msg.address,
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

/// Fetch SMS conversations for a device using signal-based loading.
async fn fetch_conversations_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
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
    use kdeconnect_dbus::plugins::{parse_sms_message, MAX_CONVERSATIONS};

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
    let overall_timeout = tokio::time::Duration::from_secs(15);
    let activity_timeout = tokio::time::Duration::from_millis(500);
    let start_time = tokio::time::Instant::now();
    let mut last_activity = tokio::time::Instant::now();
    let mut loaded_signal_received = false;

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
                                    address: msg.address,
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
                                    address: msg.address,
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

            // Check timeouts every 50ms
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => {
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
                                address: msg.address,
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
                                address: msg.address,
                                last_message: msg.body,
                                timestamp: msg.date,
                                unread: !msg.read,
                            });
                        }
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(5)) => {
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

    // Poll with increasing delays
    let delays = [500, 1000, 1500, 2000, 3000];
    let mut best_result: Vec<ConversationSummary> = Vec::new();

    for (attempt, delay) in delays.iter().enumerate() {
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

                // Stop early if we have enough conversations
                if best_result.len() >= 5 {
                    tracing::info!(
                        "Fallback: Found {} conversations, stopping early",
                        best_result.len()
                    );
                    break;
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
async fn fetch_messages_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    messages_per_page: u32,
) -> Message {
    use kdeconnect_dbus::plugins::parse_sms_message;

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

/// Send an SMS reply to a conversation thread.
async fn send_sms_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    thread_id: i64,
    message: String,
) -> Message {
    use zbus::zvariant::Value;

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
                return Message::SmsSendResult(Err(format!("Failed to create proxy: {}", e)));
            }
        },
        None => {
            return Message::SmsSendResult(Err("Failed to build proxy path".to_string()));
        }
    };

    // Send with empty attachments array
    let empty_attachments: Vec<Value<'_>> = vec![];
    match conversations_proxy
        .reply_to_conversation(thread_id, &message, empty_attachments)
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
    // D-Bus returns i32 for position/length, convert to i64
    let position = proxy.position().await.unwrap_or(0) as i64;
    let length = proxy.length().await.unwrap_or(0) as i64;
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
/// If `ensure_player` is provided, the player will be selected before performing the action.
async fn media_action_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    action: MediaAction,
    ensure_player: Option<String>,
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

    // If a specific player is requested, ensure it's selected first
    if let Some(ref player) = ensure_player {
        if let Err(e) = proxy.set_player(player).await {
            tracing::warn!("Failed to set player before action: {}", e);
            // Continue anyway - the action might still work
        }
    }

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
