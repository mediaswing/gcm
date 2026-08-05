//! The keyboard shortcut overlay (F1).
//!
//! Kept in step with [`super::keys`] by hand — if you add a shortcut there, add
//! its row here. An undiscoverable shortcut is barely a feature.

use egui::RichText;

use super::theme;

/// `COMMAND` is Ctrl everywhere except macOS, where egui maps it to Cmd.
#[cfg(target_os = "macos")]
const MOD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const MOD: &str = "Ctrl";

struct Group {
    title: &'static str,
    rows: &'static [(&'static str, &'static str)],
}

const GROUPS: &[Group] = &[
    Group {
        title: "Moving between panes",
        rows: &[
            ("F6", "Next pane (scope → results → details)"),
            ("Shift+F6", "Previous pane"),
            ("{MOD}+Tab", "Next pane"),
            ("Esc", "Clear the filter, then return to the scope tree"),
        ],
    },
    Group {
        title: "Scope tree",
        rows: &[
            ("↑ ↓", "Move between nodes"),
            ("→", "Expand, or step into the first child"),
            ("←", "Collapse, or step out to the parent"),
            ("Enter", "Move focus to the results"),
            ("Home / End", "First or last node"),
        ],
    },
    Group {
        title: "Results",
        rows: &[
            ("↑ ↓", "Move the selection"),
            ("PgUp / PgDn", "Move by a screenful"),
            ("Home / End", "First or last row"),
            ("Enter", "Move focus to the details pane"),
            ("←", "Back to the scope tree"),
            ("{MOD}+C", "Copy the selected row"),
            ("Space", "Tick the row for a bulk action, and move down"),
            ("{MOD}+click", "Tick a row without moving the cursor"),
            ("{MOD}+A", "Tick every row the filter shows"),
            ("{MOD}+Shift+A", "Clear the ticks"),
        ],
    },
    Group {
        title: "Jumping to a view",
        rows: &[
            ("{MOD}+0", "Console Root"),
            ("{MOD}+1", "Users"),
            ("{MOD}+2", "Groups"),
            ("{MOD}+3", "Directory Roles"),
            ("{MOD}+4", "Devices"),
            ("{MOD}+5", "Managed Devices"),
            ("{MOD}+6", "Licenses"),
            ("{MOD}+7", "Exchange mailboxes"),
            ("{MOD}+8", "Teams"),
            ("{MOD}+9", "Sign-in logs"),
        ],
    },
    Group {
        title: "Making changes",
        rows: &[
            ("{MOD}+Shift+W", "Turn write mode on or off"),
            ("{MOD}+N", "Create a user account"),
            (
                "Shift+F10",
                "Open the actions menu for the selection (same as right-click)",
            ),
            ("{MOD}+Enter", "Open the actions menu, where F10 is taken"),
            ("Right-click", "The same menu, by pointer"),
        ],
    },
    Group {
        title: "Inside a dialog",
        rows: &[
            ("↑ ↓", "Move through a list of choices"),
            ("Type", "Filter the choices"),
            ("Enter", "Choose, save, or confirm — once what you typed matches"),
            ("Tab", "Move between fields and buttons"),
            ("Esc", "Cancel without changing anything"),
        ],
    },
    Group {
        title: "Everything else",
        rows: &[
            ("{MOD}+F", "Focus the filter box"),
            ("{MOD}+E", "Export the current view to CSV"),
            ("{MOD}+Shift+E", "Export the current view to JSON"),
            ("{MOD}+I", "Import a CSV, with a preview before anything runs"),
            ("F5", "Refresh the current view"),
            ("{MOD}+Shift+R", "Refresh every view"),
            ("{MOD}+D", "Show or hide the details pane"),
            ("F1", "Show or hide this help"),
            ("Tab", "Move between toolbar controls"),
        ],
    },
];

pub fn show(ctx: &egui::Context, open: &mut bool) {
    let mut still_open = *open;
    egui::Window::new("Keyboard shortcuts")
        .open(&mut still_open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(
                RichText::new(
                    "Every part of Graphical Cloud Manager is reachable without a mouse.",
                )
                .color(theme::MUTED),
            );
            ui.add_space(10.0);

            egui::ScrollArea::vertical()
                .max_height(460.0)
                .show(ui, |ui| {
                    for group in GROUPS {
                        ui.label(RichText::new(group.title).strong());
                        ui.add_space(4.0);
                        egui::Grid::new(group.title)
                            .num_columns(2)
                            .spacing([20.0, 5.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (keys, description) in group.rows {
                                    ui.label(
                                        RichText::new(keys.replace("{MOD}", MOD))
                                            .monospace()
                                            .strong(),
                                    );
                                    ui.label(*description);
                                    ui.end_row();
                                }
                            });
                        ui.add_space(12.0);
                    }
                });

            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Press Esc or F1 to close.").color(theme::MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The one place somebody looks when something is wrong is
                    // the help window, so the diagnostic log is named here.
                    if ui
                        .small_button("Open the error log")
                        .on_hover_text(crate::errorlog::log_path().display().to_string())
                        .clicked()
                    {
                        let _ = open::that_detached(crate::errorlog::log_path());
                    }
                });
            });
        });
    *open = still_open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_has_a_description() {
        for group in GROUPS {
            assert!(!group.rows.is_empty(), "{} has no rows", group.title);
            for (keys, description) in group.rows {
                assert!(!keys.is_empty());
                assert!(!description.is_empty(), "{keys} has no description");
            }
        }
    }

    #[test]
    fn modifier_placeholder_is_substituted() {
        let rendered = "{MOD}+F".replace("{MOD}", MOD);
        assert!(!rendered.contains("{MOD}"));
        assert!(rendered.ends_with("+F"));
    }

    /// This table and [`super::keys`] are kept in step by hand, which is exactly
    /// the arrangement that drifts. A shortcut that exists but is not documented
    /// is barely a feature; one that is documented but does not exist is worse.
    #[test]
    fn every_jump_shortcut_is_documented() {
        let jumps = GROUPS
            .iter()
            .find(|group| group.title == "Jumping to a view")
            .expect("the jump group must exist");

        assert_eq!(
            jumps.rows.len(),
            super::super::keys::JUMP_KEYS.len(),
            "the help table lists {} jump shortcuts but {} are bound",
            jumps.rows.len(),
            super::super::keys::JUMP_KEYS.len()
        );

        for (index, (keys, _)) in jumps.rows.iter().enumerate() {
            assert_eq!(
                *keys,
                format!("{{MOD}}+{index}"),
                "jump shortcuts must be documented in the order they are bound"
            );
        }
    }
}
