//! Configuration management for the COSMIC Connected applet.

use cosmic::cosmic_config::{self, cosmic_config_derive::CosmicConfigEntry, CosmicConfigEntry};
use serde::{Deserialize, Serialize};

/// Application ID for configuration storage.
pub const APP_ID: &str = "com.github.cosmic-connected-applet";

/// Applet configuration stored in COSMIC's config system.
#[derive(Debug, Clone, Serialize, Deserialize, CosmicConfigEntry, PartialEq, Eq)]
#[version = 6]
pub struct Config {
    /// Show battery percentage in device list
    pub show_battery_percentage: bool,
    /// Show offline devices in device list
    pub show_offline_devices: bool,
    /// Enable desktop notifications for phone notifications
    pub forward_notifications: bool,
    /// Number of SMS messages to load per page/request
    pub messages_per_page: u32,
    /// Enable desktop notifications for incoming SMS messages
    pub sms_notifications: bool,
    /// Show message content in SMS notifications (privacy)
    pub sms_notification_show_content: bool,
    /// Show sender name in SMS notifications (privacy)
    pub sms_notification_show_sender: bool,
    /// Enable desktop notifications for incoming/missed calls
    pub call_notifications: bool,
    /// Show phone number in call notifications (privacy)
    pub call_notification_show_number: bool,
    /// Show contact name in call notifications (privacy)
    pub call_notification_show_name: bool,
    /// Enable desktop notifications for received files
    pub file_notifications: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_battery_percentage: true,
            show_offline_devices: true,
            forward_notifications: true,
            messages_per_page: 10,
            sms_notifications: true,
            sms_notification_show_content: true,
            sms_notification_show_sender: true,
            call_notifications: true,
            call_notification_show_number: true,
            call_notification_show_name: true,
            file_notifications: true,
        }
    }
}

impl Config {
    /// Load configuration from disk, falling back to defaults if not found.
    pub fn load() -> Self {
        match cosmic_config::Config::new(APP_ID, Self::VERSION) {
            Ok(config_handler) => {
                let config = Self::get_entry(&config_handler).unwrap_or_else(|err| {
                    tracing::error!(?err, "Failed to load config, using defaults");
                    Self::default()
                });
                tracing::info!("Loaded config: {:?}", config);
                config
            }
            Err(err) => {
                tracing::error!(?err, "Failed to create config handler, using defaults");
                Self::default()
            }
        }
    }

    /// Save configuration to disk.
    pub fn save(&self) -> Result<(), cosmic_config::Error> {
        let config_handler = cosmic_config::Config::new(APP_ID, Self::VERSION)?;
        self.write_entry(&config_handler)?;
        tracing::info!("Saved config: {:?}", self);
        Ok(())
    }
}
