//! Reusable UI widgets for the Connected applet.

use cosmic::iced::advanced::widget::text::Style as TextStyle;

/// Caption color for a warning: offline devices, oversize attachments.
pub fn warning_style(theme: &cosmic::Theme) -> TextStyle {
    TextStyle {
        color: Some(theme.cosmic().warning.base.into()),
        ..Default::default()
    }
}
