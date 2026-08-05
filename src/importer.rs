//! Turning a CSV file into a batch of actions.
//!
//! A spreadsheet is the easiest way anyone has ever disabled four hundred
//! accounts by accident, so nothing here executes: it produces a *plan* — the
//! actions it would run, and every row it could not use and why. The operator
//! reads that, and only then does it go through the same confirmation and
//! worker gate as any other batch.
//!
//! Rows that cannot be resolved are skipped rather than aborting the file, but
//! they are never silent. The preview counts them, names them and says why, so
//! a typo in one row cannot quietly become a no-op nobody noticed.

use anyhow::{Context, Result, bail};

use crate::graph::actions::{Action, MemberRole, UserPatch};
use crate::graph::models::{Group, SubscribedSku, User};

/// What a file was understood to be, decided from its header row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    UserAttributes,
    GroupMembership,
    Licences,
}

impl Kind {
    pub fn describe(self) -> &'static str {
        match self {
            Kind::UserAttributes => "user attribute updates",
            Kind::GroupMembership => "group membership changes",
            Kind::Licences => "licence assignments",
        }
    }
}

/// A row that produced no action, and the reason.
#[derive(Debug, Clone)]
pub struct Skipped {
    /// Line number in the file, counting the header as line 1.
    pub line: usize,
    /// Enough of the row to recognise it.
    pub subject: String,
    pub reason: String,
}

/// What an import would do.
pub struct Plan {
    pub kind: Kind,
    pub source: String,
    pub actions: Vec<Action>,
    pub skipped: Vec<Skipped>,
}

/// Normalise a header so `User Principal Name`, `userPrincipalName` and `upn`
/// all mean the same thing — spreadsheets come from everywhere.
fn normalise(header: &str) -> String {
    header
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Which column, if any, holds the thing named by `candidates`.
fn column_of(headers: &[String], candidates: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|header| candidates.contains(&header.as_str()))
}

const UPN_COLUMNS: &[&str] = &["userprincipalname", "upn", "user", "username", "email"];
const GROUP_COLUMNS: &[&str] = &["group", "groupname", "groupid"];
const MEMBER_COLUMNS: &[&str] = &["member", "membername", "memberupn", "memberid"];
const SKU_COLUMNS: &[&str] = &["sku", "skupartnumber", "licence", "license", "product"];
const ACTION_COLUMNS: &[&str] = &["action", "operation", "op"];
const ROLE_COLUMNS: &[&str] = &["role", "as", "membertype"];

/// Editable user attributes, keyed by their accepted column names.
const ATTRIBUTES: &[(&str, &[&str])] = &[
    ("jobTitle", &["jobtitle", "title", "job"]),
    ("department", &["department", "dept"]),
    ("officeLocation", &["officelocation", "office"]),
    ("mobilePhone", &["mobilephone", "mobile", "phone"]),
    ("usageLocation", &["usagelocation", "country", "countrycode"]),
];

/// Work out what a file is from its headers.
fn detect(headers: &[String]) -> Result<Kind> {
    // Licences are checked first: a licence file also carries a UPN column, so
    // testing for attributes first would misread it.
    if column_of(headers, SKU_COLUMNS).is_some() {
        return Ok(Kind::Licences);
    }
    if column_of(headers, GROUP_COLUMNS).is_some()
        && column_of(headers, MEMBER_COLUMNS).is_some()
    {
        return Ok(Kind::GroupMembership);
    }
    if column_of(headers, UPN_COLUMNS).is_some()
        && ATTRIBUTES
            .iter()
            .any(|(_, names)| column_of(headers, names).is_some())
    {
        return Ok(Kind::UserAttributes);
    }

    bail!(
        "Could not tell what this file is for. Expected one of:\n\
         • a userPrincipalName column plus at least one of jobTitle, department, \
         officeLocation, mobilePhone or usageLocation\n\
         • group and member columns\n\
         • userPrincipalName and sku columns"
    )
}

/// Whether an action column says to add or to remove.
///
/// Defaults to adding when the column is absent, which is what a file listing
/// only additions implies.
fn wants_add(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim).unwrap_or("add").to_lowercase().as_str() {
        "" | "add" | "assign" | "join" | "yes" | "true" => Ok(true),
        "remove" | "unassign" | "delete" | "leave" | "no" | "false" => Ok(false),
        other => Err(format!("'{other}' is not add or remove")),
    }
}

fn find_user<'a>(users: &'a [User], needle: &str) -> Option<&'a User> {
    let needle = needle.trim().to_lowercase();
    users.iter().find(|user| {
        user.id.to_lowercase() == needle
            || user
                .user_principal_name
                .as_deref()
                .is_some_and(|upn| upn.to_lowercase() == needle)
            || user
                .mail
                .as_deref()
                .is_some_and(|mail| mail.to_lowercase() == needle)
    })
}

fn find_group<'a>(groups: &'a [Group], needle: &str) -> Option<&'a Group> {
    let needle = needle.trim().to_lowercase();
    groups.iter().find(|group| {
        group.id.to_lowercase() == needle
            || group.name().to_lowercase() == needle
            || group
                .mail
                .as_deref()
                .is_some_and(|mail| mail.to_lowercase() == needle)
    })
}

fn find_sku<'a>(skus: &'a [SubscribedSku], needle: &str) -> Option<&'a SubscribedSku> {
    let needle = needle.trim().to_lowercase();
    skus.iter().find(|sku| {
        sku.part_number().to_lowercase() == needle
            || sku.display_name().to_lowercase() == needle
            || sku
                .sku_id
                .as_deref()
                .is_some_and(|id| id.to_lowercase() == needle)
    })
}

/// The directory a plan is resolved against.
pub struct Directory<'a> {
    pub users: &'a [User],
    pub groups: &'a [Group],
    pub licences: &'a [SubscribedSku],
}

/// Read a CSV and work out what it would do.
pub fn plan(csv_text: &str, source: String, directory: Directory<'_>) -> Result<Plan> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(csv_text.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .context("reading the header row")?
        .iter()
        .map(normalise)
        .collect();

    if headers.is_empty() {
        bail!("The file has no header row.");
    }

    let kind = detect(&headers)?;
    let mut actions = Vec::new();
    let mut skipped = Vec::new();

    for (index, record) in reader.records().enumerate() {
        // Header is line 1, so the first data row is line 2.
        let line = index + 2;
        let record = match record {
            Ok(record) => record,
            Err(err) => {
                skipped.push(Skipped {
                    line,
                    subject: String::new(),
                    reason: format!("the row could not be read: {err}"),
                });
                continue;
            }
        };

        let field = |names: &[&str]| -> Option<String> {
            column_of(&headers, names)
                .and_then(|index| record.get(index))
                .map(|value| value.trim().to_string())
        };

        match kind {
            Kind::UserAttributes => {
                let Some(upn) = field(UPN_COLUMNS).filter(|value| !value.is_empty()) else {
                    skipped.push(Skipped {
                        line,
                        subject: String::new(),
                        reason: "no user principal name in this row".into(),
                    });
                    continue;
                };
                let Some(user) = find_user(directory.users, &upn) else {
                    skipped.push(Skipped {
                        line,
                        subject: upn,
                        reason: "no such user in this tenant".into(),
                    });
                    continue;
                };

                // Only columns present in the file are touched. An absent
                // column leaves the attribute alone; a present but empty cell
                // clears it, which is the only way to say "remove this".
                let mut patch = UserPatch::default();
                for (name, names) in ATTRIBUTES {
                    let Some(value) = field(names) else { continue };
                    match *name {
                        "jobTitle" => patch.job_title = Some(value),
                        "department" => patch.department = Some(value),
                        "officeLocation" => patch.office_location = Some(value),
                        "mobilePhone" => patch.mobile_phone = Some(value),
                        "usageLocation" => {
                            patch.usage_location = Some(value.to_uppercase())
                        }
                        _ => {}
                    }
                }

                if patch.is_empty() {
                    skipped.push(Skipped {
                        line,
                        subject: user.name().to_string(),
                        reason: "nothing to change in this row".into(),
                    });
                    continue;
                }

                actions.push(Action::UpdateUser {
                    id: user.id.clone(),
                    name: user.name().to_string(),
                    patch,
                });
            }

            Kind::GroupMembership => {
                let group_value = field(GROUP_COLUMNS).unwrap_or_default();
                let member_value = field(MEMBER_COLUMNS).unwrap_or_default();
                let subject = format!("{member_value} → {group_value}");

                let Some(group) = find_group(directory.groups, &group_value) else {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: format!("no group called '{group_value}'"),
                    });
                    continue;
                };
                // Entra recomputes dynamic membership from the rule, so a
                // manual change here would simply be reverted.
                if group.membership() == "Dynamic" {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: format!("'{}' has dynamic membership", group.name()),
                    });
                    continue;
                }
                let Some(member) = find_user(directory.users, &member_value) else {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: format!("no user called '{member_value}'"),
                    });
                    continue;
                };

                let add = match wants_add(field(ACTION_COLUMNS).as_deref()) {
                    Ok(add) => add,
                    Err(reason) => {
                        skipped.push(Skipped {
                            line,
                            subject,
                            reason,
                        });
                        continue;
                    }
                };

                let role = match field(ROLE_COLUMNS)
                    .unwrap_or_default()
                    .to_lowercase()
                    .as_str()
                {
                    "owner" => MemberRole::Owner,
                    _ => MemberRole::Member,
                };

                actions.push(Action::SetMembership {
                    group_id: group.id.clone(),
                    group_name: group.name().to_string(),
                    member_id: member.id.clone(),
                    member_name: member.name().to_string(),
                    role,
                    add,
                });
            }

            Kind::Licences => {
                let upn = field(UPN_COLUMNS).unwrap_or_default();
                let sku_value = field(SKU_COLUMNS).unwrap_or_default();
                let subject = format!("{upn} · {sku_value}");

                let Some(user) = find_user(directory.users, &upn) else {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: format!("no user called '{upn}'"),
                    });
                    continue;
                };
                let Some(sku) = find_sku(directory.licences, &sku_value) else {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: format!("no subscription called '{sku_value}'"),
                    });
                    continue;
                };
                let Some(sku_id) = sku.sku_id.clone() else {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: "that subscription has no SKU id".into(),
                    });
                    continue;
                };

                let assign = match wants_add(field(ACTION_COLUMNS).as_deref()) {
                    Ok(assign) => assign,
                    Err(reason) => {
                        skipped.push(Skipped {
                            line,
                            subject,
                            reason,
                        });
                        continue;
                    }
                };

                // Assigning a licence the user already holds, or removing one
                // they do not, would fail at Graph. Say so here instead.
                let holds = user
                    .assigned_licenses
                    .iter()
                    .any(|licence| licence.sku_id.as_deref() == Some(sku_id.as_str()));
                if holds == assign {
                    skipped.push(Skipped {
                        line,
                        subject,
                        reason: if assign {
                            "already has that licence".into()
                        } else {
                            "does not have that licence".into()
                        },
                    });
                    continue;
                }

                actions.push(Action::SetLicense {
                    id: user.id.clone(),
                    name: user.name().to_string(),
                    sku_id,
                    sku_name: sku.display_name(),
                    assign,
                });
            }
        }
    }

    Ok(Plan {
        kind,
        source,
        actions,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::models::AssignedLicense;

    fn user(upn: &str, name: &str) -> User {
        User {
            id: format!("id-{upn}"),
            display_name: Some(name.into()),
            user_principal_name: Some(upn.into()),
            ..Default::default()
        }
    }

    fn directory_users() -> Vec<User> {
        vec![
            user("aisha@contoso.co.uk", "Aisha Rahman"),
            user("ben@contoso.co.uk", "Ben Okafor"),
        ]
    }

    fn directory_groups() -> Vec<Group> {
        vec![
            Group {
                id: "g1".into(),
                display_name: Some("Finance Team".into()),
                security_enabled: Some(true),
                ..Default::default()
            },
            Group {
                id: "g2".into(),
                display_name: Some("London Office".into()),
                group_types: vec!["DynamicMembership".into()],
                security_enabled: Some(true),
                ..Default::default()
            },
        ]
    }

    fn directory_skus() -> Vec<SubscribedSku> {
        vec![SubscribedSku {
            id: "t_s1".into(),
            sku_id: Some("sku-e3".into()),
            sku_part_number: Some("SPE_E3".into()),
            ..Default::default()
        }]
    }

    fn plan_of(csv: &str, users: &[User]) -> Plan {
        let groups = directory_groups();
        let skus = directory_skus();
        plan(
            csv,
            "test.csv".into(),
            Directory {
                users,
                groups: &groups,
                licences: &skus,
            },
        )
        .expect("should plan")
    }

    #[test]
    fn detects_each_supported_shape() {
        let headers = |list: &[&str]| list.iter().map(|h| normalise(h)).collect::<Vec<_>>();
        assert_eq!(
            detect(&headers(&["userPrincipalName", "Job Title"])).unwrap(),
            Kind::UserAttributes
        );
        assert_eq!(
            detect(&headers(&["group", "member", "action"])).unwrap(),
            Kind::GroupMembership
        );
        assert_eq!(
            detect(&headers(&["upn", "sku"])).unwrap(),
            Kind::Licences
        );
    }

    #[test]
    fn a_licence_file_is_not_mistaken_for_attributes() {
        // Both carry a UPN column, so order of detection matters.
        let headers: Vec<String> = ["userPrincipalName", "sku", "department"]
            .iter()
            .map(|h| normalise(h))
            .collect();
        assert_eq!(detect(&headers).unwrap(), Kind::Licences);
    }

    #[test]
    fn unrecognisable_files_are_refused_with_guidance() {
        let headers: Vec<String> = ["colour", "size"].iter().map(|h| normalise(h)).collect();
        let err = detect(&headers).expect_err("should not be understood");
        assert!(err.to_string().contains("userPrincipalName"));
    }

    #[test]
    fn headers_are_matched_regardless_of_spelling_style() {
        assert_eq!(normalise("User Principal Name"), "userprincipalname");
        assert_eq!(normalise("userPrincipalName"), "userprincipalname");
        assert_eq!(normalise("user_principal_name"), "userprincipalname");
    }

    #[test]
    fn attribute_rows_become_patches() {
        let users = directory_users();
        let csv = "userPrincipalName,Department\naisha@contoso.co.uk,Finance\n";
        let plan = plan_of(csv, &users);

        assert_eq!(plan.kind, Kind::UserAttributes);
        assert_eq!(plan.actions.len(), 1);
        assert!(plan.skipped.is_empty());
        match &plan.actions[0] {
            Action::UpdateUser { name, patch, .. } => {
                assert_eq!(name, "Aisha Rahman");
                assert_eq!(patch.department.as_deref(), Some("Finance"));
                // A column not in the file must not be touched.
                assert_eq!(patch.job_title, None);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn an_empty_cell_clears_the_attribute() {
        // The only way a spreadsheet can express "remove this value".
        let users = directory_users();
        let csv = "upn,department\naisha@contoso.co.uk,\n";
        let plan = plan_of(csv, &users);
        match &plan.actions[0] {
            Action::UpdateUser { patch, .. } => {
                assert_eq!(patch.department.as_deref(), Some(""));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn unknown_users_are_skipped_with_the_reason() {
        let users = directory_users();
        let csv = "upn,department\nnobody@contoso.co.uk,Finance\nben@contoso.co.uk,IT\n";
        let plan = plan_of(csv, &users);

        assert_eq!(plan.actions.len(), 1, "the valid row should still run");
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].line, 2, "line numbers count the header");
        assert!(plan.skipped[0].reason.contains("no such user"));
    }

    #[test]
    fn dynamic_groups_are_refused() {
        // Entra would recompute the membership and revert the change.
        let users = directory_users();
        let csv = "group,member\nLondon Office,aisha@contoso.co.uk\n";
        let plan = plan_of(csv, &users);
        assert!(plan.actions.is_empty());
        assert!(plan.skipped[0].reason.contains("dynamic"));
    }

    #[test]
    fn membership_rows_honour_the_action_column() {
        let users = directory_users();
        let csv = "group,member,action\nFinance Team,ben@contoso.co.uk,remove\n";
        let plan = plan_of(csv, &users);
        match &plan.actions[0] {
            Action::SetMembership { add, role, .. } => {
                assert!(!add);
                assert_eq!(*role, MemberRole::Member);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn a_missing_action_column_means_add() {
        assert_eq!(wants_add(None), Ok(true));
        assert_eq!(wants_add(Some("")), Ok(true));
        assert_eq!(wants_add(Some("Remove")), Ok(false));
        assert!(wants_add(Some("maybe")).is_err());
    }

    #[test]
    fn licences_already_held_are_skipped_rather_than_failed() {
        // Graph would reject these; catching them here keeps the batch clean.
        let mut users = directory_users();
        users[0].assigned_licenses = vec![AssignedLicense {
            sku_id: Some("sku-e3".into()),
            disabled_plans: vec![],
        }];
        let csv = "upn,sku,action\naisha@contoso.co.uk,SPE_E3,assign\n";
        let plan = plan_of(csv, &users);

        assert!(plan.actions.is_empty());
        assert!(plan.skipped[0].reason.contains("already has"));
    }

    #[test]
    fn licences_can_be_named_by_part_number_or_product() {
        let users = directory_users();
        let csv = "upn,sku\nben@contoso.co.uk,Microsoft 365 E3\n";
        let plan = plan_of(csv, &users);
        assert_eq!(plan.actions.len(), 1, "the friendly name should resolve");
    }

    #[test]
    fn quoted_values_containing_commas_survive() {
        let users = directory_users();
        let csv = "upn,department\naisha@contoso.co.uk,\"Finance, Legal\"\n";
        let plan = plan_of(csv, &users);
        match &plan.actions[0] {
            Action::UpdateUser { patch, .. } => {
                assert_eq!(patch.department.as_deref(), Some("Finance, Legal"));
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
