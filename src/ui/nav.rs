//! The scope pane: an MMC-style console tree.
//!
//! The tree is flattened into a `Vec` every frame. That makes arrow-key
//! movement a simple index step regardless of nesting, and keeps the visible
//! order and the keyboard order identical by construction — the two drifting
//! apart is the usual way a tree stops being keyboard-navigable.

use std::collections::HashSet;

use egui::{Color32, CornerRadius, Rect, RichText, Sense, StrokeKind, Vec2};

use super::{App, Pane, View, theme};

pub struct Entry {
    pub label: &'static str,
    pub view: View,
    pub depth: usize,
    /// Key into `App::expanded` when this node has children.
    pub expandable: Option<&'static str>,
}

/// The tree in display order, honouring which parents are expanded.
pub fn entries(expanded: &HashSet<&'static str>) -> Vec<Entry> {
    let mut entries = vec![
        Entry {
            label: "Console Root",
            view: View::Overview,
            depth: 0,
            expandable: None,
        },
        Entry {
            label: "Users",
            view: View::Users,
            depth: 1,
            expandable: None,
        },
        Entry {
            label: "Groups",
            view: View::Groups,
            depth: 1,
            expandable: Some("groups"),
        },
    ];

    if expanded.contains("groups") {
        entries.push(Entry {
            label: "Directory Roles",
            view: View::Roles,
            depth: 2,
            expandable: None,
        });
    }

    entries.push(Entry {
        label: "Devices",
        view: View::Devices,
        depth: 1,
        expandable: Some("devices"),
    });

    if expanded.contains("devices") {
        entries.push(Entry {
            label: "Managed Devices",
            view: View::ManagedDevices,
            depth: 2,
            expandable: None,
        });
    }

    entries.push(Entry {
        label: "Licenses",
        view: View::Licenses,
        depth: 1,
        expandable: None,
    });

    // Workloads sit below the directory, in the order an administrator is most
    // likely to want them: mail before chat.
    entries.push(Entry {
        label: "Exchange",
        view: View::Mailboxes,
        depth: 1,
        expandable: None,
    });

    entries.push(Entry {
        label: "Teams",
        view: View::Teams,
        depth: 1,
        expandable: None,
    });

    // Both logs live under one node, because "check the logs" is one thought
    // and neither is worth a top-level slot on its own.
    entries.push(Entry {
        label: "Monitoring",
        view: View::SignIns,
        depth: 1,
        expandable: Some("monitoring"),
    });

    if expanded.contains("monitoring") {
        entries.push(Entry {
            label: "Audit Logs",
            view: View::AuditLogs,
            depth: 2,
            expandable: None,
        });
    }

    // The on-premises directory goes last, after everything the tenant itself
    // provides, under its own parent rather than beside Users and Devices.
    // Both halves of a hybrid estate answer "who is this person?", but they
    // answer it about different objects with different identifiers, and
    // merging the nodes would invite reading one list as the other. The Users
    // pane is where the two are actually joined up.
    //
    // Last also because `keys::JUMP_KEYS` promises that Ctrl+0 to Ctrl+9 are
    // positions in this tree. Inserting a node above Monitoring would shift
    // Monitoring to position eleven while Ctrl+9 went on selecting it, quietly
    // making that promise false.
    entries.push(Entry {
        label: "Directory",
        view: View::AdUsers,
        depth: 1,
        expandable: Some("directory"),
    });

    if expanded.contains("directory") {
        entries.push(Entry {
            label: "AD Computers",
            view: View::AdComputers,
            depth: 2,
            expandable: None,
        });
    }

    entries
}

/// Position of a view in the flattened tree, if it is currently visible.
pub fn index_of(expanded: &HashSet<&'static str>, view: View) -> Option<usize> {
    entries(expanded).iter().position(|e| e.view == view)
}

/// The parent key that must be expanded for `view` to be reachable.
pub fn parent_of(view: View) -> Option<&'static str> {
    match view {
        View::Roles => Some("groups"),
        View::ManagedDevices => Some("devices"),
        View::AdComputers => Some("directory"),
        View::AuditLogs => Some("monitoring"),
        _ => None,
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let focused = app.pane == Pane::Nav;

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(RichText::new("CONSOLE").small().color(theme::MUTED));
    });
    ui.add_space(4.0);

    let entries = entries(&app.expanded);
    app.nav_cursor = app.nav_cursor.min(entries.len().saturating_sub(1));

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, entry) in entries.iter().enumerate() {
                row(app, ui, entry, index, focused);
            }
        });

    if focused {
        // Ring the whole pane so it is obvious where keystrokes will land.
        ui.painter().rect_stroke(
            ui.max_rect().shrink(1.0),
            CornerRadius::ZERO,
            egui::Stroke::new(2.0, theme::FOCUS_RING),
            StrokeKind::Inside,
        );
    }
}

fn row(app: &mut App, ui: &mut egui::Ui, entry: &Entry, index: usize, pane_focused: bool) {
    let selected = app.view == entry.view;
    let cursored = pane_focused && app.nav_cursor == index;

    let full_width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(full_width, theme::ROW_HEIGHT + 2.0),
        Sense::click(),
    );

    if selected {
        let fill = if pane_focused {
            theme::ACCENT
        } else {
            theme::ACCENT_INACTIVE
        };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, ui.visuals().widgets.hovered.bg_fill);
    }

    if cursored && !selected {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            CornerRadius::ZERO,
            egui::Stroke::new(1.0, theme::FOCUS_RING),
            StrokeKind::Inside,
        );
    }

    let text_color = if selected {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };

    let indent = 8.0 + entry.depth as f32 * 14.0;
    let mut cursor = rect.left() + indent;

    // Expander triangle, clickable independently of the row itself.
    if let Some(key) = entry.expandable {
        let is_open = app.expanded.contains(key);
        let glyph_rect = Rect::from_min_size(
            egui::pos2(cursor, rect.top()),
            Vec2::new(14.0, rect.height()),
        );
        // Painted rather than typed: the default font has no glyph for ▸/▾ and
        // renders them as tofu boxes.
        draw_expander(ui, glyph_rect.center(), is_open, text_color);
        let glyph_response = ui.interact(
            glyph_rect,
            ui.id().with(("expander", entry.label)),
            Sense::click(),
        );
        if glyph_response.clicked() {
            toggle(app, key);
        }
        cursor += 16.0;
    } else if entry.depth > 0 {
        cursor += 16.0;
    }

    ui.painter().text(
        egui::pos2(cursor, rect.center().y),
        egui::Align2::LEFT_CENTER,
        entry.label,
        egui::FontId::proportional(13.0),
        text_color,
    );

    // Right-aligned count, or a spinner while the collection is loading.
    let count_color = if selected {
        Color32::from_white_alpha(200)
    } else {
        theme::MUTED
    };
    if app.store.is_loading(entry.view) {
        ui.put(
            Rect::from_min_size(
                egui::pos2(rect.right() - 26.0, rect.top() + 3.0),
                Vec2::splat(16.0),
            ),
            egui::Spinner::new().size(12.0),
        );
    } else if app.store.error(entry.view).is_some() {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "!",
            egui::FontId::proportional(13.0),
            if selected { Color32::WHITE } else { theme::BAD },
        );
    } else if let Some(count) = app.store.count(entry.view) {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(11.0),
            count_color,
        );
    }

    // Announce the row to assistive technology as a selectable tree item.
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            selected,
            entry.label,
        )
    });

    if response.clicked() {
        app.view = entry.view;
        app.nav_cursor = index;
        app.pane = Pane::Nav;
    }
    if response.double_clicked()
        && let Some(key) = entry.expandable
    {
        toggle(app, key);
    }

    context_menu(app, &response, entry.view);
}

/// The right-click menu for a scope-tree node.
///
/// Collection-level rather than per-object: a tree node has no single record
/// to act on the way a list row does, so this offers `New user…`/`New
/// group…` — mirroring the toolbar button — and `Refresh`, for whichever node
/// was clicked rather than only the one currently selected.
fn context_menu(app: &mut App, response: &egui::Response, view: View) {
    let armed = app.write_mode.is_armed();

    egui::Popup::menu(response)
        .open_memory(if response.secondary_clicked() {
            Some(egui::SetOpenCommand::Bool(true))
        } else if response.clicked() {
            // A plain click dismisses an open menu, same as the list rows.
            Some(egui::SetOpenCommand::Bool(false))
        } else {
            None
        })
        .at_pointer_fixed()
        .show(|ui| {
            ui.set_min_width(170.0);

            // One source for what a node can create, shared with the toolbar,
            // so the tree and the button cannot come to disagree about which
            // nodes offer what.
            if let Some((label, disabled_hover)) = super::menu::creatable(view) {
                let clicked = ui
                    .add_enabled(armed, egui::Button::new(label))
                    .on_disabled_hover_text(disabled_hover)
                    .clicked();
                if clicked {
                    match view {
                        View::Groups => app.new_group(),
                        _ => app.new_user(),
                    }
                    ui.close();
                }
                ui.separator();
            }

            let refresh_label = if view == View::Overview {
                "Refresh all"
            } else {
                "Refresh"
            };
            if ui.button(refresh_label).clicked() {
                app.refresh_view(view);
                ui.close();
            }

            // Export belongs on every node that holds a list. It is the one
            // thing worth doing with the read-only collections, and it was
            // reachable only from the keyboard.
            if view != View::Overview {
                let exportable = app.store.count(view).is_some_and(|rows| rows > 0);
                if ui
                    .add_enabled(exportable, egui::Button::new("Export…"))
                    .on_disabled_hover_text("There is nothing loaded here to export")
                    .clicked()
                {
                    // Move to the node first: the export writes whatever the
                    // current view holds, and exporting the previous node's
                    // rows from this one's menu would be a quiet mis-fire.
                    app.go_to(view);
                    app.export(super::export::Format::Csv);
                    ui.close();
                }
            }
        });
}

/// A small solid triangle: pointing right when collapsed, down when expanded.
fn draw_expander(ui: &egui::Ui, center: egui::Pos2, open: bool, color: Color32) {
    const SIZE: f32 = 4.0;
    let points = if open {
        vec![
            egui::pos2(center.x - SIZE, center.y - SIZE * 0.6),
            egui::pos2(center.x + SIZE, center.y - SIZE * 0.6),
            egui::pos2(center.x, center.y + SIZE * 0.8),
        ]
    } else {
        vec![
            egui::pos2(center.x - SIZE * 0.6, center.y - SIZE),
            egui::pos2(center.x - SIZE * 0.6, center.y + SIZE),
            egui::pos2(center.x + SIZE * 0.8, center.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

fn toggle(app: &mut App, key: &'static str) {
    if !app.expanded.remove(key) {
        app.expanded.insert(key);
    }
}

/// Move the tree cursor and select whatever it lands on. MMC selects on
/// movement rather than requiring a separate activation, and screen reader
/// users expect the same from a tree.
pub fn move_cursor(app: &mut App, delta: i64) {
    let entries = entries(&app.expanded);
    if entries.is_empty() {
        return;
    }
    let last = entries.len() as i64 - 1;
    let next = (app.nav_cursor as i64 + delta).clamp(0, last) as usize;
    app.nav_cursor = next;
    app.view = entries[next].view;
}

/// Right-arrow: expand the node, or step into its first child if already open.
pub fn expand_or_enter(app: &mut App) {
    let entries = entries(&app.expanded);
    let Some(entry) = entries.get(app.nav_cursor) else {
        return;
    };
    match entry.expandable {
        Some(key) if !app.expanded.contains(key) => {
            app.expanded.insert(key);
        }
        Some(_) => move_cursor(app, 1),
        None => {}
    }
}

/// Left-arrow: collapse the node, or step out to its parent.
pub fn collapse_or_leave(app: &mut App) {
    let entries = entries(&app.expanded);
    let Some(entry) = entries.get(app.nav_cursor) else {
        return;
    };
    match entry.expandable {
        Some(key) if app.expanded.contains(key) => {
            app.expanded.remove(key);
        }
        _ => {
            if entry.depth > 0
                && let Some(parent) = parent_of(entry.view)
                && let Some(index) = entries.iter().position(|e| e.expandable == Some(parent))
            {
                app.nav_cursor = index;
                app.view = entries[index].view;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(keys: &[&'static str]) -> HashSet<&'static str> {
        keys.iter().copied().collect()
    }

    /// Every parent key the tree knows about.
    const ALL_PARENTS: &[&str] = &["groups", "devices", "directory", "monitoring"];

    #[test]
    fn collapsed_tree_hides_children() {
        let tree = entries(&expanded(&[]));
        let labels: Vec<_> = tree.iter().map(|e| e.label).collect();
        assert_eq!(
            labels,
            [
                "Console Root",
                "Users",
                "Groups",
                "Devices",
                "Licenses",
                "Exchange",
                "Teams",
                "Monitoring",
                "Directory",
            ]
        );
    }

    #[test]
    fn expanding_reveals_children_in_order() {
        let tree = entries(&expanded(ALL_PARENTS));
        let labels: Vec<_> = tree.iter().map(|e| e.label).collect();
        assert_eq!(
            labels,
            [
                "Console Root",
                "Users",
                "Groups",
                "Directory Roles",
                "Devices",
                "Managed Devices",
                "Licenses",
                "Exchange",
                "Teams",
                "Monitoring",
                "Audit Logs",
                "Directory",
                "AD Computers",
            ]
        );
    }

    #[test]
    fn index_of_tracks_expansion() {
        // Roles are unreachable while their parent is collapsed.
        assert_eq!(index_of(&expanded(&[]), View::Roles), None);
        assert_eq!(index_of(&expanded(&["groups"]), View::Roles), Some(3));
        assert_eq!(index_of(&expanded(&[]), View::AuditLogs), None);
    }

    /// `keys::JUMP_KEYS` documents Ctrl+0..9 as *positions* in this tree, which
    /// is only true for as long as the first ten rows stay where they are.
    /// Inserting a node above Monitoring silently broke this once already.
    #[test]
    fn the_jump_keys_still_line_up_with_the_first_ten_rows() {
        let tree = entries(&expanded(ALL_PARENTS));
        for (index, (_, view)) in super::super::keys::JUMP_KEYS.iter().enumerate() {
            assert_eq!(
                tree[index].view, *view,
                "Ctrl+{index} selects {view:?}, but position {index} in the tree is {} \
                 — either the tree moved or the shortcut did",
                tree[index].label
            );
        }
    }

    #[test]
    fn children_know_their_parent() {
        assert_eq!(parent_of(View::Roles), Some("groups"));
        assert_eq!(parent_of(View::ManagedDevices), Some("devices"));
        assert_eq!(parent_of(View::AdComputers), Some("directory"));
        assert_eq!(parent_of(View::AuditLogs), Some("monitoring"));
        assert_eq!(parent_of(View::Users), None);
    }

    #[test]
    fn every_view_except_overview_appears_when_fully_expanded() {
        let tree = entries(&expanded(ALL_PARENTS));
        for view in [
            View::Users,
            View::Groups,
            View::Roles,
            View::Devices,
            View::ManagedDevices,
            View::Licenses,
            View::Mailboxes,
            View::Teams,
            View::SignIns,
            View::AuditLogs,
            View::AdUsers,
            View::AdComputers,
        ] {
            assert!(
                tree.iter().any(|e| e.view == view),
                "{view:?} is unreachable from the scope tree"
            );
        }
    }

    /// A nested node reached by a shortcut has its parent expanded first, using
    /// [`parent_of`]. A child whose parent key were wrong would jump to a node
    /// the tree never reveals, leaving the cursor somewhere else entirely.
    #[test]
    fn every_nested_node_names_a_real_parent() {
        let tree = entries(&expanded(ALL_PARENTS));
        for entry in tree.iter().filter(|entry| entry.depth > 1) {
            let parent = parent_of(entry.view).unwrap_or_else(|| {
                panic!("{} is nested but names no parent", entry.label)
            });
            assert!(
                tree.iter().any(|e| e.expandable == Some(parent)),
                "{} names a parent key nothing expands: {parent}",
                entry.label
            );
        }
    }
}
