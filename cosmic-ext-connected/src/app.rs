//! Main application state and logic for the Connected applet.

use crate::config::Config;
use crate::constants::{
    dbus::{PENDING_REFRESH_TICK_SECS, SIGNAL_REFRESH_DEBOUNCE_SECS},
    notifications::{CALL_RING_TIMEOUT_MS, NORMAL_NOTIFICATION_TIMEOUT_MS},
    refresh,
};
use crate::device::{
    accept_pairing_async, dismiss_notification_async, fetch_devices_async, find_my_phone_async,
    force_network_change_async, reject_pairing_async, request_pair_async, send_clipboard_async,
    send_ping_async, share_file_async, share_text_async, unpair_async,
};
use crate::fl;
use crate::media::{
    fetch_media_info_async, media_action_async, view_media_controls, MediaAction,
    MediaControlsParams,
};
use crate::sms::{prefetch_conversations_async, SmsConversationStore, SmsViewMode};
use crate::subscriptions::{
    call_notification_subscription, dbus_signal_subscription, sms_notification_subscription,
};
use crate::ui;
use crate::views::send_to::{view_send_to, view_share_text, SendToParams, ShareTextParams};
use crate::views::settings::{view_about, view_settings};
use cosmic::app::Core;
use cosmic::iced::core::window;
use cosmic::iced::widget::{column, scrollable};
use cosmic::iced::{Alignment, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget;
use cosmic::{Application, Element};
use kdeconnect_dbus::{
    contacts::ContactLookup,
    plugins::{ConversationSummary, NotificationInfo, SmsMessage},
};
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
    /// Prod the daemon to re-scan the network, then refresh (manual Refresh button)
    ForceReconnect,
    /// Retry reaching KDE Connect from an error empty-list card. Re-establishes
    /// the session bus if we never had one, otherwsie re-fetches the device list.
    RetryConnection,
    /// Device list was updated
    DevicesUpdated(Vec<DeviceInfo>),
    /// D-Bus connection established
    DbusConnected(Arc<Mutex<Connection>>),
    /// D-Bus connection failed
    DbusConnectionFailed(String),
    /// The device fetch failed, so no device's reachability is currently known.
    /// Distinct from `SubscriptionRetrying`, which subscriptions emit for their
    /// own setup failures and which says nothing about device state.
    DeviceFetchFailed(DaemonUnavailable),
    // Navigation
    /// Select a device to view its detail page
    SelectDevice(String),
    /// Return to the device list
    BackToList,
    /// Open the "Send to device" submenu
    OpenSendToView(String, String), // device_id, device_type
    /// Return from SendTo view to device page
    BackFromSendTo,
    /// Open the focused Share Text compose view (non-mobile peers)
    OpenShareTextView(String, String), // device_id, device_type
    /// Return from ShareText view to device page
    BackFromShareText,

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
    /// Daemon reported a pairing failure for a device (`pairingFailed`).
    /// Carries the daemon's own localized error string.
    PairingFailed {
        device_id: String,
        error: String,
    },
    /// A subscription failed to set up and is retrying from `Init`.
    ///
    /// Not a device-state error: the stream recovers on its own, so this must
    /// never reach `self.error`, which pre-empts the entire popup view.
    SubscriptionRetrying(String),
    /// Clear the transient status message after a delay
    ClearStatusMessage,
    /// D-Bus signal received indicating device state changed
    DbusSignalReceived,
    /// Periodic tick to flush a pending refresh if the debounce window has cleared
    /// and no fetch is in flight to consume the flag naturally.
    CheckPendingRefresh,

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
    /// Open the About sub page
    OpenAbout,
    /// Return from About to main settings page
    BackFromAbout,
    /// Open an external URL in the default browser
    OpenUrl(String),
    /// Toggle a specific setting
    ToggleSetting(SettingKey),
    /// Expand/collapse a collapsible device group (Offline)
    ToggleDeviceGroup(GroupKind),
    /// Set the notification timeout duration (seconds)

    // SMS
    /// Open SMS view for a device
    OpenSmsView(String),
    /// Prefetched conversations ready from device selection background fetch
    SmsPrefetchReady(String, Vec<ConversationSummary>),
    /// Close SMS view and return to device list
    CloseSmsView,
    /// Open a specific conversation thread
    OpenConversation(i64),
    /// Close conversation and return to conversation list
    CloseConversation,
    /// Contacts loaded asynchronously for a device
    ContactsLoaded(String, ContactLookup),
    /// SMS-related error occurred
    SmsError(String),
    /// Update SMS compose text input
    SmsComposeAction(cosmic::widget::text_editor::Action),
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
    NewMessageBodyAction(cosmic::widget::text_editor::Action),
    /// Add the current input as a recipient chip (Enter key or add button)
    AddManualRecipient,
    /// Remove a recipient chip by index
    RemoveRecipient(usize),
    /// Select a contact from suggestions
    SelectContact(String, String), // name, phone
    /// Send a new message
    SendNewMessage,
    /// Deadline for the post-send sync indicator.
    NewMessageSyncSettle {
        device_id: String,
    },
    /// New message send result
    NewMessageSendResult(Result<String, String>),
    /// Ack that an older-page `requestConversation` was fired (`ok`) or failed.
    /// Carries no messages - those stream in via `ConversationMessagesReceived`.
    OlderMessagesRequested {
        thread_id: i64,
        ok: bool,
    },
    /// Message thread scrolled - used for prefetching older messages
    MessageThreadScrolled(scrollable::Viewport),
    /// User started pressing a message bubble (for long-press copy)
    BubblePressStarted {
        uid: i32,
        body: String,
    },
    /// User released press on message bubble
    BubblePressReleased,
    /// Hint timer completed (500ms elapsed) - show "Hold to copy" hint
    BubbleHintTimer,
    /// Long press timer completed (2s total elapsed) - copy to clipboard
    BubbleLongPressComplete,

    // Attachments
    /// Request and open a full-size MMS attachment
    OpenAttachment {
        device_id: String,
        device_name: String,
        part_id: i64,
        unique_identifier: String,
    },
    /// Attachment file is ready to open
    AttachmentReady(String),
    /// Attachment retrieval failed
    AttachmentError(String),

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

    // Subscription-based message loading
    /// Single message received from conversation subscription (incremental loading)
    ConversationMessageReceived {
        thread_id: i64,
        message: SmsMessage,
    },
    /// Local store read complete - scroll to bottom, keep listening for phone data
    ConversationStoreLoaded {
        thread_id: i64,
        total_count: u64,
    },
    /// All loading complete (phone response timeout) - finalize and drop subscription
    ConversationLoadComplete {
        thread_id: i64,
        total_count: u64,
    },
    /// Fire-and-forget D-Bus request completed, start listening for signals
    ConversationLoadStarted {
        thread_id: i64,
    },

    // Subscription-based conversation list loading
    /// Single conversation received via subscription (incremental update)
    ConversationReceived {
        device_id: String,
        conversation: ConversationSummary,
    },
    /// Bulk conversation delivery from the cached-drain paths. Carries a whole
    /// drain in one message so the store sorts, truncates, and re-derives once
    /// per batch instead of once per conversation.
    ConversationsBatchReceived {
        device_id: String,
        conversations: Vec<ConversationSummary>,
    },
    /// Conversation list sync started (show loading indicator)
    ConversationSyncStarted {
        device_id: String,
    },
    /// Conversation list sync complete (hide loading indicator)
    ConversationSyncComplete {
        device_id: String,
    },
    ConversationListStreamEnded {
        device_id: String,
    },
}

/// Keys for boolean settings that can be toggled.
#[derive(Debug, Clone)]
pub enum SettingKey {
    SmsNotifications,
    SmsShowContent,
    SmsShowSender,
    CallNotifications,
    CallShowNumber,
    CallShowName,
    FileNotifications,
    MergeReactionThreads,
}

/// Why the daemon could not be reached. Derived from the D-Bus error *name*.
/// Built by `device::fetch::classify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonUnavailable {
    /// `UnknownObject`/`UnknownInterface`. The bus activated the name but the
    /// daemon had not registered `/modules/kdeconnect`. Retried inside
    /// `fetch_devices_async`; reaching the UI means it outlived the budget,
    /// i.e. the daemon was launched and never finished coming up.
    Starting,
    /// `ServiceUnknown`. The bus has no activation service file for
    /// `org.kde.kdeconnect`, i.e. KDE Connect is not installed.
    NotInstalled,
    /// `Spawn.*`. Service file present, the bus could not exec it.
    FailedToStart,
    /// `NoReply`/`Timeout`. The name is owned; we got no reply.
    NotResponding,
    /// Anything else, including non-`MethodError` zbus errors.
    Other(String),
}

/// The latching error surface. While this is `Some`, `view_window` pre-empts
/// every other branch - loading, the device list, and the empty-list card.
///
/// Written by exactly two arms, `DbusConnectionFailed` and `DeviceFetchFailed`;
/// cleared by exactly two, `DbusConnected` and `DevicesUpdated`.
#[derive(Debug, Clone)]
pub enum ErrorState {
    /// The session bus itself is unreachable; nothing about the daemon is known.
    SessionBus(String),
    /// The session bus is fine; the daemon could not be reached.
    Daemon(DaemonUnavailable),
}

/// The device-list groups, in display order
/// Only `Offline` is collapsible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Connected,
    PairingRequests,
    Available,
    Offline,
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
    /// Send to device submenu (file, clipboard, ping, text) — mobile peers
    SendTo,
    /// Focused Share Text compose view — non-mobile peers
    ShareText,
    /// SMS conversation list for a device
    ConversationList,
    /// SMS message thread view
    MessageThread,
    /// New message compose view
    NewMessage,
    /// Settings view
    Settings,
    /// About sub-page
    About,
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
    error: Option<ErrorState>,
    /// Last pairing failure, as (device_id, daemon error string). Owned by the
    /// pairing flow: set when daemon reports a failure, cleared only when the
    /// user starts another attempt or leaves the page. Deliberately NOT
    /// status_message and NOT DeviceInfo - pairingFailed is followed ~20µs later
    /// by pairStateChanged, whose refresh clears status_message and
    /// replaces self.devices, so either home would erase the reason.
    pairing_error: Option<(String, String)>,
    /// Device whose pairing we cancelled ourselves. The daemon reports a
    /// user-initiated cancel as pairingFailed("Cancelled by user"), which is not
    /// an error worth showing. Consumed by the next PairingFailed.
    pairing_cancel_expected: Option<String>,
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
    /// True when at least one D-Bus signal has been dropped by the debounce
    /// window since the last served fetch. Cleared whenever a fetch is
    /// dispatched. Checked when `DevicesUpdated` arrives — if set, one more
    /// fetch is kicked to pick up state changes carried by debounced signals.
    signal_refresh_pending: bool,

    // SMS state
    sms: SmsConversationStore,

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

    // File notification deduplication
    /// Last received file URL to avoid duplicate notifications
    last_received_file: Option<String>,
}

impl ConnectApplet {
    /// The session-bus connect, shared by start and the retry affordance so
    /// both map success and failure onto the same two messages.
    fn connect_dbus_task() -> cosmic::app::Task<Message> {
        cosmic::app::Task::perform(async { Connection::session().await }, |result| {
            cosmic::Action::App(match result {
                Ok(conn) => Message::DbusConnected(Arc::new(Mutex::new(conn))),
                Err(e) => Message::DbusConnectionFailed(e.to_string()),
            })
        })
    }

    /// Set a transient status message that auto-clears after 3 seconds.
    fn set_transient_status(&mut self, msg: String) -> cosmic::app::Task<Message> {
        self.status_message = Some(msg);
        cosmic::app::Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            },
            |_| cosmic::Action::App(Message::ClearStatusMessage),
        )
    }

    /// Apply an `SmsReply` returned by `SmsConversationStore::update()`.
    /// Caller batches the returned task with the store's task.
    fn handle_sms_reply(&mut self, reply: crate::sms::SmsReply) -> cosmic::app::Task<Message> {
        match reply {
            crate::sms::SmsReply::Status(msg) => self.set_transient_status(msg),
            crate::sms::SmsReply::SetStatus(opt) => {
                self.status_message = opt;
                cosmic::app::Task::none()
            }
            crate::sms::SmsReply::NewMessageSent(msg) => {
                self.status_message = Some(msg);
                self.view_mode = ViewMode::ConversationList;
                cosmic::app::Task::none()
            }
            crate::sms::SmsReply::NoOp => cosmic::app::Task::none(),
        }
    }
}

impl Application for ConnectApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = crate::config::APP_ID;

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
            pairing_error: None,
            pairing_cancel_expected: None,
            status_message: None,
            dbus_connection: None,
            loading: true,
            view_mode: ViewMode::DeviceList,
            selected_device: None,
            pending_share_device: None,
            share_text_input: String::new(),
            last_signal_refresh: std::time::Instant::now(),
            signal_refresh_pending: false,
            // SMS state
            sms: SmsConversationStore::new(),
            // Media controls state
            media_device_id: None,
            media_device_name: None,
            media_info: None,
            media_loading: false,
            media_selected_player: None,
            // SendTo state
            sendto_device_id: None,
            sendto_device_type: None,
            // File notification deduplication
            last_received_file: None,
        };

        // Connect to D-Bus on startup
        let task = Self::connect_dbus_task();
        (app, task)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                let action = if let Some(popup_id) = self.popup.take() {
                    destroy_popup(popup_id)
                } else {
                    app_popup::<ConnectApplet>(
                        |_| Default::default(),
                        |state: &mut ConnectApplet| {
                            let new_id = window::Id::unique();
                            state.popup = Some(new_id);
                            state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            )
                        },
                        None,
                    )
                };
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(action),
                ));
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
                self.error = Some(ErrorState::SessionBus(err));
                self.loading = false;
            }
            Message::ForceReconnect => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::debug!("Forcing network re-scan");
                    self.loading = true;
                    self.status_message = None;
                    return cosmic::app::Task::perform(
                        force_network_change_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::RetryConnection => {
                self.loading = true;
                // Deliberately does NOT clear self.error -- that stays with
                // DbusConnected and DeviceUpdated, so the card only goes away
                // once something has actually succeeded
                return match &self.dbus_connection {
                    Some(conn) => cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    ),
                    None => Self::connect_dbus_task(),
                };
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
                // If the SMS device just came back reachable, the daemon likely restarted
                // or the phone rejoined and whatever we had in flight was orphaned. Bump
                // the affected subscription generations → iced re-Inits each recipe → the
                // requests re-fire from scratch (the proven full recovery).
                // Computed once, consumed by two independent guards; the list and the
                // per-thread message subscriptions have separate lifecycles
                if let Some(sms_id) = self.sms.sms_device_id.as_deref() {
                    let was_reachable = self
                        .devices
                        .iter()
                        .find(|d| d.id == sms_id)
                        .map(|d| d.is_reachable)
                        .unwrap_or(false);
                    let now_reachable = devices
                        .iter()
                        .find(|d| d.id == sms_id)
                        .map(|d| d.is_reachable)
                        .unwrap_or(false);
                    // The recovery edge is computed from `self.devices` (our previous
                    // view) against the fetch that just landed. A failed fetch leaves
                    // `self.devices` untouched, so a stale `true` can make a genuine
                    // reconnect read as `true -> true` and skip the bump entirely.
                    // Log the pair so that comparison is observable, not inferred.
                    tracing::debug!(
                        "SMS device {} reachability: was={} now={} (bump {})",
                        sms_id,
                        was_reachable,
                        now_reachable,
                        if !was_reachable && now_reachable {
                            "FIRES"
                        } else {
                            "skipped"
                        }
                    );
                    if !was_reachable && now_reachable {
                        if self.sms.conversation_list_subscription_active {
                            self.sms.conversation_list_subscription_generation = self
                                .sms
                                .conversation_list_subscription_generation
                                .wrapping_add(1);
                            tracing::info!(
                                "SMS device {} became reachable; regenerating conversation-list \
                                subscription (gen {}) to re-sync",
                                sms_id,
                                self.sms.conversation_list_subscription_generation
                            );
                        }
                        if self.sms.conversation_load_active {
                            // Recovery re-runs Init, so it must also re-run the load
                            // bookkeeping OpenConversation does - otherwise the
                            // re-streamed page is deduped against stale uids and
                            // `messages` becomes the union of pre- and post-restart
                            // data. Mirrors app.rs OpenConversation.
                            self.sms.messages_has_more = true;
                            self.sms.older_page = None;
                            self.sms.thread_has_more = self
                                .sms
                                .current_merged_thread_ids
                                .iter()
                                .map(|&t| (t, true))
                                .collect();
                            self.sms.known_message_ids.clear();
                            self.sms.messages.clear();
                            self.sms.initial_load_complete = false;
                            self.sms.conversation_message_subscription_generation = self
                                .sms
                                .conversation_message_subscription_generation
                                .wrapping_add(1);
                            tracing::info!(
                                "SMS device {} became reachable; regenerating message \
                                subscriptions for {} open thread(s) (gen {}) to re-request",
                                sms_id,
                                self.sms.current_merged_thread_ids.len(),
                                self.sms.conversation_message_subscription_generation
                            );
                        }
                    }
                }

                tracing::debug!("Devices updated: {} devices", devices.len());
                self.devices = devices;
                self.error = None;
                self.loading = false;
                self.status_message = None; // Clear status after refresh

                // If any signals were dropped while this fetch was in flight,
                // kick one more fetch to pick up settled state. See the
                // signal_refresh_pending field doc for rationale.
                if self.signal_refresh_pending {
                    self.signal_refresh_pending = false;
                    if let Some(conn) = &self.dbus_connection {
                        self.last_signal_refresh = std::time::Instant::now();
                        return cosmic::app::Task::perform(
                            fetch_devices_async(conn.clone()),
                            cosmic::Action::App,
                        );
                    }
                }
            }
            Message::SubscriptionRetrying(what) => {
                // The emitter already logged the underlying error; this arm
                // exists because an unfold must yield a message to keep the
                // stream alive. Deliberately no UI: a persistent daemon problem
                // surfaces through DeviceFetchFailed, which is about device state.
                tracing::debug!("Subscription retrying: {}", what);
            }
            Message::DeviceFetchFailed(why) => {
                // We could not reach the daemon, so no device's reachability is
                // known. Leaving the last-known `true` in place makes the next
                // successful fetch read as `true -> true`, and the recovery edge
                // in DevicesUpdated never fires - the daemon restarts, the phone
                // reconnects inside the refresh debounce, and the open thread
                // stays frozen. Mark unreachable rather than clearing: names,
                // battery and notifications survive, and the device list does
                // not flash empty.
                tracing::warn!(
                    "Device fetch failed, marking devices unreachable: {:?}",
                    why
                );
                for d in &mut self.devices {
                    d.is_reachable = false;
                }
                self.error = Some(ErrorState::Daemon(why));
                self.loading = false;
            }
            Message::ClearStatusMessage => {
                self.status_message = None;
            }

            // Navigation
            Message::SelectDevice(device_id) => {
                self.selected_device = Some(device_id.clone());
                self.view_mode = ViewMode::DevicePage;
                self.share_text_input.clear();

                // Prefetch SMS conversations so they're ready when user opens SMS
                if let Some(conn) = &self.dbus_connection {
                    return cosmic::app::Task::perform(
                        prefetch_conversations_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::BackToList => {
                self.selected_device = None;
                self.view_mode = ViewMode::DeviceList;
                self.share_text_input.clear();
                self.sms.sms_prefetch = None;
                self.pairing_error = None;
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
            Message::OpenShareTextView(device_id, device_type) => {
                self.sendto_device_id = Some(device_id);
                self.sendto_device_type = Some(device_type);
                self.view_mode = ViewMode::ShareText;
                return widget::text_input::focus(widget::Id::new("share-text-input"));
            }
            Message::BackFromShareText => {
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
                    return self.set_transient_status("Ping sent!".to_string());
                }
                Err(e) => {
                    tracing::error!("Ping failed: {}", e);
                    return self.set_transient_status(format!("Ping failed: {}", e));
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
                    return self.set_transient_status(fl!("phone-ringing"));
                }
                Err(e) => {
                    tracing::error!("Find my phone failed: {}", e);
                    return self.set_transient_status(format!(
                        "{}: {}",
                        fl!("find-phone-failed"),
                        e
                    ));
                }
            },

            // Share
            Message::ShareFile(device_id) => {
                self.pending_share_device = Some(device_id);
                return cosmic::task::future(async {
                    use cosmic::dialog::file_chooser;
                    let result = file_chooser::open::Dialog::new()
                        .title("Share File")
                        .open_file()
                        .await;
                    match result {
                        Ok(response) => Message::FileSelected(response.url().to_file_path().ok()),
                        Err(_) => Message::FileSelected(None),
                    }
                });
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
                if !text.is_empty() {
                    if let Some(conn) = &self.dbus_connection {
                        self.share_text_input.clear();
                        self.status_message = Some("Sharing text...".to_string());
                        return cosmic::app::Task::perform(
                            share_text_async(conn.clone(), device_id, text),
                            |result| cosmic::Action::App(Message::ShareComplete(result)),
                        );
                    }
                }
            }
            Message::ShareComplete(result) => match result {
                Ok(()) => {
                    tracing::info!("Share completed successfully");
                    return self.set_transient_status("Shared successfully!".to_string());
                }
                Err(e) => {
                    tracing::error!("Share failed: {}", e);
                    return self.set_transient_status(format!("Share failed: {}", e));
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
                    self.pairing_error = None;
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

                    // An offline device would re-render the "must be connected" page
                    // after unpair - pop back to the list so the status bar carries
                    // the result. Reachable devices stay so you can re-pair in place.
                    let reachable = self
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .map(|d| d.is_reachable)
                        .unwrap_or(false);
                    if !reachable {
                        self.selected_device = None;
                        self.view_mode = ViewMode::DeviceList;
                    }

                    return cosmic::app::Task::perform(
                        unpair_async(conn.clone(), device_id),
                        cosmic::Action::App,
                    );
                }
            }
            Message::AcceptPairing(device_id) => {
                if let Some(conn) = &self.dbus_connection {
                    tracing::info!("Accepting pairing from device: {}", device_id);
                    self.pairing_error = None;
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
                    self.pairing_error = None;
                    self.pairing_cancel_expected = Some(device_id.clone());
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
            Message::PairingFailed { device_id, error } => {
                // Our Reject/Cancel routes through the daemon's cancelPairing, which
                // reports back as pairingFailed("Cancelled by user") — confirmed on
                // the wire 2026-07-29. Don't show an error for a deliberate action.
                if self.pairing_cancel_expected.as_deref() == Some(device_id.as_str()) {
                    self.pairing_cancel_expected = None;
                    tracing::debug!("Ignoring self-initiated pairing cancel for {}", device_id);
                    return cosmic::app::Task::none();
                }
                tracing::warn!("Pairing failed for {}: {}", device_id, error);
                self.pairing_error = Some((device_id, error));
            }
            Message::DbusSignalReceived => {
                // D-Bus signal received - debounce to avoid excessive refreshes.
                // Signals dropped by the debounce window flip a pending flag;
                // when the in-flight fetch completes, DevicesUpdated kicks one
                // more fetch to pick up settled state from the debounced burst.
                let now = std::time::Instant::now();
                let elapsed = now.duration_since(self.last_signal_refresh);
                let debounce = std::time::Duration::from_secs(SIGNAL_REFRESH_DEBOUNCE_SECS);
                if elapsed < debounce {
                    self.signal_refresh_pending = true;
                    return cosmic::app::Task::none();
                }

                if let Some(conn) = &self.dbus_connection {
                    tracing::debug!("D-Bus signal received, refreshing devices");
                    self.last_signal_refresh = now;
                    // The dispatched fetch will see the latest state, so any
                    // signal-triggered staleness up to this moment is covered.
                    self.signal_refresh_pending = false;
                    return cosmic::app::Task::perform(
                        fetch_devices_async(conn.clone()),
                        cosmic::Action::App,
                    );
                }
            }
            Message::CheckPendingRefresh => {
                // Periodic tick: if a signal was debounced and no fetch is in
                // flight to consume the pending flag via DevicesUpdated, flush
                // it once the debounce window has cleared. Without this, a
                // user accepting a pair request quickly after the request was
                // sent could leave the UI stuck on "waiting for acceptance"
                // until the next ambient signal (battery refresh, etc.).
                if self.signal_refresh_pending {
                    let now = std::time::Instant::now();
                    let elapsed = now.duration_since(self.last_signal_refresh);
                    let debounce = std::time::Duration::from_secs(SIGNAL_REFRESH_DEBOUNCE_SECS);
                    if elapsed >= debounce {
                        if let Some(conn) = &self.dbus_connection {
                            self.signal_refresh_pending = false;
                            self.last_signal_refresh = now;
                            return cosmic::app::Task::perform(
                                fetch_devices_async(conn.clone()),
                                cosmic::Action::App,
                            );
                        }
                    }
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
                    return self.set_transient_status(msg.clone());
                }
                Err(err) => {
                    tracing::error!("Clipboard error: {}", err);
                    return self.set_transient_status(format!("Clipboard error: {}", err));
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
            Message::OpenAbout => {
                self.view_mode = ViewMode::About;
            }
            Message::BackFromAbout => {
                self.view_mode = ViewMode::DeviceList;
            }
            Message::OpenUrl(url) => {
                return cosmic::app::Task::perform(
                    async move {
                        let _ = tokio::process::Command::new("xdg-open").arg(url).spawn();
                    },
                    |_| cosmic::Action::App(Message::RefreshDevices),
                );
            }
            Message::ToggleSetting(key) => {
                match key {
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
                    SettingKey::MergeReactionThreads => {
                        self.config.merge_reaction_threads = !self.config.merge_reaction_threads;
                        self.sms.rederive_conversations(&self.config);
                    }
                }
                tracing::debug!("Settings updated");
                // Save config to disk
                if let Err(err) = self.config.save() {
                    tracing::error!(?err, "Failed to save config");
                }
            }
            Message::ToggleDeviceGroup(kind) => {
                if kind == GroupKind::Offline {
                    self.config.group_offline_expanded = !self.config.group_offline_expanded;
                }

                if let Err(err) = self.config.save() {
                    tracing::error!(?err, "Failed to save config");
                }
            }

            // SMS
            Message::OpenSmsView(device_id) => {
                if self.dbus_connection.is_some() {
                    // Find device name for header
                    let device_name = self
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .map(|d| d.name.clone());

                    // Check if we have cached conversations for this device
                    let same_device = self.sms.sms_device_id.as_ref() == Some(&device_id);
                    let has_cache = same_device && !self.sms.conversations.is_empty();

                    self.view_mode = ViewMode::ConversationList;
                    self.sms.sms_device_id = Some(device_id.clone());
                    self.sms.sms_device_name = device_name;

                    // Per-device caches; otherwise the prior device's raw entries bleed into the new list when its subscription re-derives.
                    if !same_device {
                        self.sms.contacts = ContactLookup::default();
                        self.sms.raw_conversations.clear();
                    }

                    // Load contacts if not already loaded for this device
                    let needs_contacts = self.sms.contacts.is_empty();
                    let contacts_task = if needs_contacts {
                        let device_id_for_contacts = device_id.clone();
                        cosmic::app::Task::perform(
                            async move {
                                let contacts =
                                    ContactLookup::load_for_device(&device_id_for_contacts).await;
                                Message::ContactsLoaded(device_id_for_contacts, contacts)
                            },
                            cosmic::Action::App,
                        )
                    } else {
                        cosmic::app::Task::none()
                    };

                    // Check if we have prefetched conversations for this device
                    let has_prefetch = self
                        .sms
                        .sms_prefetch
                        .as_ref()
                        .is_some_and(|(id, convs)| id == &device_id && !convs.is_empty());

                    if has_cache {
                        // Use in-memory cached conversations, enable subscription for background refresh
                        self.sms.sms_loading_state = SmsLoadingState::Idle; // Show cached data immediately
                        self.sms.conversation_sync_active = true; // Show sync indicator
                        self.sms.conversation_list_subscription_active = true; // Enable subscription
                        tracing::info!(
                            "Using cached {} conversations for device: {}, starting subscription-based sync",
                            self.sms.conversations.len(),
                            device_id
                        );
                        // Subscription will handle background sync
                        return contacts_task;
                    } else if has_prefetch {
                        // Use prefetched conversations from device selection
                        if let Some((_, prefetched)) = self.sms.sms_prefetch.take() {
                            // Seed last_seen_sms to prevent false notifications
                            for conv in &prefetched {
                                let key = (device_id.clone(), conv.thread_id);
                                let current = self.sms.last_seen_sms.get(&key).copied();
                                if current.is_none() || current < Some(conv.timestamp) {
                                    self.sms.last_seen_sms.insert(key, conv.timestamp);
                                }
                            }
                            // Seed raw_conversations and re-derive so the merge toggle works before the subscription's first refresh.
                            self.sms.raw_conversations = prefetched;
                            self.sms.rederive_conversations(&self.config);
                            self.sms.sms_loading_state = SmsLoadingState::Idle;
                            self.sms.conversation_sync_active = true;
                            self.sms.conversation_list_subscription_active = true;
                            tracing::info!(
                                "Using prefetched {} conversations for device: {}, starting subscription-based sync",
                                self.sms.conversations.len(),
                                device_id
                            );
                        }
                        return contacts_task;
                    } else {
                        // No cache or different device - subscription-based loading
                        // Conversations will arrive incrementally via signals
                        self.sms.sms_loading_state =
                            SmsLoadingState::LoadingConversations(LoadingPhase::Connecting);
                        self.sms.conversation_sync_active = true;
                        self.sms.conversation_list_subscription_active = true; // Enable subscription
                        self.sms.conversations.clear();
                        tracing::info!(
                            "Opening SMS view for device: {} (subscription-based loading)",
                            device_id
                        );

                        // Load contacts in parallel - subscription handles conversation loading
                        return contacts_task;
                    }
                }
            }
            Message::CloseSmsView => {
                self.view_mode = ViewMode::DevicePage;
                // Keep sms_device_id, sms_device_name, conversations, contacts
                // for when user returns to SMS view
                self.sms.messages.clear();
                self.sms.current_thread_id = None;
                self.sms.current_thread_addresses = None;
                self.sms.current_merged_thread_ids.clear();
                self.sms.sms_loading_state = SmsLoadingState::Idle;
                self.sms.conversation_sync_active = false;
                self.sms.new_message_sync_active = false;
                self.sms.conversation_list_subscription_active = false;
                self.sms.sms_compose_text = widget::text_editor::Content::new();
                self.sms.sms_sending = false;
                self.sms.sms_sending_body = None;
            }
            Message::OpenConversation(thread_id) => {
                // Guard: need D-Bus connection and device ID for the subscription
                if self.dbus_connection.is_some() && self.sms.sms_device_id.is_some() {
                    // Find the conversation for header info and deduplication
                    let conversation = self
                        .sms
                        .conversations
                        .iter()
                        .find(|lc| lc.primary_thread_id == thread_id);

                    let addresses = conversation.map(|c| c.addresses.clone());
                    let merged_thread_ids = conversation
                        .map(|c| c.merged_thread_ids.clone())
                        .unwrap_or_else(|| vec![thread_id]);

                    // Pre-populate last_seen_sms with current time to prevent false notifications
                    // when fetching existing messages in this thread.
                    // Using current time (in milliseconds) ensures ALL existing messages
                    // are considered "seen" - only truly new messages arriving after this
                    // point will trigger notifications.
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    if let Some(device_id) = self.sms.sms_device_id.clone() {
                        self.sms
                            .last_seen_sms
                            .insert((device_id, thread_id), now_ms);
                    }

                    self.sms.current_thread_id = Some(thread_id);
                    self.sms.current_thread_addresses = addresses;
                    self.sms.current_merged_thread_ids = merged_thread_ids;
                    self.view_mode = ViewMode::MessageThread;

                    // Reset pagination state
                    self.sms.messages_has_more = true;
                    self.sms.older_page = None;
                    self.sms.thread_has_more = self
                        .sms
                        .current_merged_thread_ids
                        .iter()
                        .map(|&t| (t, true))
                        .collect();

                    // Clear known message IDs for fresh deduplication
                    self.sms.known_message_ids.clear();
                    self.sms.messages.clear();

                    // Set up subscription-based loading state
                    // The subscription will fire the D-Bus request after setting up match rules
                    self.sms.conversation_load_active = true;
                    self.sms.initial_load_complete = false;

                    // Always load through daemon to ensure its cache is primed
                    // (replyToConversation requires the daemon to have loaded the conversation)
                    self.sms.sms_loading_state =
                        SmsLoadingState::LoadingMessages(LoadingPhase::Connecting);
                    self.sms.message_sync_active = false;
                    tracing::info!(
                        "Opening conversation thread: {} (subscription-based loading)",
                        thread_id
                    );
                    // Subscription will fire D-Bus request and handle incoming signals
                }
            }
            Message::CloseConversation => {
                self.view_mode = ViewMode::ConversationList;
                self.sms.current_thread_id = None;
                self.sms.current_thread_addresses = None;
                self.sms.current_merged_thread_ids.clear();
                self.sms.thread_has_more.clear();
                self.sms.messages.clear();
                self.sms.sms_compose_text = widget::text_editor::Content::new();
                self.sms.sms_sending = false;
                self.sms.sms_sending_body = None;
                self.sms.message_sync_active = false;

                // Clear subscription-based loading state
                self.sms.conversation_load_active = false;
                self.sms.initial_load_complete = false;
                self.sms.known_message_ids.clear();

                // The conversation-list subscription is still live and already holds
                // everything a refetch would find, so this is a re-assert rather than a
                // repair: since (e045ed5) SmsError no longer clears view-lifecycle
                // flags, and only CloseSmsView does. Let the live subscription drive the list.
                if self.sms.sms_device_id.is_some() {
                    self.sms.conversation_list_subscription_active = true;
                }

                if self.sms.conversations.is_empty() {
                    self.sms.sms_loading_state = if self.sms.conversation_list_subscription_active {
                        SmsLoadingState::LoadingConversations(LoadingPhase::Connecting)
                    } else {
                        SmsLoadingState::Idle
                    };
                }

                // Reset the thread's scroll offset on the way out, not the list's.
                // iced matches scrollable widget state by tree position, not by id
                // (a scrollable reports Internal::Set from Widget::id(), which no
                // name-matching path in libcosmic accepts), so the conversation list
                // lands on the slot the message thread just vacated and inherits its
                // offset. The thread is anchored to the bottom, so that offset is
                // measured from the newest message and is non-zero whenever the
                // user scrolled up; the top-anchored list would read the same
                // number as a distance from its top. START (relative 0) is the
                // top for the list regardless of the thread's anchor. The same
                // positional match overwrites the list scrollable's own id, so
                // "message-thread" is the id it answers to here. If that tree
                // alignment ever changes, the list gets fresh state (offset 0,
                // newest first) and this becomes a harmless no-op
                return scrollable::snap_to(
                    widget::Id::new("message-thread"),
                    scrollable::RelativeOffset::START.into(),
                );
            }

            // Subscription-based conversation list loading handlers

            // Subscription-based message loading handlers

            // New message
            Message::OpenNewMessage => {
                self.view_mode = ViewMode::NewMessage;
                self.sms.new_message_recipients.clear();
                self.sms.new_message_recipient_input.clear();
                self.sms.new_message_body = widget::text_editor::Content::new();
                self.sms.new_message_sending = false;
                self.sms.contact_suggestions.clear();
                return widget::text_input::focus(widget::Id::new("new-message-recipient"));
            }
            Message::CloseNewMessage => {
                self.view_mode = ViewMode::ConversationList;
                self.sms.new_message_recipients.clear();
                self.sms.new_message_recipient_input.clear();
                self.sms.new_message_body = widget::text_editor::Content::new();
                self.sms.new_message_sending = false;
            }

            // Attachment messages

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
                let clear_task = if let Err(err) = result {
                    self.set_transient_status(format!("Media error: {}", err))
                } else {
                    cosmic::app::Task::none()
                };
                // Refresh media info after action
                if let (Some(conn), Some(device_id)) =
                    (&self.dbus_connection, &self.media_device_id)
                {
                    return cosmic::app::Task::batch(vec![
                        cosmic::app::Task::perform(
                            fetch_media_info_async(conn.clone(), device_id.clone()),
                            cosmic::Action::App,
                        ),
                        clear_task,
                    ]);
                }
                return clear_task;
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

            // Call Notifications
            Message::CallNotification {
                device_name,
                event,
                phone_number,
                contact_name,
            } => {
                // Build notification based on event type and privacy settings
                let (summary, icon, urgency, timeout_ms) = match event.as_str() {
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
                        (
                            text,
                            "call-start-symbolic",
                            notify_rust::Urgency::Critical,
                            CALL_RING_TIMEOUT_MS,
                        )
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
                        (
                            text,
                            "call-missed-symbolic",
                            notify_rust::Urgency::Normal,
                            NORMAL_NOTIFICATION_TIMEOUT_MS,
                        )
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
                        let mut notification = notify_rust::Notification::new();
                        notification
                            .summary(&summary)
                            .body(&device_name)
                            .icon(icon)
                            .appname("Connected")
                            .urgency(urgency)
                            .timeout(notify_rust::Timeout::Milliseconds(timeout_ms));
                        match tokio::task::spawn_blocking(move || notification.show()).await {
                            Ok(Ok(_handle)) => tracing::debug!("Call notification shown"),
                            Ok(Err(e)) => tracing::warn!("Failed to show call notification: {}", e),
                            Err(e) => tracing::warn!("Call notification task panicked: {}", e),
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
                            let mut notification = notify_rust::Notification::new();
                            notification
                                .summary(&summary)
                                .body(&file_name_clone)
                                .icon("folder-download-symbolic")
                                .appname("Connected")
                                .timeout(notify_rust::Timeout::Milliseconds(
                                    NORMAL_NOTIFICATION_TIMEOUT_MS,
                                ));
                            match tokio::task::spawn_blocking(move || notification.show()).await {
                                Ok(Ok(_handle)) => tracing::debug!("File notification shown"),
                                Ok(Err(e)) => {
                                    tracing::warn!("Failed to show file notification: {}", e)
                                }
                                Err(e) => tracing::warn!("File notification task panicked: {}", e),
                            }
                        },
                        |_| cosmic::Action::App(Message::RefreshDevices),
                    );
                }
            }

            // Delegate SMS state-machine arms to SmsConversationStore.
            // The 6 SMS lifecycle arms (OpenSmsView, CloseSmsView, OpenConversation,
            // CloseConversation, OpenNewMessage, CloseNewMessage) stay inline above
            // because they touch view_mode (app-owned).
            Message::SmsPrefetchReady(_, _)
            | Message::ContactsLoaded(_, _)
            | Message::ConversationReceived { .. }
            | Message::NewMessageSyncSettle { .. }
            | Message::ConversationSyncStarted { .. }
            | Message::ConversationSyncComplete { .. }
            | Message::ConversationListStreamEnded { .. }
            | Message::OlderMessagesRequested { .. }
            | Message::MessageThreadScrolled(_)
            | Message::BubblePressStarted { .. }
            | Message::BubblePressReleased
            | Message::BubbleHintTimer
            | Message::BubbleLongPressComplete
            | Message::ConversationLoadStarted { .. }
            | Message::ConversationsBatchReceived { .. }
            | Message::ConversationMessageReceived { .. }
            | Message::ConversationStoreLoaded { .. }
            | Message::ConversationLoadComplete { .. }
            | Message::SmsError(_)
            | Message::SmsComposeAction(_)
            | Message::SendSms
            | Message::SmsSendResult(_)
            | Message::NewMessageRecipientInput(_)
            | Message::NewMessageBodyAction(_)
            | Message::AddManualRecipient
            | Message::RemoveRecipient(_)
            | Message::SelectContact(_, _)
            | Message::SendNewMessage
            | Message::NewMessageSendResult(_)
            | Message::OpenAttachment { .. }
            | Message::AttachmentReady(_)
            | Message::AttachmentError(_)
            | Message::SmsNotificationReceived(_, _) => {
                let ctx = crate::sms::SmsCtx {
                    conn: self.dbus_connection.as_ref(),
                    config: &self.config,
                };
                let (sms_task, reply) = self.sms.update(message, &ctx);
                let reply_task = self.handle_sms_reply(reply);
                return cosmic::app::Task::batch([sms_task, reply_task]);
            }
        }

        cosmic::app::Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        // Panel button with icon
        let icon_name = if self.devices.iter().any(|d| d.is_reachable && d.is_paired) {
            "io.github.nwxnw.cosmic-ext-connected-symbolic"
        } else {
            "io.github.nwxnw.cosmic-ext-connected-disconnected-symbolic"
        };

        self.core
            .applet
            .icon_button(icon_name)
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Self::Message> {
        let sp = cosmic::theme::spacing();

        // Handle error state first
        if let Some(err) = &self.error {
            let (heading, detail) = match err {
                ErrorState::SessionBus(raw) => (fl!("daemon-unreachable"), Some(raw.clone())),
                ErrorState::Daemon(DaemonUnavailable::Starting) => (fl!("daemon-starting"), None),
                ErrorState::Daemon(DaemonUnavailable::FailedToStart) => {
                    (fl!("daemon-not-started"), None)
                }
                ErrorState::Daemon(DaemonUnavailable::NotInstalled) => {
                    (fl!("daemon-not-found"), None)
                }
                ErrorState::Daemon(DaemonUnavailable::NotResponding) => {
                    (fl!("daemon-not-responding"), None)
                }
                ErrorState::Daemon(DaemonUnavailable::Other(raw)) => {
                    (fl!("error"), Some(raw.clone()))
                }
            };

            let mut col = column![widget::text::heading(heading)]
                .spacing(sp.space_xxs)
                .align_x(Alignment::Center);
            if let Some(detail) = detail {
                col = col.push(widget::text::caption(detail));
            }
            let retry: Element<Message> = if self.loading {
                widget::button::standard(fl!("retry"))
                    .leading_icon(widget::icon::from_name("process-working-symbolic").size(16))
                    .into()
            } else {
                widget::button::standard(fl!("retry"))
                    .on_press(Message::RetryConnection)
                    .into()
            };
            col = col.push(retry);
            let content: Element<Message> = widget::container(col).padding(sp.space_s).into();
            return self.core.applet.popup_container(content).into();
        }

        // Handle loading state
        if self.loading && self.view_mode == ViewMode::DeviceList {
            let content: Element<Message> = widget::container(
                column![widget::text::body(fl!("loading")),].align_x(Alignment::Center),
            )
            .padding(sp.space_s)
            .into();
            return self.core.applet.popup_container(content).into();
        }

        // Route to appropriate view based on view mode
        let content: Element<Message> = match &self.view_mode {
            ViewMode::About => view_about(),
            ViewMode::Settings => view_settings(&self.config),
            ViewMode::ConversationList => self.sms.view(
                SmsViewMode::ConversationList,
                &self.config,
                self.status_message.as_deref(),
            ),
            ViewMode::MessageThread => self.sms.view(
                SmsViewMode::MessageThread,
                &self.config,
                self.status_message.as_deref(),
            ),
            ViewMode::NewMessage => self.sms.view(
                SmsViewMode::NewMessage,
                &self.config,
                self.status_message.as_deref(),
            ),
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
            ViewMode::ShareText => view_share_text(ShareTextParams {
                device_type: self.sendto_device_type.as_deref().unwrap_or("device"),
                device_id: self.sendto_device_id.as_deref().unwrap_or_default(),
                share_text_input: &self.share_text_input,
                status_message: self.status_message.as_deref(),
            }),
            ViewMode::DevicePage => {
                if let Some(device_id) = &self.selected_device {
                    if let Some(device) = self.devices.iter().find(|d| &d.id == device_id) {
                        ui::device_page::view(
                            device,
                            self.status_message.as_deref(),
                            self.pairing_error
                                .as_ref()
                                .filter(|(id, _)| id == &device.id)
                                .map(|(_, err)| err.as_str()),
                        )
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
                            widget::text::heading(fl!("no-devices")),
                            widget::text::caption(fl!("no-devices-hint")),
                            widget::button::icon(widget::icon::from_name("notification-symbolic"))
                                .on_press(Message::ToggleSettings),
                            widget::button::standard(fl!("retry"))
                                .on_press(Message::RetryConnection),
                        ]
                        .spacing(sp.space_xxs)
                        .align_x(Alignment::Center),
                    )
                    .padding(sp.space_s)
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

        self.core.applet.popup_container(content).into()
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
            // Periodically check for a deferred refresh if signals were
            // dropped by the debounce window with no fetch in flight to
            // flush the pending flag.
            cosmic::iced::time::every(std::time::Duration::from_secs(PENDING_REFRESH_TICK_SECS))
                .map(|_| Message::CheckPendingRefresh),
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

        // SMS-state-driven subscriptions (conversation list + per-thread messages)
        subscriptions.extend(self.sms.subscriptions());

        // Note: File notifications are handled in the main dbus_signal_subscription
        // to avoid issues with multiple D-Bus connections and match rules

        Subscription::batch(subscriptions)
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}
