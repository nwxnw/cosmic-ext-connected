//! Media control view components.

use crate::app::{MediaInfo, Message};
use crate::fl;
use crate::views::helpers::format_duration;
use cosmic::iced::widget::{column, row, text};
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::Element;

/// Parameters for the media controls view.
pub struct MediaControlsParams<'a> {
    pub device_name: Option<&'a str>,
    pub media_info: Option<&'a MediaInfo>,
    pub media_loading: bool,
}

/// Render the media controls view.
pub fn view_media_controls(params: MediaControlsParams<'_>) -> Element<'_, Message> {
    let default_device = fl!("device");
    let device_name = params.device_name.unwrap_or(&default_device);

    let header = row![
        widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
            .on_press(Message::CloseMediaView),
        text(format!("{} - {}", fl!("media"), device_name)).size(16),
        widget::horizontal_space(),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([8, 12]);

    let content: Element<Message> = if params.media_loading {
        widget::container(
            column![text(fl!("loading-media")).size(14),]
                .spacing(12)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(24)
        .into()
    } else if let Some(info) = params.media_info {
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
            view_media_player(info)
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

/// Render the media player with controls.
pub fn view_media_player(info: &MediaInfo) -> Element<'_, Message> {
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

    let prev_button = widget::button::icon(widget::icon::from_name("media-skip-backward-symbolic"))
        .on_press_maybe(if info.can_previous {
            Some(Message::MediaPrevious)
        } else {
            None
        });

    let play_button =
        widget::button::icon(widget::icon::from_name(play_icon)).on_press(Message::MediaPlayPause);

    let next_button = widget::button::icon(widget::icon::from_name("media-skip-forward-symbolic"))
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
