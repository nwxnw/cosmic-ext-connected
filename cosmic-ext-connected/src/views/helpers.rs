//! Helper functions and constants for view rendering.
use crate::fl;

/// Format a Unix timestamp as a human-readable date/time string.
pub fn format_timestamp(timestamp: i64) -> String {
    use chrono::{Local, TimeZone};
    let datetime = Local.timestamp_millis_opt(timestamp).single();
    match datetime {
        Some(dt) => {
            let now = Local::now();
            if dt.date_naive() == now.date_naive() {
                dt.format("%H:%M").to_string()
            } else {
                dt.format("%b %d").to_string()
            }
        }
        None => fl!("unknown"),
    }
}

/// Format milliseconds as mm:ss time string.
pub fn format_duration(ms: i64) -> String {
    if ms <= 0 {
        return "0:00".to_string();
    }
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{}:{:02}", minutes, seconds)
}
