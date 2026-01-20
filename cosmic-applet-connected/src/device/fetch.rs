//! Device fetching and information retrieval.

use crate::app::{DeviceInfo, Message};
use kdeconnect_dbus::{
    plugins::{BatteryProxy, NotificationInfo, NotificationProxy, NotificationsProxy},
    DaemonProxy, DeviceProxy,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Fetch all devices from the KDE Connect daemon via D-Bus.
pub async fn fetch_devices_async(conn: Arc<Mutex<Connection>>) -> Message {
    let conn = conn.lock().await;

    // Get the daemon proxy
    let daemon = match DaemonProxy::new(&conn).await {
        Ok(d) => d,
        Err(e) => {
            return Message::Error(format!("Failed to connect to KDE Connect daemon: {}", e));
        }
    };

    // Get list of all device IDs
    let device_ids = match daemon.devices().await {
        Ok(ids) => ids,
        Err(e) => {
            return Message::Error(format!("Failed to get device list: {}", e));
        }
    };

    tracing::debug!("Found {} device(s)", device_ids.len());

    // Fetch info for each device
    let mut devices = Vec::new();
    for device_id in device_ids {
        match fetch_device_info(&conn, &device_id).await {
            Ok(info) => devices.push(info),
            Err(e) => {
                tracing::warn!("Failed to get info for device {}: {}", device_id, e);
            }
        }
    }

    Message::DevicesUpdated(devices)
}

/// Fetch information for a single device.
pub async fn fetch_device_info(conn: &Connection, device_id: &str) -> Result<DeviceInfo, String> {
    let device = DeviceProxy::for_device(conn, device_id)
        .await
        .map_err(|e| e.to_string())?;

    let id = device_id.to_string();
    let name = device.name().await.map_err(|e| e.to_string())?;
    let device_type = device
        .device_type()
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    let is_reachable = device.is_reachable().await.unwrap_or(false);
    let is_paired = device.is_trusted().await.unwrap_or(false);
    let is_pair_requested = device.is_pair_requested().await.unwrap_or(false);
    let is_pair_requested_by_peer = device.is_pair_requested_by_peer().await.unwrap_or(false);

    // Try to get battery info if available
    let (battery_level, battery_charging) = if is_reachable && is_paired {
        fetch_battery_info(conn, device_id).await
    } else {
        (None, None)
    };

    // Fetch notifications if device is connected and paired
    let notifications = if is_reachable && is_paired {
        fetch_notifications(conn, device_id).await
    } else {
        Vec::new()
    };

    Ok(DeviceInfo {
        id,
        name,
        device_type,
        is_reachable,
        is_paired,
        is_pair_requested,
        is_pair_requested_by_peer,
        battery_level,
        battery_charging,
        notifications,
    })
}

/// Fetch battery information for a device.
pub async fn fetch_battery_info(conn: &Connection, device_id: &str) -> (Option<i32>, Option<bool>) {
    let path = format!(
        "{}/devices/{}/battery",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    tracing::debug!("Fetching battery info from path: {}", path);

    let builder = match BatteryProxy::builder(conn).path(path.as_str()) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to create battery proxy builder: {}", e);
            return (None, None);
        }
    };

    let battery = match builder.build().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to build battery proxy: {}", e);
            return (None, None);
        }
    };

    let charge = match battery.charge().await {
        Ok(c) => {
            tracing::debug!("Battery charge: {}", c);
            Some(c)
        }
        Err(e) => {
            tracing::warn!("Failed to get battery charge: {}", e);
            None
        }
    };

    let is_charging = match battery.is_charging().await {
        Ok(c) => {
            tracing::debug!("Battery is_charging: {}", c);
            Some(c)
        }
        Err(e) => {
            tracing::warn!("Failed to get is_charging: {}", e);
            None
        }
    };

    (charge, is_charging)
}

/// Fetch notifications for a device.
pub async fn fetch_notifications(conn: &Connection, device_id: &str) -> Vec<NotificationInfo> {
    let notifications_path = format!(
        "{}/devices/{}/notifications",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    // Get the notifications proxy
    let notifications_proxy = match NotificationsProxy::builder(conn)
        .path(notifications_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to create notifications proxy: {}", e);
                return Vec::new();
            }
        },
        None => {
            tracing::warn!("Failed to build notifications proxy path");
            return Vec::new();
        }
    };

    // Get list of active notification IDs
    let notification_ids = match notifications_proxy.active_notifications().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("Failed to get active notifications: {}", e);
            return Vec::new();
        }
    };

    tracing::debug!(
        "Found {} notifications for device {}",
        notification_ids.len(),
        device_id
    );

    // Fetch info for each notification
    let mut notifications = Vec::new();
    for notif_id in notification_ids {
        let notif_path = format!(
            "{}/devices/{}/notifications/{}",
            kdeconnect_dbus::BASE_PATH,
            device_id,
            notif_id
        );

        let notif_proxy = match NotificationProxy::builder(conn)
            .path(notif_path.as_str())
            .ok()
            .map(|b| b.build())
        {
            Some(fut) => match fut.await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "Failed to create notification proxy for {}: {}",
                        notif_id,
                        e
                    );
                    continue;
                }
            },
            None => continue,
        };

        let app_name = notif_proxy.app_name().await.unwrap_or_default();
        let title = notif_proxy.title().await.unwrap_or_default();
        let text = notif_proxy.text().await.unwrap_or_default();
        let dismissable = notif_proxy.dismissable().await.unwrap_or(false);
        let reply_id = notif_proxy.reply_id().await.unwrap_or_default();

        notifications.push(NotificationInfo {
            id: notif_id,
            app_name,
            title,
            text,
            dismissable,
            repliable: !reply_id.is_empty(),
        });
    }

    notifications
}
