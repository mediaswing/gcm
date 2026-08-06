//! The Active Directory bind dialog.
//!
//! One job: collect the LDAP bind password for this session. It is a much
//! smaller dialog than the database one because it is asking for much less —
//! nothing here changes anything, on-premises or in the tenant.
//!
//! What it does insist on saying is how the connection is secured. A simple
//! bind sends the password as cleartext in the bind request, so on an
//! unencrypted connection the credential typed into this box crosses the
//! network in the clear. That is a property of the configuration rather than of
//! this dialog, but this is the last moment anyone can act on it.

use egui::RichText;

use super::theme;
use crate::config::Directory;
use crate::mariadb::Secret;

/// What the dialog produced this frame.
pub enum Outcome {
    Pending,
    Cancelled,
    /// Try this password against the domain controller.
    Bind(Secret),
}

pub struct Prompt {
    password: String,
    /// Set once the field has taken focus, so it is not re-focused every frame.
    focused: bool,
    /// Waiting on the worker's answer, so the button does not fire twice.
    in_flight: bool,
    /// Why the last attempt was refused, shown until the next one.
    error: Option<String>,
}

impl Prompt {
    pub fn new() -> Self {
        Self {
            password: String::new(),
            focused: false,
            in_flight: false,
            error: None,
        }
    }

    /// Report a refused bind, and let the operator try again.
    pub fn failed(&mut self, message: String) {
        self.in_flight = false;
        self.error = Some(message);
        // Keep the field focused but empty: the overwhelmingly likely cause is
        // a typo, and the next thing anyone does is retype it.
        self.password.clear();
        self.focused = false;
    }
}

pub fn show(ctx: &egui::Context, prompt: &mut Prompt, settings: &Directory) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("directory-bind")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(
            RichText::new("Connect to Active Directory")
                .size(15.0)
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(settings.describe())
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(12.0);

        ui.label(format!(
            "Reading {} over {}.",
            settings.base_dn,
            settings.transport()
        ));
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(110.0, 0.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.label(RichText::new("Password").color(theme::MUTED));
                },
            );
            let field = ui.add_enabled(
                !prompt.in_flight,
                egui::TextEdit::singleline(&mut prompt.password)
                    .desired_width(f32::INFINITY)
                    .password(true),
            );
            if !prompt.focused && !prompt.in_flight {
                field.request_focus();
                prompt.focused = true;
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("Kept in memory until gcm closes, and never written to disk.")
                .small()
                .color(theme::MUTED),
        );

        // The one thing worth interrupting for. A simple bind puts the password
        // in the bind request as cleartext, so without TLS this box is a
        // credential-disclosure prompt wearing a login dialog's clothes.
        if !settings.tls {
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "tls is off in the [directory] section, so this password crosses \
                     the network in the clear.",
                )
                .small()
                .color(theme::BAD),
            );
        }

        if let Some(error) = &prompt.error {
            ui.add_space(10.0);
            ui.label(RichText::new(error).small().color(theme::BAD));
        }

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);

        let ready = !prompt.in_flight && !prompt.password.trim().is_empty();

        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(ready, egui::Button::new("Connect")).clicked() {
                    outcome = take_password(prompt);
                }
                if prompt.in_flight {
                    ui.label(RichText::new("Connecting…").small().color(theme::MUTED));
                } else {
                    ui.label(
                        RichText::new("Enter to connect · Esc to cancel")
                            .small()
                            .color(theme::MUTED),
                    );
                }
            });
        });

        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if enter && ready {
            outcome = take_password(prompt);
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

/// Take the typed password out of the field rather than copying it, so the
/// plaintext does not outlive the attempt in the widget's own buffer.
fn take_password(prompt: &mut Prompt) -> Outcome {
    prompt.in_flight = true;
    prompt.error = None;
    Outcome::Bind(Secret::new(std::mem::take(&mut prompt.password)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_password_is_moved_out_of_the_field() {
        let mut prompt = Prompt::new();
        prompt.password = "hunter2".into();
        match take_password(&mut prompt) {
            Outcome::Bind(secret) => assert_eq!(secret.expose(), "hunter2"),
            _ => panic!("a typed password must be used"),
        }
        assert!(prompt.password.is_empty(), "the field must be cleared");
        assert!(prompt.in_flight, "the button must not be able to fire twice");
    }

    #[test]
    fn a_refused_bind_clears_the_field_and_lets_it_be_retried() {
        let mut prompt = Prompt::new();
        prompt.password = "wrong".into();
        let _ = take_password(&mut prompt);
        prompt.failed("rejected the credentials".into());

        assert!(!prompt.in_flight, "the dialog must accept another attempt");
        assert!(prompt.password.is_empty());
        assert!(!prompt.focused, "the field takes focus again for the retry");
        assert_eq!(prompt.error.as_deref(), Some("rejected the credentials"));
    }

    #[test]
    fn a_new_attempt_clears_the_previous_error() {
        // Leaving the old message up while a fresh bind is in flight would
        // read as though the retry had already failed.
        let mut prompt = Prompt::new();
        prompt.failed("rejected the credentials".into());
        prompt.password = "second try".into();
        let _ = take_password(&mut prompt);
        assert!(prompt.error.is_none());
    }
}
