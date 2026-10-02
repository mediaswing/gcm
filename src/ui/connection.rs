//! The Connection pane: which tenant, which app registration, and the secret
//! to sign in with. App-only sign-in — no browser, no device code.
//!
//! The one exception is granting the registration its permissions, which
//! only an administrator can do: see [`crate::graph::consent`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{RichText, Ui};

use crate::app::{App, Tab};
use crate::graph::consent::{AdminSignIn, Report};
use crate::graph::{Credentials, Graph, REQUIRED_ROLES, Session};
use crate::secrets;
use crate::task::{Task, take_finished};
use crate::ui;

pub struct State {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub remember_secret: bool,
    pub error: Option<String>,
    signing_in: Option<Task<(Graph, Session)>>,
    /// The admin sign-in that grants permissions, while it is waiting for
    /// the browser or granting.
    granting: Option<Task<Report>>,
    /// The page the admin sign-in opened, in case the browser did not.
    grant_url: Option<String>,
    grant_cancel: Option<Arc<AtomicBool>>,
    grant_error: Option<String>,
}

impl State {
    pub fn from_config(config: &crate::config::Config) -> Self {
        let stored = secrets::load();
        // A credentials file written by hand, with no settings yet, names the
        // tenant and client itself.
        let (tenant_id, client_id) = if config.tenant_id.is_empty() && config.client_id.is_empty() {
            (stored.tenant_id.clone(), stored.client_id.clone())
        } else {
            (config.tenant_id.clone(), config.client_id.clone())
        };
        let client_secret = stored
            .client_secret_for(&tenant_id, &client_id)
            .unwrap_or_default()
            .to_owned();
        Self {
            remember_secret: !client_secret.is_empty(),
            tenant_id,
            client_id,
            client_secret,
            error: None,
            signing_in: None,
            granting: None,
            grant_url: None,
            grant_cancel: None,
            grant_error: None,
        }
    }

    pub fn activity(&self) -> Option<String> {
        self.signing_in
            .as_ref()
            .map(|t| t.label.clone())
            .or_else(|| self.granting.as_ref().map(|t| t.label.clone()))
    }
}

pub fn sign_in_at_start(app: &mut App, ctx: &egui::Context) {
    let s = &app.connection;
    if !s.tenant_id.trim().is_empty() && !s.client_id.trim().is_empty() && !s.client_secret.is_empty() {
        start(app, ctx);
    }
}

fn start(app: &mut App, ctx: &egui::Context) {
    if app.connection.signing_in.is_some() || app.users.importing() {
        return;
    }
    let graph = Graph::new(Credentials {
        tenant_id: app.connection.tenant_id.trim().to_owned(),
        client_id: app.connection.client_id.trim().to_owned(),
        client_secret: app.connection.client_secret.clone(),
    });
    app.connection.error = None;
    log::info!(
        "signing in to tenant {}, client {}",
        app.connection.tenant_id.trim(),
        app.connection.client_id.trim()
    );
    app.connection.signing_in = Some(Task::spawn(ctx, "Signing in…", move || {
        let session = graph.sign_in()?;
        Ok((graph, session))
    }));
}

/// Open the browser for an administrator to sign in and grant the
/// registration its permissions.
fn start_grant(app: &mut App, ctx: &egui::Context) {
    if app.connection.granting.is_some() {
        return;
    }
    let sign_in = match AdminSignIn::prepare(&app.connection.tenant_id, &app.connection.client_id) {
        Ok(sign_in) => sign_in,
        Err(err) => {
            app.connection.grant_error = Some(err.clone());
            app.report_error(err);
            return;
        }
    };
    let url = sign_in.url().to_owned();
    log::info!("opening the browser for an admin sign-in to grant permissions");
    ctx.open_url(egui::OpenUrl::new_tab(&url));
    app.connection.grant_url = Some(url);
    app.connection.grant_cancel = Some(sign_in.cancel_flag());
    app.connection.grant_error = None;
    app.connection.granting = Some(Task::spawn(ctx, "Waiting for the administrator to sign in…", move || {
        sign_in.finish()
    }));
}

fn finish_grant(app: &mut App, ctx: &egui::Context, result: Result<Report, String>) {
    app.connection.grant_url = None;
    app.connection.grant_cancel = None;
    match result {
        Ok(report) => {
            let mut message = match report.granted.len() {
                0 => "Every permission was already granted.".to_owned(),
                1 => "Granted 1 permission.".to_owned(),
                n => format!("Granted {n} permissions."),
            };
            if !report.warnings.is_empty() {
                message.push(' ');
                message.push_str(&report.warnings.join(" "));
                app.connection.grant_error = Some(report.warnings.join("\n"));
            }
            app.report_ok(message);
            // A token carries the roles it was issued with, so sign in again
            // to pick up the new ones.
            if !app.connection.client_secret.is_empty() {
                start(app, ctx);
            }
        }
        Err(err) => {
            app.connection.grant_error = Some(err.clone());
            app.report_error(err);
        }
    }
}

pub fn poll(app: &mut App, ctx: &egui::Context) {
    if let Some(result) = take_finished(&mut app.connection.granting) {
        finish_grant(app, ctx, result);
    }
    let Some(result) = take_finished(&mut app.connection.signing_in) else {
        return;
    };
    match result {
        Ok((graph, session)) => {
            // What the sign-in used, not what is in the boxes now: they can
            // be edited while it is under way, and a secret must only ever
            // be remembered beside the tenant and client it worked for.
            let used = graph.credentials();
            app.config.tenant_id = used.tenant_id.clone();
            app.config.client_id = used.client_id.clone();

            // The credentials file holds one app registration's secret: the
            // one just used, or none if the user would rather not keep it.
            let mut stored = secrets::load();
            if app.connection.remember_secret {
                stored.tenant_id = used.tenant_id.clone();
                stored.client_id = used.client_id.clone();
                stored.client_secret = used.client_secret.clone();
            } else {
                stored.tenant_id.clear();
                stored.client_id.clear();
                stored.client_secret.clear();
            }
            let mut warning = secrets::save(&stored).err();
            if let Err(err) = app.config.save() {
                warning = Some(format!("Signed in, but the settings could not be saved: {err}"));
            }

            let name = session
                .organisation
                .clone()
                .unwrap_or_else(|| app.config.tenant_id.clone());
            app.forget_directory();
            app.graph = Some(graph);
            app.session = Some(session);
            match warning {
                Some(w) => app.report_error(w),
                None => app.report_ok(format!("Signed in to {name}.")),
            }
            if app.tab == Tab::Connection {
                app.tab = Tab::Users;
            }
        }
        Err(err) => {
            app.connection.error = Some(err.clone());
            app.report_error(err);
            app.tab = Tab::Connection;
        }
    }
}

fn sign_out(app: &mut App) {
    app.graph = None;
    app.session = None;
    app.forget_directory();
    app.report_ok("Signed out.");
}

pub fn show(app: &mut App, ui: &mut Ui) {
    ui::pane_header(
        ui,
        "Connection",
        "Sign in as an app registration in your Entra ID tenant, using its tenant ID, client ID and a client secret.",
    );
    let ctx = ui.ctx().clone();
    // Signing out or in again clears the Users pane, and with it the
    // results of an import still running, generated passwords and all.
    let importing = app.users.importing();
    let busy = app.connection.signing_in.is_some() || importing;

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(640.0);
        let s = &mut app.connection;
        ui::labelled_field(
            ui,
            "Directory (tenant) ID",
            &mut s.tenant_id,
            "00000000-0000-0000-0000-000000000000 or contoso.onmicrosoft.com",
        );
        ui::labelled_field(
            ui,
            "Application (client) ID",
            &mut s.client_id,
            "00000000-0000-0000-0000-000000000000",
        );
        ui::labelled_password(ui, "Client secret (the secret's Value, not its ID)", &mut s.client_secret);
        ui.checkbox(
            &mut s.remember_secret,
            format!(
                "Remember the secret in {} and sign in automatically",
                crate::config::tilde(&secrets::path())
            ),
        );
        ui.add_space(10.0);

        let signed_in = app.graph.is_some();
        let label = if app.connection.signing_in.is_some() {
            "Signing in…"
        } else if signed_in {
            "Sign in again"
        } else {
            "Sign in"
        };
        if ui
            .add_enabled_ui(!busy, |ui| ui::wide_button(ui, label))
            .inner
            .clicked()
        {
            start(app, &ctx);
        }
        if signed_in {
            ui.add_space(6.0);
            if ui.add_enabled(!busy, egui::Button::new("Sign out")).clicked() {
                sign_out(app);
            }
        }
        if importing {
            ui.add_space(6.0);
            ui.label(
                RichText::new("A CSV import is running. Signing in or out waits until it has finished.")
                    .size(13.0)
                    .color(ui::warn_colour(ui)),
            );
        }
        if let Some(error) = &app.connection.error {
            ui.add_space(6.0);
            ui::error_text(ui, error);
        }

        grant_section(app, ui, &ctx);

        if let Some(session) = &app.session {
            ui.add_space(18.0);
            ui.heading("Permissions");
            ui.label(
                RichText::new(
                    "The application permissions this sign-in carries. Anything missing can be granted with Grant permissions above, or in the portal under API permissions.",
                )
                .size(13.0)
                .weak(),
            );
            ui.add_space(6.0);
            egui::Grid::new("roles")
                .num_columns(3)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for (role, purpose) in REQUIRED_ROLES {
                        let granted = session.roles.iter().any(|r| r == role);
                        let (text, colour) = if granted {
                            ("Granted", ui::good_colour(ui))
                        } else {
                            ("Missing", ui::warn_colour(ui))
                        };
                        ui.label(RichText::new(text).color(colour));
                        ui.label(*role);
                        ui.label(RichText::new(*purpose).size(13.0).weak());
                        ui.end_row();
                    }
                });
            let extra: Vec<&String> = session
                .roles
                .iter()
                .filter(|r| !REQUIRED_ROLES.iter().any(|(name, _)| name == r))
                .collect();
            if !extra.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Also granted: {}",
                        extra.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                    ))
                    .size(13.0)
                    .weak(),
                );
            }
        } else {
            ui.add_space(18.0);
            ui.heading("Setting up the app registration");
            ui.add_space(4.0);
            for line in [
                "1. In the Entra admin centre, go to App registrations > New registration. No redirect URI is needed.",
                "2. Copy the Application (client) ID and Directory (tenant) ID from its Overview page.",
                "3. Under Certificates & secrets, add a client secret and copy its Value.",
                "4. Press Grant permissions above and sign in as an administrator, or add the permissions listed in the README under API permissions and Grant admin consent.",
            ] {
                ui.label(RichText::new(line).size(13.0));
            }
        }

        ui.add_space(14.0);
        ui.label(
            RichText::new(format!(
                "Settings are kept in {}. Remembered secrets are kept apart from them, in {}, which only you can read.",
                crate::config::tilde(&crate::config::config_path()),
                crate::config::tilde(&secrets::path())
            ))
            .size(12.0)
            .weak(),
        );
    });
}

/// "Grant permissions…": offered once there is a tenant and client to grant
/// them to, while any are missing or before the first sign-in has said.
fn grant_section(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    let s = &app.connection;
    let ids_entered = !s.tenant_id.trim().is_empty() && !s.client_id.trim().is_empty();
    let missing = app.session.as_ref().map(|session| {
        REQUIRED_ROLES
            .iter()
            .filter(|(role, _)| !session.roles.iter().any(|r| r == role))
            .count()
    });
    let granting = s.granting.is_some();
    if !ids_entered || (missing == Some(0) && !granting && s.grant_error.is_none()) {
        return;
    }

    ui.add_space(18.0);
    ui.heading("Grant permissions");
    let lead = match missing {
        Some(n) => format!("This app registration is missing {n} of the permissions the app uses."),
        None => "Give this app registration the permissions the app uses.".to_owned(),
    };
    ui.label(
        RichText::new(format!(
            "{lead} An administrator signs in once in the browser, and the app adds the permissions to the registration and grants admin consent. The administrator's sign-in is used for this alone and is not kept."
        ))
        .size(13.0),
    );
    ui.label(
        RichText::new(
            "Needs a Global Administrator or Privileged Role Administrator. The sign-in is through Microsoft Graph Command Line Tools, Microsoft's own app for working with Graph, which may ask for consent the first time.",
        )
        .size(12.0)
        .weak(),
    );
    ui.add_space(6.0);

    if granting {
        ui::busy(ui, "Waiting for the sign-in in your browser…");
        ui.horizontal(|ui| {
            if let Some(url) = app.connection.grant_url.clone()
                && ui.link("Open the sign-in page again").clicked()
            {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            if ui.button("Cancel").clicked()
                && let Some(cancel) = &app.connection.grant_cancel
            {
                cancel.store(true, Ordering::Relaxed);
            }
        });
    } else if ui
        .add_enabled_ui(app.connection.signing_in.is_none(), |ui| {
            ui::wide_button(ui, "Grant permissions…")
        })
        .inner
        .clicked()
    {
        start_grant(app, ctx);
    }
    if let Some(err) = &app.connection.grant_error {
        ui.add_space(4.0);
        ui::error_text(ui, err);
    }
}
