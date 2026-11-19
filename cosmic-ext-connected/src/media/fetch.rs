//! Media information fetching and control actions.

use crate::app::{MediaInfo, Message};
use kdeconnect_dbus::plugins::MprisRemoteProxy;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Media control action types.
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    SetVolume(i32),
    SelectPlayer(String),
}

/// Fetch media information from a device.
pub async fn fetch_media_info_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/mprisremote",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let proxy = match MprisRemoteProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("Failed to create mpris proxy: {}", e);
                return Message::MediaInfoLoaded(None);
            }
        },
        None => {
            tracing::debug!("Failed to build mpris proxy path");
            return Message::MediaInfoLoaded(None);
        }
    };

    // Fetch all properties
    let players = proxy.player_list().await.unwrap_or_default();
    let current_player = proxy.player().await.unwrap_or_default();
    let title = proxy.title().await.unwrap_or_default();
    let artist = proxy.artist().await.unwrap_or_default();
    let album = proxy.album().await.unwrap_or_default();
    let is_playing = proxy.is_playing().await.unwrap_or(false);
    let volume = proxy.volume().await.unwrap_or(0);
    // D-Bus returns i32 for position/length, convert to i64
    let position = proxy.position().await.unwrap_or(0) as i64;
    let length = proxy.length().await.unwrap_or(0) as i64;
    // Note: canGoNext/canGoPrevious are per-player properties not exposed on the main interface.
    // We default to true to allow actions; the phone will handle if unsupported.
    let can_next = true;
    let can_previous = true;

    Message::MediaInfoLoaded(Some(MediaInfo {
        players,
        current_player,
        title,
        artist,
        album,
        is_playing,
        volume,
        position,
        length,
        can_next,
        can_previous,
    }))
}

/// Execute a media control action on a device.
/// If `ensure_player` is provided, the player will be selected before performing the action.
pub async fn media_action_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    action: MediaAction,
    ensure_player: Option<String>,
) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/mprisremote",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let proxy = match async {
        MprisRemoteProxy::builder(&conn)
            .path(path.as_str())?
            .build()
            .await
    }
    .await
    {
        Ok(p) => p,
        Err(e) => return Message::MediaActionResult(Err(e.to_string())),
    };

    // If a specific player is requested, ensure it's selected first
    if let Some(ref player) = ensure_player {
        if let Err(e) = proxy.set_player(player).await {
            tracing::warn!("Failed to set player before action: {}", e);
            // Continue anyway - the action might still work
        }
    }

    let result = match action {
        MediaAction::PlayPause => proxy.send_action("PlayPause").await,
        MediaAction::Next => proxy.send_action("Next").await,
        MediaAction::Previous => proxy.send_action("Previous").await,
        MediaAction::SetVolume(vol) => proxy.set_volume(vol).await,
        MediaAction::SelectPlayer(player) => proxy.set_player(&player).await,
    };

    match result {
        Ok(()) => Message::MediaActionResult(Ok("OK".to_string())),
        Err(e) => Message::MediaActionResult(Err(e.to_string())),
    }
}
