//! Device list view for the applet popup.

use crate::app::{DeviceInfo, GroupKind, Message};
use crate::config::Config;
use crate::device::DeviceClass;
use crate::fl;
use crate::ui::widgets::warning_style;
use cosmic::applet;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon, text};
use cosmic::{theme, Element};

/// Render the device list view.
pub fn view<'a>(
    devices: &'a [DeviceInfo],
    config: &'a Config,
    status_message: Option<&'a str>,
) -> Element<'a, Message> {
    let sp = cosmic::theme::spacing();

    // Header with refresh and settings buttons
    let header = applet::padded_control(
        row![
            text::heading(fl!("devices")),
            widget::space::horizontal(),
            widget::tooltip(
                widget::button::icon(icon::from_name("view-refresh-symbolic"))
                    .on_press(Message::ForceReconnect),
                text::caption(fl!("refresh")),
                widget::tooltip::Position::Bottom,
            )
            .gap(sp.space_xxxs)
            .padding(sp.space_xxs),
            widget::tooltip(
                widget::button::icon(icon::from_name("notification-symbolic"))
                    .on_press(Message::ToggleSettings),
                text::caption(fl!("notifications")),
                widget::tooltip::Position::Bottom,
            )
            .gap(sp.space_xxxs)
            .padding(sp.space_xxs),
        ]
        .spacing(sp.space_xxxs)
        .align_y(Alignment::Center),
    );

    let groups = partition_devices(devices);

    let mut content = column![header].spacing(sp.space_xxxs);

    // Status message bar
    if let Some(msg) = status_message {
        content = content.push(
            widget::container(text::caption(msg))
                .padding([sp.space_xxxs, sp.space_xxs])
                .width(Length::Fill)
                .class(cosmic::theme::Container::Card),
        );
    }

    if groups.is_empty() {
        content = content.push(
            widget::container(text::caption(fl!("no-devices")))
                .padding(sp.space_s)
                .width(Length::Fill),
        );
    } else {
        let mut list = column![].spacing(sp.space_xs);
        for (kind, members) in groups {
            let collapsible = matches!(kind, GroupKind::Offline);
            if collapsible {
                let expanded = config.group_offline_expanded;
                list = list.push(group_header(kind, members.len(), expanded));
                if expanded {
                    let rows: Vec<Element<Message>> =
                        members.iter().map(|d| device_row(d)).collect();
                    list = list.push(column(rows).spacing(sp.space_xxs));
                }
            } else {
                let rows: Vec<Element<Message>> = members.iter().map(|d| device_row(d)).collect();
                list = list.push(column(rows).spacing(sp.space_xxs));
            }
        }
        content = content.push(list);
    }

    // App identity line -> About (quiet, tappable)
    let footer = applet::padded_control(
        row![
            widget::space::horizontal(),
            widget::button::custom(text::caption(format!(
                "{} · v{}",
                fl!("app-title"),
                env!("CARGO_PKG_VERSION")
            )))
            .class(cosmic::theme::Button::Link)
            .on_press(Message::OpenAbout),
            widget::space::horizontal(),
        ]
        .align_y(Alignment::Center),
    );
    content = content.push(footer);

    widget::container(content.padding(sp.space_xxs)).into()
}

/// Render a collapsible group header: "Label (N)" + disclosure chevron.
/// Only used for Offline.
fn group_header<'a>(kind: GroupKind, count: usize, expanded: bool) -> Element<'a, Message> {
    let sp = cosmic::theme::spacing();

    let label = match kind {
        GroupKind::Offline => fl!("offline"),
        _ => String::new(),
    };
    let heading = format!("{} ({})", label, count);

    let chevron = if expanded {
        "go-down-symbolic"
    } else {
        "go-next-symbolic"
    };

    let header_row = row![
        text::caption(heading),
        widget::space::horizontal(),
        icon::from_name(chevron).size(16),
    ]
    .spacing(sp.space_xs)
    .align_y(Alignment::Center);

    applet::menu_button(header_row)
        .on_press(Message::ToggleDeviceGroup(kind))
        .into()
}

/// Partition devices into ordered display groups
/// Order: Connected -> Pairing Requests -> Available -> Offline
fn partition_devices(devices: &[DeviceInfo]) -> Vec<(GroupKind, Vec<&DeviceInfo>)> {
    let mut connected = Vec::new();
    let mut pairing = Vec::new();
    let mut available = Vec::new();
    let mut offline = Vec::new();

    for d in devices {
        if d.is_pair_requested || d.is_pair_requested_by_peer {
            pairing.push(d);
        } else if d.is_reachable && d.is_paired {
            connected.push(d);
        } else if d.is_reachable && !d.is_paired {
            // "Available to pair" - all reachable unpaired devices
            available.push(d);
        } else if !d.is_reachable && d.is_paired {
            offline.push(d);
        }
        // else: !reachable && !paired -> pure noise, dropped
    }

    let mut groups = Vec::new();
    if !connected.is_empty() {
        groups.push((GroupKind::Connected, connected));
    }
    if !pairing.is_empty() {
        groups.push((GroupKind::PairingRequests, pairing));
    }
    if !available.is_empty() {
        groups.push((GroupKind::Available, available));
    }
    if !offline.is_empty() {
        groups.push((GroupKind::Offline, offline));
    }
    groups
}

/// Render a single device row.
fn device_row(device: &DeviceInfo) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();

    let (status_text, is_offline) = match (
        device.is_reachable,
        device.is_paired,
        device.is_pair_requested,
        device.is_pair_requested_by_peer,
    ) {
        (_, _, _, true) => (fl!("pairing-request"), false),
        (_, _, true, _) => (fl!("pairing"), false),
        (true, true, _, _) => (fl!("connected"), false),
        (false, true, _, _) => (fl!("offline"), true),
        (true, false, _, _) => (fl!("not-paired"), false),
        _ => (fl!("offline"), true),
    };

    // Apply warning color (yellow) to offline status text for better visual indication
    let status_widget: Element<Message> = if is_offline {
        text::caption(status_text)
            .class(theme::Text::Custom(warning_style))
            .into()
    } else {
        text::caption(status_text).into()
    };

    let class = DeviceClass::from_device_type(&device.device_type);
    let mut row_content = row![
        icon::from_name(class.icon_name()).size(24),
        column![text::body(device.name.clone()), status_widget,].spacing(2),
    ]
    .spacing(sp.space_xs)
    .align_y(Alignment::Center);

    // Add battery info if available and enabled in settings
    // KDE Connect returns -1 when battery level is unknown, so filter those out
    if let (Some(level), Some(charging)) = (device.battery_level, device.battery_charging) {
        if level >= 0 {
            let battery_text = if charging {
                format!("{}%+", level)
            } else {
                format!("{}%", level)
            };
            row_content = row_content.push(text::caption(battery_text));
        }
    }

    // Add notification count badge if there are notifications
    if !device.notifications.is_empty() {
        row_content = row_content.push(
            widget::container(text::caption(format!("{}", device.notifications.len())))
                .padding([2, sp.space_xxxs as u16 + 2])
                .class(cosmic::theme::Container::Card),
        );
    }

    // Add chevron indicator to show it's clickable
    row_content = row_content.push(widget::space::horizontal());
    row_content = row_content.push(icon::from_name("go-next-symbolic").size(16));

    applet::menu_button(row_content)
        .on_press(Message::SelectDevice(device.id.clone()))
        .into()
}
