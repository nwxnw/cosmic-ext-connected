//! Device list view for the applet popup.

use crate::app::{DeviceInfo, Message};
use crate::config::Config;
use crate::fl;
use cosmic::iced::widget::{column, row, text};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

/// Render the device list view.
pub fn view<'a>(
    devices: &'a [DeviceInfo],
    config: &'a Config,
    status_message: Option<&'a str>,
) -> Element<'a, Message> {
    // Header with refresh and settings buttons
    let header = row![
        text(fl!("devices")).size(14),
        widget::horizontal_space(),
        widget::button::icon(icon::from_name("view-refresh-symbolic"))
            .on_press(Message::RefreshDevices),
        widget::button::icon(icon::from_name("emblem-system-symbolic"))
            .on_press(Message::ToggleSettings),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .padding([4, 8]);

    // Filter devices based on config
    let filtered_devices: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| {
            // Always show reachable devices
            if d.is_reachable {
                return true;
            }
            // Show offline paired devices only if config allows
            if d.is_paired && config.show_offline_devices {
                return true;
            }
            false
        })
        .collect();

    let device_rows: Vec<Element<Message>> = filtered_devices
        .iter()
        .map(|device| device_row(device, config))
        .collect();

    let mut content = column![header, widget::divider::horizontal::default(),].spacing(4);

    // Status message bar (for feedback like "Ping sent!", "Sharing file...")
    if let Some(msg) = status_message {
        content = content.push(
            widget::container(text(msg).size(11))
                .padding([4, 8])
                .width(Length::Fill)
                .class(cosmic::theme::Container::Card),
        );
    }

    if device_rows.is_empty() {
        content = content.push(
            widget::container(text(fl!("no-devices")).size(12))
                .padding(16)
                .width(Length::Fill),
        );
    } else {
        content = content.push(column(device_rows).spacing(8));
    }

    widget::container(content.padding(8)).into()
}

/// Render a single device row.
fn device_row<'a>(device: &'a DeviceInfo, config: &'a Config) -> Element<'a, Message> {
    let icon_name = match device.device_type.as_str() {
        "phone" | "smartphone" => "phone-symbolic",
        "tablet" => "tablet-symbolic",
        "desktop" => "computer-symbolic",
        "laptop" => "computer-laptop-symbolic",
        _ => "device-symbolic",
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
        (false, true, _, _) => fl!("offline"),
        (true, false, _, _) => fl!("not-paired"),
        _ => fl!("offline"),
    };

    let mut row_content = row![
        icon::from_name(icon_name).size(24),
        column![
            text(device.name.clone()).size(14),
            text(status_text).size(11),
        ]
        .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    // Add battery info if available and enabled in settings
    if config.show_battery_percentage {
        if let (Some(level), Some(charging)) = (device.battery_level, device.battery_charging) {
            let battery_text = if charging {
                format!("{}%+", level)
            } else {
                format!("{}%", level)
            };
            row_content = row_content.push(text(battery_text).size(12));
        }
    }

    // Add notification count badge if there are notifications and notifications are enabled
    if config.forward_notifications && !device.notifications.is_empty() {
        row_content = row_content.push(
            widget::container(text(format!("{}", device.notifications.len())).size(11))
                .padding([2, 6])
                .class(cosmic::theme::Container::Card),
        );
    }

    // Add chevron indicator to show it's clickable
    row_content = row_content.push(widget::horizontal_space());
    row_content = row_content.push(icon::from_name("go-next-symbolic").size(16));

    widget::button::custom(
        widget::container(row_content)
            .padding(8)
            .width(Length::Fill),
    )
    .class(cosmic::theme::Button::Text)
    .on_press(Message::SelectDevice(device.id.clone()))
    .width(Length::Fill)
    .into()
}
