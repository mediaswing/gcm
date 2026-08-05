//! Modal forms and pickers — the input side of an action.
//!
//! Confirmation ([`super::confirm`]) asks *are you sure*; these ask *what
//! exactly*. Every form ends by producing an [`Action`], which then goes
//! through the same confirmation and worker gate as any other. Nothing here
//! talks to Graph, so a form can never be the thing that writes.
//!
//! Forms borrow the store immutably to populate pickers, so the caller lifts
//! the form out of `App` for the duration of the frame.

use egui::RichText;

use super::{Store, theme};
use crate::graph::actions::{
    Action, AutoReplySpec, DeviceOp, GroupPatch, GroupSpec, MemberRole, UserPatch, UserSpec,
};

/// Length of a generated password. Long enough to satisfy any tenant policy
/// without the operator having to think about it.
const PASSWORD_LENGTH: usize = 20;

/// Character set for generated passwords.
///
/// Excludes the glyphs that get misread when a password is dictated over the
/// phone — `0`/`O`, `1`/`l`/`I` — because that is exactly how a temporary
/// password reaches its owner.
const PASSWORD_ALPHABET: &[u8] =
    b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789@#%+=?";

/// A cryptographically random password.
fn generate_password() -> String {
    let mut bytes = [0u8; PASSWORD_LENGTH];
    if getrandom::fill(&mut bytes).is_err() {
        // Refuse to invent a weak password from a fallback source; the caller
        // surfaces this and the operator can use the portal instead.
        return String::new();
    }

    // Rejection-free mapping is not needed here: the modulo bias across a
    // 63-character alphabet is far below what matters for a one-time password
    // that must be changed at next sign-in.
    bytes
        .iter()
        .map(|byte| PASSWORD_ALPHABET[*byte as usize % PASSWORD_ALPHABET.len()] as char)
        .collect()
}

/// What a form produced this frame.
pub enum Outcome {
    /// Still open.
    Pending,
    Cancelled,
    /// The operator submitted; route this through confirmation.
    Submit(Box<Action>),
}

/// Editable text for a user, held as strings because that is what a text field
/// gives us. Empty means "clear this field", which Graph honours.
#[derive(Default)]
pub struct UserFields {
    pub job_title: String,
    pub department: String,
    pub office_location: String,
    pub mobile_phone: String,
    pub usage_location: String,
}

/// Everything the create-user form gathers before it becomes a [`UserSpec`].
pub struct NewUserFields {
    pub display_name: String,
    /// Alias part of the sign-in name, before the `@`.
    pub alias: String,
    /// Domain part, chosen from the tenant's verified domains.
    pub domain: String,
    pub job_title: String,
    pub department: String,
    pub usage_location: String,
    pub account_enabled: bool,
    pub password: String,
    /// Set once the operator edits the alias by hand, after which it stops
    /// tracking the display name.
    pub alias_edited: bool,
}

pub enum Form {
    CreateUser {
        fields: Box<NewUserFields>,
    },
    /// Somebody's out-of-office. `on` is separate from the message so that
    /// turning replies off does not require clearing what they wrote.
    AutomaticReplies {
        id: String,
        name: String,
        on: bool,
        internal: String,
        external: String,
        audience: String,
        /// True when the two messages are kept identical, which is what most
        /// people want and nobody wants to type twice.
        same_message: bool,
    },
    EditUser {
        id: String,
        name: String,
        fields: UserFields,
    },
    ResetPassword {
        id: String,
        name: String,
        password: String,
    },
    CreateGroup {
        display_name: String,
        mail_nickname: String,
        description: String,
        unified: bool,
    },
    EditGroup {
        id: String,
        name: String,
        display_name: String,
        description: String,
    },
    /// Choose a licence to assign to, or remove from, a user.
    PickLicense {
        user_id: String,
        user_name: String,
        assign: bool,
        filter: String,
        cursor: usize,
        focused: bool,
    },
    /// Choose a group to add a user to, or remove them from.
    PickGroup {
        member_id: String,
        member_name: String,
        role: MemberRole,
        add: bool,
        filter: String,
        cursor: usize,
        focused: bool,
    },
    /// Give an Intune-managed device a new name.
    RenameDevice {
        id: String,
        name: String,
        new_name: String,
    },
    /// The inverse: choose a user to add to, or remove from, a given group.
    PickMember {
        group_id: String,
        group_name: String,
        role: MemberRole,
        add: bool,
        filter: String,
        cursor: usize,
        focused: bool,
    },
}

impl Form {
    pub fn reset_password(id: String, name: String) -> Self {
        Form::ResetPassword {
            id,
            name,
            password: generate_password(),
        }
    }

    pub fn create_user() -> Self {
        Form::CreateUser {
            fields: Box::new(NewUserFields {
                display_name: String::new(),
                alias: String::new(),
                domain: String::new(),
                job_title: String::new(),
                department: String::new(),
                usage_location: String::new(),
                // A new account is expected to be usable; somebody preparing one
                // ahead of a start date can untick it.
                account_enabled: true,
                password: generate_password(),
                alias_edited: false,
            }),
        }
    }

    /// Seed the automatic-replies form from whatever the mailbox has set now,
    /// so opening it and saving without typing changes nothing.
    pub fn automatic_replies(
        id: String,
        name: String,
        current: Option<crate::graph::models::AutomaticReplies>,
    ) -> Self {
        let internal = current
            .as_ref()
            .map(|replies| replies.internal_text())
            .unwrap_or_default();
        let external = current
            .as_ref()
            .map(|replies| replies.external_text())
            .unwrap_or_default();
        Form::AutomaticReplies {
            id,
            name,
            on: current.as_ref().is_some_and(|replies| replies.is_on()),
            same_message: internal == external,
            audience: current
                .as_ref()
                .and_then(|replies| replies.external_audience.clone())
                .unwrap_or_else(|| "all".into()),
            internal,
            external,
        }
    }

    fn title(&self) -> String {
        match self {
            Form::CreateUser { .. } => "New user".into(),
            Form::AutomaticReplies { name, .. } => {
                format!("Automatic replies for {name}")
            }
            Form::EditUser { name, .. } => format!("Edit {name}"),
            Form::ResetPassword { name, .. } => format!("Reset the password for {name}"),
            Form::CreateGroup { .. } => "Create a group".into(),
            Form::EditGroup { name, .. } => format!("Edit {name}"),
            Form::PickLicense {
                user_name, assign, ..
            } => {
                if *assign {
                    format!("Assign a licence to {user_name}")
                } else {
                    format!("Remove a licence from {user_name}")
                }
            }
            Form::PickGroup {
                member_name, add, ..
            } => {
                if *add {
                    format!("Add {member_name} to a group")
                } else {
                    format!("Remove {member_name} from a group")
                }
            }
            Form::RenameDevice { name, .. } => format!("Rename {name}"),
            Form::PickMember {
                group_name,
                role,
                add,
                ..
            } => {
                let role = match role {
                    MemberRole::Member => "member",
                    MemberRole::Owner => "owner",
                };
                if *add {
                    format!("Add a {role} to {group_name}")
                } else {
                    format!("Remove a {role} from {group_name}")
                }
            }
        }
    }
}

/// Draw whichever form is open and report what the operator did.
pub fn show(ctx: &egui::Context, form: &mut Form, store: &Store) -> Outcome {
    let mut outcome = Outcome::Pending;

    let response = egui::Modal::new(egui::Id::new("form")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.label(RichText::new(form.title()).size(15.0).strong());
        ui.add_space(12.0);

        match form {
            Form::CreateUser { fields } => {
                create_user(ui, store, fields, &mut outcome);
            }
            Form::AutomaticReplies {
                id,
                name,
                on,
                internal,
                external,
                audience,
                same_message,
            } => {
                automatic_replies(
                    ui, id, name, on, internal, external, audience, same_message,
                    &mut outcome,
                );
            }
            Form::EditUser { id, name, fields } => {
                edit_user(ui, id, name, fields, &mut outcome);
            }
            Form::ResetPassword { id, name, password } => {
                reset_password(ui, id, name, password, &mut outcome);
            }
            Form::CreateGroup {
                display_name,
                mail_nickname,
                description,
                unified,
            } => {
                create_group(ui, display_name, mail_nickname, description, unified, &mut outcome);
            }
            Form::EditGroup {
                id,
                name,
                display_name,
                description,
            } => {
                edit_group(ui, id, name, display_name, description, &mut outcome);
            }
            Form::PickLicense {
                user_id,
                user_name,
                assign,
                filter,
                cursor,
                focused,
            } => {
                pick_license(
                    ui, store, user_id, user_name, *assign, filter, cursor, focused,
                    &mut outcome,
                );
            }
            Form::PickGroup {
                member_id,
                member_name,
                role,
                add,
                filter,
                cursor,
                focused,
            } => {
                pick_group(
                    ui, store, member_id, member_name, *role, *add, filter, cursor,
                    focused, &mut outcome,
                );
            }
            Form::RenameDevice { id, name, new_name } => {
                rename_device(ui, id, name, new_name, &mut outcome);
            }
            Form::PickMember {
                group_id,
                group_name,
                role,
                add,
                filter,
                cursor,
                focused,
            } => {
                pick_member(
                    ui, store, group_id, group_name, *role, *add, filter, cursor,
                    focused, &mut outcome,
                );
            }
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = Outcome::Cancelled;
        }
    });

    if matches!(outcome, Outcome::Pending) && response.should_close() {
        outcome = Outcome::Cancelled;
    }

    outcome
}

/// A labelled text field, laid out like the details pane it was opened from.
fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(130.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new(label).color(theme::MUTED));
            },
        );
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
    });
    ui.add_space(4.0);
}

/// Cancel / submit row, shared by every form.
///
/// Enter submits and Escape cancels, so a form can be completed without ever
/// reaching for the mouse. Enter is ignored while the form is invalid rather
/// than submitting something Graph would reject.
fn buttons(ui: &mut egui::Ui, submit: &str, enabled: bool, outcome: &mut Outcome) -> bool {
    let mut submitted = false;

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            *outcome = Outcome::Cancelled;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(enabled, egui::Button::new(submit))
                .clicked()
            {
                submitted = true;
            }
            ui.label(
                RichText::new("Enter to save · Esc to cancel")
                    .small()
                    .color(theme::MUTED),
            );
        });
    });

    if enabled && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        submitted = true;
    }

    submitted
}

/// A lone Close button, for panels where Enter belongs to the list rather than
/// to a submit action.
fn close_button(ui: &mut egui::Ui, outcome: &mut Outcome) {
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    if ui.button("Close").clicked() {
        *outcome = Outcome::Cancelled;
    }
}

fn edit_user(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    fields: &mut UserFields,
    outcome: &mut Outcome,
) {
    field(ui, "Job title", &mut fields.job_title);
    field(ui, "Department", &mut fields.department);
    field(ui, "Office", &mut fields.office_location);
    field(ui, "Mobile", &mut fields.mobile_phone);
    field(ui, "Usage location", &mut fields.usage_location);

    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Usage location is a two-letter country code, and must be set before \
             licences can be assigned.",
        )
        .small()
        .color(theme::MUTED),
    );

    if buttons(ui, "Save", true, outcome) {
        let patch = UserPatch {
            job_title: Some(fields.job_title.clone()),
            department: Some(fields.department.clone()),
            office_location: Some(fields.office_location.clone()),
            mobile_phone: Some(fields.mobile_phone.clone()),
            usage_location: Some(fields.usage_location.trim().to_uppercase()),
        };
        // A patch with nothing in it would be a pointless round trip and a
        // misleading line in the audit log.
        *outcome = if patch.is_empty() {
            Outcome::Cancelled
        } else {
            Outcome::Submit(Box::new(Action::UpdateUser {
                id: id.to_string(),
                name: name.to_string(),
                patch,
            }))
        };
    }
}

/// Characters Entra accepts in the alias part of a sign-in name.
///
/// Deliberately checked here rather than left to Graph: the rejection comes
/// back as a generic `Request_BadRequest` that names neither the property nor
/// the offending character, which is a miserable thing to debug from a dialog.
fn alias_is_valid(alias: &str) -> bool {
    !alias.is_empty()
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "'.-_!#^~".contains(c))
}

fn create_user(
    ui: &mut egui::Ui,
    store: &Store,
    fields: &mut NewUserFields,
    outcome: &mut Outcome,
) {
    let previous = fields.display_name.clone();
    field(ui, "Name", &mut fields.display_name);

    // The alias tracks the name until the operator types one themselves, at
    // which point it stops moving under them.
    if !fields.alias_edited && fields.alias == derive_nickname(&previous) {
        fields.alias = derive_nickname(&fields.display_name);
    }

    // Sign-in name, as alias + domain. Split because the domain must be one the
    // tenant has verified — Graph rejects anything else, and picking from a list
    // is both faster and impossible to get wrong.
    let domains: Vec<String> = store
        .org
        .as_ref()
        .map(|org| {
            org.verified_domains
                .iter()
                .filter_map(|domain| domain.name.clone())
                .collect()
        })
        .unwrap_or_default();

    if fields.domain.is_empty() {
        fields.domain = store
            .org
            .as_ref()
            .map(|org| org.default_domain())
            .filter(|domain| domain != "—")
            .unwrap_or_default();
    }

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(130.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new("Sign-in name").color(theme::MUTED));
            },
        );
        let alias = ui.add(
            egui::TextEdit::singleline(&mut fields.alias)
                .desired_width(150.0)
                .hint_text("alias"),
        );
        if alias.changed() {
            fields.alias_edited = true;
        }
        ui.label("@");
        if domains.is_empty() {
            // No tenant details yet, so there is no list to choose from.
            ui.add(
                egui::TextEdit::singleline(&mut fields.domain)
                    .desired_width(f32::INFINITY)
                    .hint_text("contoso.com"),
            );
        } else {
            egui::ComboBox::from_id_salt("new-user-domain")
                .selected_text(if fields.domain.is_empty() {
                    "Choose a domain"
                } else {
                    fields.domain.as_str()
                })
                .show_ui(ui, |ui| {
                    for domain in &domains {
                        ui.selectable_value(&mut fields.domain, domain.clone(), domain);
                    }
                });
        }
    });
    ui.add_space(4.0);

    field(ui, "Job title", &mut fields.job_title);
    field(ui, "Department", &mut fields.department);
    field(ui, "Usage location", &mut fields.usage_location);

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(130.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new("Account").color(theme::MUTED));
            },
        );
        ui.checkbox(&mut fields.account_enabled, "Can sign in immediately");
    });

    ui.add_space(10.0);
    ui.label(RichText::new("TEMPORARY PASSWORD").small().color(theme::MUTED));
    ui.add_space(4.0);

    if fields.password.is_empty() {
        ui.label(
            RichText::new(
                "Could not generate a password: the system random source is \
                 unavailable. Use the Entra portal instead.",
            )
            .color(theme::BAD),
        );
        close_button(ui, outcome);
        return;
    }

    // Selectable and monospaced, for the same reason as a password reset: this
    // gets read aloud or pasted into a message.
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(fields.password.as_str()).monospace().size(15.0));
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(fields.password.clone());
        }
        if ui.button("Generate another").clicked() {
            fields.password = generate_password();
        }
    });

    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "The account must change this at first sign-in. Copy it now — gcm does \
             not store it, and it cannot be shown again once this dialog closes.",
        )
        .small()
        .color(theme::WARN),
    );

    let alias = fields.alias.trim();
    let domain = fields.domain.trim();
    let location = fields.usage_location.trim();
    let valid = !fields.display_name.trim().is_empty()
        && alias_is_valid(alias)
        && !domain.is_empty()
        // Two letters or nothing; a half-typed country code would be rejected
        // by Graph after the fact.
        && (location.is_empty() || location.len() == 2);

    if !valid && !fields.alias.trim().is_empty() && !alias_is_valid(alias) {
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "A sign-in name may contain only letters, digits and ' . - _ ! # ^ ~",
            )
            .small()
            .color(theme::BAD),
        );
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Usage location is a two-letter country code. Leaving it blank is fine, \
             but a licence cannot be assigned until it is set.",
        )
        .small()
        .color(theme::MUTED),
    );

    if buttons(ui, "Create", valid, outcome) {
        let optional = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        };
        *outcome = Outcome::Submit(Box::new(Action::CreateUser {
            spec: Box::new(UserSpec {
                display_name: fields.display_name.trim().to_string(),
                user_principal_name: format!("{alias}@{domain}"),
                mail_nickname: alias.to_string(),
                password: fields.password.clone(),
                account_enabled: fields.account_enabled,
                job_title: optional(&fields.job_title),
                department: optional(&fields.department),
                usage_location: optional(location).map(|code| code.to_uppercase()),
            }),
        }));
    }
}

#[expect(clippy::too_many_arguments, reason = "a form needs its whole state")]
fn automatic_replies(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    on: &mut bool,
    internal: &mut String,
    external: &mut String,
    audience: &mut String,
    same_message: &mut bool,
    outcome: &mut Outcome,
) {
    ui.checkbox(on, "Send automatic replies");
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Replies are sent until they are switched off again. Scheduling a start \
             and end is only available in Outlook.",
        )
        .small()
        .color(theme::MUTED),
    );
    ui.add_space(10.0);

    // The whole form below is inert while replies are off, rather than hidden,
    // so the dialog does not change height as the tick box is toggled.
    ui.add_enabled_ui(*on, |ui| {
        ui.label(RichText::new("Reply to colleagues").color(theme::MUTED));
        ui.add(
            egui::TextEdit::multiline(internal)
                .desired_width(f32::INFINITY)
                .desired_rows(3),
        );
        if *same_message {
            external.clone_from(internal);
        }

        ui.add_space(8.0);
        ui.checkbox(same_message, "Send the same reply outside the organisation");
        ui.add_space(8.0);

        ui.label(RichText::new("Reply to people outside").color(theme::MUTED));
        ui.add_enabled(
            !*same_message,
            egui::TextEdit::multiline(external)
                .desired_width(f32::INFINITY)
                .desired_rows(3),
        );

        ui.add_space(10.0);
        ui.label(RichText::new("Send the external reply to").color(theme::MUTED));
        ui.horizontal(|ui| {
            for (value, label) in [
                ("all", "Everyone"),
                ("contactsOnly", "Contacts only"),
                ("none", "Nobody"),
            ] {
                ui.radio_value(audience, value.to_string(), label);
            }
        });
    });

    // A reply that says nothing is worse than none: the sender learns only that
    // the mailbox is automated.
    let valid = !*on || !internal.trim().is_empty();
    if buttons(ui, "Save", valid, outcome) {
        *outcome = Outcome::Submit(Box::new(Action::SetAutomaticReplies {
            id: id.to_string(),
            name: name.to_string(),
            spec: Box::new(AutoReplySpec {
                enabled: *on,
                internal_message: internal.trim().to_string(),
                external_message: if *same_message {
                    internal.trim().to_string()
                } else {
                    external.trim().to_string()
                },
                external_audience: audience.clone(),
            }),
        }));
    }
}

fn reset_password(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    password: &mut String,
    outcome: &mut Outcome,
) {
    if password.is_empty() {
        ui.label(
            RichText::new(
                "Could not generate a password: the system random source is \
                 unavailable. Use the Entra portal instead.",
            )
            .color(theme::BAD),
        );
        close_button(ui, outcome);
        return;
    }

    ui.label("The account will be given this password and required to change it at next sign-in:");
    ui.add_space(10.0);

    // Selectable and monospaced: this has to be read aloud or copied accurately.
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(password.as_str()).monospace().size(15.0));
        });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(password.clone());
        }
        if ui.button("Generate another").clicked() {
            *password = generate_password();
        }
    });

    ui.add_space(10.0);
    ui.label(
        RichText::new(
            "Copy it now — gcm does not store it, and it cannot be shown again \
             once this dialog closes.",
        )
        .small()
        .color(theme::WARN),
    );

    if buttons(ui, "Reset password", true, outcome) {
        *outcome = Outcome::Submit(Box::new(Action::ResetPassword {
            id: id.to_string(),
            name: name.to_string(),
            password: password.clone(),
        }));
    }
}

fn create_group(
    ui: &mut egui::Ui,
    display_name: &mut String,
    mail_nickname: &mut String,
    description: &mut String,
    unified: &mut bool,
    outcome: &mut Outcome,
) {
    let previous = display_name.clone();
    field(ui, "Name", display_name);

    // Keep the alias tracking the name until the operator edits it themselves.
    if *mail_nickname == derive_nickname(&previous) || mail_nickname.is_empty() {
        *mail_nickname = derive_nickname(display_name);
    }

    field(ui, "Alias", mail_nickname);
    field(ui, "Description", description);

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(130.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new("Type").color(theme::MUTED));
            },
        );
        ui.radio_value(unified, false, "Security");
        ui.radio_value(unified, true, "Microsoft 365");
    });

    let valid = !display_name.trim().is_empty() && !mail_nickname.trim().is_empty();
    if buttons(ui, "Create", valid, outcome) {
        *outcome = Outcome::Submit(Box::new(Action::CreateGroup {
            spec: GroupSpec {
                display_name: display_name.trim().to_string(),
                mail_nickname: mail_nickname.trim().to_string(),
                description: (!description.trim().is_empty())
                    .then(|| description.trim().to_string()),
                unified: *unified,
            },
        }));
    }
}

/// Turn a display name into a mail alias Graph will accept.
///
/// Dropped punctuation must not leave a gap behind: `R&D / Special` has to come
/// out as `rd-special`, not `rd--special`, so runs of separators collapse.
fn derive_nickname(display_name: &str) -> String {
    let mut out = String::with_capacity(display_name.len());
    for c in display_name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').to_string()
}

fn edit_group(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    display_name: &mut String,
    description: &mut String,
    outcome: &mut Outcome,
) {
    field(ui, "Name", display_name);
    field(ui, "Description", description);

    let valid = !display_name.trim().is_empty();
    if buttons(ui, "Save", valid, outcome) {
        let patch = GroupPatch {
            display_name: Some(display_name.trim().to_string()),
            description: Some(description.clone()),
        };
        *outcome = if patch.is_empty() {
            Outcome::Cancelled
        } else {
            Outcome::Submit(Box::new(Action::UpdateGroup {
                id: id.to_string(),
                name: name.to_string(),
                patch,
            }))
        };
    }
}

/// A scrolling, filterable list of choices. Returns the chosen id.
///
/// Focus starts in the filter box and the arrow keys drive the list from there,
/// so choosing a licence out of ninety is type-a-few-letters-then-Enter rather
/// than tabbing through every option.
fn picker(
    ui: &mut egui::Ui,
    filter: &mut String,
    cursor: &mut usize,
    focused: &mut bool,
    rows: Vec<(String, String, String)>,
) -> Option<String> {
    let field = ui.add(
        egui::TextEdit::singleline(filter)
            .desired_width(f32::INFINITY)
            .hint_text("Filter"),
    );
    if !*focused {
        field.request_focus();
        *focused = true;
    }
    ui.add_space(8.0);

    let needle = filter.trim().to_lowercase();
    let matching: Vec<_> = rows
        .into_iter()
        .filter(|(_, primary, secondary)| {
            needle.is_empty()
                || primary.to_lowercase().contains(&needle)
                || secondary.to_lowercase().contains(&needle)
        })
        .collect();

    if matching.is_empty() {
        ui.label(RichText::new("Nothing matches.").color(theme::MUTED));
        return None;
    }

    // Keep the cursor on a real row as the filter narrows.
    if *cursor >= matching.len() {
        *cursor = matching.len() - 1;
    }

    let (up, down, enter) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
        )
    });
    if down {
        *cursor = (*cursor + 1).min(matching.len() - 1);
    }
    if up {
        *cursor = cursor.saturating_sub(1);
    }

    let mut chosen = None;
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, (id, primary, secondary)) in matching.iter().enumerate() {
                let highlighted = index == *cursor;
                let response = ui.selectable_label(highlighted, RichText::new(primary));
                if highlighted {
                    response.scroll_to_me(None);
                }
                if !secondary.is_empty() {
                    ui.label(RichText::new(secondary).small().color(theme::MUTED));
                }
                ui.add_space(2.0);
                if response.clicked() {
                    chosen = Some(id.clone());
                }
            }
        });

    if enter && let Some((id, _, _)) = matching.get(*cursor) {
        chosen = Some(id.clone());
    }

    chosen
}

#[expect(clippy::too_many_arguments, reason = "a picker needs its whole context")]
fn pick_license(
    ui: &mut egui::Ui,
    store: &Store,
    user_id: &str,
    user_name: &str,
    assign: bool,
    filter: &mut String,
    cursor: &mut usize,
    focused: &mut bool,
    outcome: &mut Outcome,
) {
    // When removing, offer only what the user actually holds.
    let held: Vec<String> = store
        .users
        .iter()
        .find(|user| user.id == user_id)
        .map(|user| {
            user.assigned_licenses
                .iter()
                .filter_map(|licence| licence.sku_id.clone())
                .collect()
        })
        .unwrap_or_default();

    let rows: Vec<(String, String, String)> = store
        .licenses
        .iter()
        .filter_map(|sku| {
            let sku_id = sku.sku_id.clone()?;
            let holds = held.contains(&sku_id);
            if assign == holds {
                return None;
            }
            let availability = if assign {
                format!("{} of {} seats free", sku.available(), sku.total_seats())
            } else {
                sku.part_number().to_string()
            };
            Some((sku_id, sku.display_name(), availability))
        })
        .collect();

    if rows.is_empty() {
        let message = if assign {
            "This user already holds every licence in the tenant."
        } else {
            "This user holds no licences."
        };
        ui.label(RichText::new(message).color(theme::MUTED));
        close_button(ui, outcome);
        return;
    }

    if let Some(sku_id) = picker(ui, filter, cursor, focused, rows) {
        let sku_name = store
            .licenses
            .iter()
            .find(|sku| sku.sku_id.as_deref() == Some(sku_id.as_str()))
            .map(|sku| sku.display_name())
            .unwrap_or_else(|| sku_id.clone());

        *outcome = Outcome::Submit(Box::new(Action::SetLicense {
            id: user_id.to_string(),
            name: user_name.to_string(),
            sku_id,
            sku_name,
            assign,
        }));
        return;
    }

    close_button(ui, outcome);
}

#[expect(clippy::too_many_arguments, reason = "a picker needs its whole context")]
fn pick_group(
    ui: &mut egui::Ui,
    store: &Store,
    member_id: &str,
    member_name: &str,
    role: MemberRole,
    add: bool,
    filter: &mut String,
    cursor: &mut usize,
    focused: &mut bool,
    outcome: &mut Outcome,
) {
    // Dynamic groups compute their own membership; offering to edit it by hand
    // would produce a change Entra silently reverts.
    let rows: Vec<(String, String, String)> = store
        .groups
        .iter()
        .filter(|group| group.membership() != "Dynamic")
        .map(|group| {
            (
                group.id.clone(),
                group.name().to_string(),
                group.kind().to_string(),
            )
        })
        .collect();

    if rows.is_empty() {
        ui.label(RichText::new("No groups accept manual membership.").color(theme::MUTED));
        close_button(ui, outcome);
        return;
    }

    if let Some(group_id) = picker(ui, filter, cursor, focused, rows) {
        let group_name = store
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| group.name().to_string())
            .unwrap_or_else(|| group_id.clone());

        *outcome = Outcome::Submit(Box::new(Action::SetMembership {
            group_id,
            group_name,
            member_id: member_id.to_string(),
            member_name: member_name.to_string(),
            role,
            add,
        }));
        return;
    }

    close_button(ui, outcome);
}

fn rename_device(
    ui: &mut egui::Ui,
    id: &str,
    name: &str,
    new_name: &mut String,
    outcome: &mut Outcome,
) {
    field(ui, "Device name", new_name);
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "The device applies the new name at its next check-in, and may need a \
             restart before it takes effect.",
        )
        .small()
        .color(theme::MUTED),
    );

    let trimmed = new_name.trim();
    let valid = !trimmed.is_empty() && trimmed != name;
    if buttons(ui, "Rename", valid, outcome) {
        *outcome = Outcome::Submit(Box::new(Action::ManagedDevice {
            id: id.to_string(),
            name: name.to_string(),
            op: DeviceOp::Rename(trimmed.to_string()),
        }));
    }
}

#[expect(clippy::too_many_arguments, reason = "a picker needs its whole context")]
fn pick_member(
    ui: &mut egui::Ui,
    store: &Store,
    group_id: &str,
    group_name: &str,
    role: MemberRole,
    add: bool,
    filter: &mut String,
    cursor: &mut usize,
    focused: &mut bool,
    outcome: &mut Outcome,
) {
    // When removing, offer only the people actually in the group — picking from
    // the whole directory would mostly produce failures.
    let existing: Vec<String> = store
        .group_members
        .get(group_id)
        .map(|(members, owners)| {
            let source = match role {
                MemberRole::Member => members,
                MemberRole::Owner => owners,
            };
            source.iter().map(|member| member.id.clone()).collect()
        })
        .unwrap_or_default();

    let rows: Vec<(String, String, String)> = if add {
        store
            .users
            .iter()
            .filter(|user| !existing.contains(&user.id))
            .map(|user| (user.id.clone(), user.name().to_string(), user.upn().to_string()))
            .collect()
    } else {
        store
            .group_members
            .get(group_id)
            .map(|(members, owners)| {
                let source = match role {
                    MemberRole::Member => members,
                    MemberRole::Owner => owners,
                };
                source
                    .iter()
                    .map(|member| {
                        (
                            member.id.clone(),
                            member.name().to_string(),
                            member.kind(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    if rows.is_empty() {
        let message = if add {
            "Everyone in the directory is already in this group."
        } else {
            "This group has no members to remove, or they have not loaded yet."
        };
        ui.label(RichText::new(message).color(theme::MUTED));
        close_button(ui, outcome);
        return;
    }

    if let Some(member_id) = picker(ui, filter, cursor, focused, rows) {
        let member_name = store
            .users
            .iter()
            .find(|user| user.id == member_id)
            .map(|user| user.name().to_string())
            .or_else(|| {
                store.group_members.get(group_id).and_then(|(members, owners)| {
                    members
                        .iter()
                        .chain(owners.iter())
                        .find(|member| member.id == member_id)
                        .map(|member| member.name().to_string())
                })
            })
            .unwrap_or_else(|| member_id.clone());

        *outcome = Outcome::Submit(Box::new(Action::SetMembership {
            group_id: group_id.to_string(),
            group_name: group_name.to_string(),
            member_id,
            member_name,
            role,
            add,
        }));
        return;
    }

    close_button(ui, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_are_long_and_varied() {
        let a = generate_password();
        let b = generate_password();
        assert_eq!(a.chars().count(), PASSWORD_LENGTH);
        assert_ne!(a, b, "two generated passwords must not collide");
    }

    #[test]
    fn generated_passwords_avoid_ambiguous_glyphs() {
        // A temporary password often gets read aloud; 0/O and 1/l/I are where
        // that goes wrong.
        for forbidden in ['0', 'O', '1', 'l', 'I'] {
            assert!(
                !PASSWORD_ALPHABET.contains(&(forbidden as u8)),
                "{forbidden} should not be in the alphabet"
            );
        }
    }

    #[test]
    fn nicknames_are_derived_safely() {
        assert_eq!(derive_nickname("Finance Team"), "finance-team");
        assert_eq!(derive_nickname("R&D / Special!"), "rd-special");
        assert_eq!(derive_nickname("  Leading and trailing  "), "leading-and-trailing");
    }

    #[test]
    fn nicknames_collapse_runs_of_separators() {
        // Dropped punctuation must not leave a doubled dash behind.
        assert_eq!(derive_nickname("R & D"), "r-d");
        assert_eq!(derive_nickname("Sales -- EMEA"), "sales-emea");
        assert_eq!(derive_nickname("a___b"), "a-b");
    }

    #[test]
    fn an_empty_patch_is_recognised() {
        // Submitting one would be a wasted round trip and a misleading audit line.
        assert!(UserPatch::default().is_empty());
        assert!(GroupPatch::default().is_empty());
        assert!(
            !GroupPatch {
                display_name: Some("x".into()),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn sign_in_names_accept_what_entra_accepts() {
        assert!(alias_is_valid("nadia.ferrero"));
        assert!(alias_is_valid("o'brien"));
        assert!(alias_is_valid("a_b-c!d#e^f~g"));
        assert!(alias_is_valid("svc01"));
    }

    #[test]
    fn sign_in_names_reject_what_entra_rejects() {
        // Checked here so the operator is told which character is wrong,
        // rather than getting Graph's generic Request_BadRequest back.
        assert!(!alias_is_valid(""));
        assert!(!alias_is_valid("has space"));
        assert!(!alias_is_valid("has@at"));
        assert!(!alias_is_valid("accented-é"));
        assert!(!alias_is_valid("comma,name"));
    }

    #[test]
    fn a_new_user_form_starts_ready_to_submit() {
        let Form::CreateUser { fields } = Form::create_user() else {
            panic!("create_user must produce a CreateUser form");
        };
        // A generated password up front is the point: the operator should never
        // have to think of one, and an empty field would invite a weak choice.
        assert_eq!(fields.password.chars().count(), PASSWORD_LENGTH);
        assert!(fields.account_enabled);
        assert!(!fields.alias_edited);
    }

    #[test]
    fn automatic_replies_open_showing_what_is_already_set() {
        // Opening the form and saving without typing must change nothing, so
        // every field has to be seeded from the mailbox as it stands.
        let current = crate::graph::models::AutomaticReplies {
            status: Some("alwaysEnabled".into()),
            external_audience: Some("contactsOnly".into()),
            internal_reply_message: Some("<p>On leave.</p>".into()),
            external_reply_message: Some("<p>Away.</p>".into()),
            ..Default::default()
        };
        let form = Form::automatic_replies("id".into(), "Elena Marsh".into(), Some(current));
        let Form::AutomaticReplies {
            on,
            internal,
            external,
            audience,
            same_message,
            ..
        } = form
        else {
            panic!("must produce an AutomaticReplies form");
        };
        assert!(on);
        assert_eq!(internal, "On leave.");
        assert_eq!(external, "Away.");
        assert_eq!(audience, "contactsOnly");
        // The two messages differ, so they must not be linked together.
        assert!(!same_message);
    }

    #[test]
    fn matching_replies_open_with_the_messages_linked() {
        let current = crate::graph::models::AutomaticReplies {
            status: Some("alwaysEnabled".into()),
            internal_reply_message: Some("<p>Back Monday.</p>".into()),
            external_reply_message: Some("<p>Back Monday.</p>".into()),
            ..Default::default()
        };
        let form = Form::automatic_replies("id".into(), "Elena".into(), Some(current));
        let Form::AutomaticReplies { same_message, .. } = form else {
            panic!("must produce an AutomaticReplies form");
        };
        assert!(same_message);
    }

    #[test]
    fn a_mailbox_with_no_replies_set_opens_switched_off() {
        let form = Form::automatic_replies("id".into(), "Ben".into(), None);
        let Form::AutomaticReplies {
            on, same_message, ..
        } = form
        else {
            panic!("must produce an AutomaticReplies form");
        };
        assert!(!on);
        // Two empty messages are trivially the same, so they start linked.
        assert!(same_message);
    }

    #[test]
    fn nicknames_cope_with_unusable_names() {
        // Non-ASCII names leave nothing to derive from; the form requires the
        // operator to supply an alias rather than submitting an empty one.
        assert_eq!(derive_nickname("!!!"), "");
        assert_eq!(derive_nickname(""), "");
    }
}
