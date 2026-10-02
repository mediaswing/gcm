// Graphical Cloud Manager — users, groups and devices in Microsoft Entra ID.
// Copyright (c) 2026 Will Richards. Released under the MIT licence; see LICENSE.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod csvio;
mod export;
mod graph;
mod logging;
mod secrets;
mod task;
mod theme;
mod ui;

/// The name shown in the title bar, and the one the README uses. The binary
/// itself stays `gcm`.
pub const APP_NAME: &str = "Graphical Cloud Manager";

fn main() -> eframe::Result {
    logging::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("gcm")
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([860.0, 520.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
