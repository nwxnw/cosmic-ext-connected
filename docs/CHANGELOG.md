# Changelog

All notable changes to COSMIC Connected will be documented in this file.

## [Unreleased]

### Changed
- Renamed applet from "COSMIC Connect" to "COSMIC Connected"
- Renamed package from `cosmic-applet-connect` to `cosmic-applet-connected`
- Renamed APP_ID from `com.github.cosmic-connect-applet` to `com.github.cosmic-connected-applet`
- SMS compose now sends message on Enter key press

### Added
- File receive notifications with cross-process deduplication
- Call notifications for incoming and missed calls (with privacy controls)
- SMS desktop notifications (with privacy controls)
- Find My Phone feature to ring connected devices
- Media controls (play/pause, next/previous, volume, player selection)
- SMS messaging with conversation list, message threads, and compose
- Contact name resolution from synced vCards
- Settings panel with notification privacy options
- Battery status display
- Clipboard sync (send to device)
- File and URL sharing
- Ping functionality
- Device pairing/unpairing

## [0.1.0] - Initial Release

- Native COSMIC desktop applet for phone connectivity
- Integration with KDE Connect daemon via D-Bus
- Device discovery and management
