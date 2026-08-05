//! What can be done to an object, in one place.
//!
//! The details pane draws these as a button bar and the result list draws them
//! as a right-click menu. Both render from [`for_object`], so the two surfaces
//! cannot drift — an action added here appears in both, and one removed
//! disappears from both. Duplicating the lists per surface is how a context
//! menu ends up quietly offering something the buttons no longer do.
//!
//! Nothing here executes anything. Items carry an [`Action`] or a
//! [`FormFactory`]; the caller routes them through `request_actions` or
//! `open_form`, which is where the write gate and confirmation live.

use super::forms::{Form, UserFields};
use super::{App, View};
use crate::graph::Fetch;
use crate::graph::actions::{Action, DeviceOp, MemberRole, Severity};

/// Which form an item opens, built lazily so the store is not borrowed while
/// the menu is still being drawn from it.
pub enum FormFactory {
    EditUser {
        id: String,
        name: String,
        fields: Box<UserFields>,
    },
    ResetPassword {
        id: String,
        name: String,
    },
    EditGroup {
        id: String,
        name: String,
        display_name: String,
        description: String,
    },
    CreateGroup,
    PickLicense {
        user_id: String,
        user_name: String,
        assign: bool,
    },
    PickGroup {
        member_id: String,
        member_name: String,
        add: bool,
    },
    PickGroupMember {
        group_id: String,
        group_name: String,
        role: MemberRole,
        add: bool,
    },
    RenameDevice {
        id: String,
        name: String,
    },
}

impl FormFactory {
    pub fn build(self) -> Form {
        match self {
            FormFactory::EditUser { id, name, fields } => Form::EditUser {
                id,
                name,
                fields: *fields,
            },
            FormFactory::ResetPassword { id, name } => Form::reset_password(id, name),
            FormFactory::EditGroup {
                id,
                name,
                display_name,
                description,
            } => Form::EditGroup {
                id,
                name,
                display_name,
                description,
            },
            FormFactory::CreateGroup => Form::CreateGroup {
                display_name: String::new(),
                mail_nickname: String::new(),
                description: String::new(),
                unified: false,
            },
            FormFactory::PickLicense {
                user_id,
                user_name,
                assign,
            } => Form::PickLicense {
                user_id,
                user_name,
                assign,
                filter: String::new(),
                cursor: 0,
                focused: false,
            },
            FormFactory::PickGroup {
                member_id,
                member_name,
                add,
            } => Form::PickGroup {
                member_id,
                member_name,
                role: MemberRole::Member,
                add,
                filter: String::new(),
                cursor: 0,
                focused: false,
            },
            FormFactory::PickGroupMember {
                group_id,
                group_name,
                role,
                add,
            } => Form::PickMember {
                group_id,
                group_name,
                role,
                add,
                filter: String::new(),
                cursor: 0,
                focused: false,
            },
            FormFactory::RenameDevice { id, name } => Form::RenameDevice {
                id,
                name: name.clone(),
                new_name: name,
            },
        }
    }
}

/// One entry in a menu or button bar.
pub enum Item {
    /// Runs immediately, subject to confirmation.
    Act { label: String, action: Action },
    /// Opens a form to gather more input first. Labelled with an ellipsis.
    Open { label: String, form: FormFactory },
    /// A visual break; ignored by the button bar.
    Separator,
}

impl Item {
    fn act(label: &str, action: Action) -> Self {
        Item::Act {
            label: label.into(),
            action,
        }
    }

    fn open(label: &str, form: FormFactory) -> Self {
        Item::Open {
            label: label.into(),
            form,
        }
    }
}

/// Everything that can be done to the object at `source` in `view`.
pub fn for_object(app: &App, view: View, source: usize) -> Vec<Item> {
    match view {
        View::Users => user_items(app, source),
        View::Groups => group_items(app, source),
        View::Devices => device_items(app, source),
        View::ManagedDevices => managed_items(app, source),
        // Roles and licences are read-only surfaces: role assignment is done
        // by editing the role's members, and licences by editing a user.
        View::Roles | View::Licenses | View::Overview => Vec::new(),
    }
}

fn user_items(app: &App, source: usize) -> Vec<Item> {
    let Some(user) = app.store.users.get(source) else {
        return Vec::new();
    };
    let id = user.id.clone();
    let name = user.name().to_string();
    let enabled = user.account_enabled.unwrap_or(false);

    vec![
        Item::act(
            if enabled {
                "Disable account"
            } else {
                "Enable account"
            },
            Action::SetUserEnabled {
                id: id.clone(),
                name: name.clone(),
                enabled: !enabled,
            },
        ),
        Item::open(
            "Edit…",
            FormFactory::EditUser {
                id: id.clone(),
                name: name.clone(),
                fields: Box::new(UserFields {
                    job_title: blank(&user.job_title),
                    department: blank(&user.department),
                    office_location: blank(&user.office_location),
                    mobile_phone: blank(&user.mobile_phone),
                    usage_location: blank(&user.usage_location),
                }),
            },
        ),
        Item::open(
            "Reset password…",
            FormFactory::ResetPassword {
                id: id.clone(),
                name: name.clone(),
            },
        ),
        Item::Separator,
        Item::open(
            "Assign licence…",
            FormFactory::PickLicense {
                user_id: id.clone(),
                user_name: name.clone(),
                assign: true,
            },
        ),
        Item::open(
            "Remove licence…",
            FormFactory::PickLicense {
                user_id: id.clone(),
                user_name: name.clone(),
                assign: false,
            },
        ),
        Item::Separator,
        Item::open(
            "Add to group…",
            FormFactory::PickGroup {
                member_id: id.clone(),
                member_name: name.clone(),
                add: true,
            },
        ),
        Item::open(
            "Remove from group…",
            FormFactory::PickGroup {
                member_id: id.clone(),
                member_name: name.clone(),
                add: false,
            },
        ),
        Item::Separator,
        Item::act("Delete", Action::DeleteUser { id, name }),
    ]
}

fn group_items(app: &App, source: usize) -> Vec<Item> {
    let Some(group) = app.store.groups.get(source) else {
        return Vec::new();
    };
    let id = group.id.clone();
    let name = group.name().to_string();

    let mut items = vec![
        Item::open(
            "Edit…",
            FormFactory::EditGroup {
                id: id.clone(),
                name: name.clone(),
                display_name: name.clone(),
                description: blank(&group.description),
            },
        ),
        Item::open("New group…", FormFactory::CreateGroup),
    ];

    // Entra recomputes dynamic membership from the rule, so a hand-made change
    // would simply be reverted. Better to not offer it than to have it undone.
    if group.membership() != "Dynamic" {
        items.push(Item::Separator);
        for (label, role, add) in [
            ("Add member…", MemberRole::Member, true),
            ("Remove member…", MemberRole::Member, false),
            ("Add owner…", MemberRole::Owner, true),
            ("Remove owner…", MemberRole::Owner, false),
        ] {
            items.push(Item::open(
                label,
                FormFactory::PickGroupMember {
                    group_id: id.clone(),
                    group_name: name.clone(),
                    role,
                    add,
                },
            ));
        }
    }

    items.push(Item::Separator);
    items.push(Item::act("Delete", Action::DeleteGroup { id, name }));
    items
}

fn device_items(app: &App, source: usize) -> Vec<Item> {
    let Some(device) = app.store.devices.get(source) else {
        return Vec::new();
    };
    let id = device.id.clone();
    let name = device.name().to_string();
    let enabled = device.account_enabled.unwrap_or(false);

    vec![
        Item::act(
            if enabled { "Disable" } else { "Enable" },
            Action::SetDeviceEnabled {
                id: id.clone(),
                name: name.clone(),
                enabled: !enabled,
            },
        ),
        Item::Separator,
        Item::act("Delete", Action::DeleteDevice { id, name }),
    ]
}

fn managed_items(app: &App, source: usize) -> Vec<Item> {
    let devices = match &app.store.managed {
        Some(Fetch::Ready(devices)) => devices,
        _ => return Vec::new(),
    };
    let Some(device) = devices.get(source) else {
        return Vec::new();
    };
    let id = device.id.clone();
    let name = device.name().to_string();

    let op = |label: &str, op: DeviceOp| {
        Item::act(
            label,
            Action::ManagedDevice {
                id: id.clone(),
                name: name.clone(),
                op,
            },
        )
    };

    vec![
        op("Sync", DeviceOp::Sync),
        op("Restart", DeviceOp::Restart),
        op("Remote lock", DeviceOp::RemoteLock),
        Item::open(
            "Rename…",
            FormFactory::RenameDevice {
                id: id.clone(),
                name: name.clone(),
            },
        ),
        Item::Separator,
        op("Autopilot reset", DeviceOp::AutopilotReset),
        op("Retire", DeviceOp::Retire),
        op("Wipe", DeviceOp::Wipe),
        Item::Separator,
        op("Delete Intune record", DeviceOp::Delete),
    ]
}

/// Bulk actions applicable to every object in `marked`.
///
/// Only actions that make sense for the whole set are offered: a bulk button
/// that half-fails is worse than no bulk button.
pub fn bulk_for(app: &App, view: View, marked: &[usize]) -> Vec<(String, Vec<Action>)> {
    let mut out: Vec<(String, Vec<Action>)> = Vec::new();

    let mut push = |label: &str, actions: Vec<Action>| {
        if actions.len() == marked.len() && !actions.is_empty() {
            out.push((label.to_string(), actions));
        }
    };

    match view {
        View::Users => {
            for (label, enabled) in [("Enable", true), ("Disable", false)] {
                push(
                    label,
                    marked
                        .iter()
                        .filter_map(|source| {
                            let user = app.store.users.get(*source)?;
                            Some(Action::SetUserEnabled {
                                id: user.id.clone(),
                                name: user.name().to_string(),
                                enabled,
                            })
                        })
                        .collect(),
                );
            }
            push(
                "Delete",
                marked
                    .iter()
                    .filter_map(|source| {
                        let user = app.store.users.get(*source)?;
                        Some(Action::DeleteUser {
                            id: user.id.clone(),
                            name: user.name().to_string(),
                        })
                    })
                    .collect(),
            );
        }

        View::Groups => {
            push(
                "Delete",
                marked
                    .iter()
                    .filter_map(|source| {
                        let group = app.store.groups.get(*source)?;
                        Some(Action::DeleteGroup {
                            id: group.id.clone(),
                            name: group.name().to_string(),
                        })
                    })
                    .collect(),
            );
        }

        View::Devices => {
            for (label, enabled) in [("Enable", true), ("Disable", false)] {
                push(
                    label,
                    marked
                        .iter()
                        .filter_map(|source| {
                            let device = app.store.devices.get(*source)?;
                            Some(Action::SetDeviceEnabled {
                                id: device.id.clone(),
                                name: device.name().to_string(),
                                enabled,
                            })
                        })
                        .collect(),
                );
            }
            push(
                "Delete",
                marked
                    .iter()
                    .filter_map(|source| {
                        let device = app.store.devices.get(*source)?;
                        Some(Action::DeleteDevice {
                            id: device.id.clone(),
                            name: device.name().to_string(),
                        })
                    })
                    .collect(),
            );
        }

        View::ManagedDevices => {
            let devices = match &app.store.managed {
                Some(Fetch::Ready(devices)) => devices.clone(),
                _ => return out,
            };
            for (label, op) in [
                ("Sync", DeviceOp::Sync),
                ("Restart", DeviceOp::Restart),
                ("Retire", DeviceOp::Retire),
                ("Wipe", DeviceOp::Wipe),
            ] {
                push(
                    label,
                    marked
                        .iter()
                        .filter_map(|source| {
                            let device = devices.get(*source)?;
                            Some(Action::ManagedDevice {
                                id: device.id.clone(),
                                name: device.name().to_string(),
                                op: op.clone(),
                            })
                        })
                        .collect(),
                );
            }
        }

        View::Roles | View::Licenses | View::Overview => {}
    }

    out
}

/// An optional string as editable text: absent becomes empty, not an em dash.
fn blank(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

// ---- The keyboard route to the same menu ------------------------------------

/// The actions menu as a keyboard-driven dialog.
///
/// A right-click menu is unreachable without a pointer, so `Shift+F10` — the
/// long-standing convention for "open the context menu for what is focused" —
/// opens this instead. It lists exactly what [`for_object`] and [`bulk_for`]
/// offer, navigated with the arrow keys and filtered by typing.
pub struct Palette {
    /// Entries for a single object, or empty when this is a bulk palette.
    items: Vec<Item>,
    /// Entries for a ticked set.
    bulk: Vec<(String, Vec<Action>)>,
    /// What the palette is acting on, shown in the title.
    pub subject: String,
    filter: String,
    cursor: usize,
    focused: bool,
}

/// What the palette produced this frame.
pub enum Chosen {
    Pending,
    Cancelled,
    Act(Vec<Action>),
    Open(Box<FormFactory>),
}

impl Palette {
    pub fn for_single(items: Vec<Item>, subject: String) -> Self {
        Self {
            items,
            bulk: Vec::new(),
            subject,
            filter: String::new(),
            cursor: 0,
            focused: false,
        }
    }

    pub fn for_bulk(bulk: Vec<(String, Vec<Action>)>, count: usize) -> Self {
        Self {
            items: Vec::new(),
            bulk,
            subject: format!("{count} selected"),
            filter: String::new(),
            cursor: 0,
            focused: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items
            .iter()
            .all(|item| matches!(item, Item::Separator))
            && self.bulk.is_empty()
    }

    /// Labels that survive the filter, paired with their index in the source.
    fn matching(&self) -> Vec<(usize, String, bool)> {
        let needle = self.filter.trim().to_lowercase();
        let keep = |label: &str| {
            needle.is_empty() || label.to_lowercase().contains(needle.as_str())
        };

        if !self.bulk.is_empty() {
            return self
                .bulk
                .iter()
                .enumerate()
                .filter(|(_, (label, _))| keep(label))
                .map(|(index, (label, actions))| {
                    let destructive = actions
                        .iter()
                        .any(|a| a.severity() == Severity::Destructive);
                    (index, label.clone(), destructive)
                })
                .collect();
        }

        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                // Separators are structure, not choices; they never take focus.
                Item::Separator => None,
                Item::Act { label, action } => Some((
                    index,
                    label.clone(),
                    action.severity() == Severity::Destructive,
                )),
                Item::Open { label, .. } => Some((index, label.clone(), false)),
            })
            .filter(|(_, label, _)| keep(label))
            .collect()
    }
}

/// Draw the palette and report what the operator chose.
pub fn palette(ctx: &egui::Context, palette: &mut Palette, armed: bool) -> Chosen {
    use egui::{Key, RichText};

    let mut chosen = Chosen::Pending;
    let matching = palette.matching();

    // Keep the cursor on a real entry as the filter narrows.
    if palette.cursor >= matching.len() {
        palette.cursor = matching.len().saturating_sub(1);
    }

    let response = egui::Modal::new(egui::Id::new("actions-palette")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new("Actions").size(15.0).strong());
        ui.label(RichText::new(&palette.subject).small().color(super::theme::MUTED));
        ui.add_space(10.0);

        if !armed {
            ui.label(
                RichText::new("Read-only — press Ctrl+Shift+W to make changes")
                    .small()
                    .color(super::theme::WARN),
            );
            ui.add_space(8.0);
        }

        // Focus starts in the filter so typing narrows immediately, while the
        // arrow keys still drive the list below.
        let field = ui.add(
            egui::TextEdit::singleline(&mut palette.filter)
                .desired_width(f32::INFINITY)
                .hint_text("Type to filter"),
        );
        if !palette.focused {
            field.request_focus();
            palette.focused = true;
        }
        ui.add_space(8.0);

        if matching.is_empty() {
            ui.label(
                RichText::new("Nothing available here.")
                    .color(super::theme::MUTED),
            );
        }

        egui::ScrollArea::vertical()
            .max_height(320.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (position, (source_index, label, destructive)) in
                    matching.iter().enumerate()
                {
                    let highlighted = position == palette.cursor;
                    let text = if *destructive {
                        RichText::new(label).color(super::theme::BAD)
                    } else {
                        RichText::new(label)
                    };

                    let response = ui
                        .add_enabled_ui(armed, |ui| ui.selectable_label(highlighted, text))
                        .inner;
                    if highlighted {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        chosen = pick(palette, *source_index);
                    }
                }
            });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(
            RichText::new("↑ ↓ to move · Enter to run · Esc to close")
                .small()
                .color(super::theme::MUTED),
        );

        // Arrow keys work while the filter box has focus, which is what makes
        // type-then-Enter a single fluid motion.
        let (up, down, enter, escape) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });

        if down && !matching.is_empty() {
            palette.cursor = (palette.cursor + 1).min(matching.len() - 1);
        }
        if up {
            palette.cursor = palette.cursor.saturating_sub(1);
        }
        if enter && armed && let Some((source_index, _, _)) = matching.get(palette.cursor) {
            chosen = pick(palette, *source_index);
        }
        if escape {
            chosen = Chosen::Cancelled;
        }
    });

    if matches!(chosen, Chosen::Pending) && response.should_close() {
        chosen = Chosen::Cancelled;
    }

    chosen
}

/// Take the chosen entry out of the palette.
fn pick(palette: &mut Palette, index: usize) -> Chosen {
    if !palette.bulk.is_empty() {
        return match palette.bulk.get(index) {
            Some((_, actions)) => Chosen::Act(actions.clone()),
            None => Chosen::Pending,
        };
    }

    // Swap a placeholder in so the item can be moved out by value.
    match std::mem::replace(&mut palette.items[index], Item::Separator) {
        Item::Act { action, .. } => Chosen::Act(vec![action]),
        Item::Open { form, .. } => Chosen::Open(Box::new(form)),
        Item::Separator => Chosen::Pending,
    }
}

