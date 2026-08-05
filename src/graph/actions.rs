//! Every mutation gcm can perform, as data.
//!
//! Modelling actions as a single enum rather than as methods buys three things
//! that matter for a tool that can wipe a laptop:
//!
//! * **Nothing executes without declaring a [`Severity`].** Adding a variant
//!   without extending `severity()` fails to compile, so a destructive action
//!   cannot quietly skip confirmation.
//! * **Actions are inspectable before they run.** The confirmation modal and
//!   the audit log both read the same description the executor will act on,
//!   so what the operator approved and what happened cannot drift apart.
//! * **Batching is trivial** — a bulk operation is a `Vec<Action>`.
//!
//! Each variant carries its target's display name alongside its id, so nothing
//! downstream needs to reach back into the store to describe what it is doing.

use anyhow::Result;
use reqwest::Method;
use serde_json::json;

use super::GraphClient;
use crate::worker::Collection;

/// How much care an action warrants before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Reversible and low-impact; runs without a prompt.
    Safe,
    /// Changes access or entitlement. Confirmed, but not typed.
    Caution,
    /// Irreversible, or destroys data. Requires typing the target's name.
    Destructive,
}

/// What to do to an Intune-managed device.
///
/// Defined and classified here ahead of its UI, which lands with the Intune
/// device pane, so the severity table stays reviewable in one piece.
#[allow(dead_code, reason = "buttons for these arrive with the Intune pane")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOp {
    Sync,
    Restart,
    RemoteLock,
    Rename(String),
    AutopilotReset,
    /// Removes company data and unenrols, leaving the device usable.
    Retire,
    /// Factory reset. Unrecoverable.
    Wipe,
    /// Removes the Intune record without touching the device.
    Delete,
}

impl DeviceOp {
    fn verb(&self) -> &'static str {
        match self {
            DeviceOp::Sync => "Sync",
            DeviceOp::Restart => "Restart",
            DeviceOp::RemoteLock => "Remote lock",
            DeviceOp::Rename(_) => "Rename",
            DeviceOp::AutopilotReset => "Autopilot reset",
            DeviceOp::Retire => "Retire",
            DeviceOp::Wipe => "Wipe",
            DeviceOp::Delete => "Delete Intune record for",
        }
    }

    fn severity(&self) -> Severity {
        match self {
            DeviceOp::Sync | DeviceOp::Restart | DeviceOp::Rename(_) => Severity::Safe,
            DeviceOp::RemoteLock => Severity::Caution,
            DeviceOp::AutopilotReset
            | DeviceOp::Retire
            | DeviceOp::Wipe
            | DeviceOp::Delete => Severity::Destructive,
        }
    }

    /// The extra warning shown in the confirmation modal.
    fn consequence(&self) -> Option<&'static str> {
        match self {
            DeviceOp::AutopilotReset => Some(
                "The device is reset to a fresh Windows install and re-provisioned \
                 through Autopilot. Local data and settings are lost.",
            ),
            DeviceOp::Retire => Some(
                "Company data and policies are removed and the device is unenrolled. \
                 Personal data is left alone and the device stays usable.",
            ),
            DeviceOp::Wipe => Some(
                "The device is restored to factory settings. Everything on it is \
                 destroyed. This cannot be undone or recalled once accepted.",
            ),
            DeviceOp::Delete => Some(
                "Only the Intune record is removed. The device itself keeps its \
                 company data until it is retired or wiped.",
            ),
            _ => None,
        }
    }
}

/// What to do to a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamOp {
    /// Makes the team read-only in the Teams client without removing anything.
    Archive,
    Unarchive,
    /// Deletes the team by deleting the Microsoft 365 group behind it, which is
    /// the only way Graph offers.
    Delete,
}

impl TeamOp {
    fn verb(self) -> &'static str {
        match self {
            TeamOp::Archive => "Archive",
            TeamOp::Unarchive => "Restore",
            TeamOp::Delete => "Delete",
        }
    }

    fn severity(self) -> Severity {
        match self {
            // Unarchiving simply undoes an archive, so it needs no ceremony.
            TeamOp::Unarchive => Severity::Safe,
            TeamOp::Archive => Severity::Caution,
            TeamOp::Delete => Severity::Destructive,
        }
    }

    fn consequence(self) -> Option<&'static str> {
        match self {
            TeamOp::Archive => Some(
                "The team becomes read-only: nobody can post, and its channels and \
                 files stay where they are. It can be restored at any time.",
            ),
            TeamOp::Delete => Some(
                "A team is deleted by deleting the Microsoft 365 group behind it, so \
                 this also removes the group, its mailbox, its SharePoint site and \
                 every channel conversation. It can be restored for 30 days.",
            ),
            TeamOp::Unarchive => None,
        }
    }
}

/// A new user to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSpec {
    pub display_name: String,
    pub user_principal_name: String,
    pub mail_nickname: String,
    pub password: String,
    /// Whether the account can sign in the moment it exists. Off is a
    /// legitimate choice for an account prepared ahead of a start date.
    pub account_enabled: bool,
    pub job_title: Option<String>,
    pub department: Option<String>,
    /// Two-letter country code. Graph refuses to assign a licence without one,
    /// so the form asks for it up front rather than letting the first licence
    /// assignment fail confusingly.
    pub usage_location: Option<String>,
}

/// How somebody's automatic replies should be set.
///
/// Scheduled replies are deliberately not modelled: they need a start and end
/// instant in the mailbox's own time zone, and a console that got that subtly
/// wrong would silently stop answering somebody's mail on the wrong day.
/// Turning replies on and off covers what an administrator is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoReplySpec {
    pub enabled: bool,
    /// Reply sent to colleagues.
    pub internal_message: String,
    /// Reply sent outside the organisation, when `external_audience` allows it.
    pub external_message: String,
    /// `none`, `contactsOnly` or `all`.
    pub external_audience: String,
}

/// Which membership list an object is being added to or removed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRole {
    Member,
    Owner,
}

impl MemberRole {
    fn path(self) -> &'static str {
        match self {
            MemberRole::Member => "members",
            MemberRole::Owner => "owners",
        }
    }

    fn label(self) -> &'static str {
        match self {
            MemberRole::Member => "member",
            MemberRole::Owner => "owner",
        }
    }
}

/// Fields of a user that gcm can edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserPatch {
    pub job_title: Option<String>,
    pub department: Option<String>,
    pub office_location: Option<String>,
    pub mobile_phone: Option<String>,
    pub usage_location: Option<String>,
}

impl UserPatch {
    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        // An empty string clears a field in Graph, so blank is meaningful and
        // must be sent rather than skipped.
        if let Some(value) = &self.job_title {
            map.insert("jobTitle".into(), json!(value));
        }
        if let Some(value) = &self.department {
            map.insert("department".into(), json!(value));
        }
        if let Some(value) = &self.office_location {
            map.insert("officeLocation".into(), json!(value));
        }
        if let Some(value) = &self.mobile_phone {
            map.insert("mobilePhone".into(), json!(value));
        }
        if let Some(value) = &self.usage_location {
            map.insert("usageLocation".into(), json!(value));
        }
        serde_json::Value::Object(map)
    }

    pub fn is_empty(&self) -> bool {
        self.to_json().as_object().is_none_or(|map| map.is_empty())
    }
}

/// Fields of a group that gcm can edit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPatch {
    pub display_name: Option<String>,
    pub description: Option<String>,
}

impl GroupPatch {
    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        if let Some(value) = &self.display_name {
            map.insert("displayName".into(), json!(value));
        }
        if let Some(value) = &self.description {
            map.insert("description".into(), json!(value));
        }
        serde_json::Value::Object(map)
    }

    pub fn is_empty(&self) -> bool {
        self.to_json().as_object().is_none_or(|map| map.is_empty())
    }
}

/// A new group to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    pub display_name: String,
    pub mail_nickname: String,
    pub description: Option<String>,
    /// Microsoft 365 group when true, security group when false.
    pub unified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CreateUser {
        spec: Box<UserSpec>,
    },
    SetUserEnabled {
        id: String,
        name: String,
        enabled: bool,
    },
    ResetPassword {
        id: String,
        name: String,
        password: String,
    },
    UpdateUser {
        id: String,
        name: String,
        patch: UserPatch,
    },
    SetLicense {
        id: String,
        name: String,
        sku_id: String,
        sku_name: String,
        assign: bool,
    },
    DeleteUser {
        id: String,
        name: String,
    },
    CreateGroup {
        spec: GroupSpec,
    },
    UpdateGroup {
        id: String,
        name: String,
        patch: GroupPatch,
    },
    SetMembership {
        group_id: String,
        group_name: String,
        member_id: String,
        member_name: String,
        role: MemberRole,
        add: bool,
    },
    DeleteGroup {
        id: String,
        name: String,
    },
    SetDeviceEnabled {
        id: String,
        name: String,
        enabled: bool,
    },
    DeleteDevice {
        id: String,
        name: String,
    },
    #[allow(dead_code, reason = "wired up with the Intune device pane")]
    ManagedDevice {
        id: String,
        name: String,
        op: DeviceOp,
    },
    Team {
        /// The team's id, which is also its backing group's id.
        id: String,
        name: String,
        op: TeamOp,
    },
    SetAutomaticReplies {
        /// The mailbox owner's object id.
        id: String,
        name: String,
        spec: Box<AutoReplySpec>,
    },
}

impl Action {
    /// One line describing the action, used in confirmations, the status bar
    /// and the audit log.
    pub fn label(&self) -> String {
        match self {
            Action::CreateUser { spec } => format!(
                "Create the user {} ({})",
                spec.display_name, spec.user_principal_name
            ),
            Action::SetUserEnabled { name, enabled, .. } => {
                let verb = if *enabled { "Enable" } else { "Disable" };
                format!("{verb} {name}")
            }
            Action::ResetPassword { name, .. } => format!("Reset the password for {name}"),
            Action::UpdateUser { name, .. } => format!("Update {name}"),
            Action::SetLicense {
                name,
                sku_name,
                assign,
                ..
            } => {
                if *assign {
                    format!("Assign {sku_name} to {name}")
                } else {
                    format!("Remove {sku_name} from {name}")
                }
            }
            Action::DeleteUser { name, .. } => format!("Delete {name}"),
            Action::CreateGroup { spec } => format!("Create the group {}", spec.display_name),
            Action::UpdateGroup { name, .. } => format!("Update the group {name}"),
            Action::SetMembership {
                group_name,
                member_name,
                role,
                add,
                ..
            } => {
                if *add {
                    format!("Add {member_name} to {group_name} as {}", role.label())
                } else {
                    format!(
                        "Remove {member_name} as {} of {group_name}",
                        role.label()
                    )
                }
            }
            Action::DeleteGroup { name, .. } => format!("Delete the group {name}"),
            Action::SetDeviceEnabled { name, enabled, .. } => {
                let verb = if *enabled { "Enable" } else { "Disable" };
                format!("{verb} the device {name}")
            }
            Action::DeleteDevice { name, .. } => format!("Delete the device {name}"),
            Action::ManagedDevice { name, op, .. } => format!("{} {name}", op.verb()),
            Action::Team { name, op, .. } => format!("{} the team {name}", op.verb()),
            Action::SetAutomaticReplies { name, spec, .. } => {
                if spec.enabled {
                    format!("Turn on automatic replies for {name}")
                } else {
                    format!("Turn off automatic replies for {name}")
                }
            }
        }
    }

    /// The bare verb, used to summarise a batch: "DELETE 12".
    pub fn verb(&self) -> &'static str {
        match self {
            Action::SetUserEnabled { enabled, .. }
            | Action::SetDeviceEnabled { enabled, .. } => {
                if *enabled { "ENABLE" } else { "DISABLE" }
            }
            Action::ResetPassword { .. } => "RESET",
            Action::UpdateUser { .. } | Action::UpdateGroup { .. } => "UPDATE",
            Action::SetLicense { assign, .. } => {
                if *assign { "ASSIGN" } else { "UNASSIGN" }
            }
            Action::SetMembership { add, .. } => if *add { "ADD" } else { "REMOVE" },
            Action::CreateGroup { .. } | Action::CreateUser { .. } => "CREATE",
            Action::SetAutomaticReplies { spec, .. } => {
                if spec.enabled { "ENABLE" } else { "DISABLE" }
            }
            Action::DeleteUser { .. }
            | Action::DeleteGroup { .. }
            | Action::DeleteDevice { .. } => "DELETE",
            Action::Team { op, .. } => match op {
                TeamOp::Archive => "ARCHIVE",
                TeamOp::Unarchive => "RESTORE",
                TeamOp::Delete => "DELETE",
            },
            Action::ManagedDevice { op, .. } => match op {
                DeviceOp::Sync => "SYNC",
                DeviceOp::Restart => "RESTART",
                DeviceOp::RemoteLock => "LOCK",
                DeviceOp::Rename(_) => "RENAME",
                DeviceOp::AutopilotReset => "RESET",
                DeviceOp::Retire => "RETIRE",
                DeviceOp::Wipe => "WIPE",
                DeviceOp::Delete => "DELETE",
            },
        }
    }

    /// How much confirmation this action demands.
    ///
    /// Every variant must answer, which is what stops a new destructive action
    /// from slipping past the modal.
    pub fn severity(&self) -> Severity {
        match self {
            Action::ManagedDevice { op, .. } => op.severity(),
            Action::Team { op, .. } => op.severity(),

            Action::DeleteUser { .. }
            | Action::DeleteGroup { .. }
            | Action::DeleteDevice { .. } => Severity::Destructive,

            // Creating a user is not destructive, but it does mint a sign-in
            // identity with a password — more than enough to be worth a look
            // before it happens, unlike creating an empty group.
            Action::CreateUser { .. }
            | Action::SetUserEnabled { .. }
            | Action::ResetPassword { .. }
            | Action::SetLicense { .. }
            | Action::SetMembership { .. }
            | Action::SetDeviceEnabled { .. }
            // Answering somebody's mail on their behalf is visible to everyone
            // who writes to them, so it is not a quiet change.
            | Action::SetAutomaticReplies { .. } => Severity::Caution,

            Action::UpdateUser { .. }
            | Action::UpdateGroup { .. }
            | Action::CreateGroup { .. } => Severity::Safe,
        }
    }

    /// The name the operator must type to confirm a destructive action.
    pub fn confirm_phrase(&self) -> Option<String> {
        if self.severity() != Severity::Destructive {
            return None;
        }
        Some(self.target_name().to_string())
    }

    /// Extra warning text explaining what will actually happen.
    pub fn consequence(&self) -> Option<&'static str> {
        match self {
            Action::ManagedDevice { op, .. } => op.consequence(),
            Action::Team { op, .. } => op.consequence(),
            Action::CreateUser { .. } => Some(
                "The account can sign in as soon as it exists, using the password \
                 shown on the previous screen. Copy that password now — it is not \
                 stored and cannot be shown again.",
            ),
            Action::SetAutomaticReplies { spec, .. } if spec.enabled => Some(
                "Everyone who writes to this mailbox gets the reply below, until \
                 automatic replies are turned off again.",
            ),
            Action::DeleteUser { .. } => Some(
                "The account is moved to the deleted items container, where it can be \
                 restored for 30 days before being purged permanently.",
            ),
            Action::DeleteGroup { .. } => Some(
                "Microsoft 365 groups can be restored for 30 days. Security groups are \
                 removed immediately and cannot be recovered.",
            ),
            Action::DeleteDevice { .. } => Some(
                "The Entra device record is removed. Any Conditional Access policy that \
                 depends on device state will stop trusting this device.",
            ),
            _ => None,
        }
    }

    /// The display name of whatever this action targets.
    pub fn target_name(&self) -> &str {
        match self {
            Action::SetUserEnabled { name, .. }
            | Action::ResetPassword { name, .. }
            | Action::UpdateUser { name, .. }
            | Action::SetLicense { name, .. }
            | Action::DeleteUser { name, .. }
            | Action::UpdateGroup { name, .. }
            | Action::DeleteGroup { name, .. }
            | Action::SetDeviceEnabled { name, .. }
            | Action::DeleteDevice { name, .. }
            | Action::ManagedDevice { name, .. }
            | Action::Team { name, .. }
            | Action::SetAutomaticReplies { name, .. } => name,
            Action::CreateGroup { spec } => &spec.display_name,
            Action::CreateUser { spec } => &spec.display_name,
            Action::SetMembership { member_name, .. } => member_name,
        }
    }

    /// The object id this action targets, for the audit log.
    pub fn target_id(&self) -> &str {
        match self {
            Action::SetUserEnabled { id, .. }
            | Action::ResetPassword { id, .. }
            | Action::UpdateUser { id, .. }
            | Action::SetLicense { id, .. }
            | Action::DeleteUser { id, .. }
            | Action::UpdateGroup { id, .. }
            | Action::DeleteGroup { id, .. }
            | Action::SetDeviceEnabled { id, .. }
            | Action::DeleteDevice { id, .. }
            | Action::ManagedDevice { id, .. }
            | Action::Team { id, .. }
            | Action::SetAutomaticReplies { id, .. } => id,
            // The object does not exist yet, so there is no id to record. The
            // audit line still names it, which is what makes it findable.
            Action::CreateGroup { .. } | Action::CreateUser { .. } => "",
            Action::SetMembership { group_id, .. } => group_id,
        }
    }

    /// Which collection to reload once this action succeeds.
    pub fn collection(&self) -> Collection {
        match self {
            Action::CreateUser { .. }
            | Action::SetUserEnabled { .. }
            | Action::ResetPassword { .. }
            | Action::UpdateUser { .. }
            | Action::SetLicense { .. }
            | Action::DeleteUser { .. } => Collection::Users,

            Action::CreateGroup { .. }
            | Action::UpdateGroup { .. }
            | Action::SetMembership { .. }
            | Action::DeleteGroup { .. } => Collection::Groups,

            Action::SetDeviceEnabled { .. } | Action::DeleteDevice { .. } => {
                Collection::Devices
            }
            Action::ManagedDevice { .. } => Collection::ManagedDevices,
            Action::Team { .. } => Collection::Teams,
            // The mailbox list comes from a usage report that lags by a day, so
            // it would not show this change however hard it were refreshed. The
            // settings themselves are re-read on selection instead.
            Action::SetAutomaticReplies { .. } => Collection::Mailboxes,
        }
    }

    /// Perform the action against Graph.
    ///
    /// Callers must have checked write mode first; the worker is the only
    /// caller and does exactly that.
    pub async fn execute(&self, client: &mut GraphClient) -> Result<()> {
        let encode = urlencoding::encode;

        match self {
            Action::CreateUser { spec } => {
                let mut body = json!({
                    "accountEnabled": spec.account_enabled,
                    "displayName": spec.display_name,
                    "mailNickname": spec.mail_nickname,
                    "userPrincipalName": spec.user_principal_name,
                    "passwordProfile": {
                        "password": spec.password,
                        "forceChangePasswordNextSignIn": true
                    }
                });
                // Optional properties are omitted rather than sent empty: a
                // blank string would set the field to blank, which for a brand
                // new account is not the same as leaving it unset.
                for (key, value) in [
                    ("jobTitle", &spec.job_title),
                    ("department", &spec.department),
                    ("usageLocation", &spec.usage_location),
                ] {
                    if let Some(value) = value {
                        body[key] = json!(value);
                    }
                }
                client.write(Method::POST, "/users", Some(body)).await
            }

            Action::SetUserEnabled { id, enabled, .. } => {
                client
                    .write(
                        Method::PATCH,
                        &format!("/users/{}", encode(id)),
                        Some(json!({ "accountEnabled": enabled })),
                    )
                    .await
            }

            Action::ResetPassword { id, password, .. } => {
                client
                    .write(
                        Method::PATCH,
                        &format!("/users/{}", encode(id)),
                        Some(json!({
                            "passwordProfile": {
                                "password": password,
                                "forceChangePasswordNextSignIn": true
                            }
                        })),
                    )
                    .await
            }

            Action::UpdateUser { id, patch, .. } => {
                client
                    .write(
                        Method::PATCH,
                        &format!("/users/{}", encode(id)),
                        Some(patch.to_json()),
                    )
                    .await
            }

            Action::SetLicense {
                id, sku_id, assign, ..
            } => {
                // assignLicense takes both lists every time; the unused one is
                // sent empty rather than omitted.
                let body = if *assign {
                    json!({
                        "addLicenses": [{ "skuId": sku_id, "disabledPlans": [] }],
                        "removeLicenses": []
                    })
                } else {
                    json!({ "addLicenses": [], "removeLicenses": [sku_id] })
                };
                client
                    .write(
                        Method::POST,
                        &format!("/users/{}/assignLicense", encode(id)),
                        Some(body),
                    )
                    .await
            }

            Action::DeleteUser { id, .. } => {
                client
                    .write(Method::DELETE, &format!("/users/{}", encode(id)), None)
                    .await
            }

            Action::CreateGroup { spec } => {
                let mut body = json!({
                    "displayName": spec.display_name,
                    "mailNickname": spec.mail_nickname,
                    "mailEnabled": spec.unified,
                    "securityEnabled": !spec.unified,
                });
                if spec.unified {
                    body["groupTypes"] = json!(["Unified"]);
                } else {
                    body["groupTypes"] = json!([]);
                }
                if let Some(description) = &spec.description {
                    body["description"] = json!(description);
                }
                client.write(Method::POST, "/groups", Some(body)).await
            }

            Action::UpdateGroup { id, patch, .. } => {
                client
                    .write(
                        Method::PATCH,
                        &format!("/groups/{}", encode(id)),
                        Some(patch.to_json()),
                    )
                    .await
            }

            Action::SetMembership {
                group_id,
                member_id,
                role,
                add,
                ..
            } => {
                if *add {
                    // Adding takes an @odata.id reference to the directory object.
                    let reference = format!(
                        "{}/directoryObjects/{}",
                        client.graph_base(),
                        member_id
                    );
                    client
                        .write(
                            Method::POST,
                            &format!("/groups/{}/{}/$ref", encode(group_id), role.path()),
                            Some(json!({ "@odata.id": reference })),
                        )
                        .await
                } else {
                    client
                        .write(
                            Method::DELETE,
                            &format!(
                                "/groups/{}/{}/{}/$ref",
                                encode(group_id),
                                role.path(),
                                encode(member_id)
                            ),
                            None,
                        )
                        .await
                }
            }

            Action::DeleteGroup { id, .. } => {
                client
                    .write(Method::DELETE, &format!("/groups/{}", encode(id)), None)
                    .await
            }

            Action::SetDeviceEnabled { id, enabled, .. } => {
                client
                    .write(
                        Method::PATCH,
                        &format!("/devices/{}", encode(id)),
                        Some(json!({ "accountEnabled": enabled })),
                    )
                    .await
            }

            Action::DeleteDevice { id, .. } => {
                client
                    .write(Method::DELETE, &format!("/devices/{}", encode(id)), None)
                    .await
            }

            Action::ManagedDevice { id, op, .. } => {
                let base = format!("/deviceManagement/managedDevices/{}", encode(id));
                match op {
                    DeviceOp::Sync => {
                        client
                            .write(Method::POST, &format!("{base}/syncDevice"), None)
                            .await
                    }
                    DeviceOp::Restart => {
                        client
                            .write(Method::POST, &format!("{base}/rebootNow"), None)
                            .await
                    }
                    DeviceOp::RemoteLock => {
                        client
                            .write(Method::POST, &format!("{base}/remoteLock"), None)
                            .await
                    }
                    DeviceOp::Rename(new_name) => {
                        client
                            .write(
                                Method::POST,
                                &format!("{base}/setDeviceName"),
                                Some(json!({ "deviceName": new_name })),
                            )
                            .await
                    }
                    DeviceOp::AutopilotReset => {
                        client
                            .write(
                                Method::POST,
                                &format!("{base}/windowsAutopilotReset"),
                                None,
                            )
                            .await
                    }
                    DeviceOp::Retire => {
                        client
                            .write(Method::POST, &format!("{base}/retire"), None)
                            .await
                    }
                    DeviceOp::Wipe => {
                        client
                            .write(
                                Method::POST,
                                &format!("{base}/wipe"),
                                Some(json!({
                                    "keepEnrollmentData": false,
                                    "keepUserData": false
                                })),
                            )
                            .await
                    }
                    DeviceOp::Delete => client.write(Method::DELETE, &base, None).await,
                }
            }

            Action::Team { id, op, .. } => match op {
                TeamOp::Archive => {
                    client
                        .write(
                            Method::POST,
                            &format!("/teams/{}/archive", encode(id)),
                            // Leaving the SharePoint site writable would let
                            // members keep changing files in a team they can no
                            // longer post in, which is not what "archived" means
                            // to anyone reading the word.
                            Some(json!({ "shouldSetSpoSiteReadOnlyForMembers": true })),
                        )
                        .await
                }
                TeamOp::Unarchive => {
                    client
                        .write(
                            Method::POST,
                            &format!("/teams/{}/unarchive", encode(id)),
                            None,
                        )
                        .await
                }
                // Graph has no endpoint that deletes a team; a team is deleted
                // by deleting the Microsoft 365 group it is built on, whose id
                // is the same as the team's.
                TeamOp::Delete => {
                    client
                        .write(Method::DELETE, &format!("/groups/{}", encode(id)), None)
                        .await
                }
            },

            Action::SetAutomaticReplies { id, spec, .. } => {
                let setting = if spec.enabled {
                    json!({
                        "status": "alwaysEnabled",
                        "externalAudience": spec.external_audience,
                        "internalReplyMessage": spec.internal_message,
                        "externalReplyMessage": spec.external_message,
                    })
                } else {
                    // Only the status is sent when switching off, so the message
                    // somebody wrote survives to be turned back on unchanged.
                    json!({ "status": "disabled" })
                };
                client
                    .write(
                        Method::PATCH,
                        &format!("/users/{}/mailboxSettings", encode(id)),
                        Some(json!({ "automaticRepliesSetting": setting })),
                    )
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_action() -> Action {
        Action::SetUserEnabled {
            id: "user-1".into(),
            name: "Aisha Rahman".into(),
            enabled: false,
        }
    }

    fn wipe() -> Action {
        Action::ManagedDevice {
            id: "device-1".into(),
            name: "LON-LT-0041".into(),
            op: DeviceOp::Wipe,
        }
    }

    #[test]
    fn destructive_actions_demand_a_typed_phrase() {
        assert_eq!(wipe().severity(), Severity::Destructive);
        assert_eq!(wipe().confirm_phrase().as_deref(), Some("LON-LT-0041"));
    }

    #[test]
    fn non_destructive_actions_need_no_phrase() {
        assert_eq!(user_action().severity(), Severity::Caution);
        assert_eq!(user_action().confirm_phrase(), None);
    }

    /// The safety property the whole design rests on: anything that destroys
    /// data or cannot be undone must be classified Destructive.
    #[test]
    fn everything_irreversible_is_destructive() {
        let irreversible = [
            Action::DeleteUser {
                id: "u".into(),
                name: "n".into(),
            },
            Action::DeleteGroup {
                id: "g".into(),
                name: "n".into(),
            },
            Action::DeleteDevice {
                id: "d".into(),
                name: "n".into(),
            },
            Action::ManagedDevice {
                id: "d".into(),
                name: "n".into(),
                op: DeviceOp::Wipe,
            },
            Action::ManagedDevice {
                id: "d".into(),
                name: "n".into(),
                op: DeviceOp::Retire,
            },
            Action::ManagedDevice {
                id: "d".into(),
                name: "n".into(),
                op: DeviceOp::Delete,
            },
            Action::ManagedDevice {
                id: "d".into(),
                name: "n".into(),
                op: DeviceOp::AutopilotReset,
            },
        ];
        for action in irreversible {
            assert_eq!(
                action.severity(),
                Severity::Destructive,
                "{} must be Destructive",
                action.label()
            );
            assert!(
                action.confirm_phrase().is_some(),
                "{} must require a typed confirmation",
                action.label()
            );
        }
    }

    #[test]
    fn routine_reads_of_state_stay_safe() {
        let sync = Action::ManagedDevice {
            id: "d".into(),
            name: "n".into(),
            op: DeviceOp::Sync,
        };
        assert_eq!(sync.severity(), Severity::Safe);
        assert_eq!(sync.confirm_phrase(), None);
    }

    #[test]
    fn labels_read_as_sentences() {
        assert_eq!(user_action().label(), "Disable Aisha Rahman");
        assert_eq!(wipe().label(), "Wipe LON-LT-0041");

        let membership = Action::SetMembership {
            group_id: "g".into(),
            group_name: "Finance Team".into(),
            member_id: "m".into(),
            member_name: "Ben Okafor".into(),
            role: MemberRole::Owner,
            add: true,
        };
        assert_eq!(
            membership.label(),
            "Add Ben Okafor to Finance Team as owner"
        );
    }

    #[test]
    fn actions_name_the_collection_to_refresh() {
        assert_eq!(user_action().collection(), Collection::Users);
        assert_eq!(wipe().collection(), Collection::ManagedDevices);
    }

    #[test]
    fn destructive_actions_explain_themselves() {
        // The modal is the last thing between an operator and an unrecoverable
        // change; it must say what actually happens.
        for action in [
            wipe(),
            Action::DeleteUser {
                id: "u".into(),
                name: "n".into(),
            },
        ] {
            assert!(
                action.consequence().is_some(),
                "{} needs consequence text",
                action.label()
            );
        }
    }

    #[test]
    fn user_patch_sends_only_named_fields() {
        let patch = UserPatch {
            job_title: Some("Analyst".into()),
            ..Default::default()
        };
        let json = patch.to_json();
        assert_eq!(json["jobTitle"], "Analyst");
        assert!(json.get("department").is_none());
        assert!(!patch.is_empty());
        assert!(UserPatch::default().is_empty());
    }

    fn new_user() -> Action {
        Action::CreateUser {
            spec: Box::new(UserSpec {
                display_name: "Nadia Ferrero".into(),
                user_principal_name: "nadia.ferrero@contoso.co.uk".into(),
                mail_nickname: "nadia.ferrero".into(),
                password: "correct-horse".into(),
                account_enabled: true,
                job_title: None,
                department: None,
                usage_location: Some("GB".into()),
            }),
        }
    }

    #[test]
    fn creating_a_user_is_confirmed_but_not_typed() {
        // It mints a sign-in identity, so it warrants a look; it destroys
        // nothing, so demanding a typed name would be theatre.
        assert_eq!(new_user().severity(), Severity::Caution);
        assert_eq!(new_user().confirm_phrase(), None);
        assert!(new_user().consequence().is_some());
        assert_eq!(
            new_user().label(),
            "Create the user Nadia Ferrero (nadia.ferrero@contoso.co.uk)"
        );
        assert_eq!(new_user().collection(), Collection::Users);
    }

    #[test]
    fn deleting_a_team_is_destructive_and_archiving_is_not() {
        let team = |op| Action::Team {
            id: "team-1".into(),
            name: "Project Falcon".into(),
            op,
        };
        assert_eq!(team(TeamOp::Delete).severity(), Severity::Destructive);
        assert_eq!(
            team(TeamOp::Delete).confirm_phrase().as_deref(),
            Some("Project Falcon")
        );
        assert_eq!(team(TeamOp::Archive).severity(), Severity::Caution);
        // Undoing an archive cannot lose anything, so it runs without a prompt.
        assert_eq!(team(TeamOp::Unarchive).severity(), Severity::Safe);
        assert_eq!(team(TeamOp::Unarchive).confirm_phrase(), None);

        // Deleting a team removes the group behind it; the modal must say so.
        assert!(
            team(TeamOp::Delete)
                .consequence()
                .is_some_and(|text| text.contains("Microsoft 365 group"))
        );
        assert_eq!(team(TeamOp::Archive).collection(), Collection::Teams);
    }

    #[test]
    fn automatic_replies_read_as_on_or_off() {
        let replies = |enabled| Action::SetAutomaticReplies {
            id: "user-1".into(),
            name: "Elena Marsh".into(),
            spec: Box::new(AutoReplySpec {
                enabled,
                internal_message: "On leave.".into(),
                external_message: "On leave.".into(),
                external_audience: "all".into(),
            }),
        };
        assert_eq!(
            replies(true).label(),
            "Turn on automatic replies for Elena Marsh"
        );
        assert_eq!(replies(false).verb(), "DISABLE");
        assert_eq!(replies(true).severity(), Severity::Caution);
        // Switching replies off tells nobody anything, so it needs no warning.
        assert!(replies(true).consequence().is_some());
        assert!(replies(false).consequence().is_none());
    }

    #[test]
    fn creating_an_object_records_no_target_id() {
        // Nothing exists yet to have an id; the label carries the name instead.
        assert_eq!(new_user().target_id(), "");
        assert_eq!(new_user().target_name(), "Nadia Ferrero");
    }

    #[test]
    fn a_blank_value_clears_rather_than_being_skipped() {
        // Clearing someone's job title is a real edit and must reach Graph.
        let patch = UserPatch {
            job_title: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(patch.to_json()["jobTitle"], "");
        assert!(!patch.is_empty());
    }
}
