//! Connected applet entry point.
//!
//! This applet provides phone-to-desktop connectivity via KDE Connect,
//! with a native COSMIC desktop interface.

mod app;
mod config;
mod constants;
mod device;
mod i18n;
mod media;
mod notifications;
mod sms;
mod subscriptions;
mod ui;
mod views;

use app::ConnectApplet;

fn main() -> cosmic::iced::Result {
    use std::io::IsTerminal;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    let default_directive = if cfg!(debug_assertions) {
        "error,cosmic_ext_connected=debug"
    } else {
        "error,cosmic_ext_connected=warn"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_directive));

    let fmt_layer = fmt::layer()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());

    let journald_layer = tracing_journald::layer().ok();

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(journald_layer)
        .init();

    // Initialize localization
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    tracing::info!("Starting Connected applet");
    cosmic::applet::run::<ConnectApplet>(())
}
