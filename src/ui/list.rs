//! The result pane: a dense, virtualized, keyboard-driven table.
//!
//! Only the rows actually on screen are built, so a tenant with fifty thousand
//! users costs the same per frame as one with fifty. Selection is an index, and
//! the scroll position is computed from that index rather than from whichever
//! rows happened to be rendered — otherwise arrowing past the viewport edge
//! would silently do nothing.

use egui::{Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, StrokeKind, Vec2};

use super::{App, Pane, View, menu, theme};
use crate::graph::actions::Severity;
use crate::graph::Fetch;
use crate::graph::models::*;

/// Width reserved at the left of every row for the bulk-selection tick.
const MARK_GUTTER: f32 = 18.0;

/// A column in the result table. `weight` is a share of the available width.
pub struct Column {
    pub title: &'static str,
    pub weight: f32,
}

const fn col(title: &'static str, weight: f32) -> Column {
    Column { title, weight }
}

// Weights are tuned so the longest realistic value in each column fits at the
// default window width: "Disabled", "Mail-enabled security", "Windows Server
// AD", "Entra registered", "In grace period". Anything longer is clipped by
// design — the details pane carries the full value.
const USER_COLUMNS: &[Column] = &[
    col("Name", 2.2),
    col("User principal name", 2.6),
    col("Job title", 1.5),
    col("Department", 1.2),
    col("Status", 1.0),
    col("Type", 0.9),
];

const GROUP_COLUMNS: &[Column] = &[
    col("Name", 2.2),
    col("Type", 1.8),
    col("Membership", 1.1),
    col("Email", 1.9),
    col("Source", 1.5),
];

const ROLE_COLUMNS: &[Column] = &[col("Role", 2.2), col("Description", 4.5)];

const DEVICE_COLUMNS: &[Column] = &[
    col("Name", 2.2),
    col("Operating system", 1.7),
    col("Join type", 1.6),
    col("Compliant", 1.0),
    col("Managed", 1.0),
    col("Last sign-in", 1.4),
];

const MANAGED_COLUMNS: &[Column] = &[
    col("Name", 2.0),
    col("Primary user", 2.1),
    col("Operating system", 1.6),
    col("Compliance", 1.5),
    col("Managed by", 1.4),
    col("Last check-in", 1.4),
];

const LICENSE_COLUMNS: &[Column] = &[
    col("Product", 2.6),
    col("SKU part number", 1.8),
    col("Assigned", 0.9),
    col("Total", 0.9),
    col("Available", 0.9),
    col("Usage", 1.6),
];

pub fn columns(view: View) -> &'static [Column] {
    match view {
        View::Overview => &[],
        View::Users => USER_COLUMNS,
        View::Groups => GROUP_COLUMNS,
        View::Roles => ROLE_COLUMNS,
        View::Devices => DEVICE_COLUMNS,
        View::ManagedDevices => MANAGED_COLUMNS,
        View::Licenses => LICENSE_COLUMNS,
    }
}

/// The text for one cell. Called only for visible rows.
fn cell(app: &App, view: View, source: usize, column: usize) -> String {
    match view {
        View::Overview => String::new(),
        View::Users => match app.store.users.get(source) {
            Some(user) => match column {
                0 => user.name().to_string(),
                1 => user.upn().to_string(),
                2 => fmt_opt(&user.job_title),
                3 => fmt_opt(&user.department),
                4 => user.status().to_string(),
                _ => fmt_opt(&user.user_type),
            },
            None => String::new(),
        },
        View::Groups => match app.store.groups.get(source) {
            Some(group) => match column {
                0 => group.name().to_string(),
                1 => group.kind().to_string(),
                2 => group.membership().to_string(),
                3 => fmt_opt(&group.mail),
                _ => group.source().to_string(),
            },
            None => String::new(),
        },
        View::Roles => match app.store.roles.get(source) {
            Some(role) => match column {
                0 => role.name().to_string(),
                _ => fmt_opt(&role.description),
            },
            None => String::new(),
        },
        View::Devices => match app.store.devices.get(source) {
            Some(device) => match column {
                0 => device.name().to_string(),
                1 => device.os_display(),
                2 => device.join_type().to_string(),
                3 => fmt_bool(&device.is_compliant),
                4 => fmt_bool(&device.is_managed),
                _ => fmt_date(&device.approximate_last_sign_in_date_time),
            },
            None => String::new(),
        },
        View::ManagedDevices => match managed(app).get(source) {
            Some(device) => match column {
                0 => device.name().to_string(),
                1 => fmt_opt(&device.user_principal_name),
                2 => device.os_display(),
                3 => device.compliance_display(),
                4 => device.agent_display(),
                _ => fmt_date(&device.last_sync_date_time),
            },
            None => String::new(),
        },
        View::Licenses => match app.store.licenses.get(source) {
            Some(sku) => match column {
                0 => sku.display_name(),
                1 => sku.part_number().to_string(),
                2 => sku.consumed().to_string(),
                3 => sku.total_seats().to_string(),
                4 => sku.available().to_string(),
                _ => String::new(), // drawn as a bar
            },
            None => String::new(),
        },
    }
}

fn managed(app: &App) -> &[ManagedDevice] {
    match &app.store.managed {
        Some(Fetch::Ready(devices)) => devices.as_slice(),
        _ => &[],
    }
}

// ---- Filtering -------------------------------------------------------------
//
// Each predicate receives an already-lowercased needle and searches the fields
// an administrator would plausibly type: names, addresses, and identifiers.

fn contains(haystack: &Option<String>, needle: &str) -> bool {
    haystack
        .as_deref()
        .is_some_and(|value| value.to_lowercase().contains(needle))
}

pub fn user_matches(user: &User, needle: &str) -> bool {
    user.name().to_lowercase().contains(needle)
        || contains(&user.user_principal_name, needle)
        || contains(&user.mail, needle)
        || contains(&user.job_title, needle)
        || contains(&user.department, needle)
        || contains(&user.office_location, needle)
}

pub fn group_matches(group: &Group, needle: &str) -> bool {
    group.name().to_lowercase().contains(needle)
        || contains(&group.mail, needle)
        || contains(&group.description, needle)
        || group.kind().to_lowercase().contains(needle)
}

pub fn role_matches(role: &DirectoryRole, needle: &str) -> bool {
    role.name().to_lowercase().contains(needle) || contains(&role.description, needle)
}

pub fn device_matches(device: &Device, needle: &str) -> bool {
    device.name().to_lowercase().contains(needle)
        || device.os_display().to_lowercase().contains(needle)
        || contains(&device.model, needle)
        || contains(&device.manufacturer, needle)
        || device.join_type().to_lowercase().contains(needle)
}

pub fn managed_matches(device: &ManagedDevice, needle: &str) -> bool {
    device.name().to_lowercase().contains(needle)
        || contains(&device.user_principal_name, needle)
        || contains(&device.serial_number, needle)
        || contains(&device.model, needle)
        || device.os_display().to_lowercase().contains(needle)
        || device.compliance_display().to_lowercase().contains(needle)
}

pub fn sku_matches(sku: &SubscribedSku, needle: &str) -> bool {
    sku.display_name().to_lowercase().contains(needle)
        || sku.part_number().to_lowercase().contains(needle)
}

// ---- Rendering -------------------------------------------------------------

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.view == View::Overview {
        overview(app, ui);
        return;
    }

    let view = app.view;
    header(app, ui, view);

    // A tenant that has not licensed Intune gets an explanation, not an
    // empty table that looks like a bug.
    if view == View::ManagedDevices
        && let Some(Fetch::Unavailable(reason)) = &app.store.managed
    {
        let reason = reason.clone();
        unavailable(ui, &reason);
        return;
    }

    if let Some(error) = app.store.error(view) {
        let error = error.clone();
        error_panel(app, ui, &error);
        return;
    }

    table(app, ui, view);

    if app.pane == Pane::List {
        ui.painter().rect_stroke(
            ui.max_rect().shrink(1.0),
            CornerRadius::ZERO,
            egui::Stroke::new(2.0, theme::FOCUS_RING),
            StrokeKind::Inside,
        );
    }
}

fn header(app: &mut App, ui: &mut egui::Ui, view: View) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(view.title()).size(15.0).strong());

        let shown = app.views.get(&view).map(|s| s.filtered.len()).unwrap_or(0);
        let total = app.store.count(view).unwrap_or(0);
        let caption = if shown == total {
            format!("{total} items")
        } else {
            format!("{shown} of {total} items")
        };
        ui.label(RichText::new(caption).color(theme::MUTED));

        if app.store.is_loading(view) {
            ui.spinner();
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let state = app.views.entry(view).or_default();
            let filter = egui::TextEdit::singleline(&mut state.filter)
                .desired_width(240.0)
                .hint_text("Filter (Ctrl+F)");
            let response = ui.add(filter);

            if app.focus_filter {
                response.request_focus();
                app.focus_filter = false;
            }
            // Typing in the filter must not also drive list navigation.
            if response.has_focus() {
                app.pane = Pane::List;
            }
        });
    });
    ui.add_space(4.0);
    bulk_bar(app, ui, view);
    ui.separator();
}

/// Bulk actions for whatever is ticked. Only appears once rows are ticked, so
/// it never competes for attention during ordinary browsing.
fn bulk_bar(app: &mut App, ui: &mut egui::Ui, view: View) {
    let marked: Vec<usize> = match app.views.get(&view) {
        Some(state) if state.has_marks() => state.marked.iter().copied().collect(),
        _ => return,
    };

    let count = marked.len();
    let armed = app.write_mode.is_armed();

    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{count} selected"))
                .strong()
                .color(theme::ACCENT),
        );

        if ui
            .button("Clear")
            .on_hover_text("Clear the selection (Ctrl+Shift+A)")
            .clicked()
        {
            app.view_state(view).clear_marks();
        }

        ui.separator();

        for (label, actions) in menu::bulk_for(app, view, &marked) {
            let destructive = actions
                .iter()
                .any(|action| action.severity() == Severity::Destructive);
            let text = if destructive {
                RichText::new(&label).color(if armed { theme::BAD } else { theme::MUTED })
            } else {
                RichText::new(&label)
            };
            if ui.add_enabled(armed, egui::Button::new(text)).clicked() {
                app.request_actions(actions);
            }
        }

        if !armed {
            ui.label(
                RichText::new("Enable write mode to act on these")
                    .small()
                    .color(theme::MUTED),
            );
        }
    });
    ui.add_space(4.0);
}

fn table(app: &mut App, ui: &mut egui::Ui, view: View) {
    let cols = columns(view);
    let total_weight: f32 = cols.iter().map(|c| c.weight).sum();
    let available = ui.available_width() - 16.0 - MARK_GUTTER;
    let widths: Vec<f32> = cols
        .iter()
        .map(|c| (c.weight / total_weight) * available)
        .collect();

    // Column headings.
    let (header_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    ui.painter()
        .rect_filled(header_rect, CornerRadius::ZERO, ui.visuals().faint_bg_color);
    let mut x = header_rect.left() + MARK_GUTTER;
    for (index, column) in cols.iter().enumerate() {
        ui.painter().text(
            egui::pos2(x, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            column.title,
            egui::FontId::proportional(11.5),
            theme::MUTED,
        );
        x += widths[index];
    }
    ui.painter().hline(
        header_rect.x_range(),
        header_rect.bottom(),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );

    let row_count = app.views.get(&view).map(|s| s.filtered.len()).unwrap_or(0);
    if row_count == 0 {
        let empty = if app.store.is_loading(view) {
            "Loading…"
        } else if app
            .views
            .get(&view)
            .is_some_and(|s| !s.filter.trim().is_empty())
        {
            "No items match the filter."
        } else {
            "No items."
        };
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(empty).color(theme::MUTED));
        });
        return;
    }

    let row_height = theme::ROW_HEIGHT;
    let viewport_height = ui.available_height();

    // Drive the scroll position from the selection index. Doing this before the
    // scroll area is shown is what lets arrow keys move past the viewport edge.
    let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false, false]);
    {
        let state = app.views.entry(view).or_default();
        if state.scroll_to_selection {
            let top = state.selected as f32 * row_height;
            let bottom = top + row_height;
            let offset = state.last_offset;
            let target = if top < offset {
                top
            } else if bottom > offset + viewport_height {
                bottom - viewport_height
            } else {
                offset
            };
            scroll_area = scroll_area.vertical_scroll_offset(target);
            state.scroll_to_selection = false;
        }
    }

    let output = scroll_area.show_rows(ui, row_height, row_count, |ui, range| {
        for row_index in range {
            row(app, ui, view, row_index, &widths);
        }
    });

    let state = app.views.entry(view).or_default();
    state.last_offset = output.state.offset.y;
    state.last_viewport = viewport_height;
}

fn row(app: &mut App, ui: &mut egui::Ui, view: View, row_index: usize, widths: &[f32]) {
    let Some(source) = app
        .views
        .get(&view)
        .and_then(|s| s.filtered.get(row_index).copied())
    else {
        return;
    };
    let selected = app
        .views
        .get(&view)
        .is_some_and(|s| s.selected == row_index);
    let pane_focused = app.pane == Pane::List;

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), theme::ROW_HEIGHT),
        Sense::hover(),
    );
    // Keyed on the object, not the row position: `show_rows` recycles auto-ids
    // as you scroll, which would leave an open menu attached to whatever row
    // later took that slot.
    let response = ui.interact(rect, ui.id().with(("row", view, source)), Sense::click());

    if selected {
        let fill = if pane_focused {
            theme::ACCENT
        } else {
            theme::ACCENT_INACTIVE
        };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    } else if row_index % 2 == 1 {
        // Banding makes wide rows scannable across six columns.
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, ui.visuals().faint_bg_color);
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::ZERO,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }

    let default_color = if selected {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };

    // Tick in the left gutter for rows in the bulk set.
    let marked = app
        .views
        .get(&view)
        .is_some_and(|state| state.marked.contains(&source));
    if marked {
        ui.painter().text(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            "✓",
            egui::FontId::proportional(12.0),
            if selected { Color32::WHITE } else { theme::ACCENT },
        );
    }

    let mut x = rect.left() + MARK_GUTTER;
    for (index, width) in widths.iter().enumerate() {
        // The licence usage column is a bar, not text.
        if view == View::Licenses && index == widths.len() - 1 {
            if let Some(sku) = app.store.licenses.get(source) {
                usage_bar(ui, rect, x, *width, sku, selected);
            }
            break;
        }

        let text = cell(app, view, source, index);
        let color = if selected {
            default_color
        } else {
            match view {
                View::Users if index == 4 => theme::status_color(&text),
                View::Devices if index == 3 => theme::status_color(&text),
                View::ManagedDevices if index == 3 => theme::status_color(&text),
                _ => default_color,
            }
        };

        // Clip so a long value cannot bleed into the neighbouring column.
        let clip = Rect::from_min_size(
            egui::pos2(x, rect.top()),
            Vec2::new((*width - 8.0).max(0.0), rect.height()),
        );
        let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(12.5),
            color,
        );
        x += width;
    }

    // Give assistive technology the whole row, not six disconnected labels.
    let label = row_label(app, view, source);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            label.as_str(),
        )
    });

    let ctrl_menu_click =
        cfg!(target_os = "macos") && ui.input(|i| i.modifiers.ctrl) && response.clicked();

    if response.clicked() && !ctrl_menu_click {
        app.pane = Pane::List;
        // Cmd+click (Ctrl+click off macOS) ticks the row for a bulk operation.
        let mark = ui.input(|i| i.modifiers.command);
        let state = app.views.entry(view).or_default();
        state.selected = row_index;
        if mark {
            state.toggle_mark();
        }
    }
    if response.double_clicked() {
        app.pane = Pane::Details;
        app.show_details = true;
    }

    // Right-clicking a row that is not part of the ticked set moves the cursor
    // to it first, so the menu always acts on what the operator pointed at
    // rather than on a stale selection elsewhere in the list.
    if response.secondary_clicked() || ctrl_menu_click {
        app.pane = Pane::List;
        let state = app.views.entry(view).or_default();
        if !state.marked.contains(&source) {
            state.selected = row_index;
        }
    }

    context_menu(app, &response, view, source);
}

/// The right-click menu for a row.
///
/// Draws from [`super::menu`], the same source as the details pane button bar,
/// so the two surfaces cannot offer different things. Right-clicking inside a
/// ticked set acts on the whole set; right-clicking outside it acts on that one
/// row.
fn context_menu(app: &mut App, response: &egui::Response, view: View, source: usize) {
    let marked: Vec<usize> = app
        .views
        .get(&view)
        .map(|state| state.marked.iter().copied().collect())
        .unwrap_or_default();
    let bulk = marked.len() > 1 && marked.contains(&source);
    let armed = app.write_mode.is_armed();

    // On macOS, Control+click is the conventional secondary click, but the
    // window system reports it as a primary click carrying a modifier — so
    // egui's own `secondary_clicked` never sees it. Recognise it here rather
    // than leaving Mac users without a context menu.
    let ctrl_click = cfg!(target_os = "macos")
        && response.clicked()
        && response.ctx.input(|i| i.modifiers.ctrl);
    let wants_menu = response.secondary_clicked() || ctrl_click;

    egui::Popup::menu(response)
        .open_memory(if wants_menu {
            Some(egui::SetOpenCommand::Bool(true))
        } else if response.clicked() {
            // A plain click dismisses an open menu.
            Some(egui::SetOpenCommand::Bool(false))
        } else {
            None
        })
        .at_pointer_fixed()
        .show(|ui| {
        ui.set_min_width(190.0);

        if !armed {
            ui.label(RichText::new("Read-only").small().color(theme::MUTED));
            ui.label(
                RichText::new("Press Ctrl+Shift+W to make changes")
                    .small()
                    .color(theme::MUTED),
            );
            ui.separator();
        }

        if bulk {
            ui.label(
                RichText::new(format!("{} selected", marked.len()))
                    .small()
                    .color(theme::MUTED),
            );
            ui.separator();

            for (label, actions) in menu::bulk_for(app, view, &marked) {
                let destructive = actions
                    .iter()
                    .any(|action| action.severity() == Severity::Destructive);
                let text = if destructive {
                    RichText::new(&label).color(theme::BAD)
                } else {
                    RichText::new(&label)
                };
                if ui.add_enabled(armed, egui::Button::new(text)).clicked() {
                    app.request_actions(actions);
                    ui.close();
                }
            }
            return;
        }

        for item in menu::for_object(app, view, source) {
            match item {
                menu::Item::Separator => {
                    ui.separator();
                }
                menu::Item::Act { label, action } => {
                    let text = if action.severity() == Severity::Destructive {
                        RichText::new(&label).color(theme::BAD)
                    } else {
                        RichText::new(&label)
                    };
                    if ui.add_enabled(armed, egui::Button::new(text)).clicked() {
                        app.request_action(action);
                        ui.close();
                    }
                }
                menu::Item::Open { label, form } => {
                    if ui.add_enabled(armed, egui::Button::new(&label)).clicked() {
                        app.open_form(form.build());
                        ui.close();
                    }
                }
            }
        }
    });
}

/// The primary name of a row, for menu titles.
pub fn row_name(app: &App, view: View, source: usize) -> String {
    cell(app, view, source, 0)
}

/// A one-line spoken summary of a row, for screen readers and for Ctrl+C.
pub fn row_label(app: &App, view: View, source: usize) -> String {
    let cols = columns(view);
    (0..cols.len())
        .map(|index| {
            let value = cell(app, view, source, index);
            if view == View::Licenses && index == cols.len() - 1 {
                app.store
                    .licenses
                    .get(source)
                    .map(|sku| format!("{} of {} seats used", sku.consumed(), sku.total_seats()))
                    .unwrap_or_default()
            } else {
                format!("{}: {}", cols[index].title, value)
            }
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn usage_bar(
    ui: &mut egui::Ui,
    row_rect: Rect,
    x: f32,
    width: f32,
    sku: &SubscribedSku,
    selected: bool,
) {
    let fraction = sku.usage_fraction();
    let bar = Rect::from_min_size(
        egui::pos2(x, row_rect.center().y - 5.0),
        Vec2::new((width - 60.0).max(20.0), 10.0),
    );

    let track = if selected {
        Color32::from_white_alpha(60)
    } else {
        Color32::from_gray(224)
    };
    ui.painter().rect_filled(bar, CornerRadius::ZERO, track);

    let filled = Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction, bar.height()));
    let fill = if selected {
        Color32::WHITE
    } else {
        theme::usage_color(fraction)
    };
    ui.painter().rect_filled(filled, CornerRadius::ZERO, fill);

    ui.painter().text(
        egui::pos2(bar.right() + 6.0, row_rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{:.0}%", fraction * 100.0),
        egui::FontId::proportional(11.0),
        if selected { Color32::WHITE } else { theme::MUTED },
    );
}

fn unavailable(ui: &mut egui::Ui, reason: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Not available in this tenant").size(15.0).strong());
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            Vec2::new(520.0, 0.0),
            Layout::top_down(Align::Center),
            |ui| {
                ui.label(RichText::new(reason).color(theme::MUTED));
            },
        );
    });
}

fn error_panel(app: &mut App, ui: &mut egui::Ui, error: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("This view could not be loaded")
                .size(15.0)
                .color(theme::BAD),
        );
        ui.add_space(10.0);
        ui.allocate_ui_with_layout(
            Vec2::new(560.0, 0.0),
            Layout::top_down(Align::Center),
            |ui| {
                ui.label(RichText::new(error).monospace().color(theme::MUTED));
            },
        );
        ui.add_space(14.0);
        if ui.button("Try again (F5)").clicked() {
            app.refresh_current();
        }
    });
}

fn overview(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(super::FRIENDLY_NAME).size(17.0).strong());
    });
    ui.label(RichText::new("Tenant summary").color(theme::MUTED));
    ui.add_space(12.0);
    ui.separator();
    ui.add_space(12.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(org) = &app.store.org {
                egui::Grid::new("tenant")
                    .num_columns(2)
                    .spacing([24.0, 8.0])
                    .show(ui, |ui| {
                        pair(ui, "Tenant", org.name());
                        pair(ui, "Default domain", &org.default_domain());
                        pair(ui, "Tenant ID", &org.id);
                        pair(ui, "Tenant type", &fmt_opt(&org.tenant_type));
                        pair(ui, "Country", &fmt_opt(&org.country_letter_code));
                        pair(ui, "Created", &fmt_date(&org.created_date_time));
                        pair(
                            ui,
                            "Intune",
                            if org.has_intune() {
                                "Enabled"
                            } else {
                                "Not enabled"
                            },
                        );
                        pair(
                            ui,
                            "Verified domains",
                            &org.verified_domains.len().to_string(),
                        );
                    });
            } else {
                ui.label(RichText::new("Reading tenant details…").color(theme::MUTED));
            }

            ui.add_space(20.0);
            ui.label(RichText::new("DIRECTORY").small().color(theme::MUTED));
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                for view in [
                    View::Users,
                    View::Groups,
                    View::Roles,
                    View::Devices,
                    View::ManagedDevices,
                    View::Licenses,
                ] {
                    tile(app, ui, view);
                }
            });

            ui.add_space(20.0);
            ui.label(
                RichText::new(
                    "Choose a node in the console tree, or press Ctrl+1 to Ctrl+6. \
                     Press F1 for the full list of shortcuts.",
                )
                .color(theme::MUTED),
            );
        });
}

fn tile(app: &App, ui: &mut egui::Ui, view: View) {
    egui::Frame::group(ui.style())
        .fill(Color32::WHITE)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(150.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(view.title()).color(theme::MUTED).small());
                ui.add_space(2.0);
                let value = match app.store.count(view) {
                    Some(count) => count.to_string(),
                    None if view == View::ManagedDevices => "n/a".into(),
                    None => "—".into(),
                };
                ui.label(RichText::new(value).size(22.0).strong());
            });
        });
}

fn pair(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).color(theme::MUTED));
    ui.label(value);
    ui.end_row();
}
