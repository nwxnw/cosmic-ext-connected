//! Individual device page view.
//!
//! Shows detailed information and actions for a specific device.

use crate::app::{DeviceInfo, Message};
use crate::fl;
use cosmic::iced::widget::{column, row, text, tooltip};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;
use kdeconnect_dbus::plugins::NotificationInfo;

/// Render the device detail page.
pub fn view<'a>(device: &'a DeviceInfo, status_message: Option<&'a str>) -> Element<'a, Message> {
    // Back button
    let back_btn = widget::button::text(fl!("back"))
        .leading_icon(icon::from_name("go-previous-symbolic").size(16))
        .on_press(Message::BackToList);

    // Device icon based on type
    let icon_name = match device.device_type.as_str() {
        "phone" | "smartphone" => "phone-symbolic",
        "tablet" => "tablet-symbolic",
        "desktop" => "computer-symbolic",
        "laptop" => "computer-laptop-symbolic",
        _ => "device-symbolic",
    };

    // Build header row with device info and optional ping button
    let header: Element<Message> = {
        let mut header_row = row![
            icon::from_name(icon_name).size(48),
            column![
                text(device.name.clone()).size(18),
                text(device.device_type.clone()).size(12),
            ]
            .spacing(4),
            widget::horizontal_space(),
        ]
        .spacing(16)
        .align_y(Alignment::Center);

        // Add ping button if device is reachable and paired
        if device.is_reachable && device.is_paired {
            let device_id_for_ping = device.id.clone();
            let ping_btn =
                widget::button::icon(icon::from_name("emblem-synchronizing-symbolic").size(20))
                    .on_press(Message::SendPing(device_id_for_ping))
                    .padding(8);
            let ping_with_tooltip = tooltip(
                ping_btn,
                text(fl!("send-ping")).size(11),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .padding(8);
            header_row = header_row.push(ping_with_tooltip);
        }

        header_row.into()
    };

    // Build the combined status row with connected, paired, and battery
    let status_row = build_status_row(device);

    // Actions section - only available for connected and paired devices
    let actions: Element<Message> = if device.is_reachable && device.is_paired {
        let device_id_for_sms = device.id.clone();
        let device_id_for_sendto = device.id.clone();
        let device_type_for_sendto = device.device_type.clone();
        let device_id_for_media = device.id.clone();
        let device_id_for_find = device.id.clone();

        // SMS Messages action item
        let sms_row = row![
            icon::from_name("mail-message-new-symbolic").size(24),
            text(fl!("sms-messages")).size(14),
            widget::horizontal_space(),
            icon::from_name("go-next-symbolic").size(16),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let sms_item =
            widget::button::custom(widget::container(sms_row).padding(8).width(Length::Fill))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::OpenSmsView(device_id_for_sms))
                .width(Length::Fill);

        // Send to device action item
        let sendto_row = row![
            icon::from_name("document-send-symbolic").size(24),
            text(fl!("send-to", device = device.device_type.as_str())).size(14),
            widget::horizontal_space(),
            icon::from_name("go-next-symbolic").size(16),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let sendto_item =
            widget::button::custom(widget::container(sendto_row).padding(8).width(Length::Fill))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::OpenSendToView(
                    device_id_for_sendto,
                    device_type_for_sendto,
                ))
                .width(Length::Fill);

        // Media controls action item
        let media_row = row![
            icon::from_name("multimedia-player-symbolic").size(24),
            text(fl!("media-controls")).size(14),
            widget::horizontal_space(),
            icon::from_name("go-next-symbolic").size(16),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let media_item =
            widget::button::custom(widget::container(media_row).padding(8).width(Length::Fill))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::OpenMediaView(device_id_for_media))
                .width(Length::Fill);

        // Find Phone action item (no chevron - immediate action)
        let find_row = row![
            icon::from_name("phonelink-ring-symbolic").size(24),
            text(fl!("find-phone")).size(14),
            widget::horizontal_space(),
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        let find_item =
            widget::button::custom(widget::container(find_row).padding(8).width(Length::Fill))
                .class(cosmic::theme::Button::Text)
                .on_press(Message::FindMyPhone(device_id_for_find))
                .width(Length::Fill);

        column![sms_item, sendto_item, media_item, find_item,]
            .spacing(4)
            .into()
    } else if !device.is_paired {
        // Not paired - show nothing (pairing section will be shown below)
        widget::Space::new(Length::Shrink, Length::Shrink).into()
    } else {
        text(fl!("device-must-be-connected")).size(12).into()
    };

    // Pairing section
    let pairing_section: Element<Message> = build_pairing_section(device);

    // Notifications section
    let notifications_section: Element<Message> = build_notifications_section(device);

    // Build status message element if present
    let status_bar: Element<Message> = if let Some(msg) = status_message {
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
            status_row,
            widget::divider::horizontal::default(),
            actions,
            pairing_section,
            notifications_section,
        ]
        .spacing(12)
        .padding(16),
    )
    .into()
}

/// Build the combined status row showing connected, paired, and battery status.
fn build_status_row<'a>(device: &'a DeviceInfo) -> Element<'a, Message> {
    // Connected status (left-aligned) - use icon to indicate status
    let connected_icon_name = if device.is_reachable {
        "emblem-ok-symbolic" // Green checkmark
    } else {
        "window-close-symbolic" // X mark
    };
    let connected_content = row![
        icon::from_name(connected_icon_name).size(16),
        text(fl!("connected")).size(12),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let connected_tooltip_text = if device.is_reachable {
        fl!("tooltip-connected")
    } else {
        fl!("tooltip-not-connected")
    };
    let connected_element = tooltip(
        connected_content,
        text(connected_tooltip_text).size(11),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .padding(8);

    // Paired status (center-aligned) - use icon to indicate status
    let paired_icon_name = if device.is_paired {
        "emblem-ok-symbolic" // Green checkmark
    } else {
        "window-close-symbolic" // X mark
    };
    let paired_content = row![
        icon::from_name(paired_icon_name).size(16),
        text(fl!("paired")).size(12),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let paired_tooltip_text = if device.is_paired {
        fl!("tooltip-paired")
    } else {
        fl!("tooltip-not-paired")
    };
    let paired_element = tooltip(
        paired_content,
        text(paired_tooltip_text).size(11),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .padding(8);

    // Battery status (right-aligned) - percentage text + icon
    let battery_element: Element<Message> =
        if let (Some(level), Some(charging)) = (device.battery_level, device.battery_charging) {
            let icon_name = get_battery_icon_name(level, charging);
            let battery_content = row![
                text(format!("{}%", level)).size(12),
                icon::from_name(icon_name).size(24),
            ]
            .spacing(4)
            .align_y(Alignment::Center);
            let tooltip_text = if charging {
                fl!("tooltip-battery-charging", level = level)
            } else {
                fl!("tooltip-battery", level = level)
            };
            tooltip(
                battery_content,
                text(tooltip_text).size(11),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .padding(8)
            .into()
        } else {
            // No battery info available - empty space
            widget::Space::new(Length::Shrink, Length::Shrink).into()
        };

    row![
        connected_element,
        widget::horizontal_space(),
        paired_element,
        widget::horizontal_space(),
        battery_element,
    ]
    .align_y(Alignment::Center)
    .into()
}

/// Get the appropriate battery icon name based on level and charging state.
fn get_battery_icon_name(level: i32, charging: bool) -> &'static str {
    if charging {
        match level {
            0..=10 => "battery-level-10-charging-symbolic",
            11..=20 => "battery-level-20-charging-symbolic",
            21..=30 => "battery-level-30-charging-symbolic",
            31..=40 => "battery-level-40-charging-symbolic",
            41..=50 => "battery-level-50-charging-symbolic",
            51..=60 => "battery-level-60-charging-symbolic",
            61..=70 => "battery-level-70-charging-symbolic",
            71..=80 => "battery-level-80-charging-symbolic",
            81..=90 => "battery-level-90-charging-symbolic",
            _ => "battery-level-100-charging-symbolic",
        }
    } else {
        match level {
            0..=10 => "battery-level-10-symbolic",
            11..=20 => "battery-level-20-symbolic",
            21..=30 => "battery-level-30-symbolic",
            31..=40 => "battery-level-40-symbolic",
            41..=50 => "battery-level-50-symbolic",
            51..=60 => "battery-level-60-symbolic",
            61..=70 => "battery-level-70-symbolic",
            71..=80 => "battery-level-80-symbolic",
            81..=90 => "battery-level-90-symbolic",
            _ => "battery-level-100-symbolic",
        }
    }
}

/// Build the pairing section based on device state.
fn build_pairing_section<'a>(device: &'a DeviceInfo) -> Element<'a, Message> {
    let device_id = device.id.clone();

    // If peer requested pairing, show accept/reject buttons
    if device.is_pair_requested_by_peer {
        let accept_id = device_id.clone();
        let reject_id = device_id;
        return column![
            text(fl!("pairing-request")).size(14),
            text(fl!("device-wants-to-pair")).size(12),
            row![
                widget::button::suggested(fl!("accept"))
                    .leading_icon(icon::from_name("emblem-ok-symbolic").size(16))
                    .on_press(Message::AcceptPairing(accept_id)),
                widget::button::destructive(fl!("reject"))
                    .leading_icon(icon::from_name("window-close-symbolic").size(16))
                    .on_press(Message::RejectPairing(reject_id)),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into();
    }

    // If we requested pairing, show cancel button
    if device.is_pair_requested {
        return column![
            text(fl!("pairing")).size(14),
            text(fl!("waiting-for-device")).size(12),
            widget::button::standard(fl!("cancel")).on_press(Message::RejectPairing(device_id)),
        ]
        .spacing(8)
        .into();
    }

    // If paired, show unpair button
    if device.is_paired {
        return column![
            text(fl!("pairing")).size(14),
            widget::button::destructive(fl!("unpair"))
                .leading_icon(icon::from_name("list-remove-symbolic").size(16))
                .on_press(Message::Unpair(device_id)),
        ]
        .spacing(8)
        .into();
    }

    // If reachable but not paired, show pair button
    if device.is_reachable {
        return column![
            text(fl!("pairing")).size(14),
            text(fl!("device-not-paired")).size(12),
            widget::button::suggested(fl!("pair"))
                .leading_icon(icon::from_name("list-add-symbolic").size(16))
                .on_press(Message::RequestPair(device_id)),
        ]
        .spacing(8)
        .into();
    }

    // Offline and not paired
    column![
        text(fl!("pairing")).size(14),
        text(fl!("device-offline")).size(12),
    ]
    .spacing(8)
    .into()
}

/// Build the notifications section.
fn build_notifications_section<'a>(device: &'a DeviceInfo) -> Element<'a, Message> {
    if device.notifications.is_empty() {
        return widget::Space::new(Length::Shrink, Length::Shrink).into();
    }

    let mut notif_column = column![text(format!(
        "{} ({})",
        fl!("notifications"),
        device.notifications.len()
    ))
    .size(14),]
    .spacing(8);

    for notif in &device.notifications {
        let notif_widget = build_notification_row(device, notif);
        notif_column = notif_column.push(notif_widget);
    }

    notif_column
        .push(widget::divider::horizontal::default())
        .into()
}

/// Build a single notification row.
fn build_notification_row<'a>(
    device: &'a DeviceInfo,
    notif: &'a NotificationInfo,
) -> Element<'a, Message> {
    let notif_title = if notif.title.is_empty() {
        notif.app_name.clone()
    } else {
        format!("{}: {}", notif.app_name, notif.title)
    };

    let notif_content = column![text(notif_title).size(13), text(&notif.text).size(11),].spacing(2);

    let mut notif_row = row![
        icon::from_name("notification-symbolic").size(20),
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
            widget::button::icon(icon::from_name("window-close-symbolic"))
                .on_press(Message::DismissNotification(device_id, notif_id)),
        );
    }

    widget::container(notif_row)
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}
