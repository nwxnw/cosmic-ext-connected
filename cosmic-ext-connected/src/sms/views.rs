//! SMS view components for conversation list and message threads.

use crate::app::{LoadingPhase, Message, SettingKey, SmsLoadingState};
use crate::fl;
use crate::sms::logical::LogicalConversation;
use crate::views::helpers::format_timestamp;
use base64::Engine;
use cosmic::applet;
use cosmic::iced::advanced::image::Handle as ImageHandle;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, ContentFit, Length};
use cosmic::widget::{self, text};
use cosmic::Element;
use kdeconnect_dbus::contacts::ContactLookup;
use kdeconnect_dbus::plugins::{
    is_address_valid, Attachment, MessageType, SmsMessage, OPTIMISTIC_MESSAGE_UID,
};

// --- Helper functions for loading state ---

/// Get display text for conversation loading state.
fn conversation_loading_text(state: &SmsLoadingState) -> String {
    match state {
        SmsLoadingState::LoadingConversations(phase) => match phase {
            LoadingPhase::Connecting => fl!("loading-connecting"),
            LoadingPhase::Requesting => fl!("loading-requesting"),
        },
        _ => fl!("loading-conversations"),
    }
}

/// Get display text for message loading state.
fn message_loading_text(state: &SmsLoadingState) -> String {
    match state {
        SmsLoadingState::LoadingMessages(phase) => match phase {
            LoadingPhase::Connecting => fl!("loading-connecting"),
            LoadingPhase::Requesting => fl!("loading-requesting"),
        },
        _ => fl!("loading-messages"),
    }
}

/// Check if conversations are in a loading state.
fn is_loading_conversations(state: &SmsLoadingState) -> bool {
    matches!(state, SmsLoadingState::LoadingConversations(_))
}

/// Check if messages are in a loading state (not pagination).
fn is_loading_messages(state: &SmsLoadingState) -> bool {
    matches!(state, SmsLoadingState::LoadingMessages(_))
}

/// Check if loading more messages (pagination).
fn is_loading_more(state: &SmsLoadingState) -> bool {
    matches!(state, SmsLoadingState::LoadingMoreMessages)
}

// --- Helper functions for conversation list ---

fn normalize_snippet(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(50)
        .collect::<String>()
}

// --- Attachment helpers ---

/// Determine the icon name for a MIME type.
fn attachment_icon(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image-x-generic-symbolic"
    } else if mime.starts_with("video/") {
        "video-x-generic-symbolic"
    } else if mime.starts_with("audio/") {
        "audio-x-generic-symbolic"
    } else {
        "mail-attachment-symbolic"
    }
}

/// Render a single attachment element within a message bubble.
fn view_attachment<'a>(
    attachment: &Attachment,
    device_id: &str,
    device_name: &str,
) -> Element<'a, Message> {
    let sp = cosmic::theme::spacing();

    // For images with a base64 thumbnail, try to decode and display inline
    if attachment.mime_type.starts_with("image/") && !attachment.base64_thumbnail.is_empty() {
        // KDE Connect sends base64 with embedded newlines — strip before decoding
        let clean_b64: String = attachment
            .base64_thumbnail
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&clean_b64) {
            let handle = ImageHandle::from_bytes(decoded);
            let img = cosmic::iced::widget::image(handle)
                .height(Length::Fixed(200.0))
                .content_fit(ContentFit::Contain);

            // Wrap in mouse_area for click-to-open
            return widget::mouse_area(img)
                .on_press(Message::OpenAttachment {
                    device_id: device_id.to_string(),
                    device_name: device_name.to_string(),
                    part_id: attachment.part_id,
                    unique_identifier: attachment.unique_identifier.clone(),
                })
                .into();
        }
    }

    // Fallback: icon + MIME type label for non-image or failed decode
    let icon_name = attachment_icon(&attachment.mime_type);
    let label = if attachment.mime_type.starts_with("image/") {
        fl!("attachment")
    } else {
        // Show short MIME subtype (e.g. "video/mp4" → "mp4")
        attachment
            .mime_type
            .split('/')
            .nth(1)
            .unwrap_or(&attachment.mime_type)
            .to_string()
    };

    let placeholder = row![
        widget::icon::from_name(icon_name).size(24),
        text::body(label),
    ]
    .spacing(sp.space_xxs)
    .align_y(Alignment::Center);

    widget::mouse_area(
        widget::container(placeholder)
            .padding([sp.space_xxs, sp.space_xs])
            .class(cosmic::theme::Container::Card),
    )
    .on_press(Message::OpenAttachment {
        device_id: device_id.to_string(),
        device_name: device_name.to_string(),
        part_id: attachment.part_id,
        unique_identifier: attachment.unique_identifier.clone(),
    })
    .into()
}

// --- View params and functions ---

/// Parameters for the conversation list view.
pub struct ConversationListParams<'a> {
    pub device_name: Option<&'a str>,
    pub conversations: &'a [LogicalConversation],
    pub contacts: &'a ContactLookup,
    pub loading_state: &'a SmsLoadingState,
    /// Whether background sync is active (syncing conversations from phone)
    pub sync_active: bool,
    /// Current value of `config.merge_reaction_threads`. Drives the header
    /// toggle's icon + tooltip; also controls whether per-entry merge
    /// markers can appear (no markers when merging is off because no
    /// `LogicalConversation` will have `merged_thread_ids.len() > 1`).
    pub merge_reaction_threads: bool,
}

/// Render the SMS conversation list view.
pub fn view_conversation_list(params: ConversationListParams<'_>) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();
    let default_device = fl!("device");
    let device_name = params.device_name.unwrap_or(&default_device);

    // Build header with optional sync indicator
    let mut header_row = row![
        widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::CloseSmsView),
        text::heading(fl!("messages-title", device = device_name)),
    ]
    .spacing(sp.space_xxs)
    .align_y(Alignment::Center);

    // Show sync indicator when background sync is active
    if params.sync_active {
        header_row = header_row.push(
            widget::tooltip(
                widget::icon::from_name("emblem-synchronizing-symbolic").size(16),
                text::caption(fl!("syncing")),
                widget::tooltip::Position::Bottom,
            )
            .padding(sp.space_xxxs),
        );
    }

    let new_msg_btn = widget::tooltip(
        widget::button::icon(widget::icon::from_name("list-add-symbolic"))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::OpenNewMessage),
        text::caption(fl!("new-message")),
        widget::tooltip::Position::Bottom,
    )
    .gap(sp.space_xxxs)
    .padding(sp.space_xxs);

    let (merge_toggle_icon, merge_toggle_tooltip) = if params.merge_reaction_threads {
        (
            "io.github.nwxnw.cosmic-ext-connected-merged-symbolic",
            fl!("merge-toggle-on-tooltip"),
        )
    } else {
        (
            "io.github.nwxnw.cosmic-ext-connected-split-symbolic",
            fl!("merge-toggle-off-tooltip"),
        )
    };
    let merge_toggle_btn = widget::tooltip(
        widget::button::icon(widget::icon::from_name(merge_toggle_icon))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::ToggleSetting(SettingKey::MergeReactionThreads)),
        text::caption(merge_toggle_tooltip),
        widget::tooltip::Position::Bottom,
    )
    .gap(sp.space_xxxs)
    .padding(sp.space_xxs);

    let header = applet::padded_control(
        header_row
            .push(widget::space::horizontal())
            .push(merge_toggle_btn)
            .push(new_msg_btn),
    );

    let content: Element<Message> = if is_loading_conversations(params.loading_state)
        && params.conversations.is_empty()
    {
        widget::container(
            column![text::body(conversation_loading_text(params.loading_state)),]
                .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else if params.conversations.is_empty() {
        widget::container(
            column![
                widget::icon::from_name("mail-message-new-symbolic").size(48),
                text::heading(fl!("no-conversations")),
                text::caption(fl!("start-new-message")),
            ]
            .spacing(sp.space_xs)
            .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else {
        // Build conversation list (limited to conversations_displayed)
        let mut conv_column = column![].spacing(sp.space_xxxs);
        for conv in params.conversations.iter() {
            let display_name = params.contacts.get_group_display_name(&conv.addresses, 3);
            let date_str = format_timestamp(conv.last_message_timestamp);

            // Build snippet: show attachment indicator if needed
            let snippet_element: Element<Message> =
                if conv.has_attachments && conv.last_message_preview.is_empty() {
                    // MMS with only attachments (no text body)
                    row![
                        widget::icon::from_name("mail-attachment-symbolic").size(14),
                        text::caption(fl!("attachment"))
                            .wrapping(cosmic::iced::widget::text::Wrapping::None),
                    ]
                    .spacing(sp.space_xxxs)
                    .align_y(Alignment::Center)
                    .into()
                } else if conv.has_attachments {
                    // MMS with both text and attachments
                    let snippet = normalize_snippet(&conv.last_message_preview);
                    row![
                        widget::icon::from_name("mail-attachment-symbolic").size(14),
                        text::caption(snippet).wrapping(cosmic::iced::widget::text::Wrapping::None),
                    ]
                    .spacing(sp.space_xxxs)
                    .align_y(Alignment::Center)
                    .into()
                } else {
                    let snippet = normalize_snippet(&conv.last_message_preview);
                    text::caption(snippet)
                        .wrapping(cosmic::iced::widget::text::Wrapping::None)
                        .into()
                };

            let marker_glyph: Option<&str> = if conv.merged_thread_ids.len() > 1 {
                Some("io.github.nwxnw.cosmic-ext-connected-merged-symbolic")
            } else if conv.is_split_candidate {
                Some("io.github.nwxnw.cosmic-ext-connected-split-symbolic")
            } else {
                None
            };

            let snippet_row: Element<Message> = if let Some(glyph) = marker_glyph {
                let marker_icon = widget::icon::from_name(glyph).size(14).icon().class(
                    cosmic::theme::Svg::custom(|theme| cosmic::iced::widget::svg::Style {
                        color: Some(theme.cosmic().accent_text_color().into()),
                    }),
                );
                row![marker_icon, snippet_element,]
                    .spacing(sp.space_xxxs)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                snippet_element
            };

            let conv_row = applet::menu_button(
                row![
                    widget::container(
                        column![
                            text::body(display_name)
                                .wrapping(cosmic::iced::widget::text::Wrapping::None),
                            snippet_row,
                        ]
                        .spacing(2),
                    )
                    .width(Length::Fill)
                    .clip(true),
                    text::caption(date_str),
                    widget::icon::from_name("go-next-symbolic").size(16),
                ]
                .spacing(sp.space_xxs)
                .align_y(Alignment::Center),
            )
            .on_press(Message::OpenConversation(conv.primary_thread_id));

            conv_column = conv_column.push(conv_row);
        }

        // Show sync progress indicator at bottom when still syncing
        if params.sync_active {
            conv_column = conv_column.push(
                applet::padded_control(
                    row![
                        widget::icon::from_name("emblem-synchronizing-symbolic").size(16),
                        text::caption(fl!("syncing-conversations")),
                    ]
                    .spacing(sp.space_xxs)
                    .align_y(Alignment::Center),
                )
                .align_x(Alignment::Center),
            );
        }

        widget::container(
            widget::scrollable(conv_column.padding([0, sp.space_xxs as u16])).width(Length::Fill),
        )
        .max_height(crate::constants::sms::CONVERSATION_LIST_MAX_HEIGHT)
        .into()
    };

    column![header, content,]
        .spacing(sp.space_xxs)
        .width(Length::Fill)
        .into()
}

/// Parameters for the message thread view.
pub struct MessageThreadParams<'a> {
    pub device_id: &'a str,
    pub device_name: &'a str,
    pub thread_addresses: Option<&'a [String]>,
    pub messages: &'a [SmsMessage],
    pub contacts: &'a ContactLookup,
    pub loading_state: &'a SmsLoadingState,
    pub sms_compose_text: &'a widget::text_editor::Content,
    pub sms_sending: bool,
    /// Whether background sync is active (syncing messages from phone)
    pub sync_active: bool,
    /// UID of message bubble currently being pressed (for visual feedback)
    pub pressed_bubble_uid: Option<i32>,
    /// Whether to show the "Hold to copy" hint (500ms elapsed)
    pub show_copy_hint: bool,
    /// Status message to display (e.g. send confirmation or error)
    pub status_message: Option<&'a str>,
}

/// Enter sends; Shift+Enter falls through to default newline binding
fn compose_key_binding(
    mut kp: widget::text_editor::KeyPress,
    submit: Message,
) -> Option<widget::text_editor::Binding<Message>> {
    use cosmic::iced::keyboard::{key::Named, Key};
    use widget::text_editor::Binding;

    // COSMIC/winit delivers navigation keys with `text = Some("")` rather than
    // `None`; normalize empty/control-only text to None so motions apply
    if kp
        .text
        .as_deref()
        .is_some_and(|t| t.chars().all(|c| c.is_control()))
    {
        kp.text = None;
    }

    match kp.key {
        Key::Named(Named::Enter) if !kp.modifiers.shift() => Some(Binding::Custom(submit)),
        _ => Binding::from_key_press(kp),
    }
}

/// Render the SMS message thread view.
pub fn view_message_thread(params: MessageThreadParams<'_>) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();
    let display_name = match params.thread_addresses {
        Some(addrs) => params.contacts.get_group_display_name(addrs, 3),
        None => fl!("unknown"),
    };

    // Build header with optional sync indicator
    let mut header_row = row![
        widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::CloseConversation),
        text::heading(display_name),
    ]
    .spacing(sp.space_xxs)
    .align_y(Alignment::Center);

    // Show sync indicator when background sync is active
    if params.sync_active {
        header_row = header_row.push(
            widget::tooltip(
                widget::icon::from_name("emblem-synchronizing-symbolic").size(16),
                text::caption(fl!("syncing")),
                widget::tooltip::Position::Bottom,
            )
            .padding(sp.space_xxxs),
        );
    }

    let header = applet::padded_control(header_row.push(widget::space::horizontal()));

    // Show loading indicator only when loading AND no messages yet
    // Once messages start arriving, show them (scrolled to bottom)
    let content: Element<Message> = if is_loading_messages(params.loading_state)
        && params.messages.is_empty()
    {
        widget::container(
            column![text::body(message_loading_text(params.loading_state)),]
                .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else if params.messages.is_empty() {
        widget::container(column![text::body(fl!("no-messages")),].align_x(Alignment::Center))
            .center(Length::Fill)
            .into()
    } else {
        // Build message list with improved styling
        // Max width for bubbles is ~75% of popup width for better readability
        let bubble_max_width = (360.0_f32 * 0.75) as u16;
        let loading_more = is_loading_more(params.loading_state);

        let mut msg_column = column![]
            .spacing(sp.space_xs)
            .padding([sp.space_xxs, sp.space_xs]);

        // Show loading indicator at top when fetching older messages
        if loading_more {
            let loading_indicator: Element<Message> = widget::container(
                row![
                    widget::icon::from_name("process-working-symbolic").size(16),
                    text::body(fl!("loading-older")),
                ]
                .spacing(sp.space_xxs)
                .align_y(Alignment::Center),
            )
            .padding(sp.space_xxs)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .into();

            msg_column = msg_column.push(loading_indicator);
        }

        for msg in params.messages {
            // MessageType::Inbox (1) = incoming/received, MessageType::Sent (2) = outgoing/sent
            let is_received = msg.message_type == MessageType::Inbox;
            let time_str = format_timestamp(msg.date);
            let is_pressed = params.pressed_bubble_uid == Some(msg.uid);
            let show_hint = is_pressed && params.show_copy_hint;

            // Message bubble content: attachments first, then text body
            let mut bubble_content = column![].spacing(sp.space_xxxs);

            // Render attachment thumbnails/placeholders
            for att in &msg.attachments {
                bubble_content =
                    bubble_content.push(view_attachment(att, params.device_id, params.device_name));
            }

            // Add text body (skip if empty, e.g. image-only MMS)
            if !msg.body.is_empty() {
                bubble_content = bubble_content.push(
                    text::body(&msg.body).wrapping(cosmic::iced::widget::text::Wrapping::Word),
                );
            }

            let is_pending = msg.uid == OPTIMISTIC_MESSAGE_UID;
            if is_pending {
                bubble_content = bubble_content.push(
                    row![
                        widget::icon::from_name("emblem-synchronizing-symbolic").size(12),
                        text::caption(fl!("sending")),
                    ]
                    .spacing(sp.space_xxxs)
                    .align_y(Alignment::Center),
                );
            } else {
                bubble_content = bubble_content.push(text::caption(time_str));
            }

            // Use highlighted style when pressed for high contrast visual feedback
            let bubble: Element<Message> = if is_pressed {
                // Wrap in two containers for a "selected" border effect
                let inner = widget::container(bubble_content)
                    .padding([sp.space_xxs, sp.space_xs])
                    .max_width(bubble_max_width - 8)
                    .class(cosmic::theme::Container::Primary);
                widget::container(inner)
                    .padding(sp.space_xxxs)
                    .class(cosmic::theme::Container::Dropdown)
                    .into()
            } else {
                widget::container(bubble_content)
                    .padding([sp.space_xxs, sp.space_xs])
                    .max_width(bubble_max_width)
                    .class(if is_received {
                        cosmic::theme::Container::Card
                    } else {
                        cosmic::theme::Container::Primary
                    })
                    .into()
            };

            // Wrap bubble in mouse_area for long-press detection
            let bubble_with_press = widget::mouse_area(bubble)
                .on_press(Message::BubblePressStarted {
                    uid: msg.uid,
                    body: msg.body.clone(),
                })
                .on_release(Message::BubblePressReleased);

            // Bubble with optional "Hold to copy" hint (only after 500ms)
            let bubble_element: Element<Message> = if show_hint {
                column![bubble_with_press, text::caption(fl!("hold-to-copy")),]
                    .spacing(2)
                    .into()
            } else {
                bubble_with_press.into()
            };

            // Received messages: align left, show sender name only in group chats
            // Sent messages: align right
            // Note: thread_addresses may contain duplicates or both user + recipient,
            // so we deduplicate and use a threshold of >1 unique addresses for "group"
            let is_group = params.thread_addresses.is_some_and(|addrs| {
                let unique: std::collections::HashSet<_> = addrs.iter().collect();
                unique.len() > 1
            });
            let msg_row: Element<Message> = if is_received {
                if is_group {
                    let sender_name = params.contacts.get_name_or_number(msg.primary_address());
                    column![
                        text::caption(sender_name),
                        row![bubble_element, widget::space::horizontal(),].width(Length::Fill),
                    ]
                    .spacing(sp.space_xxxs)
                    .width(Length::Fill)
                    .into()
                } else {
                    row![bubble_element, widget::space::horizontal(),]
                        .width(Length::Fill)
                        .into()
                }
            } else {
                row![widget::space::horizontal(), bubble_element,]
                    .width(Length::Fill)
                    .into()
            };

            msg_column = msg_column.push(msg_row);
        }

        widget::scrollable(msg_column)
            .anchor_bottom()
            .id(widget::Id::new("message-thread"))
            .width(Length::Fill)
            .height(Length::Fill)
            .on_scroll(Message::MessageThreadScrolled)
            .into()
    };

    // Compose row
    let compose_input = widget::text_editor::text_editor(params.sms_compose_text)
        .placeholder(fl!("type-message"))
        .on_action(Message::SmsComposeAction)
        .key_binding(|kp| compose_key_binding(kp, Message::SendSms))
        .height(Length::Shrink)
        .padding(sp.space_xs)
        .max_height(120.0);

    let send_btn: Element<Message> = if params.sms_sending {
        widget::button::standard(fl!("sending"))
            .leading_icon(widget::icon::from_name("process-working-symbolic").size(16))
            .into()
    } else {
        let can_send = !params.sms_compose_text.text().trim().is_empty() && !params.sms_sending;
        widget::button::suggested(fl!("send"))
            .leading_icon(widget::icon::from_name("mail-send-symbolic").size(16))
            .on_press_maybe(if can_send {
                Some(Message::SendSms)
            } else {
                None
            })
            .into()
    };

    let compose_row = applet::padded_control(
        row![compose_input, send_btn,]
            .spacing(sp.space_xxs)
            .align_y(Alignment::Center),
    );

    let mut thread_column = column![header, content, compose_row,]
        .spacing(sp.space_xxxs)
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(msg) = params.status_message {
        thread_column = thread_column.push(
            widget::container(
                text::caption(msg).wrapping(cosmic::iced::widget::text::Wrapping::Word),
            )
            .padding([sp.space_xxxs, sp.space_xs])
            .width(Length::Fill)
            .class(cosmic::theme::Container::Card),
        );
    }

    thread_column.into()
}

/// Parameters for the new message view.
pub struct NewMessageParams<'a> {
    pub recipients: &'a [(String, String)],
    pub recipient_input: &'a str,
    pub body: &'a widget::text_editor::Content,
    pub sending: bool,
    /// Contact suggestions as (contact_name, phone_number) tuples
    pub contact_suggestions: &'a [(String, String)],
}

/// Render the new message compose view.
pub fn view_new_message(params: NewMessageParams<'_>) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();

    // Dynamic header: "New Group Message" with 2+ recipients
    let heading_text = if params.recipients.len() >= 2 {
        fl!("new-group-message")
    } else {
        fl!("new-message")
    };

    let header = applet::padded_control(
        row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .class(cosmic::theme::Button::Link)
                .on_press(Message::CloseNewMessage),
            text::heading(heading_text),
            widget::space::horizontal(),
        ]
        .spacing(sp.space_xxs)
        .align_y(Alignment::Center),
    );

    // Chip area — vertical list of committed recipients
    let chips_section: Element<Message> = if params.recipients.is_empty() {
        widget::Space::new().into()
    } else {
        // Compact pills that wrap across lines. Name-only labels for known contacts
        let mut pills: Vec<Element<Message>> = Vec::new();
        for (i, (display_name, phone)) in params.recipients.iter().enumerate() {
            let label = if display_name != phone {
                display_name.clone()
            } else {
                phone.clone()
            };

            let pill = widget::container(
                row![
                    text::body(label),
                    widget::button::icon(widget::icon::from_name("edit-clear-symbolic").size(16))
                        .on_press(Message::RemoveRecipient(i))
                        .class(cosmic::theme::Button::Link),
                ]
                .spacing(sp.space_xxs)
                .align_y(Alignment::Center),
            )
            .padding([sp.space_xxxs, sp.space_xs])
            .class(cosmic::theme::Container::Card)
            .into();
            pills.push(pill);
        }

        widget::container(
            widget::flex_row(pills)
                .spacing(sp.space_xs as u16)
                .width(Length::Fill),
        )
        .padding([0, sp.space_xs as u16])
        .width(Length::Fill)
        .into()
    };

    // Recipient input - use text_input's own leading/trailing slots
    let input_valid = is_address_valid(params.recipient_input);
    let action_icon: Element<Message> = if params.recipient_input.is_empty() {
        widget::Space::new()
            .width(Length::Fixed(20.0))
            .height(Length::Fixed(20.0))
            .into()
    } else if input_valid {
        widget::button::icon(widget::icon::from_name("list-add-symbolic").size(20))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::AddManualRecipient)
            .into()
    } else if params.recipient_input.contains('@') {
        // Malformed email entry - sho an error hint. Plain name queries
        // and partial phone numbers stay neutral
        widget::icon::from_name("dialog-error-symbolic")
            .size(20)
            .into()
    } else {
        widget::Space::new()
            .width(Length::Fixed(20.0))
            .height(Length::Fixed(20.0))
            .into()
    };

    let recipient_input = widget::text_input(fl!("recipient-placeholder"), params.recipient_input)
        .on_input(Message::NewMessageRecipientInput)
        .on_submit(|_| Message::AddManualRecipient)
        .leading_icon(text::body(fl!("to")).into())
        .trailing_icon(action_icon)
        .width(Length::Fill)
        .id(widget::Id::new("new-message-recipient"));

    let recipient_row = applet::padded_control(recipient_input);

    // Contact suggestions (show if input is being typed and we have matches)
    let suggestions_section: Element<Message> = if !params.recipient_input.is_empty()
        && !is_address_valid(params.recipient_input)
        && !params.contact_suggestions.is_empty()
    {
        let mut suggestions_col = column![].spacing(sp.space_xxxs);
        for (name, phone) in params
            .contact_suggestions
            .iter()
            .take(crate::constants::sms::MAX_SUGGESTIONS_SHOWN)
        {
            let contact_row = applet::menu_button(
                row![
                    widget::icon::from_name("contact-new-symbolic").size(20),
                    column![text::body(name.clone()), text::caption(phone.clone()),].spacing(2),
                ]
                .spacing(sp.space_xxs)
                .align_y(Alignment::Center),
            )
            .on_press(Message::SelectContact(name.clone(), phone.clone()));
            suggestions_col = suggestions_col.push(contact_row);
        }
        widget::container(suggestions_col)
            .padding([0, sp.space_xs as u16])
            .width(Length::Fill)
            .into()
    } else {
        widget::Space::new().into()
    };

    // Message input
    let message_input = widget::text_editor::text_editor(params.body)
        .placeholder(fl!("type-message"))
        .on_action(Message::NewMessageBodyAction)
        .key_binding(|kp| compose_key_binding(kp, Message::SendNewMessage))
        .height(Length::Shrink)
        .padding(sp.space_xs)
        .max_height(120.0);

    // Send button — enabled when at least one recipient and body is non-empty
    let send_enabled =
        !params.recipients.is_empty() && !params.body.text().trim().is_empty() && !params.sending;

    let send_btn = if params.sending {
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

    let send_row = applet::padded_control(
        row![widget::space::horizontal(), send_btn,]
            .spacing(sp.space_xxs)
            .align_y(Alignment::Center),
    );

    column![
        header,
        recipient_row,
        chips_section,
        suggestions_section,
        applet::padded_control(message_input),
        send_row,
        widget::space::vertical(),
    ]
    .spacing(sp.space_xxxs)
    .width(Length::Fill)
    .into()
}
