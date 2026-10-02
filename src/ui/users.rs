//! The Users pane: every user in the tenant, searchable, with a details panel
//! for the one selected, and bulk import and export through CSV files.

use std::sync::{Arc, Mutex};

use egui::{RichText, Ui};
use egui_extras::Column;

use crate::app::{App, Tab};
use crate::csvio::{self, ImportResult, ImportRow};
use crate::graph::models::{DirectoryObject, User, short_time};
use crate::graph::users::{NewUser, UserEdit, generate_password};
use crate::graph::{Graph, Result};
use crate::task::{Task, take_finished};
use crate::ui;

/// What a finished action changed, so the list can be patched in place rather
/// than read again from the start.
enum Change {
    Upsert(Box<User>),
    Removed(String),
}

enum Form {
    Create(NewUser),
    Edit { id: String, edit: UserEdit },
}

struct ResetForm {
    id: String,
    name: String,
    password: String,
    force_change: bool,
}

struct ImportPreview {
    file: String,
    rows: Vec<ImportRow>,
}

#[derive(Default)]
pub struct State {
    users: Vec<User>,
    loaded: bool,
    load_error: Option<String>,
    load: Option<Task<Vec<User>>>,
    query: String,
    selected: Option<String>,
    memberships: Option<(String, Result<Vec<DirectoryObject>>)>,
    memberships_load: Option<Task<(String, Vec<DirectoryObject>)>>,
    action: Option<Task<(String, Change)>>,
    form: Option<(Form, Option<String>)>,
    reset: Option<ResetForm>,
    confirm_delete: Option<(String, String)>,
    preview: Option<ImportPreview>,
    import: Option<Task<Vec<ImportResult>>>,
    import_progress: Arc<Mutex<String>>,
    results: Option<Vec<ImportResult>>,
}

impl State {
    pub fn activity(&self) -> Option<String> {
        if self.import.is_some() {
            return self.import_progress.lock().ok().map(|p| p.clone());
        }
        self.load
            .as_ref()
            .map(|t| t.label.clone())
            .or_else(|| self.action.as_ref().map(|t| t.label.clone()))
    }

    /// The users that match the search, as indices into `users`.
    fn shown(&self) -> Vec<usize> {
        let terms = ui::search_terms(&self.query);
        self.users
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                ui::matches_search(
                    &terms,
                    &[
                        u.name(),
                        u.upn(),
                        u.mail.as_deref().unwrap_or(""),
                        u.department.as_deref().unwrap_or(""),
                        u.job_title.as_deref().unwrap_or(""),
                    ],
                )
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_user(&self) -> Option<&User> {
        let id = self.selected.as_deref()?;
        self.users.iter().find(|u| u.id == id)
    }
}

fn graph(app: &App) -> Option<Graph> {
    app.graph.clone()
}

/// Run a change against Graph in the background.
fn run(
    app: &mut App,
    ctx: &egui::Context,
    label: &str,
    work: impl FnOnce(&Graph) -> Result<(String, Change)> + Send + 'static,
) {
    let Some(graph) = graph(app) else { return };
    if app.users.action.is_some() {
        return;
    }
    app.users.action = Some(Task::spawn(ctx, label, move || work(&graph)));
}

fn reload(app: &mut App, ctx: &egui::Context) {
    let Some(graph) = graph(app) else { return };
    if app.users.load.is_some() {
        return;
    }
    app.users.load = Some(Task::spawn(ctx, "Loading users…", move || graph.list_users()));
}

pub fn poll(app: &mut App) {
    if let Some(result) = take_finished(&mut app.users.load) {
        app.users.loaded = true;
        match result {
            Ok(users) => {
                app.users.users = users;
                app.users.load_error = None;
            }
            Err(err) => {
                app.users.load_error = Some(err.clone());
                app.report_error(format!("Could not load users: {err}"));
            }
        }
    }

    if let Some(result) = take_finished(&mut app.users.memberships_load) {
        match result {
            Ok((id, groups)) => app.users.memberships = Some((id, Ok(groups))),
            Err(err) => {
                if let Some(id) = app.users.selected.clone() {
                    app.users.memberships = Some((id, Err(err)));
                }
            }
        }
    }

    if let Some(result) = take_finished(&mut app.users.action) {
        match result {
            Ok((message, change)) => {
                let users = &mut app.users.users;
                match change {
                    Change::Upsert(user) => {
                        let user = *user;
                        match users.iter_mut().find(|u| u.id == user.id) {
                            Some(existing) => *existing = user,
                            None => {
                                app.users.selected = Some(user.id.clone());
                                users.push(user);
                                users.sort_by_key(|u| u.name().to_lowercase());
                            }
                        }
                    }
                    Change::Removed(id) => {
                        users.retain(|u| u.id != id);
                        if app.users.selected.as_deref() == Some(id.as_str()) {
                            app.users.selected = None;
                        }
                    }
                }
                app.report_ok(message);
            }
            Err(err) => app.report_error(err),
        }
    }

    if let Some(result) = take_finished(&mut app.users.import) {
        match result {
            Ok(results) => {
                let created = results.iter().filter(|r| r.outcome.is_ok()).count();
                let failed = results.len() - created;
                let message = format!("Import finished: {created} created, {failed} failed.");
                if failed == 0 {
                    app.report_ok(message);
                } else {
                    app.report_error(message);
                }
                app.users.results = Some(results);
                // Read the list again, so the new users are in it.
                app.users.loaded = false;
            }
            Err(err) => app.report_error(err),
        }
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if app.graph.is_none() {
        ui::pane_header(ui, "Users", "");
        if ui::not_connected(ui) {
            app.tab = Tab::Connection;
        }
        return;
    }
    if !app.users.loaded && app.users.load.is_none() {
        reload(app, &ctx);
    }

    let shown = app.users.shown();
    let subtitle = if app.users.load.is_some() && !app.users.loaded {
        "Loading…".to_owned()
    } else if app.users.query.trim().is_empty() {
        format!("{} users", app.users.users.len())
    } else {
        format!("{} of {} users match", shown.len(), app.users.users.len())
    };
    ui::pane_header(ui, "Users", &subtitle);

    toolbar(app, ui, &ctx, &shown);
    ui.add_space(4.0);
    ui::search_box(ui, &mut app.users.query, "Search by name, sign-in name, mail, department or job title");
    ui.add_space(6.0);

    if let Some(err) = &app.users.load_error {
        ui::error_text(ui, err);
    }
    import_results(app, ui);

    if app.users.selected_user().is_some() {
        egui::Panel::right("user-details")
            .resizable(true)
            .default_size(320.0)
            .min_size(260.0)
            .show(ui, |ui| details(app, ui, &ctx));
    }

    let selected_index = app
        .users
        .selected
        .as_deref()
        .and_then(|id| shown.iter().position(|&i| app.users.users[i].id == id));
    let users = &app.users.users;
    let clicked = ui::select_table(
        ui,
        "users",
        &[
            ("Display name", Column::initial(200.0).at_least(80.0)),
            ("User principal name", Column::initial(260.0).at_least(80.0)),
            ("Department", Column::initial(130.0).at_least(60.0)),
            ("Job title", Column::initial(130.0).at_least(60.0)),
            ("Status", Column::remainder().at_least(70.0)),
        ],
        shown.len(),
        selected_index,
        |row, column, ui| {
            let u = &users[shown[row]];
            match column {
                0 => ui::cell_text(ui, u.name()),
                1 => ui::cell_text(ui, u.upn()),
                2 => ui::cell_text(ui, u.department.as_deref().unwrap_or("")),
                3 => ui::cell_text(ui, u.job_title.as_deref().unwrap_or("")),
                _ => {
                    if u.account_enabled == Some(false) {
                        ui.label(RichText::new("Disabled").color(ui::warn_colour(ui)));
                    } else {
                        ui::cell_text(ui, "Enabled");
                    }
                }
            }
        },
    );
    if let Some(row) = clicked {
        let id = app.users.users[shown[row]].id.clone();
        app.users.selected = if app.users.selected.as_deref() == Some(&id) {
            None
        } else {
            Some(id)
        };
    }
}

fn toolbar(app: &mut App, ui: &mut Ui, ctx: &egui::Context, shown: &[usize]) {
    let idle = app.users.load.is_none() && app.users.import.is_none();
    ui.horizontal_wrapped(|ui| {
        if ui::tool_button(ui, idle, "Refresh").clicked() {
            reload(app, ctx);
        }
        if ui::tool_button(ui, app.users.action.is_none(), "+ New user").clicked() {
            app.users.form = Some((
                Form::Create(NewUser {
                    password: generate_password(),
                    account_enabled: true,
                    force_change_password: true,
                    ..Default::default()
                }),
                None,
            ));
        }
        ui.separator();
        if ui::tool_button(ui, idle, "Import CSV…").clicked() {
            pick_import(app);
        }
        let export_label = if app.users.query.trim().is_empty() {
            "Export CSV…".to_owned()
        } else {
            format!("Export {} shown…", shown.len())
        };
        if ui::tool_button(ui, !shown.is_empty(), &export_label).clicked() {
            export_csv(app, shown);
        }
        if ui::tool_button(ui, true, "Save CSV template…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("gcm-users-template.csv")
                .add_filter("CSV", &["csv"])
                .save_file()
        {
            match csvio::write_template(&path) {
                Ok(()) => app.report_ok(format!("Template saved to {}.", crate::config::tilde(&path))),
                Err(err) => app.report_error(format!("Could not save the template: {err}")),
            }
        }
    });
}

fn pick_import(app: &mut App) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("CSV", &["csv"])
        .pick_file()
    else {
        return;
    };
    match csvio::read_import(&path) {
        Ok(rows) if rows.is_empty() => app.report_error("That file has no users in it."),
        Ok(rows) => {
            app.users.preview = Some(ImportPreview {
                file: crate::config::tilde(&path),
                rows,
            })
        }
        Err(err) => app.report_error(err),
    }
}

fn export_csv(app: &mut App, shown: &[usize]) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("users.csv")
        .add_filter("CSV", &["csv"])
        .save_file()
    else {
        return;
    };
    let users: Vec<&User> = shown.iter().map(|&i| &app.users.users[i]).collect();
    let count = users.len();
    match csvio::write_users(&path, &users) {
        Ok(()) => app.report_ok(format!(
            "Exported {count} users to {}.",
            crate::config::tilde(&path)
        )),
        Err(err) => app.report_error(format!("Could not export: {err}")),
    }
}

/// What the last import did, until it is dismissed.
fn import_results(app: &mut App, ui: &mut Ui) {
    let Some(results) = &app.users.results else {
        return;
    };
    let created = results.iter().filter(|r| r.outcome.is_ok()).count();
    let failures: Vec<&ImportResult> = results.iter().filter(|r| r.outcome.is_err()).collect();
    let passwords = results.iter().any(|r| r.password.is_some() && r.outcome.is_ok());

    let mut dismiss = false;
    let mut save = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!(
                "Last import: {created} created, {} failed.",
                failures.len()
            )));
            if ui.button("Save results…").clicked() {
                save = true;
            }
            if ui.button("Dismiss").clicked() {
                dismiss = true;
            }
        });
        if passwords {
            ui.label(
                RichText::new(
                    "Some passwords were generated. They are only in the results file — save it before dismissing.",
                )
                .size(13.0)
                .color(ui::warn_colour(ui)),
            );
        }
        for f in failures.iter().take(8) {
            if let Err(err) = &f.outcome {
                ui::error_text(
                    ui,
                    &format!("Line {} ({}): {err}", f.line, f.user_principal_name),
                );
            }
        }
        if failures.len() > 8 {
            ui.label(format!("…and {} more in the results file.", failures.len() - 8));
        }
    });
    ui.add_space(6.0);

    if save
        && let Some(path) = rfd::FileDialog::new()
            .set_file_name("gcm-import-results.csv")
            .add_filter("CSV", &["csv"])
            .save_file()
    {
        let results = app.users.results.as_deref().unwrap_or_default();
        match csvio::write_results(&path, results) {
            Ok(()) => app.report_ok(format!("Results saved to {}.", crate::config::tilde(&path))),
            Err(err) => app.report_error(format!("Could not save the results: {err}")),
        }
    }
    if dismiss {
        app.users.results = None;
    }
}

fn details(app: &mut App, ui: &mut Ui, ctx: &egui::Context) {
    let Some(user) = app.users.selected_user().cloned() else {
        return;
    };

    // Group memberships for whoever is selected, read once per selection.
    let have = app.users.memberships.as_ref().map(|(id, _)| id.as_str());
    if have != Some(user.id.as_str())
        && app.users.memberships_load.is_none()
        && let Some(graph) = graph(app)
    {
        let id = user.id.clone();
        app.users.memberships_load = Some(Task::spawn(ctx, "Loading memberships…", move || {
            Ok((id.clone(), graph.user_memberships(&id)?))
        }));
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(8.0);
        ui.heading(user.name());
        ui.add_space(6.0);

        let idle = app.users.action.is_none();
        ui.horizontal_wrapped(|ui| {
            if ui::tool_button(ui, idle, "Edit").clicked() {
                app.users.form = Some((
                    Form::Edit {
                        id: user.id.clone(),
                        edit: UserEdit::from_user(&user),
                    },
                    None,
                ));
            }
            let enabled = user.account_enabled != Some(false);
            if ui::tool_button(ui, idle, if enabled { "Disable" } else { "Enable" }).clicked() {
                let id = user.id.clone();
                let name = user.name().to_owned();
                let mut updated = user.clone();
                run(app, ctx, "Updating user…", move |g| {
                    g.set_user_enabled(&id, !enabled)?;
                    updated.account_enabled = Some(!enabled);
                    let verb = if enabled { "disabled" } else { "enabled" };
                    Ok((format!("{name} {verb}."), Change::Upsert(Box::new(updated))))
                });
            }
            if ui::tool_button(ui, idle, "Reset password").clicked() {
                app.users.reset = Some(ResetForm {
                    id: user.id.clone(),
                    name: user.name().to_owned(),
                    password: generate_password(),
                    force_change: true,
                });
            }
            if ui::tool_button(ui, idle, "Delete").clicked() {
                app.users.confirm_delete = Some((user.id.clone(), user.name().to_owned()));
            }
        });
        if user.on_premises_sync_enabled == Some(true) {
            ui.label(
                RichText::new("Synchronised from on-premises Active Directory: most changes have to be made there.")
                    .size(12.0)
                    .color(ui::warn_colour(ui)),
            );
        }
        ui.add_space(10.0);

        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        ui::property(ui, "User principal name", user.upn());
        ui::property(ui, "Mail", &s(&user.mail));
        ui::property(ui, "Given name", &s(&user.given_name));
        ui::property(ui, "Surname", &s(&user.surname));
        ui::property(ui, "Job title", &s(&user.job_title));
        ui::property(ui, "Department", &s(&user.department));
        ui::property(ui, "Office", &s(&user.office_location));
        ui::property(ui, "Mobile phone", &s(&user.mobile_phone));
        ui::property(ui, "Usage location", &s(&user.usage_location));
        ui::property(ui, "User type", &s(&user.user_type));
        ui::property(ui, "Created (UTC)", &short_time(user.created_date_time.as_deref()));
        ui::property(ui, "Object ID", &user.id);

        ui.add_space(8.0);
        ui.label(RichText::new("Member of").strong());
        match &app.users.memberships {
            Some((id, Ok(groups))) if *id == user.id => {
                if groups.is_empty() {
                    ui.label(RichText::new("No groups.").weak());
                }
                for g in groups {
                    ui.label(format!("{}  ·  {}", g.name(), g.kind()));
                }
            }
            Some((id, Err(err))) if *id == user.id => ui::error_text(ui, err),
            _ => ui::busy(ui, "Loading…"),
        }
    });
}

pub fn modals(app: &mut App, ctx: &egui::Context) {
    form_modal(app, ctx);
    reset_modal(app, ctx);
    delete_modal(app, ctx);
    preview_modal(app, ctx);
}

fn form_modal(app: &mut App, ctx: &egui::Context) {
    let Some((form, error)) = app.users.form.as_mut() else {
        return;
    };
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::new("user-form")).show(ctx, |ui| {
        ui.set_width(460.0);
        egui::ScrollArea::vertical()
            .max_height(ctx.content_rect().height() - 160.0)
            .show(ui, |ui| match form {
                Form::Create(u) => {
                    ui.heading("New user");
                    ui.add_space(6.0);
                    let first = ui::labelled_field(ui, "Display name", &mut u.display_name, "Jo Bloggs");
                    ui::focus_on_open(ui, egui::Id::new("user-form"), &first);
                    ui::labelled_field(ui, "User principal name", &mut u.user_principal_name, "jo.bloggs@contoso.com");
                    ui::labelled_field(ui, "Mail nickname (optional)", &mut u.mail_nickname, "from the user principal name");
                    ui::labelled_field(ui, "Initial password", &mut u.password, "");
                    ui.checkbox(&mut u.force_change_password, "Must change password at next sign-in");
                    ui.checkbox(&mut u.account_enabled, "Account enabled");
                    ui.add_space(6.0);
                    ui::labelled_field(ui, "Given name", &mut u.given_name, "");
                    ui::labelled_field(ui, "Surname", &mut u.surname, "");
                    ui::labelled_field(ui, "Job title", &mut u.job_title, "");
                    ui::labelled_field(ui, "Department", &mut u.department, "");
                    ui::labelled_field(ui, "Office", &mut u.office_location, "");
                    ui::labelled_field(ui, "Mobile phone", &mut u.mobile_phone, "");
                    ui::labelled_field(ui, "Usage location", &mut u.usage_location, "GB");
                }
                Form::Edit { edit, .. } => {
                    ui.heading("Edit user");
                    ui.add_space(6.0);
                    let first = ui::labelled_field(ui, "Display name", &mut edit.display_name, "");
                    ui::focus_on_open(ui, egui::Id::new("user-form"), &first);
                    ui::labelled_field(ui, "Given name", &mut edit.given_name, "");
                    ui::labelled_field(ui, "Surname", &mut edit.surname, "");
                    ui::labelled_field(ui, "Job title", &mut edit.job_title, "");
                    ui::labelled_field(ui, "Department", &mut edit.department, "");
                    ui::labelled_field(ui, "Office", &mut edit.office_location, "");
                    ui::labelled_field(ui, "Mobile phone", &mut edit.mobile_phone, "");
                    ui::labelled_field(ui, "Usage location", &mut edit.usage_location, "GB");
                }
            });
        if let Some(err) = error.as_ref() {
            ui::error_text(ui, err);
        }
        let label = match form {
            Form::Create(_) => "Create",
            Form::Edit { .. } => "Save",
        };
        answer = ui::form_buttons(ui, label, true);
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }

    match answer {
        Some(false) => app.users.form = None,
        Some(true) => {
            let Some((form, _)) = app.users.form.take() else { return };
            // Check here what can be checked here, and keep the form open.
            let check = match &form {
                Form::Create(u) => u.validate(),
                Form::Edit { .. } => Ok(()),
            };
            if let Err(err) = check {
                app.users.form = Some((form, Some(err)));
                return;
            }
            match form {
                Form::Create(new) => run(app, ctx, "Creating user…", move |g| {
                    let user = g.create_user(&new)?;
                    Ok((format!("Created {}.", user.upn()), Change::Upsert(Box::new(user))))
                }),
                Form::Edit { id, edit } => {
                    let mut updated = app
                        .users
                        .users
                        .iter()
                        .find(|u| u.id == id)
                        .cloned()
                        .unwrap_or_default();
                    run(app, ctx, "Saving user…", move |g| {
                        g.update_user(&id, &edit)?;
                        let opt = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
                        updated.display_name = opt(&edit.display_name);
                        updated.given_name = opt(&edit.given_name);
                        updated.surname = opt(&edit.surname);
                        updated.job_title = opt(&edit.job_title);
                        updated.department = opt(&edit.department);
                        updated.office_location = opt(&edit.office_location);
                        updated.mobile_phone = opt(&edit.mobile_phone);
                        updated.usage_location = opt(&edit.usage_location.to_uppercase());
                        Ok((format!("Saved {}.", updated.name()), Change::Upsert(Box::new(updated))))
                    })
                }
            }
        }
        None => {}
    }
}

fn reset_modal(app: &mut App, ctx: &egui::Context) {
    let Some(reset) = app.users.reset.as_mut() else {
        return;
    };
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::new("reset-password")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.heading("Reset password");
        ui.label(RichText::new(&reset.name).weak());
        ui.add_space(8.0);
        let field = ui::labelled_field(ui, "New password", &mut reset.password, "");
        ui::focus_on_open(ui, egui::Id::new("reset-password"), &field);
        if ui.small_button("Generate another").clicked() {
            reset.password = generate_password();
        }
        ui.checkbox(&mut reset.force_change, "Must change password at next sign-in");
        ui.label(
            RichText::new("Copy the password before resetting: it is not shown again.")
                .size(12.0)
                .weak(),
        );
        answer = ui::form_buttons(ui, "Reset", !reset.password.is_empty());
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }
    match answer {
        Some(true) => {
            let Some(r) = app.users.reset.take() else { return };
            let user = app.users.selected_user().cloned().unwrap_or_default();
            run(app, ctx, "Resetting password…", move |g| {
                g.reset_password(&r.id, &r.password, r.force_change)?;
                Ok((format!("Password reset for {}.", r.name), Change::Upsert(Box::new(user))))
            });
        }
        Some(false) => app.users.reset = None,
        None => {}
    }
}

fn delete_modal(app: &mut App, ctx: &egui::Context) {
    let Some((id, name)) = app.users.confirm_delete.clone() else {
        return;
    };
    let answer = ui::confirm_modal(
        ctx,
        egui::Id::new("delete-user"),
        "Delete user?",
        &format!("{name} will be moved to deleted users, and can be restored from the Entra admin centre for 30 days."),
        "Delete",
    );
    if answer == ui::Confirmation::Waiting {
        return;
    }
    app.users.confirm_delete = None;
    if answer == ui::Confirmation::Confirmed {
        run(app, ctx, "Deleting user…", move |g| {
            g.delete_user(&id)?;
            Ok((format!("Deleted {name}."), Change::Removed(id)))
        });
    }
}

fn preview_modal(app: &mut App, ctx: &egui::Context) {
    let Some(preview) = &app.users.preview else {
        return;
    };
    let ready = preview.rows.iter().filter(|r| r.user.is_ok()).count();
    let generated = preview
        .rows
        .iter()
        .filter(|r| r.user.is_ok() && r.generated_password)
        .count();
    let mut answer = None;

    let modal = egui::Modal::new(egui::Id::new("import-preview")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading("Import users");
        ui.label(RichText::new(&preview.file).size(12.0).weak());
        ui.add_space(6.0);
        ui.label(format!(
            "{ready} of {} rows are ready to create.",
            preview.rows.len()
        ));
        if generated > 0 {
            ui.label(
                RichText::new(format!(
                    "{generated} rows have no password: one will be generated for each, and written to the results file."
                ))
                .size(13.0)
                .color(ui::warn_colour(ui)),
            );
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            egui::Grid::new("import-rows")
                .num_columns(3)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for row in &preview.rows {
                        ui.label(RichText::new(format!("Line {}", row.line)).weak());
                        match &row.user {
                            Ok(u) => {
                                ui.label(&u.display_name);
                                ui.label(&u.user_principal_name);
                            }
                            Err(err) => {
                                ui.label(RichText::new("Skipped").color(ui::bad_colour(ui)));
                                ui.label(RichText::new(err).color(ui::bad_colour(ui)));
                            }
                        }
                        ui.end_row();
                    }
                });
        });
        answer = ui::form_buttons(ui, &format!("Create {ready} users"), ready > 0);
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }

    match answer {
        Some(true) => {
            let Some(preview) = app.users.preview.take() else { return };
            let Some(graph) = graph(app) else { return };
            let progress = app.users.import_progress.clone();
            app.users.results = None;
            app.users.import = Some(Task::spawn(ctx, "Importing users…", move || {
                let rows: Vec<_> = preview
                    .rows
                    .into_iter()
                    .filter_map(|r| r.user.ok().map(|u| (r.line, u, r.generated_password)))
                    .collect();
                let total = rows.len();
                let mut results = Vec::with_capacity(total);
                for (n, (line, user, generated)) in rows.into_iter().enumerate() {
                    if let Ok(mut p) = progress.lock() {
                        *p = format!("Creating user {} of {total}…", n + 1);
                    }
                    let outcome = graph.create_user(&user).map(|u| u.id);
                    if let Err(err) = &outcome {
                        log::warn!("import line {line} ({}) failed: {err}", user.user_principal_name);
                    }
                    results.push(ImportResult {
                        line,
                        user_principal_name: user.user_principal_name.clone(),
                        password: generated.then(|| user.password.clone()),
                        outcome,
                    });
                }
                Ok(results)
            }));
        }
        Some(false) => app.users.preview = None,
        None => {}
    }
}
