//! The last thing between a keystroke and an irreversible change.
//!
//! Confirmation is driven entirely by [`Severity`], so the modal cannot
//! disagree with the action about how dangerous it is — there is no second
//! list of "things that need confirming" to fall out of step.
//!
//! For destructive actions the operator must type the target's name. That is
//! not ceremony: it defeats the muscle memory of hammering Enter, and it forces
//! the eyes onto *which* object is about to be destroyed, which is the mistake
//! that actually happens in practice.

use egui::{Color32, RichText};

use super::theme;
use crate::graph::actions::{Action, Severity};
use crate::ldap::actions::DirectoryAction;

/// What is waiting to be approved.
///
/// The two cannot be mixed in one dialog, and that is deliberate: approving a
/// tenant change and an on-premises change together would obscure that they
/// land in different directories, with different blast radiuses, under
/// different permissions.
pub enum Subject {
    /// One or more tenant changes, confirmed as a unit.
    Tenant(Vec<Action>),
    /// One on-premises change. Never batched — the AD panes act on a single
    /// selected object.
    Directory(Box<DirectoryAction>),
}

/// Actions waiting for the operator to approve or reject them.
///
/// A batch is confirmed as a unit, because approving twelve wipes one dialog at
/// a time trains exactly the reflex this is meant to defeat.
pub struct Pending {
    pub subject: Subject,
    /// What the operator has typed into the confirmation field.
    pub typed: String,
    /// Set once, to move focus into the field on the first frame.
    focused: bool,
}

impl Pending {
    pub fn new(actions: Vec<Action>) -> Self {
        Self {
            subject: Subject::Tenant(actions),
            typed: String::new(),
            focused: false,
        }
    }

    /// A single on-premises change, confirmed by exactly the same rules.
    ///
    /// Routing AD writes through this dialog rather than a parallel one is
    /// what stops the two drifting: a destructive change still has to be
    /// typed out, whichever directory it lands in.
    pub fn for_directory(action: DirectoryAction) -> Self {
        Self {
            subject: Subject::Directory(Box::new(action)),
            typed: String::new(),
            focused: false,
        }
    }

    /// The tenant actions, if that is what this is.
    fn tenant_actions(&self) -> &[Action] {
        match &self.subject {
            Subject::Tenant(actions) => actions,
            Subject::Directory(_) => &[],
        }
    }

    /// The most dangerous severity in the batch governs the whole batch.
    pub fn severity(&self) -> Severity {
        match &self.subject {
            Subject::Tenant(actions) => actions
                .iter()
                .map(Action::severity)
                .max()
                .unwrap_or(Severity::Safe),
            Subject::Directory(action) => action.severity(),
        }
    }

    /// What the operator must type, if anything.
    ///
    /// A single action asks for its target's name — which forces the eye onto
    /// *which* object. A batch asks for the verb and the count, which forces it
    /// onto *how many*; naming one of twelve would prove nothing.
    pub fn confirm_phrase(&self) -> Option<String> {
        if self.severity() != Severity::Destructive {
            return None;
        }
        match &self.subject {
            // A single on-premises deletion names its target, for the same
            // reason a single tenant one does: it forces the eye onto *which*
            // object is about to go.
            Subject::Directory(action) => Some(action.target_name().to_string()),
            Subject::Tenant(actions) => match actions.as_slice() {
                [] => None,
                [single] => single.confirm_phrase(),
                many => Some(format!("{} {}", many[0].verb(), many.len())),
            },
        }
    }

    /// One line describing the whole batch.
    pub fn label(&self) -> String {
        match &self.subject {
            Subject::Directory(action) => action.label(),
            Subject::Tenant(actions) => match actions.as_slice() {
                [] => String::new(),
                [single] => single.label(),
                many => format!("{} {} objects", capitalise(many[0].verb()), many.len()),
            },
        }
    }

    /// Warning text, when every action in the batch shares one.
    fn consequence(&self) -> Option<&'static str> {
        match &self.subject {
            Subject::Directory(action) => action.consequence(),
            Subject::Tenant(actions) => {
                let first = actions.first()?.consequence()?;
                actions
                    .iter()
                    .all(|action| action.consequence() == Some(first))
                    .then_some(first)
            }
        }
    }

    /// Whether the typed text satisfies the confirmation requirement.
    ///
    /// Trimmed and case-insensitive: the point is to prove the operator read
    /// what this affects, not to test their typing.
    pub fn satisfied(&self) -> bool {
        match self.confirm_phrase() {
            None => true,
            Some(phrase) => self.typed.trim().eq_ignore_ascii_case(phrase.trim()),
        }
    }
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => {
            first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
        }
        None => String::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Pending,
    Cancelled,
    Confirmed,
}

fn severity_color(severity: Severity) -> Color32 {
    match severity {
        Severity::Safe => theme::ACCENT,
        Severity::Caution => theme::WARN,
        Severity::Destructive => theme::BAD,
    }
}

fn severity_title(severity: Severity) -> &'static str {
    match severity {
        Severity::Safe => "Confirm",
        Severity::Caution => "Confirm change",
        Severity::Destructive => "This cannot be undone",
    }
}

/// Show the confirmation modal for a pending action.
pub fn action_modal(ctx: &egui::Context, pending: &mut Pending) -> Outcome {
    let severity = pending.severity();
    let color = severity_color(severity);
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("confirm-action")).show(ctx, |ui| {
        ui.set_width(460.0);

        ui.label(
            RichText::new(severity_title(severity))
                .size(15.0)
                .strong()
                .color(color),
        );
        ui.add_space(10.0);

        ui.label(RichText::new(pending.label()).size(14.0));

        // For a batch, name every object. Being able to see what is in the set
        // is the whole reason a bulk confirmation is trustworthy.
        if pending.tenant_actions().len() > 1 {
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for action in pending.tenant_actions() {
                        ui.label(
                            RichText::new(format!("• {}", action.target_name()))
                                .small()
                                .color(theme::MUTED),
                        );
                    }
                });
        }

        if let Some(consequence) = pending.consequence() {
            ui.add_space(10.0);
            egui::Frame::group(ui.style())
                .fill(ui.visuals().faint_bg_color)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(consequence).color(theme::MUTED));
                });
        }

        if let Some(phrase) = pending.confirm_phrase() {
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!("Type {phrase} to confirm:")).color(theme::MUTED),
            );
            ui.add_space(4.0);

            let field = ui.add(
                egui::TextEdit::singleline(&mut pending.typed)
                    .desired_width(f32::INFINITY)
                    .hint_text(&phrase),
            );
            if !pending.focused {
                field.request_focus();
                pending.focused = true;
            }
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        let satisfied = pending.satisfied();

        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match severity {
                    Severity::Destructive => {
                        RichText::new(pending.label()).color(Color32::WHITE).strong()
                    }
                    _ => RichText::new("Confirm"),
                };

                let button = egui::Button::new(label).fill(if satisfied {
                    color
                } else {
                    theme::MUTED
                });

                if ui.add_enabled(satisfied, button).clicked() {
                    outcome = Outcome::Confirmed;
                }

                if !satisfied {
                    ui.label(
                        RichText::new("Does not match")
                            .small()
                            .color(theme::MUTED),
                    );
                }
            });
        });

        // Enter confirms, but only once the typed name matches — so holding
        // Enter through a dialog cannot complete a destructive action.
        if satisfied && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            outcome = Outcome::Confirmed;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
    });

    // Clicking the backdrop is a cancel, never a confirm.
    if outcome == Outcome::Pending && response.should_close() {
        outcome = Outcome::Cancelled;
    }

    outcome
}

/// Show the modal that arms write mode.
pub fn arm_modal(ctx: &egui::Context) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("confirm-arm")).show(ctx, |ui| {
        ui.set_width(440.0);

        ui.label(
            RichText::new("Enable write mode")
                .size(15.0)
                .strong()
                .color(theme::WARN),
        );
        ui.add_space(10.0);

        ui.label("Write mode lets this console change your tenant — including disabling accounts, removing licences, and wiping devices.");
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "It turns itself off after 15 minutes of inactivity, and whenever gcm restarts.",
            )
            .color(theme::MUTED),
        );

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui.button("Stay read-only").clicked() {
                outcome = Outcome::Cancelled;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(
                        RichText::new("Enable write mode").color(Color32::WHITE),
                    ).fill(theme::WARN))
                    .clicked()
                {
                    outcome = Outcome::Confirmed;
                }
            });
        });

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
    });

    if outcome == Outcome::Pending && response.should_close() {
        outcome = Outcome::Cancelled;
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::actions::DeviceOp;

    fn pending_wipe() -> Pending {
        Pending::new(vec![Action::ManagedDevice {
            id: "d".into(),
            name: "LON-LT-0041".into(),
            op: DeviceOp::Wipe,
        }])
    }

    #[test]
    fn destructive_actions_start_unsatisfied() {
        // An empty field must never be a valid confirmation.
        assert!(!pending_wipe().satisfied());
    }

    #[test]
    fn the_exact_name_satisfies() {
        let mut pending = pending_wipe();
        pending.typed = "LON-LT-0041".into();
        assert!(pending.satisfied());
    }

    #[test]
    fn matching_tolerates_case_and_surrounding_space() {
        let mut pending = pending_wipe();
        pending.typed = "  lon-lt-0041 ".into();
        assert!(pending.satisfied());
    }

    #[test]
    fn a_different_device_name_does_not_satisfy() {
        // The failure this whole mechanism exists to prevent: confirming the
        // wipe of the device next to the one you meant.
        let mut pending = pending_wipe();
        pending.typed = "LON-LT-0042".into();
        assert!(!pending.satisfied());
    }

    #[test]
    fn a_prefix_does_not_satisfy() {
        let mut pending = pending_wipe();
        pending.typed = "LON-LT".into();
        assert!(!pending.satisfied());
    }

    fn delete_users(count: usize) -> Pending {
        Pending::new(
            (0..count)
                .map(|i| Action::DeleteUser {
                    id: format!("u{i}"),
                    name: format!("User {i}"),
                })
                .collect(),
        )
    }

    #[test]
    fn a_batch_asks_for_the_verb_and_the_count() {
        // Naming one of twelve would prove nothing about the other eleven.
        let pending = delete_users(12);
        assert_eq!(pending.confirm_phrase().as_deref(), Some("DELETE 12"));
        assert_eq!(pending.label(), "Delete 12 objects");
    }

    #[test]
    fn a_batch_phrase_must_match_the_real_count() {
        let mut pending = delete_users(12);
        pending.typed = "DELETE 11".into();
        assert!(!pending.satisfied(), "a wrong count must not confirm");
        pending.typed = "delete 12".into();
        assert!(pending.satisfied());
    }

    #[test]
    fn one_action_batches_still_name_their_target() {
        let pending = delete_users(1);
        assert_eq!(pending.confirm_phrase().as_deref(), Some("User 0"));
    }

    #[test]
    fn the_worst_severity_governs_a_mixed_batch() {
        // A single delete among otherwise harmless edits must pull the whole
        // batch up to a typed confirmation.
        let pending = Pending::new(vec![
            Action::SetUserEnabled {
                id: "u".into(),
                name: "n".into(),
                enabled: true,
            },
            Action::DeleteUser {
                id: "d".into(),
                name: "gone".into(),
            },
        ]);
        assert_eq!(pending.severity(), Severity::Destructive);
        assert!(pending.confirm_phrase().is_some());
        assert!(!pending.satisfied());
    }

    #[test]
    fn consequence_text_only_shows_when_the_batch_agrees() {
        // Mixed batches get no blanket warning, because it would be wrong for
        // some of the members.
        let uniform = delete_users(3);
        assert!(uniform.consequence().is_some());

        let mixed = Pending::new(vec![
            Action::DeleteUser {
                id: "u".into(),
                name: "n".into(),
            },
            Action::DeleteGroup {
                id: "g".into(),
                name: "n".into(),
            },
        ]);
        assert!(mixed.consequence().is_none());
    }

    #[test]
    fn non_destructive_actions_are_satisfied_immediately() {
        let pending = Pending::new(vec![Action::SetUserEnabled {
            id: "u".into(),
            name: "Aisha Rahman".into(),
            enabled: false,
        }]);
        assert!(pending.satisfied());
    }
}
