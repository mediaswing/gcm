//! The Settings pane: light, dark, or follow the system.

use egui::{RichText, Ui};

use crate::app::App;
use crate::config::Appearance;
use crate::ui;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui::pane_header(ui, "Settings", "How the app looks.");

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(640.0);
        ui.label(RichText::new("Appearance").strong());
        ui.add_space(6.0);
        let mut changed = false;
        for appearance in Appearance::ALL {
            let selected = app.config.appearance == appearance;
            let button = egui::Button::selectable(
                selected,
                ui::centred(RichText::new(appearance.label()).size(15.0)),
            )
            .corner_radius(6.0)
            .frame_when_inactive(true)
            .min_size(egui::vec2(ui.available_width(), 38.0));
            if ui.add(button).clicked() && !selected {
                app.config.appearance = appearance;
                changed = true;
            }
            ui.label(RichText::new(appearance.description()).size(12.0).weak());
            ui.add_space(8.0);
        }
        if changed && let Err(err) = app.config.save() {
            app.report_error(format!("The setting could not be saved: {err}"));
        }

        ui.add_space(16.0);
        ui.label(
            RichText::new(format!(
                "Settings are kept in {}.",
                crate::config::tilde(&crate::config::config_path())
            ))
            .size(12.0)
            .weak(),
        );
        ui.label(
            RichText::new(concat!(
                "Graphical Cloud Manager ",
                env!("CARGO_PKG_VERSION"),
                " · MIT licence"
            ))
            .size(12.0)
            .weak(),
        );
    });
}
