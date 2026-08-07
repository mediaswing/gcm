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

use super::forms::{AdFields, Form, UserFields};
use super::{App, View};
use crate::graph::Fetch;
use crate::graph::actions::{Action, AutoReplySpec, DeviceOp, MemberRole, Severity, TeamOp};
use crate::graph::models::AutomaticReplies;
use crate::ldap::actions::DirectoryAction;

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
    CreateUser,
    /// Edit somebody's automatic replies, seeded with whatever is set now.
    AutomaticReplies {
        id: String,
        name: String,
        current: Box<Option<AutomaticReplies>>,
    },
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
    /// Reset an on-premises password.
    AdResetPassword {
        dn: String,
        name: String,
    },
    /// Edit an on-premises account, seeded with what is on it now.
    AdEditUser {
        dn: String,
        name: String,
        fields: Box<AdFields>,
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
            FormFactory::CreateUser => Form::create_user(),
            FormFactory::AutomaticReplies { id, name, current } => {
                Form::automatic_replies(id, name, *current)
            }
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
            FormFactory::AdResetPassword { dn, name } => Form::ad_reset_password(dn, name),
            FormFactory::AdEditUser { dn, name, fields } => Form::AdEditUser {
                dn,
                name,
                // Two copies: one the operator edits, one to diff against, so
                // only genuinely changed attributes are written.
                fields: fields.clone(),
                original: fields,
            },
        }
    }
}

/// Something the console does to itself rather than to the tenant.
///
/// These exist because every one of them already had a keyboard shortcut and
/// no other way in. A view that Graph will not let anyone change — a licence, a
/// past sign-in — still has plenty somebody wants to *do* with it: copy the
/// row, tick a few, export the list, read it again. Leaving those off the menu
/// is what made half the console's right-click menus look broken.
///
/// Never gated on write mode, because none of it changes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewCommand {
    CopyRow,
    ToggleMark,
    MarkAllFiltered,
    ClearMarks,
    ExportCsv,
    ExportJson,
    Refresh,
}

/// One entry in a menu or button bar.
pub enum Item {
    /// Runs immediately, subject to confirmation.
    Act { label: String, action: Action },
    /// The same, but against the on-premises domain rather than the tenant.
    ///
    /// A separate variant rather than a shared one because the two go to
    /// different places and carry different types; everything downstream —
    /// the write gate, the confirmation, the audit log — treats them alike.
    ActDirectory {
        label: String,
        action: Box<DirectoryAction>,
    },
    /// Opens a form to gather more input first. Labelled with an ellipsis.
    Open { label: String, form: FormFactory },
    /// Acts on the console rather than the tenant, and is never disabled.
    View {
        label: String,
        /// Shown greyed at the right of the entry, the way a menu names its
        /// accelerator — these all had shortcuts before they had entries.
        shortcut: &'static str,
        command: ViewCommand,
    },
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

    fn act_directory(label: &str, action: DirectoryAction) -> Self {
        Item::ActDirectory {
            label: label.into(),
            action: Box::new(action),
        }
    }

    fn open(label: &str, form: FormFactory) -> Self {
        Item::Open {
            label: label.into(),
            form,
        }
    }

    fn view(label: &str, shortcut: &'static str, command: ViewCommand) -> Self {
        Item::View {
            label: label.into(),
            shortcut,
            command,
        }
    }

    /// True for the entries that write to the tenant, and so need write mode.
    pub fn needs_write_mode(&self) -> bool {
        matches!(
            self,
            Item::Act { .. } | Item::ActDirectory { .. } | Item::Open { .. }
        )
    }

    /// True for the entries that bring a new object into existence rather than
    /// acting on the selected one.
    ///
    /// The right-click menu wants these — creating an account is an errand
    /// somebody arrives at the Users node to run, and it does not depend on
    /// what is selected. A toolbar that already carries a New button one line
    /// above does not.
    pub fn creates_object(&self) -> bool {
        matches!(
            self,
            Item::Open {
                form: FormFactory::CreateUser | FormFactory::CreateGroup,
                ..
            }
        )
    }
}

/// What can be *created* from a given node, for the toolbar and the tree menu.
///
/// Only two things in the console can be brought into existence, so this is
/// deliberately narrow. It returns `None` everywhere else rather than falling
/// back to "New user…", which is what made the button lie about what it would
/// do on nine of the eleven nodes.
pub fn creatable(view: View) -> Option<(&'static str, &'static str)> {
    match view {
        View::Users => Some((
            "New user…",
            "Enable write mode (Ctrl+Shift+W) to create an account",
        )),
        View::Groups => Some((
            "New group…",
            "Enable write mode (Ctrl+Shift+W) to create a group",
        )),
        _ => None,
    }
}

/// Everything that can be done to the object at `source` in `view`.
pub fn for_object(app: &App, view: View, source: usize) -> Vec<Item> {
    match view {
        View::Users => user_items(app, source),
        View::Groups => group_items(app, source),
        View::Devices => device_items(app, source),
        View::ManagedDevices => managed_items(app, source),
        View::Teams => team_items(app, source),
        View::Mailboxes => mailbox_items(app, source),
        View::AdUsers => ad_user_items(app, source),
        View::AdComputers => ad_computer_items(app, source),
        // Roles and licences are read-only surfaces: role assignment is done
        // by editing the role's members, and licences by editing a user.
        //
        // The logs are read-only in a stronger sense — there is nothing in
        // Graph that could change a past sign-in, and there should not be.
        View::Roles | View::Licenses | View::Overview | View::SignIns | View::AuditLogs => {
            Vec::new()
        }
    }
}

/// The rows of a collection that has loaded, or nothing.
fn ready<T>(fetch: &Option<Fetch<std::sync::Arc<Vec<T>>>>) -> &[T] {
    match fetch {
        Some(Fetch::Ready(items)) => items.as_slice(),
        _ => &[],
    }
}

/// What can be done to an on-premises account.
///
/// Everything here is subject to two gates rather than one: gcm's write mode,
/// and Active Directory's own access check on the bound identity. Offering an
/// entry the operator turns out not to be delegated is deliberate — the
/// console cannot know what they may do without asking the DC, and a refusal
/// says so clearly. Hiding them would mean guessing, and guessing wrong in the
/// direction of "you cannot do this" is worse.
fn ad_user_items(app: &App, source: usize) -> Vec<Item> {
    let Some(user) = ready(&app.store.ad_users).get(source) else {
        return Vec::new();
    };

    let dn = user.dn.clone();
    let name = user.name().to_string();
    let mut items = Vec::new();

    // Offered in the direction that would change something: a disabled
    // account gets "Enable", an enabled one gets "Disable". Showing both would
    // make one of them a no-op that still writes an audit line.
    let enabling = user.is_disabled();
    items.push(Item::act_directory(
        if enabling { "Enable account" } else { "Disable account" },
        DirectoryAction::SetEnabled {
            dn: dn.clone(),
            name: name.clone(),
            enabled: enabling,
        },
    ));

    if user.is_locked_out() {
        items.push(Item::act_directory(
            "Unlock account",
            DirectoryAction::Unlock {
                dn: dn.clone(),
                name: name.clone(),
            },
        ));
    }

    items.push(Item::open(
        "Reset password…",
        FormFactory::AdResetPassword {
            dn: dn.clone(),
            name: name.clone(),
        },
    ));
    items.push(Item::open(
        "Edit attributes…",
        FormFactory::AdEditUser {
            dn: dn.clone(),
            name: name.clone(),
            fields: Box::new(AdFields {
                display_name: user.display_name.clone().unwrap_or_default(),
                description: user.description.clone().unwrap_or_default(),
                title: user.title.clone().unwrap_or_default(),
                department: user.department.clone().unwrap_or_default(),
                company: user.company.clone().unwrap_or_default(),
                office: user.office.clone().unwrap_or_default(),
                telephone: user.telephone.clone().unwrap_or_default(),
                mobile: user.mobile.clone().unwrap_or_default(),
                mail: user.mail.clone().unwrap_or_default(),
                employee_id: user.employee_id.clone().unwrap_or_default(),
            }),
        },
    ));
    items.push(Item::Separator);
    items.push(Item::act_directory(
        "Delete account",
        DirectoryAction::Delete { dn, name },
    ));

    items
}

/// What can be done to an on-premises computer.
///
/// A shorter list than a user's, because most of what an account offers makes
/// no sense here: a computer's password is managed by the machine itself, and
/// it cannot be locked out by somebody mistyping at a sign-in prompt.
fn ad_computer_items(app: &App, source: usize) -> Vec<Item> {
    let Some(computer) = ready(&app.store.ad_computers).get(source) else {
        return Vec::new();
    };

    let dn = computer.dn.clone();
    let name = computer.name().to_string();
    let mut items = Vec::new();

    let enabling = computer.is_disabled();
    items.push(Item::act_directory(
        if enabling { "Enable computer" } else { "Disable computer" },
        DirectoryAction::SetEnabled {
            dn: dn.clone(),
            name: name.clone(),
            enabled: enabling,
        },
    ));

    items.push(Item::Separator);
    items.push(Item::act_directory(
        "Delete computer",
        DirectoryAction::Delete { dn, name },
    ));

    items
}

/// Everything offered on a right-click, which is [`for_object`] plus the
/// things that need no write mode.
///
/// Kept separate from `for_object` so the details-pane button bar and the
/// toolbar go on showing tenant actions only — a Copy button beside Disable
/// account would be a category error, and the details pane has its own.
pub fn context_items(app: &App, view: View, source: usize) -> Vec<Item> {
    let has_marks = app.views.get(&view).is_some_and(|state| state.has_marks());
    let mut items = for_object(app, view, source);
    let common = common_items(view, has_marks);
    if !items.is_empty() && !common.is_empty() {
        items.push(Item::Separator);
    }
    items.extend(common);
    items
}

/// The entries every list view offers, whatever it holds.
///
/// Takes `has_marks` rather than the whole `App` so the guarantee that matters
/// — no view is ever left with an empty menu — can be asserted directly.
fn common_items(view: View, has_marks: bool) -> Vec<Item> {
    // The console root is a summary with no rows to act on.
    if view == View::Overview {
        return Vec::new();
    }

    let mut items = vec![
        Item::view("Copy row", "Ctrl+C", ViewCommand::CopyRow),
        Item::Separator,
        Item::view("Tick for bulk", "Space", ViewCommand::ToggleMark),
        Item::view("Tick all shown", "Ctrl+A", ViewCommand::MarkAllFiltered),
    ];

    // Only worth offering once there is something to clear.
    if has_marks {
        items.push(Item::view(
            "Clear ticks",
            "Ctrl+Shift+A",
            ViewCommand::ClearMarks,
        ));
    }

    items.push(Item::Separator);
    items.push(Item::view("Export as CSV…", "Ctrl+E", ViewCommand::ExportCsv));
    items.push(Item::view(
        "Export as JSON…",
        "Ctrl+Shift+E",
        ViewCommand::ExportJson,
    ));
    items.push(Item::view("Refresh", "F5", ViewCommand::Refresh));
    items
}

fn user_items(app: &App, source: usize) -> Vec<Item> {
    let Some(user) = app.store.users.get(source) else {
        return Vec::new();
    };
    let id = user.id.clone();
    let name = user.name().to_string();
    let enabled = user.account_enabled.unwrap_or(false);

    vec![
        // First, because creating an account is the errand somebody arrives at
        // the Users node to run, and it does not depend on what is selected.
        Item::open("New user…", FormFactory::CreateUser),
        Item::Separator,
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

fn team_items(app: &App, source: usize) -> Vec<Item> {
    let teams = match &app.store.teams {
        Some(Fetch::Ready(teams)) => teams,
        _ => return Vec::new(),
    };
    let Some(team) = teams.get(source) else {
        return Vec::new();
    };
    let id = team.id.clone();
    let name = team.name().to_string();

    let op = |label: &str, op: TeamOp| {
        Item::act(
            label,
            Action::Team {
                id: id.clone(),
                name: name.clone(),
                op,
            },
        )
    };

    // Only ever one of the pair: offering "Archive" on an archived team would
    // be a button that exists solely to return an error.
    let mut items = vec![if team.archived() {
        op("Restore", TeamOp::Unarchive)
    } else {
        op("Archive", TeamOp::Archive)
    }];

    items.push(Item::Separator);
    items.push(op("Delete team", TeamOp::Delete));
    items
}

fn mailbox_items(app: &App, source: usize) -> Vec<Item> {
    let mailboxes = match &app.store.mailboxes {
        Some(Fetch::Ready(mailboxes)) => mailboxes,
        _ => return Vec::new(),
    };
    let Some(mailbox) = mailboxes.get(source) else {
        return Vec::new();
    };

    // A report that has been anonymised gives no usable identifier, so there is
    // nothing that could be acted on even if the permissions were there.
    if mailbox.is_concealed() || mailbox.user_principal_name.is_empty() {
        return Vec::new();
    }

    let upn = mailbox.user_principal_name.clone();
    let name = mailbox.name().to_string();

    // Prefer the directory object id, falling back to the UPN — Graph accepts
    // either, and a mailbox can appear in the report for an account this
    // console never loaded.
    let id = app
        .store
        .users
        .iter()
        .find(|user| user.upn().eq_ignore_ascii_case(&upn))
        .map(|user| user.id.clone())
        .unwrap_or_else(|| upn.clone());

    let current = app
        .store
        .mailbox_settings
        .get(&upn)
        .and_then(|settings| match settings {
            Fetch::Ready(settings) => settings.automatic_replies_setting.clone(),
            Fetch::Unavailable(_) => None,
        });

    let mut items = vec![Item::open(
        "Set automatic replies…",
        FormFactory::AutomaticReplies {
            id: id.clone(),
            name: name.clone(),
            current: Box::new(current.clone()),
        },
    )];

    // The direct "off" switch only appears when replies are demonstrably on,
    // so it never claims to have turned off something that was already off.
    if current.as_ref().is_some_and(|replies| replies.is_on()) {
        items.push(Item::act(
            "Turn off automatic replies",
            Action::SetAutomaticReplies {
                id,
                name,
                spec: Box::new(AutoReplySpec {
                    enabled: false,
                    internal_message: String::new(),
                    external_message: String::new(),
                    external_audience: "all".into(),
                }),
            },
        ));
    }

    items
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

        View::Teams => {
            // Archive and restore are offered separately rather than as a
            // toggle: a mixed selection has no sensible "toggle", and half the
            // batch failing is exactly what `push` refuses to allow.
            let teams = match &app.store.teams {
                Some(Fetch::Ready(teams)) => teams.clone(),
                _ => return out,
            };
            for (label, op, wanted) in [
                ("Archive", TeamOp::Archive, false),
                ("Restore", TeamOp::Unarchive, true),
            ] {
                push(
                    label,
                    marked
                        .iter()
                        .filter_map(|source| {
                            let team = teams.get(*source)?;
                            // Only teams the operation would actually change.
                            (team.archived() == wanted).then(|| Action::Team {
                                id: team.id.clone(),
                                name: team.name().to_string(),
                                op,
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
                        let team = teams.get(*source)?;
                        Some(Action::Team {
                            id: team.id.clone(),
                            name: team.name().to_string(),
                            op: TeamOp::Delete,
                        })
                    })
                    .collect(),
            );
        }

        // Mailboxes carry one action, and applying the same out-of-office
        // message to a ticked set of people is not something anybody wants.
        // The logs, roles and licences have nothing to act on at all, and the
        // on-premises views are read-only in this release.
        View::Mailboxes
        | View::Roles
        | View::Licenses
        | View::Overview
        | View::SignIns
        | View::AuditLogs
        | View::AdUsers
        | View::AdComputers => {}
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
    /// One on-premises change, never batched.
    ActDirectory(Box<DirectoryAction>),
    Open(Box<FormFactory>),
    /// Something that acts on the console rather than the tenant.
    View(ViewCommand),
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

    /// True when nothing on offer would survive the write gate — used to decide
    /// whether the "read-only" warning is worth showing at all. A palette of
    /// nothing but Copy and Export has no business complaining about it.
    fn all_read_only(&self) -> bool {
        self.bulk.is_empty() && !self.items.iter().any(Item::needs_write_mode)
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
                Item::ActDirectory { label, action } => Some((
                    index,
                    label.clone(),
                    action.severity() == Severity::Destructive,
                )),
                Item::Open { label, .. } => Some((index, label.clone(), false)),
                Item::View { label, .. } => Some((index, label.clone(), false)),
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

        // Not shown when nothing here would be gated anyway: telling somebody
        // the console is read-only above a list of Copy and Export is noise,
        // and worse, implies those are unavailable too.
        if !armed && !palette.all_read_only() {
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

                    // Only the entries that would write to the tenant are
                    // gated. Copy and Export are not the write gate's business.
                    let gated = palette
                        .items
                        .get(*source_index)
                        .is_none_or(Item::needs_write_mode);
                    let response = ui
                        .add_enabled_ui(armed || !gated, |ui| {
                            ui.selectable_label(highlighted, text)
                        })
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
        if enter && let Some((source_index, _, _)) = matching.get(palette.cursor) {
            // Same rule as the click path above: the write gate applies to the
            // entries that write, not to Copy and Export. A bulk palette has no
            // `items`, and everything it offers is a write, so it stays gated.
            let gated = palette
                .items
                .get(*source_index)
                .is_none_or(Item::needs_write_mode);
            if armed || !gated {
                chosen = pick(palette, *source_index);
            }
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
        Item::ActDirectory { action, .. } => Chosen::ActDirectory(action),
        Item::Open { form, .. } => Chosen::Open(Box::new(form)),
        Item::View { command, .. } => Chosen::View(command),
        Item::Separator => Chosen::Pending,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Every view, including the console root, so a new one cannot be added
    /// without this file having an opinion about it.
    fn every_view() -> Vec<View> {
        let mut views = vec![View::Overview];
        views.extend_from_slice(View::ALL);
        views
    }

    #[test]
    fn no_list_view_is_left_with_an_empty_menu() {
        // The bug this exists to prevent: `for_object` returns nothing for
        // Roles, Licenses and both logs, so right-clicking those produced a
        // popup with nothing in it — indistinguishable from a dead menu.
        for view in every_view() {
            let items = common_items(view, false);
            if view == View::Overview {
                assert!(items.is_empty(), "the console root has no rows to act on");
                continue;
            }
            assert!(
                items.iter().any(|item| matches!(item, Item::View { .. })),
                "{view:?} would open an empty context menu"
            );
        }
    }

    #[test]
    fn the_common_entries_never_need_write_mode() {
        // They are the whole reason a read-only view now has a usable menu; a
        // single gated entry among them would put it back where it was.
        for view in every_view() {
            for item in common_items(view, true) {
                assert!(
                    !item.needs_write_mode(),
                    "{view:?} offers a common entry that the write gate would disable"
                );
            }
        }
    }

    #[test]
    fn clearing_ticks_is_offered_only_once_there_are_ticks() {
        let labels = |has_marks| {
            common_items(View::Users, has_marks)
                .iter()
                .filter_map(|item| match item {
                    Item::View { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(!labels(false).iter().any(|l| l.contains("Clear")));
        assert!(labels(true).iter().any(|l| l.contains("Clear")));
    }

    #[test]
    fn only_users_and_groups_can_be_created() {
        // The toolbar used to offer "New user…" from every node, including
        // Licenses and the logs, where it named the wrong object entirely.
        assert!(creatable(View::Users).is_some());
        assert!(creatable(View::Groups).is_some());
        for view in every_view() {
            if matches!(view, View::Users | View::Groups) {
                continue;
            }
            assert!(
                creatable(view).is_none(),
                "{view:?} offers a New button for something it cannot create"
            );
        }
    }

    #[test]
    fn only_tenant_actions_answer_to_the_write_gate() {
        assert!(Item::act("Delete", Action::DeleteUser {
            id: "x".into(),
            name: "x".into(),
        })
        .needs_write_mode());
        assert!(Item::open("New user…", FormFactory::CreateUser).needs_write_mode());
        assert!(!Item::view("Copy row", "Ctrl+C", ViewCommand::CopyRow).needs_write_mode());
        // An on-premises change answers to the gate exactly as a tenant one
        // does. Missing this would leave the palette offering AD entries as
        // enabled while write mode is off, and refusing them on click.
        assert!(
            Item::act_directory(
                "Disable account",
                DirectoryAction::SetEnabled {
                    dn: "CN=a,DC=b".into(),
                    name: "a".into(),
                    enabled: false,
                }
            )
            .needs_write_mode()
        );
    }

    #[test]
    fn creating_is_told_apart_from_acting_on_the_selection() {
        // The toolbar filters these out because it carries a New button of its
        // own; the right-click menu keeps them.
        assert!(Item::open("New user…", FormFactory::CreateUser).creates_object());
        assert!(Item::open("New group…", FormFactory::CreateGroup).creates_object());
        assert!(
            !Item::open("Edit…", FormFactory::ResetPassword {
                id: "x".into(),
                name: "x".into(),
            })
            .creates_object()
        );
        assert!(!Item::view("Copy row", "Ctrl+C", ViewCommand::CopyRow).creates_object());
    }
}
