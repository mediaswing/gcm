//! The database export dialog.
//!
//! Two jobs in one modal, because they are one decision: collect the password
//! for this session, and show exactly what is about to be overwritten.
//!
//! The second half matters more than it looks. Everything else gcm writes goes
//! to the tenant, where the write gate and the confirmation modal apply. This
//! writes somewhere else entirely — a database gcm does not own, which somebody
//! else's dashboards read — and it *replaces* what is there. So the dialog
//! names the server, names every table, and gives the row count for each,
//! rather than asking "export?" and hoping.

use egui::RichText;

use super::theme;
use crate::config::MariaDb;
use crate::mariadb::{Secret, Table};

/// What the dialog produced this frame.
pub enum Outcome {
    Pending,
    Cancelled,
    /// Go ahead, with this password.
    Export(Secret),
}

pub struct Prompt {
    /// Tables that will be written, in order.
    pub tables: Vec<Table>,
    /// Typed this session, or empty when it has not been asked for yet.
    password: String,
    /// Set once the field has taken focus, so it is not re-focused every frame.
    focused: bool,
    /// True when a password is already held for this session, in which case
    /// the field is not shown at all.
    remembered: bool,
}

impl Prompt {
    pub fn new(tables: Vec<Table>, remembered: bool) -> Self {
        Self {
            tables,
            password: String::new(),
            focused: false,
            remembered,
        }
    }

    pub fn rows(&self) -> usize {
        self.tables.iter().map(|table| table.rows.len()).sum()
    }
}

pub fn show(
    ctx: &egui::Context,
    prompt: &mut Prompt,
    settings: &MariaDb,
    held: Option<&Secret>,
) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("database-export")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.label(RichText::new("Export to MariaDB").size(15.0).strong());
        ui.add_space(4.0);
        ui.label(
            RichText::new(settings.describe())
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(12.0);

        // Said plainly and near the top: this replaces rather than appends, and
        // it is not the tenant that is being written to.
        ui.label(
            RichText::new(format!(
                "{} tables will be replaced with {} rows. Anything already in them is \
                 discarded.",
                prompt.tables.len(),
                prompt.rows()
            ))
            .color(theme::WARN),
        );
        ui.add_space(10.0);

        egui::ScrollArea::vertical()
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for table in &prompt.tables {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(settings.table_for(table.stem)).monospace(),
                        );
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    RichText::new(format!("{} rows", table.rows.len()))
                                        .small()
                                        .color(theme::MUTED),
                                );
                            },
                        );
                    });
                }
            });

        ui.add_space(12.0);

        if prompt.remembered {
            ui.label(
                RichText::new("Using the password entered earlier this session.")
                    .small()
                    .color(theme::MUTED),
            );
        } else {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(110.0, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.label(RichText::new("Password").color(theme::MUTED));
                    },
                );
                let field = ui.add(
                    egui::TextEdit::singleline(&mut prompt.password)
                        .desired_width(f32::INFINITY)
                        .password(true),
                );
                if !prompt.focused {
                    field.request_focus();
                    prompt.focused = true;
                }
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Kept in memory until gcm closes, and never written to disk.",
                )
                .small()
                .color(theme::MUTED),
            );
        }

        if !settings.require_tls {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "require_tls is off, so this connection — and this password — \
                     cross the network unencrypted.",
                )
                .small()
                .color(theme::BAD),
            );
        }

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);

        // Nothing to send is not an error, but it is not an export either.
        let ready = !prompt.tables.is_empty()
            && (prompt.remembered || !prompt.password.trim().is_empty());

        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(ready, egui::Button::new("Replace tables"))
                    .clicked()
                {
                    outcome = take_password(prompt, held);
                }
                ui.label(
                    RichText::new("Enter to run · Esc to cancel")
                        .small()
                        .color(theme::MUTED),
                );
            });
        });

        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if enter && ready {
            outcome = take_password(prompt, held);
        }
        if escape {
            outcome = Outcome::Cancelled;
        }
    });

    if matches!(outcome, Outcome::Pending) && response.should_close() {
        outcome = Outcome::Cancelled;
    }

    outcome
}

/// Use the session's password where there is one, otherwise what was typed.
fn take_password(prompt: &mut Prompt, held: Option<&Secret>) -> Outcome {
    match held {
        Some(secret) if prompt.remembered => Outcome::Export(secret.clone()),
        // Taken out of the field rather than copied, so the plaintext does not
        // outlive the dialog in the widget's own buffer.
        _ => Outcome::Export(Secret::new(std::mem::take(&mut prompt.password))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(stem: &'static str, rows: usize) -> Table {
        Table {
            stem,
            columns: vec!["Name".into()],
            rows: vec![vec!["x".into()]; rows],
        }
    }

    #[test]
    fn the_dialog_totals_every_table() {
        // The count in the warning is the whole basis on which somebody decides
        // whether this is the export they meant to run.
        let prompt = Prompt::new(vec![table("users", 12), table("groups", 8)], false);
        assert_eq!(prompt.rows(), 20);
        assert_eq!(prompt.tables.len(), 2);
    }

    #[test]
    fn an_empty_export_totals_zero() {
        assert_eq!(Prompt::new(Vec::new(), false).rows(), 0);
    }

    #[test]
    fn a_remembered_password_reuses_the_session_secret() {
        let mut prompt = Prompt::new(vec![table("users", 1)], true);
        let held = Secret::new("hunter2".into());
        match take_password(&mut prompt, Some(&held)) {
            Outcome::Export(secret) => assert_eq!(secret.expose(), "hunter2"),
            _ => panic!("a remembered password must be reused"),
        }
    }

    #[test]
    fn a_typed_password_is_moved_out_of_the_field() {
        // The widget's buffer would otherwise keep the plaintext alive for as
        // long as the dialog struct does.
        let mut prompt = Prompt::new(vec![table("users", 1)], false);
        prompt.password = "typed".into();
        match take_password(&mut prompt, None) {
            Outcome::Export(secret) => assert_eq!(secret.expose(), "typed"),
            _ => panic!("a typed password must be used"),
        }
        assert!(prompt.password.is_empty(), "the field must be cleared");
    }

    #[test]
    fn a_password_is_never_rendered_by_debug() {
        // Belt and braces around the Secret newtype: this is the property the
        // whole per-session design rests on.
        let secret = Secret::new("hunter2".into());
        assert!(!format!("{secret:?}").contains("hunter2"));
    }
}
