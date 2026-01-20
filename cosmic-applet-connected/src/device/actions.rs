//! Device actions: ping, find my phone, share, pairing, clipboard, notifications.

use crate::app::Message;
use kdeconnect_dbus::{
    plugins::{ClipboardProxy, FindMyPhoneProxy, NotificationProxy, PingProxy, ShareProxy},
    DeviceProxy,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::Connection;

/// Send a ping to a device.
pub async fn send_ping_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let path = format!("{}/devices/{}/ping", kdeconnect_dbus::BASE_PATH, device_id);

    let ping = PingProxy::builder(&conn)
        .path(path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    ping.send_ping().await.map_err(|e| e.to_string())
}

/// Trigger a device to ring so the user can find it.
pub async fn find_my_phone_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/findmyphone",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let findmyphone = FindMyPhoneProxy::builder(&conn)
        .path(path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    findmyphone.ring().await.map_err(|e| e.to_string())
}

/// Share a file to a device.
pub async fn share_file_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    path: PathBuf,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    let url = format!("file://{}", path.display());
    share.share_url(&url).await.map_err(|e| e.to_string())
}

/// Share text to a device.
pub async fn share_text_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    text: String,
) -> Result<(), String> {
    let conn = conn.lock().await;
    let share_path = format!("{}/devices/{}/share", kdeconnect_dbus::BASE_PATH, device_id);

    let share = ShareProxy::builder(&conn)
        .path(share_path.as_str())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;

    share.share_text(&text).await.map_err(|e| e.to_string())
}

/// Request pairing with a device.
pub async fn request_pair_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.request_pair().await {
        Ok(()) => Message::PairingResult(Ok(
            "Pairing request sent. Please accept on your device.".to_string()
        )),
        Err(e) => Message::PairingResult(Err(format!("Failed to request pairing: {}", e))),
    }
}

/// Unpair from a device.
pub async fn unpair_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.unpair().await {
        Ok(()) => Message::PairingResult(Ok("Device unpaired successfully.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to unpair: {}", e))),
    }
}

/// Accept incoming pairing request.
pub async fn accept_pairing_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.accept_pairing().await {
        Ok(()) => Message::PairingResult(Ok("Pairing accepted.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to accept pairing: {}", e))),
    }
}

/// Reject or cancel a pairing request.
pub async fn reject_pairing_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;

    let device = match DeviceProxy::for_device(&conn, &device_id).await {
        Ok(d) => d,
        Err(e) => {
            return Message::PairingResult(Err(format!("Failed to connect to device: {}", e)));
        }
    };

    match device.reject_pairing().await {
        Ok(()) => Message::PairingResult(Ok("Pairing rejected/cancelled.".to_string())),
        Err(e) => Message::PairingResult(Err(format!("Failed to reject pairing: {}", e))),
    }
}

/// Dismiss a notification on a device.
pub async fn dismiss_notification_async(
    conn: Arc<Mutex<Connection>>,
    device_id: String,
    notification_id: String,
) -> Message {
    let conn = conn.lock().await;

    let notif_path = format!(
        "{}/devices/{}/notifications/{}",
        kdeconnect_dbus::BASE_PATH,
        device_id,
        notification_id
    );

    let notif_proxy = match NotificationProxy::builder(&conn)
        .path(notif_path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(p) => p,
            Err(e) => {
                return Message::DismissResult(Err(format!(
                    "Failed to create notification proxy: {}",
                    e
                )));
            }
        },
        None => {
            return Message::DismissResult(Err(
                "Failed to build notification proxy path".to_string()
            ));
        }
    };

    match notif_proxy.dismiss().await {
        Ok(()) => Message::DismissResult(Ok("Notification dismissed".to_string())),
        Err(e) => Message::DismissResult(Err(format!("Failed to dismiss: {}", e))),
    }
}

/// Send current desktop clipboard to a device.
pub async fn send_clipboard_async(conn: Arc<Mutex<Connection>>, device_id: String) -> Message {
    let conn = conn.lock().await;
    let path = format!(
        "{}/devices/{}/clipboard",
        kdeconnect_dbus::BASE_PATH,
        device_id
    );

    let clipboard = match ClipboardProxy::builder(&conn)
        .path(path.as_str())
        .ok()
        .map(|b| b.build())
    {
        Some(fut) => match fut.await {
            Ok(c) => c,
            Err(e) => {
                return Message::ClipboardResult(Err(format!("Failed to create proxy: {}", e)))
            }
        },
        None => return Message::ClipboardResult(Err("Failed to build proxy path".to_string())),
    };

    match clipboard.send_clipboard().await {
        Ok(()) => Message::ClipboardResult(Ok("Clipboard sent to device".to_string())),
        Err(e) => Message::ClipboardResult(Err(format!("Failed to send clipboard: {}", e))),
    }
}
