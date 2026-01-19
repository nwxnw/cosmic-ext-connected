//! COSMIC Connected applet entry point.
//!
//! This applet provides phone-to-desktop connectivity via KDE Connect,
//! with a native COSMIC desktop interface.

mod app;
mod config;
mod i18n;
mod ui;

use app::ConnectApplet;

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("cosmic_applet_connected=debug".parse().unwrap()),
        )
        .init();

    // Initialize localization
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();
    i18n::init(&requested_languages);

    tracing::info!("Starting COSMIC Connected applet");
    cosmic::applet::run::<ConnectApplet>(())
}
