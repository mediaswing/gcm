//! The "an update is available" dialog.
//!
//! Lowest priority of every modal the console shows: it is drawn last in
//! [`super::App::modals`], behind forms, confirmations, the database export
//! and everything else, so a pending release never interrupts something the
//! operator is already in the middle of.

use egui::RichText;

use super::theme;
use crate::update::Release;

pub enum Outcome {
    Pending,
    Dismissed,
    Update,
    OpenReleasePage,
}

pub fn show(ctx: &egui::Context, release: &Release) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("update-available")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(
            RichText::new(format!("gcm {} is available", release.version))
                .size(15.0)
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("You are running {}.", env!("CARGO_PKG_VERSION")))
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(10.0);

        if !release.notes.trim().is_empty() {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(release.notes.trim());
                });
            ui.add_space(10.0);
        }

        ui.label(
            RichText::new(if release.can_self_update {
                "gcm will download the update, close, and reopen on the new version."
            } else {
                // Reached when gcm is not running from an installed
                // application — a loose binary, or a build run from a source
                // tree. There is no install to replace, so the honest offer is
                // the release page.
                "gcm cannot replace this install on its own — this opens the release page \
                 so you can update it the way you installed it."
            })
            .small()
            .color(theme::MUTED),
        );

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("Not now").clicked() {
                outcome = Outcome::Dismissed;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if release.can_self_update {
                    "Update now"
                } else {
                    "Open release page"
                };
                if ui.button(label).clicked() {
                    outcome = if release.can_self_update {
                        Outcome::Update
                    } else {
                        Outcome::OpenReleasePage
                    };
                }
            });
        });
    });

    if matches!(outcome, Outcome::Pending) && response.should_close() {
        outcome = Outcome::Dismissed;
    }

    outcome
}
