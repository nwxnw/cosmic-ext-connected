//! Main application state and logic for the COSMIC Connected applet.

use crate::config::Config;
use crate::constants::{
    dbus::SIGNAL_REFRESH_DEBOUNCE_SECS, notifications::FILE_TIMEOUT_MS, refresh,
};
use crate::device::{
    accept_pairing_async, dismiss_notification_async, fetch_devices_async, find_my_phone_async,
    reject_pairing_async, request_pair_async, send_clipboard_async, send_ping_async,
    share_file_async, share_text_async, unpair_async,
};
use crate::fl;
use crate::media::{
    fetch_media_info_async, media_action_async, view_media_controls, MediaAction,
    MediaControlsParams,
};
use crate::sms::{
    fetch_conversations_async, fetch_messages_async, fetch_older_messages_async,
    send_new_sms_async, send_sms_async, view_conversation_list, view_message_thread,
    view_new_message, ConversationListParams, MessageThreadParams, NewMessageParams,
};
use crate::subscriptions::{
    call_notification_subscription, dbus_signal_subscription, sms_notification_subscription,
};
use crate::ui;
use crate::views::helpers::{
    popup_container, DEFAULT_POPUP_WIDTH, POPUP_MAX_HEIGHT, WIDE_POPUP_WIDTH,
};
use crate::views::send_to::{view_send_to, SendToParams};
use crate::views::settings::view_settings;
use cosmic::app::Core;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::{column, scrollable, text};
use cosmic::iced::{Alignment, Subscription};
use cosmic::iced_core::layout::Limits;
use cosmic::iced_runtime::core::window;
use cosmic::widget;
use cosmic::{Application, Element};
use kdeconnect_dbus::{
    contacts::{Contact, ContactLookup},
    plugins::{is_address_valid, ConversationSummary, NotificationInfo, SmsMessage},
};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
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

    // Find My Phone actions
    /// Trigger the phone to ring
    FindMyPhone(String),
    /// Find My Phone operation completed
    FindMyPhoneComplete(Result<(), String>),

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
    /// Contacts loaded asynchronously for a device
    ContactsLoaded(String, ContactLookup),
    /// User clicked "Load More" button in conversation list
    LoadMoreConversations,
    /// Messages loaded for a specific thread (thread_id, messages, total_count)
    MessagesLoaded(i64, Vec<SmsMessage>, Option<u64>),
    /// SMS-related error occurred
    SmsError(String),
    /// Update SMS compose text input
    SmsComposeInput(String),
    /// Send SMS in current thread
    SendSms,
    /// SMS send operation completed (Ok contains the sent message body for optimistic update)
    SmsSendResult(Result<String, String>),
    /// Delayed refresh of messages after sending (to give KDE Connect time to sync)
    DelayedMessageRefresh(i64),
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
    /// User clicked "Load More" button to load older messages
    LoadMoreMessages,
    /// Older messages fetched successfully (thread_id, messages, has_more_heuristic, total_count)
    OlderMessagesLoaded(i64, Vec<SmsMessage>, bool, Option<u64>),
    /// Message thread scrolled - used for prefetching older messages
    MessageThreadScrolled(scrollable::Viewport),

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

    // Call Notifications
    /// Incoming or missed call received via D-Bus signal
    CallNotification {
        device_name: String,
        event: String,
        phone_number: String,
        contact_name: String,
    },

    // File Notifications
    /// File received via D-Bus signal
    FileReceived {
        device_name: String,
        file_url: String,
        file_name: String,
    },
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
    CallNotifications,
    CallShowNumber,
    CallShowName,
    FileNotifications,
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

/// Loading state for SMS operations with phase tracking.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SmsLoadingState {
    #[default]
    Idle,
    /// Loading conversations from device
    LoadingConversations(LoadingPhase),
    /// Loading messages for a specific thread
    LoadingMessages(LoadingPhase),
    /// Loading older messages (pagination)
    LoadingMoreMessages,
}

/// Phases of a loading operation.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum LoadingPhase {
    /// Setting up D-Bus connection and signal streams
    #[default]
    Connecting,
    /// Request sent to phone, waiting for response
    Requesting,
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
    /// Current conversation addresses (all participants, for sending and header)
    current_thread_addresses: Option<Vec<String>>,
    /// Current conversation's SIM subscription ID (for MMS group messages)
    current_thread_sub_id: Option<i64>,
    /// Messages in the current thread
    messages: Vec<SmsMessage>,
    /// SMS loading state with phase tracking
    sms_loading_state: SmsLoadingState,
    /// Contact lookup for resolving phone numbers to names
    contacts: ContactLookup,
    /// Key to reset conversation list scroll position
    conversation_list_key: u32,
    /// Number of conversations currently displayed (for pagination)
    conversations_displayed: usize,
    /// Text input for composing SMS reply
    sms_compose_text: String,
    /// Whether SMS is currently being sent
    sms_sending: bool,
    /// LRU cache of messages by thread_id for faster loading (limited to avoid unbounded growth)
    message_cache: LruCache<i64, Vec<SmsMessage>>,

    // Message pagination state
    /// Number of messages currently loaded for pagination offset
    messages_loaded_count: u32,
    /// Whether more older messages are available
    messages_has_more: bool,

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

    // File notification deduplication
    /// Last received file URL to avoid duplicate notifications
    last_received_file: Option<String>,
}

impl ConnectApplet {
    /// Check if loading more messages (pagination)
    fn is_loading_more_messages(&self) -> bool {
        matches!(self.sms_loading_state, SmsLoadingState::LoadingMoreMessages)
    }
}

impl Application for ConnectApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.cosmic-connected-applet";

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
            current_thread_addresses: None,
            current_thread_sub_id: None,
            messages: Vec::new(),
            sms_loading_state: SmsLoadingState::Idle,
            contacts: ContactLookup::default(),
            conversation_list_key: 0,
            conversations_displayed: 10,
            sms_compose_text: String::new(),
            sms_sending: false,
            message_cache: LruCache::new(
                NonZeroUsize::new(crate::constants::sms::MESSAGE_CACHE_MAX_CONVERSATIONS).unwrap(),
            ),
            // Message pagination state
            messages_loaded_count: 0,
            messages_has_more: true,
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
            // File notification deduplication
            last_received_file: None,
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

            // Find My Phone
            Message::FindMyPhone(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    self.status_message = Some(fl!("ringing-phone"));
                    return cosmic::app::Task::perform(
                        find_my_phone_async(conn.clone(), device_id),
                        |result| cosmic::Action::App(Message::FindMyPhoneComplete(result)),
                    );
                }
            }
            Message::FindMyPhoneComplete(result) => match result {
                Ok(()) => {
                    tracing::info!("Find my phone triggered successfully");
                    self.status_message = Some(fl!("phone-ringing"));
                }
                Err(e) => {
                    tracing::error!("Find my phone failed: {}", e);
                    self.status_message = Some(format!("{}: {}", fl!("find-phone-failed"), e));
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
                if now.duration_since(self.last_signal_refresh)
                    < std::time::Duration::from_secs(SIGNAL_REFRESH_DEBOUNCE_SECS)
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
                    SettingKey::CallNotifications => {
                        self.config.call_notifications = !self.config.call_notifications;
                    }
                    SettingKey::CallShowNumber => {
                        self.config.call_notification_show_number =
                            !self.config.call_notification_show_number;
                    }
                    SettingKey::CallShowName => {
                        self.config.call_notification_show_name =
                            !self.config.call_notification_show_name;
                    }
                    SettingKey::FileNotifications => {
                        self.config.file_notifications = !self.config.file_notifications;
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
                        // Use cached conversations, set to Requesting phase for background refresh
                        self.sms_loading_state =
                            SmsLoadingState::LoadingConversations(LoadingPhase::Requesting);
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
                        // No cache or different device - clear and fetch from Connecting phase
                        self.sms_loading_state =
                            SmsLoadingState::LoadingConversations(LoadingPhase::Connecting);
                        self.conversations.clear();
                        self.conversations_displayed = 10;
                        self.message_cache.clear();
                        self.contacts = ContactLookup::default(); // Will be loaded async
                        tracing::info!("Opening SMS view for device: {}", device_id);

                        // Load contacts and conversations in parallel (non-blocking)
                        let device_id_for_contacts = device_id.clone();
                        return cosmic::app::Task::batch(vec![
                            cosmic::app::Task::perform(
                                fetch_conversations_async(conn.clone(), device_id),
                                cosmic::Action::App,
                            ),
                            cosmic::app::Task::perform(
                                async move {
                                    let contacts =
                                        ContactLookup::load_for_device(&device_id_for_contacts)
                                            .await;
                                    Message::ContactsLoaded(device_id_for_contacts, contacts)
                                },
                                cosmic::Action::App,
                            ),
                        ]);
                    }
                }
            }
            Message::CloseSmsView => {
                self.view_mode = ViewMode::DevicePage;
                // Keep sms_device_id, sms_device_name, conversations, contacts, and
                // message_cache for when user returns to SMS view
                self.messages.clear();
                self.current_thread_id = None;
                self.current_thread_addresses = None;
                self.current_thread_sub_id = None;
                self.sms_loading_state = SmsLoadingState::Idle;
                self.sms_compose_text.clear();
                self.sms_sending = false;
            }
            Message::OpenConversation(thread_id) => {
                if let Some(conn) = &self.dbus_connection {
                    if let Some(device_id) = &self.sms_device_id {
                        // Find the conversation for header info and deduplication
                        let conversation =
                            self.conversations.iter().find(|c| c.thread_id == thread_id);

                        let addresses = conversation.map(|c| c.addresses.clone());

                        // Pre-populate last_seen_sms with current time to prevent false notifications
                        // when fetching existing messages in this thread.
                        // Using current time (in milliseconds) ensures ALL existing messages
                        // are considered "seen" - only truly new messages arriving after this
                        // point will trigger notifications.
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        self.last_seen_sms.insert(thread_id, now_ms);

                        self.current_thread_id = Some(thread_id);
                        self.current_thread_addresses = addresses;
                        self.view_mode = ViewMode::MessageThread;

                        // Reset pagination state
                        self.messages_loaded_count = 0;
                        self.messages_has_more = true;

                        // Check if we have cached messages
                        let has_cache = if let Some(cached) = self.message_cache.get(&thread_id) {
                            self.messages = cached.clone();
                            tracing::debug!(
                                "Using cached {} messages for thread {}",
                                cached.len(),
                                thread_id
                            );
                            // Cached - set to Requesting for background refresh
                            self.sms_loading_state =
                                SmsLoadingState::LoadingMessages(LoadingPhase::Requesting);
                            true
                        } else {
                            // No cache - start from Connecting phase
                            self.sms_loading_state =
                                SmsLoadingState::LoadingMessages(LoadingPhase::Connecting);
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
                self.current_thread_addresses = None;
                self.current_thread_sub_id = None;
                self.messages.clear();
                self.sms_compose_text.clear();
                self.sms_sending = false;

                // Increment key to reset scroll position
                self.conversation_list_key = self.conversation_list_key.wrapping_add(1);

                // Refresh conversations in background
                if let (Some(conn), Some(device_id)) = (&self.dbus_connection, &self.sms_device_id)
                {
                    if self.conversations.is_empty() {
                        self.sms_loading_state =
                            SmsLoadingState::LoadingConversations(LoadingPhase::Connecting);
                    }
                    return cosmic::app::Task::perform(
                        fetch_conversations_async(conn.clone(), device_id.clone()),
                        cosmic::Action::App,
                    );
                }
                self.sms_loading_state = SmsLoadingState::Idle;
            }
            Message::ConversationsLoaded(convs) => {
                tracing::info!(
                    "Loaded {} conversations (had {} cached)",
                    convs.len(),
                    self.conversations.len()
                );
                // Only update if we got conversations back
                if !convs.is_empty() {
                    // Pre-populate last_seen_sms to prevent false notifications
                    // for messages that already exist in loaded conversations
                    for conv in &convs {
                        // Only update if we don't have a newer timestamp already
                        let current = self.last_seen_sms.get(&conv.thread_id).copied();
                        if current.is_none() || current < Some(conv.timestamp) {
                            self.last_seen_sms.insert(conv.thread_id, conv.timestamp);
                        }
                    }

                    self.conversations = convs;
                    self.conversation_list_key = self.conversation_list_key.wrapping_add(1);
                }
                // Only reset to Idle if we're currently loading conversations
                if matches!(
                    self.sms_loading_state,
                    SmsLoadingState::LoadingConversations(_)
                ) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }
            }
            Message::ContactsLoaded(device_id, contacts) => {
                // Only update if contacts are for the current SMS device
                if self.sms_device_id.as_ref() == Some(&device_id) {
                    tracing::info!(
                        "Loaded {} contacts for device {}",
                        contacts.len(),
                        device_id
                    );
                    self.contacts = contacts;
                } else {
                    tracing::debug!(
                        "Ignoring contacts for device {} (current: {:?})",
                        device_id,
                        self.sms_device_id
                    );
                }
            }
            Message::LoadMoreConversations => {
                // Show 10 more conversations (up to total available)
                self.conversations_displayed =
                    (self.conversations_displayed + 10).min(self.conversations.len());
            }
            Message::MessagesLoaded(thread_id, msgs, total_count) => {
                if self.current_thread_id == Some(thread_id) {
                    let had_messages = !self.messages.is_empty();
                    tracing::info!(
                        "Loaded {} messages for thread {} (had {} cached, total: {:?})",
                        msgs.len(),
                        thread_id,
                        self.messages.len(),
                        total_count
                    );
                    // Only update if we got more messages than currently shown
                    if msgs.len() >= self.messages.len() {
                        // Extract sub_id from the first message (for MMS group messaging)
                        if let Some(first_msg) = msgs.first() {
                            self.current_thread_sub_id = Some(first_msg.sub_id);
                            tracing::debug!(
                                "Set sub_id to {} for thread {}",
                                first_msg.sub_id,
                                thread_id
                            );
                        }

                        // Update last_seen_sms with the newest message timestamp
                        // to prevent false notifications for messages we just loaded
                        if let Some(newest) = msgs.iter().map(|m| m.date).max() {
                            let current = self.last_seen_sms.get(&thread_id).copied();
                            if current.is_none() || current < Some(newest) {
                                self.last_seen_sms.insert(thread_id, newest);
                            }
                        }

                        // Update cache
                        self.message_cache.put(thread_id, msgs.clone());
                        // Update pagination state
                        self.messages_loaded_count = msgs.len() as u32;
                        // Use total_count for accurate pagination if available,
                        // otherwise fall back to heuristic
                        self.messages_has_more = match total_count {
                            Some(total) => (msgs.len() as u64) < total,
                            None => msgs.len() >= self.config.messages_per_page as usize,
                        };
                        self.messages = msgs;
                    }
                    // Only reset to Idle if we're currently loading messages
                    if matches!(self.sms_loading_state, SmsLoadingState::LoadingMessages(_)) {
                        self.sms_loading_state = SmsLoadingState::Idle;
                    }

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
            Message::LoadMoreMessages => {
                // Guard: skip if already loading or no more messages
                if self.is_loading_more_messages() || !self.messages_has_more {
                    return cosmic::app::Task::none();
                }

                if let (Some(conn), Some(device_id), Some(thread_id)) = (
                    &self.dbus_connection,
                    &self.sms_device_id,
                    self.current_thread_id,
                ) {
                    self.sms_loading_state = SmsLoadingState::LoadingMoreMessages;
                    tracing::info!(
                        "Loading more messages for thread {} from offset {}",
                        thread_id,
                        self.messages_loaded_count
                    );
                    return cosmic::app::Task::perform(
                        fetch_older_messages_async(
                            conn.clone(),
                            device_id.clone(),
                            thread_id,
                            self.messages_loaded_count,
                            self.config.messages_per_page,
                        ),
                        cosmic::Action::App,
                    );
                }
            }
            Message::OlderMessagesLoaded(
                thread_id,
                older_msgs,
                has_more_heuristic,
                total_count,
            ) => {
                // Only reset to Idle if we're currently loading more messages
                if matches!(self.sms_loading_state, SmsLoadingState::LoadingMoreMessages) {
                    self.sms_loading_state = SmsLoadingState::Idle;
                }

                if self.current_thread_id == Some(thread_id) {
                    if !older_msgs.is_empty() {
                        tracing::info!(
                            "Prepending {} older messages to thread {} (had {}, total: {:?})",
                            older_msgs.len(),
                            thread_id,
                            self.messages.len(),
                            total_count
                        );

                        // Prepend older messages (they come sorted oldest first)
                        let mut combined = older_msgs;
                        combined.append(&mut self.messages);
                        self.messages = combined;

                        // Update loaded count
                        self.messages_loaded_count = self.messages.len() as u32;

                        // Update cache with combined messages
                        self.message_cache.put(thread_id, self.messages.clone());

                        // Use total_count for accurate pagination if available,
                        // otherwise fall back to heuristic
                        self.messages_has_more = match total_count {
                            Some(total) => (self.messages.len() as u64) < total,
                            None => has_more_heuristic,
                        };
                    } else {
                        tracing::info!("No older messages returned for thread {}", thread_id);
                        // No more messages available
                        self.messages_has_more = false;
                    }
                }
            }
            Message::MessageThreadScrolled(viewport) => {
                // Prefetch older messages when user scrolls near the top
                // Trigger when within 100 pixels of the top and not already loading
                const PREFETCH_THRESHOLD_PX: f32 = 100.0;

                let scroll_offset = viewport.absolute_offset().y;

                if scroll_offset < PREFETCH_THRESHOLD_PX
                    && self.messages_has_more
                    && !self.is_loading_more_messages()
                    && !self.messages.is_empty()
                {
                    tracing::debug!(
                        "Prefetching older messages (scroll_y={:.1}px)",
                        scroll_offset
                    );

                    // Trigger loading older messages (same logic as LoadMoreMessages)
                    if let (Some(conn), Some(device_id), Some(thread_id)) = (
                        &self.dbus_connection,
                        &self.sms_device_id,
                        self.current_thread_id,
                    ) {
                        self.sms_loading_state = SmsLoadingState::LoadingMoreMessages;
                        let start_index = self.messages_loaded_count;
                        let count = self.config.messages_per_page;

                        return cosmic::app::Task::perform(
                            fetch_older_messages_async(
                                conn.clone(),
                                device_id.clone(),
                                thread_id,
                                start_index,
                                count,
                            ),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::SmsError(err) => {
                tracing::error!("SMS error: {}", err);
                self.status_message = Some(format!("SMS error: {}", err));
                self.sms_loading_state = SmsLoadingState::Idle;
            }
            Message::SmsComposeInput(text) => {
                self.sms_compose_text = text;
            }
            Message::SendSms => {
                tracing::info!("SendSms triggered");
                tracing::info!(
                    "State: conn={}, device_id={:?}, thread_id={:?}, addresses={:?}, text_empty={}, sending={}",
                    self.dbus_connection.is_some(),
                    self.sms_device_id,
                    self.current_thread_id,
                    self.current_thread_addresses,
                    self.sms_compose_text.is_empty(),
                    self.sms_sending
                );
                if let (Some(conn), Some(device_id), Some(thread_id), Some(addresses)) = (
                    &self.dbus_connection,
                    &self.sms_device_id,
                    self.current_thread_id,
                    &self.current_thread_addresses,
                ) {
                    if !self.sms_compose_text.is_empty()
                        && !self.sms_sending
                        && !addresses.is_empty()
                    {
                        // Check if this is a group conversation (multiple unique recipients)
                        let mut unique_addresses = std::collections::HashSet::new();
                        for addr in addresses {
                            unique_addresses.insert(addr.as_str());
                        }

                        if unique_addresses.len() > 1 {
                            // Group MMS sending is not supported by KDE Connect
                            tracing::warn!(
                                "Group MMS sending not supported ({} recipients)",
                                unique_addresses.len()
                            );
                            self.status_message = Some(fl!("group-sms-not-supported"));
                            return cosmic::app::Task::none();
                        }

                        let message_text = self.sms_compose_text.clone();
                        let recipients = addresses.clone();
                        let sub_id = self.current_thread_sub_id.unwrap_or(-1);
                        self.sms_sending = true;
                        tracing::info!(
                            "Dispatching send_sms_async with {} recipients, sub_id={}",
                            recipients.len(),
                            sub_id
                        );
                        return cosmic::app::Task::perform(
                            send_sms_async(
                                conn.clone(),
                                device_id.clone(),
                                thread_id,
                                recipients,
                                message_text,
                                sub_id,
                            ),
                            cosmic::Action::App,
                        );
                    } else {
                        tracing::warn!("SendSms conditions not met: text_empty={}, sending={}, addresses_empty={}",
                            self.sms_compose_text.is_empty(), self.sms_sending, addresses.is_empty());
                    }
                } else {
                    tracing::warn!("SendSms missing required state");
                }
            }
            Message::SmsSendResult(result) => {
                self.sms_sending = false;
                match result {
                    Ok(sent_body) => {
                        tracing::info!("SMS sent successfully");
                        self.sms_compose_text.clear();
                        self.status_message = Some(fl!("sms-sent"));

                        // Optimistic update: add the sent message to the local list immediately
                        if let Some(thread_id) = self.current_thread_id {
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(0);

                            // Update conversation list so it reflects the new message
                            // when user navigates back
                            if let Some(conv) = self
                                .conversations
                                .iter_mut()
                                .find(|c| c.thread_id == thread_id)
                            {
                                conv.last_message = sent_body.clone();
                                conv.timestamp = now_ms;
                            }
                            // Re-sort conversations by timestamp (newest first)
                            self.conversations
                                .sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

                            let sent_message = SmsMessage {
                                body: sent_body,
                                addresses: self
                                    .current_thread_addresses
                                    .clone()
                                    .unwrap_or_default(),
                                date: now_ms,
                                message_type: kdeconnect_dbus::plugins::MessageType::Sent,
                                read: true,
                                thread_id,
                                uid: 0, // Placeholder for optimistic message; will be replaced on sync
                                sub_id: self.current_thread_sub_id.unwrap_or(-1),
                            };

                            self.messages.push(sent_message.clone());

                            // Update cache as well
                            if let Some(cached) = self.message_cache.get_mut(&thread_id) {
                                cached.push(sent_message);
                            }

                            // Trigger delayed refresh to sync with server
                            // (gives KDE Connect time to process the sent message)
                            return cosmic::app::Task::batch(vec![
                                cosmic::app::Task::perform(
                                    async move {
                                        tokio::time::sleep(std::time::Duration::from_secs(
                                            refresh::POST_SEND_DELAY_SECS,
                                        ))
                                        .await;
                                        thread_id
                                    },
                                    |tid| cosmic::Action::App(Message::DelayedMessageRefresh(tid)),
                                ),
                                scrollable::snap_to(
                                    widget::Id::new("message-thread"),
                                    scrollable::RelativeOffset::END,
                                ),
                            ]);
                        }
                    }
                    Err(err) => {
                        tracing::error!("SMS send error: {}", err);
                        self.status_message = Some(format!("{}: {}", fl!("sms-failed"), err));
                    }
                }
            }
            Message::DelayedMessageRefresh(thread_id) => {
                // Refresh messages after a delay to sync sent message from server
                if self.current_thread_id == Some(thread_id) {
                    if let (Some(conn), Some(device_id)) =
                        (&self.dbus_connection, &self.sms_device_id)
                    {
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
                        // Show loading state since new conversation won't be in cache
                        if let (Some(conn), Some(device_id)) =
                            (&self.dbus_connection, &self.sms_device_id)
                        {
                            self.sms_loading_state =
                                SmsLoadingState::LoadingConversations(LoadingPhase::Requesting);
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
                // Freshness check: only notify for messages received within the last 30 seconds.
                // This prevents false notifications when fetching historical messages and handles
                // cross-process deduplication (COSMIC spawns multiple applet instances).
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let message_age_ms = now_ms - message.date;
                if message_age_ms > 30_000 {
                    // Message is older than 30 seconds, skip notification
                    return cosmic::app::Task::none();
                }

                // Check if we've already seen this message (deduplication)
                let last_seen = self.last_seen_sms.get(&message.thread_id).copied();
                if last_seen.is_some() && last_seen >= Some(message.date) {
                    // Already seen this message or an older one
                    return cosmic::app::Task::none();
                }

                // Update last seen timestamp for this thread
                self.last_seen_sms.insert(message.thread_id, message.date);

                // Capture config settings for the async block
                let show_sender = self.config.sms_notification_show_sender;
                let show_content = self.config.sms_notification_show_content;
                let message_body = message.body.clone();
                let primary_address = message.primary_address().to_string();

                // Show notification asynchronously (loads contacts without blocking UI)
                return cosmic::app::Task::perform(
                    async move {
                        // Load contacts asynchronously to resolve sender name
                        let contacts = ContactLookup::load_for_device(&device_id).await;
                        let sender_name = contacts.get_name_or_number(&primary_address);

                        // Build notification based on privacy settings
                        let summary = if show_sender {
                            fl!("sms-notification-title-from", sender = sender_name)
                        } else {
                            fl!("sms-notification-title")
                        };

                        let body = if show_content {
                            message_body
                        } else {
                            fl!("sms-notification-body-hidden")
                        };

                        // Use spawn_blocking to run notify_rust in a blocking context
                        // to avoid "Cannot start a runtime from within a runtime" panics
                        let result = tokio::task::spawn_blocking(move || {
                            notify_rust::Notification::new()
                                .summary(&summary)
                                .body(&body)
                                .icon("phone-symbolic")
                                .appname("COSMIC Connected")
                                .show()
                        })
                        .await;

                        if let Ok(Err(e)) = result {
                            tracing::warn!("Failed to show SMS notification: {}", e);
                        }
                    },
                    |_| cosmic::Action::App(Message::RefreshDevices),
                );
            }

            // Call Notifications
            Message::CallNotification {
                device_name,
                event,
                phone_number,
                contact_name,
            } => {
                // Build notification based on event type and privacy settings
                let (summary, icon, urgency) = match event.as_str() {
                    "callReceived" => {
                        let text = if self.config.call_notification_show_name
                            && !contact_name.is_empty()
                            && contact_name != phone_number
                        {
                            fl!("incoming-call-from", name = contact_name.clone())
                        } else if self.config.call_notification_show_number {
                            fl!("incoming-call-from", name = phone_number.clone())
                        } else {
                            fl!("incoming-call")
                        };
                        (text, "call-start-symbolic", notify_rust::Urgency::Critical)
                    }
                    "missedCall" => {
                        let text = if self.config.call_notification_show_name
                            && !contact_name.is_empty()
                            && contact_name != phone_number
                        {
                            fl!("missed-call-from", name = contact_name.clone())
                        } else if self.config.call_notification_show_number {
                            fl!("missed-call-from", name = phone_number.clone())
                        } else {
                            fl!("missed-call")
                        };
                        (text, "call-missed-symbolic", notify_rust::Urgency::Normal)
                    }
                    _ => {
                        tracing::debug!("Unknown call event type: {}", event);
                        return cosmic::app::Task::none();
                    }
                };

                tracing::info!(
                    "Call notification: {} - {} from {}",
                    event,
                    contact_name,
                    device_name
                );

                // Show notification
                return cosmic::app::Task::perform(
                    async move {
                        // Use spawn_blocking to run notify_rust in a blocking context
                        // to avoid "Cannot start a runtime from within a runtime" panics
                        let result = tokio::task::spawn_blocking(move || {
                            notify_rust::Notification::new()
                                .summary(&summary)
                                .body(&device_name)
                                .icon(icon)
                                .appname("COSMIC Connected")
                                .urgency(urgency)
                                .show()
                        })
                        .await;

                        if let Ok(Err(e)) = result {
                            tracing::warn!("Failed to show call notification: {}", e);
                        }
                    },
                    |_| cosmic::Action::App(Message::RefreshDevices),
                );
            }

            // File Notifications
            Message::FileReceived {
                device_name: device_id,
                file_url,
                file_name,
            } => {
                // Secondary deduplication check (primary is file-based cross-process dedup)
                if self.last_received_file.as_ref() == Some(&file_url) {
                    return cosmic::app::Task::none();
                }
                self.last_received_file = Some(file_url.clone());

                // Look up actual device name from cached devices
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| device_id.clone());

                // Only show notification if file notifications are enabled
                if self.config.file_notifications {
                    let summary = fl!("file-received-from", device = device_name.clone());
                    let file_name_clone = file_name.clone();

                    return cosmic::app::Task::perform(
                        async move {
                            // Use spawn_blocking to run notify_rust in a blocking context
                            let result = tokio::task::spawn_blocking(move || {
                                notify_rust::Notification::new()
                                    .summary(&summary)
                                    .body(&file_name_clone)
                                    .icon("folder-download-symbolic")
                                    .appname("COSMIC Connected")
                                    .timeout(notify_rust::Timeout::Milliseconds(FILE_TIMEOUT_MS))
                                    .show()
                            })
                            .await;

                            if let Ok(Err(e)) = result {
                                tracing::warn!("Failed to show file notification: {}", e);
                            }
                        },
                        |_| cosmic::Action::App(Message::RefreshDevices),
                    );
                }
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
            return popup_container(content, popup_width, self.core.applet.anchor);
        }

        // Handle loading state
        if self.loading && self.view_mode == ViewMode::DeviceList {
            let content: Element<Message> = widget::container(
                column![text(fl!("loading")).size(14),].align_x(Alignment::Center),
            )
            .padding(16)
            .into();
            return popup_container(content, popup_width, self.core.applet.anchor);
        }

        // Route to appropriate view based on view mode
        let content: Element<Message> = match &self.view_mode {
            ViewMode::Settings => view_settings(&self.config),
            ViewMode::ConversationList => view_conversation_list(ConversationListParams {
                device_name: self.sms_device_name.as_deref(),
                conversations: &self.conversations,
                conversations_displayed: self.conversations_displayed,
                contacts: &self.contacts,
                loading_state: &self.sms_loading_state,
            }),
            ViewMode::MessageThread => view_message_thread(MessageThreadParams {
                thread_addresses: self.current_thread_addresses.as_deref(),
                messages: &self.messages,
                contacts: &self.contacts,
                loading_state: &self.sms_loading_state,
                sms_compose_text: &self.sms_compose_text,
                sms_sending: self.sms_sending,
                messages_has_more: self.messages_has_more,
            }),
            ViewMode::NewMessage => view_new_message(NewMessageParams {
                recipient: &self.new_message_recipient,
                body: &self.new_message_body,
                recipient_valid: self.new_message_recipient_valid,
                sending: self.new_message_sending,
                contact_suggestions: &self.contact_suggestions,
            }),
            ViewMode::MediaControls => view_media_controls(MediaControlsParams {
                device_name: self.media_device_name.as_deref(),
                media_info: self.media_info.as_ref(),
                media_loading: self.media_loading,
            }),
            ViewMode::SendTo => view_send_to(SendToParams {
                device_type: self.sendto_device_type.as_deref().unwrap_or("device"),
                device_id: self.sendto_device_id.as_deref().unwrap_or_default(),
                share_text_input: &self.share_text_input,
                status_message: self.status_message.as_deref(),
            }),
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

        popup_container(content, popup_width, self.core.applet.anchor)
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
                cosmic::iced::time::every(std::time::Duration::from_secs(
                    refresh::MEDIA_INTERVAL_SECS,
                ))
                .map(|_| Message::MediaRefresh),
            );
        }

        // Add SMS notification subscription when enabled and devices are connected
        if self.config.sms_notifications
            && self.devices.iter().any(|d| d.is_reachable && d.is_paired)
        {
            subscriptions.push(Subscription::run(sms_notification_subscription));
        }

        // Add call notification subscription when enabled and devices are connected
        if self.config.call_notifications
            && self.devices.iter().any(|d| d.is_reachable && d.is_paired)
        {
            subscriptions.push(Subscription::run(call_notification_subscription));
        }

        // Note: File notifications are handled in the main dbus_signal_subscription
        // to avoid issues with multiple D-Bus connections and match rules

        Subscription::batch(subscriptions)
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}
