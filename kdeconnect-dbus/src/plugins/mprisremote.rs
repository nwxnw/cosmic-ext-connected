//! D-Bus proxy for the MPRIS Remote plugin.
//!
//! Provides control of media players running on the remote device (phone).

use zbus::proxy;

/// Proxy for the MPRIS Remote plugin D-Bus interface.
///
/// This allows controlling media players on the connected phone from the desktop.
///
/// Note: Media control actions (play, pause, next, etc.) are sent via the
/// `sendAction` method with the action name as a string parameter.
#[proxy(
    interface = "org.kde.kdeconnect.device.mprisremote",
    default_service = "org.kde.kdeconnect.daemon"
)]
pub trait MprisRemote {
    /// Get the list of available media players on the device.
    #[zbus(property, name = "playerList")]
    fn player_list(&self) -> zbus::Result<Vec<String>>;

    /// Get the currently selected player name.
    #[zbus(property, name = "player")]
    fn player(&self) -> zbus::Result<String>;

    /// Check if the current player is playing.
    #[zbus(property, name = "isPlaying")]
    fn is_playing(&self) -> zbus::Result<bool>;

    /// Get the current volume (0-100).
    #[zbus(property, name = "volume")]
    fn volume(&self) -> zbus::Result<i32>;

    /// Get the track length in milliseconds.
    /// Note: D-Bus returns int32, converted to i64 for consistency with position calculations.
    #[zbus(property, name = "length")]
    fn length(&self) -> zbus::Result<i32>;

    /// Get the current playback position in milliseconds.
    /// Note: D-Bus returns int32, converted to i64 for consistency with position calculations.
    #[zbus(property, name = "position")]
    fn position(&self) -> zbus::Result<i32>;

    /// Get the current track title.
    #[zbus(property, name = "title")]
    fn title(&self) -> zbus::Result<String>;

    /// Get the current track artist.
    #[zbus(property, name = "artist")]
    fn artist(&self) -> zbus::Result<String>;

    /// Get the current track album.
    #[zbus(property, name = "album")]
    fn album(&self) -> zbus::Result<String>;

    /// Check if the player can seek.
    #[zbus(property, name = "canSeek")]
    fn can_seek(&self) -> zbus::Result<bool>;

    /// Seek by a relative offset in milliseconds.
    fn seek(&self, offset: i32) -> zbus::Result<()>;

    /// Set the playback position in milliseconds (writable property).
    #[zbus(property, name = "position")]
    fn set_position(&self, position: i32) -> zbus::Result<()>;

    /// Set the volume (0-100) (writable property).
    #[zbus(property, name = "volume")]
    fn set_volume(&self, volume: i32) -> zbus::Result<()>;

    /// Select a player by name (writable property).
    #[zbus(property, name = "player")]
    fn set_player(&self, player: &str) -> zbus::Result<()>;

    /// Request an updated player list from the device.
    #[zbus(name = "requestPlayerList")]
    fn request_player_list(&self) -> zbus::Result<()>;

    /// Send a media control action to the device.
    ///
    /// Valid actions: "Play", "Pause", "PlayPause", "Stop", "Next", "Previous"
    #[zbus(name = "sendAction")]
    fn send_action(&self, action: &str) -> zbus::Result<()>;
}
