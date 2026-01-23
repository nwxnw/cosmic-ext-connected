//! SMS view components for conversation list and message threads.

use crate::app::{LoadingPhase, Message, SmsLoadingState};
use crate::fl;
use crate::views::helpers::{format_timestamp, WIDE_POPUP_WIDTH};
use cosmic::iced::widget::{column, row, text};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;
use kdeconnect_dbus::contacts::{Contact, ContactLookup};
use kdeconnect_dbus::plugins::{is_address_valid, ConversationSummary, MessageType, SmsMessage};

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

// --- View params and functions ---

/// Parameters for the conversation list view.
pub struct ConversationListParams<'a> {
    pub device_name: Option<&'a str>,
    pub conversations: &'a [ConversationSummary],
    pub conversations_displayed: usize,
    pub contacts: &'a ContactLookup,
    pub loading_state: &'a SmsLoadingState,
    /// Whether background sync is active (syncing conversations from phone)
    pub sync_active: bool,
}

/// Render the SMS conversation list view.
pub fn view_conversation_list(params: ConversationListParams<'_>) -> Element<'_, Message> {
    let default_device = fl!("device");
    let device_name = params.device_name.unwrap_or(&default_device);

    // Build header with optional sync indicator
    let mut header_row = row![
        widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
            .on_press(Message::CloseSmsView),
        text(fl!("messages-title", device = device_name)).size(16),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // Show sync indicator when background sync is active
    if params.sync_active {
        header_row = header_row.push(
            widget::tooltip(
                widget::icon::from_name("emblem-synchronizing-symbolic").size(16),
                text(fl!("syncing")).size(12),
                widget::tooltip::Position::Bottom,
            )
            .padding(4),
        );
    }

    let header = header_row
        .push(widget::horizontal_space())
        .push(
            widget::button::icon(widget::icon::from_name("list-add-symbolic"))
                .on_press(Message::OpenNewMessage),
        )
        .padding([8, 12]);

    let content: Element<Message> = if is_loading_conversations(params.loading_state)
        && params.conversations.is_empty()
    {
        widget::container(
            column![text(conversation_loading_text(params.loading_state)).size(14),]
                .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else if params.conversations.is_empty() {
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
        // Build conversation list (limited to conversations_displayed)
        let mut conv_column = column![].spacing(4);
        for conv in params
            .conversations
            .iter()
            .take(params.conversations_displayed)
        {
            let display_name = params.contacts.get_name_or_number(conv.primary_address());

            let snippet = conv.last_message.chars().take(50).collect::<String>();
            let date_str = format_timestamp(conv.timestamp);

            let conv_row = widget::button::custom(
                widget::container(
                    row![
                        column![text(display_name).size(14), text(snippet).size(11),].spacing(2),
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

        // Add "Load More" button if there are more conversations
        if params.conversations_displayed < params.conversations.len() {
            let load_more_row = row![
                widget::icon::from_name("go-down-symbolic").size(16),
                text(fl!("load-more-conversations")).size(14),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let load_more_button = widget::button::custom(
                widget::container(load_more_row)
                    .padding(8)
                    .width(Length::Fill)
                    .align_x(Alignment::Center),
            )
            .class(cosmic::theme::Button::Text)
            .on_press(Message::LoadMoreConversations)
            .width(Length::Fill);

            conv_column = conv_column.push(widget::divider::horizontal::default());
            conv_column = conv_column.push(load_more_button);
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

/// Parameters for the message thread view.
pub struct MessageThreadParams<'a> {
    pub thread_addresses: Option<&'a [String]>,
    pub messages: &'a [SmsMessage],
    pub contacts: &'a ContactLookup,
    pub loading_state: &'a SmsLoadingState,
    pub sms_compose_text: &'a str,
    pub sms_sending: bool,
    /// Whether background sync is active (syncing messages from phone)
    pub sync_active: bool,
    /// UID of message bubble currently being pressed (for visual feedback)
    pub pressed_bubble_uid: Option<i32>,
    /// Whether to show the "Hold to copy" hint (500ms elapsed)
    pub show_copy_hint: bool,
}

/// Render the SMS message thread view.
pub fn view_message_thread(params: MessageThreadParams<'_>) -> Element<'_, Message> {
    let default_unknown = fl!("unknown");
    let address = params
        .thread_addresses
        .and_then(|addrs| addrs.first())
        .map(|s| s.as_str())
        .unwrap_or(&default_unknown);
    let display_name = params.contacts.get_name_or_number(address);

    // Build header with optional sync indicator
    let mut header_row = row![
        widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
            .on_press(Message::CloseConversation),
        text(display_name).size(16),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // Show sync indicator when background sync is active
    if params.sync_active {
        header_row = header_row.push(
            widget::tooltip(
                widget::icon::from_name("emblem-synchronizing-symbolic").size(16),
                text(fl!("syncing")).size(12),
                widget::tooltip::Position::Bottom,
            )
            .padding(4),
        );
    }

    let header = header_row
        .push(widget::horizontal_space())
        .padding([8, 12]);

    // Show loading indicator only when loading AND no messages yet
    // Once messages start arriving, show them (scrolled to bottom)
    let content: Element<Message> = if is_loading_messages(params.loading_state)
        && params.messages.is_empty()
    {
        widget::container(
            column![text(message_loading_text(params.loading_state)).size(14),]
                .align_x(Alignment::Center),
        )
        .center(Length::Fill)
        .into()
    } else if params.messages.is_empty() {
        widget::container(column![text(fl!("no-messages")).size(14),].align_x(Alignment::Center))
            .center(Length::Fill)
            .into()
    } else {
        // Build message list with improved styling
        // Max width for bubbles is ~75% of popup width for better readability
        let bubble_max_width = (WIDE_POPUP_WIDTH * 0.75) as u16;
        let loading_more = is_loading_more(params.loading_state);

        let mut msg_column = column![].spacing(12).padding([8, 12]);

        // Show loading indicator at top when fetching older messages
        if loading_more {
            let loading_indicator: Element<Message> = widget::container(
                row![
                    widget::icon::from_name("process-working-symbolic").size(16),
                    text(fl!("loading-older")).size(14),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(8)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .into();

            msg_column = msg_column.push(loading_indicator);
            msg_column = msg_column.push(widget::divider::horizontal::default());
        }

        for msg in params.messages {
            // MessageType::Inbox (1) = incoming/received, MessageType::Sent (2) = outgoing/sent
            let is_received = msg.message_type == MessageType::Inbox;
            let time_str = format_timestamp(msg.date);
            let is_pressed = params.pressed_bubble_uid == Some(msg.uid);
            let show_hint = is_pressed && params.show_copy_hint;

            // Message bubble content (long-press to copy)
            let bubble_content =
                column![text(&msg.body).size(13).wrapping(text::Wrapping::Word), text(time_str).size(9),]
                    .spacing(4);

            // Use highlighted style when pressed for high contrast visual feedback
            let bubble: Element<Message> = if is_pressed {
                // Wrap in two containers for a "selected" border effect
                let inner = widget::container(bubble_content)
                    .padding([8, 12])
                    .max_width(bubble_max_width - 8)
                    .class(cosmic::theme::Container::Primary);
                widget::container(inner)
                    .padding(4)
                    .class(cosmic::theme::Container::Dropdown)
                    .into()
            } else {
                widget::container(bubble_content)
                    .padding([8, 12])
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
                column![
                    bubble_with_press,
                    text(fl!("hold-to-copy")).size(10),
                ]
                .spacing(2)
                .into()
            } else {
                bubble_with_press.into()
            };

            // Received messages: show sender name above and align left
            // Sent messages: align right
            let msg_row: Element<Message> = if is_received {
                let sender_name = params.contacts.get_name_or_number(msg.primary_address());
                column![
                    text(sender_name).size(11),
                    row![bubble_element, widget::horizontal_space(),].width(Length::Fill),
                ]
                .spacing(4)
                .width(Length::Fill)
                .into()
            } else {
                row![widget::horizontal_space(), bubble_element,]
                    .width(Length::Fill)
                    .into()
            };

            msg_column = msg_column.push(msg_row);
        }

        widget::scrollable(msg_column)
            .id(widget::Id::new("message-thread"))
            .width(Length::Fill)
            .height(Length::Fill)
            .on_scroll(Message::MessageThreadScrolled)
            .into()
    };

    // Compose row
    let compose_input = widget::text_input(fl!("type-message"), params.sms_compose_text)
        .on_input(Message::SmsComposeInput)
        .width(Length::Fill);

    // Check if this is a group conversation (can't send to groups)
    let unique_addresses: std::collections::HashSet<&str> = params
        .thread_addresses
        .map(|addrs| addrs.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    let is_group = unique_addresses.len() > 1;

    let send_btn: Element<Message> = if is_group {
        // Show disabled indicator for group conversations
        widget::container(
            text(fl!("group-sms-not-supported"))
                .size(11)
                .wrapping(text::Wrapping::Word),
        )
        .padding([4, 8])
        .width(Length::Fill)
        .into()
    } else if params.sms_sending {
        widget::button::standard(fl!("sending"))
            .leading_icon(widget::icon::from_name("process-working-symbolic").size(16))
            .into()
    } else {
        let can_send = !params.sms_compose_text.is_empty() && !params.sms_sending;
        widget::button::suggested(fl!("send"))
            .leading_icon(widget::icon::from_name("mail-send-symbolic").size(16))
            .on_press_maybe(if can_send {
                Some(Message::SendSms)
            } else {
                None
            })
            .into()
    };

    let compose_row = if is_group {
        widget::container(column![row![compose_input].width(Length::Fill), send_btn,].spacing(8))
            .padding([8, 12])
    } else {
        widget::container(
            row![compose_input, send_btn,]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([8, 12])
    };

    column![
        header,
        widget::divider::horizontal::default(),
        content,
        widget::divider::horizontal::default(),
        compose_row,
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
}

/// Parameters for the new message view.
pub struct NewMessageParams<'a> {
    pub recipient: &'a str,
    pub body: &'a str,
    pub recipient_valid: bool,
    pub sending: bool,
    pub contact_suggestions: &'a [Contact],
}

/// Render the new message compose view.
pub fn view_new_message(params: NewMessageParams<'_>) -> Element<'_, Message> {
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
    let recipient_input = widget::text_input(fl!("recipient-placeholder"), params.recipient)
        .on_input(Message::NewMessageRecipientInput)
        .width(Length::Fill);

    let validation_icon: Element<Message> = if params.recipient.is_empty() {
        widget::Space::new(Length::Fixed(20.0), Length::Fixed(20.0)).into()
    } else if params.recipient_valid {
        widget::icon::from_name("emblem-ok-symbolic")
            .size(20)
            .into()
    } else {
        widget::icon::from_name("dialog-error-symbolic")
            .size(20)
            .into()
    };

    let recipient_row = widget::container(
        row![text(fl!("to")).size(14), recipient_input, validation_icon,]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([8, 12]);

    // Contact suggestions (show if recipient is being typed and we have matches)
    let suggestions_section: Element<Message> = if !params.recipient.is_empty()
        && !is_address_valid(params.recipient)
        && !params.contact_suggestions.is_empty()
    {
        let mut suggestions_col = column![].spacing(4);
        for contact in params.contact_suggestions.iter().take(5) {
            let name = contact.name.clone();
            let phone = contact.phone_numbers.first().cloned().unwrap_or_default();
            // Clone for display since we need to move into on_press
            let display_name = name.clone();
            let display_phone = phone.clone();
            let contact_row = widget::button::custom(
                widget::container(
                    row![
                        widget::icon::from_name("contact-new-symbolic").size(20),
                        column![text(display_name).size(13), text(display_phone).size(11),]
                            .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding(8)
                .width(Length::Fill),
            )
            .class(cosmic::theme::Button::Text)
            .on_press(Message::SelectContact(name, phone))
            .width(Length::Fill);
            suggestions_col = suggestions_col.push(contact_row);
        }
        widget::container(suggestions_col)
            .padding([0, 12])
            .width(Length::Fill)
            .into()
    } else {
        widget::Space::new(Length::Shrink, Length::Shrink).into()
    };

    // Message input
    let message_input = widget::text_input(fl!("type-message"), params.body)
        .on_input(Message::NewMessageBodyInput)
        .width(Length::Fill);

    // Send button
    let send_enabled = params.recipient_valid && !params.body.is_empty() && !params.sending;

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
