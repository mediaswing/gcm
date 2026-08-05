//! The import preview — what a file would do, before it does it.
//!
//! The operator sees the resolved actions and every skipped row with its
//! reason, then approves. Approval hands the batch to the ordinary
//! confirmation and worker gate; nothing here shortcuts either.
//!
//! Skipped rows are given as much room as applied ones. A file that silently
//! did half of what was intended is the failure mode worth designing against,
//! and the only defence is making the omissions impossible to miss.

use egui::RichText;

use super::theme;
use crate::importer::Plan;

pub enum Outcome {
    Pending,
    Cancelled,
    /// Run the plan's actions.
    Apply,
}

pub fn show(ctx: &egui::Context, plan: &Plan, armed: bool) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("import-preview")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.label(RichText::new("Import preview").size(15.0).strong());
        ui.label(
            RichText::new(format!("{} — {}", plan.source, plan.kind.describe()))
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} to apply", plan.actions.len()))
                    .strong()
                    .color(if plan.actions.is_empty() {
                        theme::MUTED
                    } else {
                        theme::ACCENT
                    }),
            );
            if !plan.skipped.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(format!("{} skipped", plan.skipped.len()))
                        .strong()
                        .color(theme::WARN),
                );
            }
        });
        ui.add_space(10.0);

        egui::ScrollArea::vertical()
            .max_height(340.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if !plan.actions.is_empty() {
                    ui.label(RichText::new("WILL APPLY").small().color(theme::MUTED));
                    ui.add_space(4.0);
                    for action in &plan.actions {
                        ui.label(RichText::new(format!("• {}", action.label())).small());
                    }
                    ui.add_space(12.0);
                }

                if !plan.skipped.is_empty() {
                    ui.label(RichText::new("SKIPPED").small().color(theme::MUTED));
                    ui.add_space(4.0);
                    for skipped in &plan.skipped {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!("line {}", skipped.line))
                                    .small()
                                    .color(theme::MUTED),
                            );
                            if !skipped.subject.is_empty() {
                                ui.label(RichText::new(&skipped.subject).small());
                            }
                            ui.label(
                                RichText::new(&skipped.reason)
                                    .small()
                                    .color(theme::WARN),
                            );
                        });
                    }
                }
            });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        if !armed {
            ui.label(
                RichText::new("Write mode is off — enable it with Ctrl+Shift+W to apply this.")
                    .small()
                    .color(theme::WARN),
            );
            ui.add_space(6.0);
        }

        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }
            if !plan.skipped.is_empty() && ui.button("Copy skipped rows").clicked() {
                let report = plan
                    .skipped
                    .iter()
                    .map(|skipped| {
                        format!("line {}: {} — {}", skipped.line, skipped.subject, skipped.reason)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.ctx().copy_text(report);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_apply = armed && !plan.actions.is_empty();
                if ui
                    .add_enabled(
                        can_apply,
                        egui::Button::new(format!("Apply {}", plan.actions.len())),
                    )
                    .clicked()
                {
                    outcome = Outcome::Apply;
                }
            });
        });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
    });

    if matches!(outcome, Outcome::Pending) && response.should_close() {
        outcome = Outcome::Cancelled;
    }

    outcome
}
