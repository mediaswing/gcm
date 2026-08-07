//! gcm — Graphical Cloud Manager
//!
//! A Microsoft 365 administration console: users, groups and directory roles,
//! Entra and Intune-managed devices, and licence consumption, in an MMC-style
//! three-pane window that works entirely from the keyboard.
//!
//! It opens read-only and stays that way until write mode is deliberately
//! armed; the gate itself lives in [`worker`], the single point every mutation
//! passes through.

// This is a GUI application; opening a console window behind it on Windows
// would be noise.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actionlog;
mod auth;
mod config;
#[cfg(debug_assertions)]
mod demo;
mod errorlog;
mod graph;
mod ldap;
mod mariadb;
mod importer;
// The schema and INI rendering compile everywhere so `cargo test` can cover
// them on any platform; only the calls into the registry itself are Windows-
// only. That leaves the pure half genuinely unused in a non-Windows build,
// which is the intent rather than an oversight.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "only the tests exercise this off Windows")
)]
mod registry;
mod ui;
mod update;
mod worker;

use ui::{App, FRIENDLY_NAME};

fn main() -> eframe::Result<()> {
    // First line of the run, so every later entry has a version and a start
    // time above it. Diagnosing from a log that does not say which build
    // produced it is guesswork.
    errorlog::info(
        "startup",
        &format!(
            "gcm {} starting on {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        ),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(FRIENDLY_NAME)
            .with_app_id("gcm")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([880.0, 560.0]),
        ..Default::default()
    };

    // Developing the interface without a tenant. Compiled out of release
    // builds, so it cannot be switched on in a shipped binary.
    #[cfg(debug_assertions)]
    if demo::enabled() {
        return eframe::run_native(
            FRIENDLY_NAME,
            options,
            Box::new(|cc| Ok(Box::new(App::demo(cc)))),
        );
    }

    // A configuration problem is reported inside the window rather than on
    // stderr: this binary is launched from a dock or a menu as often as from a
    // shell, and a silent exit would be indistinguishable from a crash.
    let config = config::load();

    eframe::run_native(
        FRIENDLY_NAME,
        options,
        Box::new(|cc| match config {
            Ok(config) => Ok(Box::new(App::new(cc, config))),
            Err(err) => {
                let message = format!("{err:#}");
                errorlog::error("config", &message);
                Ok(Box::new(App::config_error(cc, message)))
            }
        }),
    )
}
