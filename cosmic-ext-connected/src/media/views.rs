//! Media control view components.

use crate::app::{MediaInfo, Message};
use crate::fl;
use crate::views::helpers::format_duration;
use cosmic::applet;
use cosmic::iced::widget::{column, row};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, text};
use cosmic::Element;

/// Parameters for the media controls view.
pub struct MediaControlsParams<'a> {
    pub device_name: Option<&'a str>,
    pub media_info: Option<&'a MediaInfo>,
    pub media_loading: bool,
}

/// Render the media controls view.
pub fn view_media_controls(params: MediaControlsParams<'_>) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();
    let default_device = fl!("device");
    let device_name = params.device_name.unwrap_or(&default_device);

    let header = applet::padded_control(
        row![
            widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                .class(cosmic::theme::Button::Link)
                .on_press(Message::CloseMediaView),
            text::heading(format!("{} - {}", fl!("media"), device_name)),
            widget::space::horizontal(),
        ]
        .spacing(sp.space_xxs)
        .align_y(Alignment::Center),
    );

    let content: Element<Message> = if params.media_loading {
        widget::container(
            column![text::body(fl!("loading-media")),]
                .spacing(sp.space_xs)
                .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(sp.space_m)
        .into()
    } else if let Some(info) = params.media_info {
        if info.players.is_empty() {
            // No active media players
            widget::container(
                column![
                    widget::icon::from_name("multimedia-player-symbolic").size(48),
                    text::body(fl!("no-media-players")),
                    text::caption(fl!("start-playing")),
                ]
                .spacing(sp.space_xs)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding(sp.space_m)
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
                text::body(fl!("media-not-available")),
                text::caption(fl!("enable-mpris")),
            ]
            .spacing(sp.space_xs)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(sp.space_m)
        .into()
    };

    column![header, content,]
        .spacing(sp.space_xxs)
        .width(Length::Fill)
        .into()
}

/// Render the media player with controls.
pub fn view_media_player(info: &MediaInfo) -> Element<'_, Message> {
    let sp = cosmic::theme::spacing();

    // Player selector (if multiple players)
    let player_selector: Element<Message> = if info.players.len() > 1 {
        // Find selected index, defaulting to first player if current_player is empty or not found
        let selected_idx = if info.current_player.is_empty() {
            Some(0)
        } else {
            info.players
                .iter()
                .position(|p| p == &info.current_player)
                .or(Some(0))
        };
        let players_for_closure: Vec<String> = info.players.clone();

        applet::padded_control(
            row![
                text::caption(fl!("player")),
                widget::dropdown(&info.players[..], selected_idx, move |idx| {
                    Message::MediaSelectPlayer(players_for_closure[idx].clone())
                })
                .width(Length::Fill),
            ]
            .spacing(sp.space_xxs)
            .align_y(Alignment::Center),
        )
        .into()
    } else {
        applet::padded_control(text::caption(info.current_player.clone())).into()
    };

    // Track info
    let title_text = if info.title.is_empty() {
        fl!("no-track-playing")
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
        text::body(title_text),
        text::caption(artist_text),
        text::caption(album_text),
    ]
    .spacing(sp.space_xxxs)
    .align_x(Alignment::Center)
    .width(Length::Fill);

    // Position display
    let position_str = format_duration(info.position);
    let length_str = format_duration(info.length);
    let position_display = row![
        text::caption(position_str),
        widget::space::horizontal(),
        text::caption(length_str),
    ]
    .padding([0, sp.space_xs as u16]);

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
        .spacing(sp.space_s)
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
        text::caption(format!("{}%", info.volume)).width(Length::Fixed(36.0)),
    ]
    .spacing(sp.space_xxs)
    .align_y(Alignment::Center)
    .padding([0, sp.space_xs as u16]);

    let divider = || applet::padded_control(widget::divider::horizontal::default());

    // Assemble the view
    column![
        player_selector,
        divider(),
        widget::container(widget::icon::from_name("multimedia-player-symbolic").size(48))
            .width(Length::Fill)
            .align_x(Alignment::Center),
        applet::padded_control(track_info),
        divider(),
        position_display,
        controls_container,
        divider(),
        volume_row,
    ]
    .spacing(sp.space_xxxs)
    .padding([0, 0, sp.space_s as u16, 0])
    .width(Length::Fill)
    .into()
}
