//! The details pane: the property sheet for whatever the result list has
//! selected.
//!
//! Membership lists are fetched on demand rather than with the parent
//! collection — expanding every group's membership up front would be thousands
//! of requests for data the user will mostly never look at.

use egui::{Color32, CornerRadius, RichText, StrokeKind};

use super::{App, Pane, View, theme};
use crate::graph::Fetch;
use super::menu;
use crate::graph::actions::Severity;
use crate::graph::models::*;

/// The button bar at the top of a property sheet.
///
/// Renders whatever [`super::menu::for_object`] offers, which is the same list
/// the right-click menu draws, so the two can never disagree. Buttons stay
/// visible but disabled while read-only, so nothing shifts position when write
/// mode is armed.
fn action_bar(app: &mut App, ui: &mut egui::Ui, source: usize) {
    let view = app.view;
    let items = menu::for_object(app, view, source);
    if items.is_empty() {
        return;
    }

    let armed = app.write_mode.is_armed();
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for item in items {
            match item {
                // Separators structure the menu; the bar just wraps.
                menu::Item::Separator => {}
                menu::Item::Act { label, action } => {
                    let destructive = action.severity() == Severity::Destructive;
                    let text = if destructive {
                        RichText::new(&label)
                            .color(if armed { theme::BAD } else { theme::MUTED })
                    } else {
                        RichText::new(&label)
                    };
                    if enabled_button(ui, armed, egui::Button::new(text)) {
                        app.request_action(action);
                    }
                }
                menu::Item::Open { label, form } => {
                    if enabled_button(ui, armed, egui::Button::new(&label)) {
                        app.open_form(form.build());
                    }
                }
            }
        }
    });
    ui.add_space(4.0);
}

/// A button that explains itself when write mode is off.
fn enabled_button(ui: &mut egui::Ui, armed: bool, button: egui::Button<'_>) -> bool {
    let response = ui.add_enabled(armed, button);
    let response = if armed {
        response
    } else {
        response.on_disabled_hover_text("Enable write mode (Ctrl+Shift+W) to use this")
    };
    response.clicked()
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        ui.label(RichText::new("DETAILS").small().color(theme::MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Copy")
                .on_hover_text("Copy this record (Ctrl+C)")
                .clicked()
            {
                let text = copy_text(app);
                ui.ctx().copy_text(text);
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(6.0);
            match app.view {
                View::Overview => hint(ui, "Select a node in the console tree."),
                View::Users => user_details(app, ui),
                View::Groups => group_details(app, ui),
                View::Roles => role_details(app, ui),
                View::Devices => device_details(app, ui),
                View::ManagedDevices => managed_details(app, ui),
                View::Licenses => license_details(app, ui),
            }
        });

    if app.pane == Pane::Details {
        ui.painter().rect_stroke(
            ui.max_rect().shrink(1.0),
            CornerRadius::ZERO,
            egui::Stroke::new(2.0, theme::FOCUS_RING),
            StrokeKind::Inside,
        );
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).color(theme::MUTED));
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong());
    ui.add_space(8.0);
}

fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).small().color(theme::MUTED));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
}

/// One label/value line. Values wrap rather than truncate — the details pane is
/// where the full value belongs, since the table already clips.
fn field(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.set_min_height(18.0);
        ui.allocate_ui_with_layout(
            egui::vec2(126.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new(label).color(theme::MUTED));
            },
        );
        ui.label(value);
    });
}

fn field_colored(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    ui.horizontal_top(|ui| {
        ui.set_min_height(18.0);
        ui.allocate_ui_with_layout(
            egui::vec2(126.0, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.label(RichText::new(label).color(theme::MUTED));
            },
        );
        ui.label(RichText::new(value).color(color).strong());
    });
}

// ---- Per-view property sheets ---------------------------------------------

fn user_details(app: &mut App, ui: &mut egui::Ui) {
    let Some(user) = app.selected_user().cloned() else {
        hint(ui, "Select a user to see their details.");
        return;
    };

    title(ui, user.name());

    if let Some(source) = app.views.get(&View::Users).and_then(|s| s.selected_source()) {
        action_bar(app, ui, source);
    }

    field(ui, "Sign-in name", user.upn());
    field_colored(
        ui,
        "Account",
        user.status(),
        theme::status_color(user.status()),
    );
    field(ui, "Type", &fmt_opt(&user.user_type));
    field(ui, "Email", &fmt_opt(&user.mail));

    section(ui, "ORGANISATION");
    field(ui, "Job title", &fmt_opt(&user.job_title));
    field(ui, "Department", &fmt_opt(&user.department));
    field(ui, "Office", &fmt_opt(&user.office_location));
    field(ui, "Mobile", &fmt_opt(&user.mobile_phone));
    if let Some(phone) = user.business_phones.first() {
        field(ui, "Business phone", phone);
    }

    section(ui, "DIRECTORY");
    field(ui, "Object ID", &user.id);
    field(
        ui,
        "Source",
        if user.on_premises_sync_enabled.unwrap_or(false) {
            "Synced from Windows Server AD"
        } else {
            "Cloud only"
        },
    );
    if let Some(sam) = &user.on_premises_sam_account_name {
        field(ui, "SAM account", sam);
    }
    field(ui, "Usage location", &fmt_opt(&user.usage_location));
    field(ui, "Created", &fmt_date(&user.created_date_time));
    field(
        ui,
        "Password changed",
        &fmt_date(&user.last_password_change_date_time),
    );

    // Proxy addresses carry the mailbox aliases, which is what an admin is
    // usually hunting for when a message bounced.
    let aliases: Vec<&str> = user
        .proxy_addresses
        .iter()
        .filter_map(|address| address.strip_prefix("smtp:"))
        .collect();
    if !aliases.is_empty() {
        field(ui, "Email aliases", &aliases.join(", "));
    }

    section(ui, "LICENSES");
    if user.assigned_licenses.is_empty() {
        ui.label(RichText::new("No licenses assigned").color(theme::MUTED));
    } else {
        // Resolve each assigned SKU id against the tenant's subscriptions so the
        // pane shows product names rather than GUIDs.
        for assigned in &user.assigned_licenses {
            let name = assigned
                .sku_id
                .as_ref()
                .and_then(|sku_id| {
                    app.store
                        .licenses
                        .iter()
                        .find(|sku| sku.sku_id.as_ref() == Some(sku_id))
                })
                .map(|sku| sku.display_name())
                .or_else(|| assigned.sku_id.clone())
                .unwrap_or_else(|| "Unknown SKU".into());

            ui.horizontal(|ui| {
                ui.label("•");
                ui.label(name);
            });
            if !assigned.disabled_plans.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "   {} service plan(s) disabled",
                        assigned.disabled_plans.len()
                    ))
                    .small()
                    .color(theme::MUTED),
                );
            }
        }
    }

    section(ui, "MEMBER OF");
    match app.store.user_memberships.get(&user.id) {
        Some(memberships) if memberships.is_empty() => {
            ui.label(RichText::new("No group or role memberships").color(theme::MUTED));
        }
        Some(memberships) => member_list(ui, memberships),
        None => loading(ui),
    }
}

fn group_details(app: &mut App, ui: &mut egui::Ui) {
    let Some(group) = app.selected_group().cloned() else {
        hint(ui, "Select a group to see its details.");
        return;
    };

    title(ui, group.name());

    let dynamic = group.membership() == "Dynamic";
    if let Some(source) = app.views.get(&View::Groups).and_then(|s| s.selected_source()) {
        action_bar(app, ui, source);
    }
    if dynamic {
        ui.label(
            RichText::new(
                "Membership is computed from the rule below and cannot be edited by hand.",
            )
            .small()
            .color(theme::MUTED),
        );
    }
    ui.add_space(4.0);

    field(ui, "Type", group.kind());
    field(ui, "Membership", group.membership());
    field(ui, "Email", &fmt_opt(&group.mail));
    field(ui, "Description", &fmt_opt(&group.description));

    section(ui, "DIRECTORY");
    field(ui, "Object ID", &group.id);
    field(ui, "Alias", &fmt_opt(&group.mail_nickname));
    field(ui, "Source", group.source());
    field(ui, "Visibility", &fmt_opt(&group.visibility));
    field(
        ui,
        "Role-assignable",
        &fmt_bool(&group.is_assignable_to_role),
    );
    field(ui, "Created", &fmt_date(&group.created_date_time));

    if let Some(rule) = &group.membership_rule {
        section(ui, "DYNAMIC MEMBERSHIP RULE");
        ui.label(RichText::new(rule).monospace().small());
        field(
            ui,
            "Processing",
            &fmt_opt(&group.membership_rule_processing_state),
        );
    }

    match app.store.group_members.get(&group.id) {
        Some((members, owners)) => {
            section(ui, &format!("OWNERS ({})", owners.len()));
            if owners.is_empty() {
                ui.label(RichText::new("No owners").color(theme::MUTED));
            } else {
                member_list(ui, owners);
            }

            section(ui, &format!("MEMBERS ({})", members.len()));
            if members.is_empty() {
                ui.label(RichText::new("No members").color(theme::MUTED));
            } else {
                member_list(ui, members);
            }
        }
        None => {
            section(ui, "MEMBERS");
            loading(ui);
        }
    }
}

fn role_details(app: &mut App, ui: &mut egui::Ui) {
    let Some(role) = app.selected_role().cloned() else {
        hint(ui, "Select a directory role to see its details.");
        return;
    };

    title(ui, role.name());
    field(ui, "Description", &fmt_opt(&role.description));

    section(ui, "DIRECTORY");
    field(ui, "Object ID", &role.id);
    field(ui, "Template ID", &fmt_opt(&role.role_template_id));

    match app.store.role_members.get(&role.id) {
        Some(members) => {
            section(ui, &format!("ASSIGNED TO ({})", members.len()));
            if members.is_empty() {
                ui.label(RichText::new("Nobody holds this role").color(theme::MUTED));
            } else {
                member_list(ui, members);
            }
        }
        None => {
            section(ui, "ASSIGNED TO");
            loading(ui);
        }
    }
}

fn device_details(app: &mut App, ui: &mut egui::Ui) {
    let Some(state) = app.views.get(&View::Devices) else {
        hint(ui, "Select a device to see its details.");
        return;
    };
    let Some(device) = state
        .selected_source()
        .and_then(|index| app.store.devices.get(index))
    else {
        hint(ui, "Select a device to see its details.");
        return;
    };

    let device = device.clone();
    title(ui, device.name());

    if let Some(source) = app.views.get(&View::Devices).and_then(|s| s.selected_source()) {
        action_bar(app, ui, source);
    }

    field(ui, "Operating system", &device.os_display());
    field(ui, "Join type", device.join_type());

    let compliant = fmt_bool(&device.is_compliant);
    field_colored(ui, "Compliant", &compliant, theme::status_color(&compliant));
    field(ui, "Managed", &fmt_bool(&device.is_managed));
    field(ui, "Enabled", &fmt_bool(&device.account_enabled));

    section(ui, "HARDWARE");
    field(ui, "Manufacturer", &fmt_opt(&device.manufacturer));
    field(ui, "Model", &fmt_opt(&device.model));
    field(ui, "Profile", &fmt_opt(&device.profile_type));

    section(ui, "DIRECTORY");
    field(ui, "Object ID", &device.id);
    field(ui, "Device ID", &fmt_opt(&device.device_id));
    field(
        ui,
        "Registered",
        &fmt_date(&device.registration_date_time),
    );
    field(
        ui,
        "Last sign-in",
        &fmt_date(&device.approximate_last_sign_in_date_time),
    );
    field(ui, "Source", if device.on_premises_sync_enabled.unwrap_or(false) {
        "Synced from Windows Server AD"
    } else {
        "Cloud only"
    });
}

fn managed_details(app: &mut App, ui: &mut egui::Ui) {
    if let Some(Fetch::Unavailable(reason)) = &app.store.managed {
        title(ui, "Intune not available");
        ui.label(RichText::new(reason).color(theme::MUTED));
        return;
    }

    let devices = match &app.store.managed {
        Some(Fetch::Ready(devices)) => devices.clone(),
        _ => {
            loading(ui);
            return;
        }
    };

    let Some(device) = app
        .views
        .get(&View::ManagedDevices)
        .and_then(|state| state.selected_source())
        .and_then(|index| devices.get(index))
    else {
        hint(ui, "Select a managed device to see its details.");
        return;
    };

    title(ui, device.name());
    field(ui, "Primary user", &fmt_opt(&device.user_principal_name));
    field(ui, "Operating system", &device.os_display());

    let compliance = device.compliance_display();
    field_colored(
        ui,
        "Compliance",
        &compliance,
        theme::status_color(&compliance),
    );
    field(ui, "Managed by", &device.agent_display());
    field(ui, "Ownership", &fmt_opt(&device.managed_device_owner_type));
    field(ui, "Enrollment", &fmt_opt(&device.device_enrollment_type));

    section(ui, "HARDWARE");
    field(ui, "Manufacturer", &fmt_opt(&device.manufacturer));
    field(ui, "Model", &fmt_opt(&device.model));
    field(ui, "Serial number", &fmt_opt(&device.serial_number));
    field(ui, "IMEI", &fmt_opt(&device.imei));
    field(ui, "Storage", &device.storage_display());

    section(ui, "SECURITY");
    field(ui, "Encrypted", &fmt_bool(&device.is_encrypted));
    field(ui, "Supervised", &fmt_bool(&device.is_supervised));
    field(ui, "Jailbroken", &fmt_opt(&device.jail_broken));

    section(ui, "MANAGEMENT");
    field(ui, "Intune ID", &device.id);
    field(ui, "Enrolled", &fmt_date(&device.enrolled_date_time));
    field(ui, "Last check-in", &fmt_date(&device.last_sync_date_time));
}

fn license_details(app: &mut App, ui: &mut egui::Ui) {
    let Some(sku) = app
        .views
        .get(&View::Licenses)
        .and_then(|state| state.selected_source())
        .and_then(|index| app.store.licenses.get(index))
    else {
        hint(ui, "Select a license to see its details.");
        return;
    };

    title(ui, &sku.display_name());
    if !crate::graph::skus::is_known(sku.part_number()) {
        ui.label(
            RichText::new("Showing the raw SKU part number — this product is not in gcm's name table.")
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(6.0);
    }

    field(ui, "SKU part number", sku.part_number());
    field(ui, "SKU ID", &fmt_opt(&sku.sku_id));
    field(ui, "Applies to", &fmt_opt(&sku.applies_to));
    field(ui, "Status", &fmt_opt(&sku.capability_status));

    section(ui, "SEATS");
    field(ui, "Assigned", &sku.consumed().to_string());
    field(ui, "Purchased", &sku.total_seats().to_string());

    let available = sku.available();
    let color = if available == 0 { theme::BAD } else { theme::OK };
    field_colored(ui, "Available", &available.to_string(), color);

    if let Some(units) = &sku.prepaid_units {
        if units.warning.unwrap_or(0) > 0 {
            field_colored(
                ui,
                "In warning",
                &units.warning.unwrap_or(0).to_string(),
                theme::WARN,
            );
        }
        if units.suspended.unwrap_or(0) > 0 {
            field_colored(
                ui,
                "Suspended",
                &units.suspended.unwrap_or(0).to_string(),
                theme::BAD,
            );
        }
    }

    if sku.consumed() > sku.total_seats() {
        ui.add_space(6.0);
        ui.label(
            RichText::new("More seats are assigned than purchased.")
                .color(theme::BAD)
                .small(),
        );
    }

    section(ui, &format!("SERVICE PLANS ({})", sku.service_plans.len()));
    for plan in &sku.service_plans {
        let name = fmt_opt(&plan.service_plan_name);
        let status = fmt_opt(&plan.provisioning_status);
        ui.horizontal(|ui| {
            ui.label(RichText::new("•").color(theme::MUTED));
            ui.label(RichText::new(name).small());
            // Company-scoped plans are provisioned once for the tenant rather
            // than per seat, which explains counts that look wrong otherwise.
            if plan.applies_to.as_deref() == Some("Company") {
                ui.label(RichText::new("tenant-wide").small().color(theme::MUTED));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(status).small().color(theme::MUTED));
            });
        });
    }
}

fn member_list(ui: &mut egui::Ui, members: &[DirectoryMember]) {
    // Very large groups would make the pane unscrollable in practice; show a
    // useful prefix and say how many are hidden.
    const LIMIT: usize = 200;
    for member in members.iter().take(LIMIT) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("•").color(theme::MUTED));
            ui.label(member.name());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(member.kind()).small().color(theme::MUTED));
            });
        });
    }
    if members.len() > LIMIT {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("… and {} more", members.len() - LIMIT))
                .small()
                .color(theme::MUTED),
        );
    }
}

fn loading(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new("Loading…").color(theme::MUTED));
    });
}

/// Plain-text rendering of the selected record, for Ctrl+C.
pub fn copy_text(app: &App) -> String {
    let view = app.view;
    match app.views.get(&view).and_then(|s| s.selected_source()) {
        Some(source) => super::list::row_label(app, view, source),
        None => String::new(),
    }
}
