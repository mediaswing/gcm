//! Synthetic data for developing the console without a tenant.
//!
//! Enabled by running a **debug** build with `GCM_DEMO=1`. The whole module is
//! `#[cfg(debug_assertions)]`, so it does not exist in a release binary and
//! cannot be switched on in one.
//!
//! This exists so the layout, the keyboard model, and the "Intune is not
//! available" path can all be exercised offline — the last of which is
//! otherwise only reachable by finding a tenant that lacks Intune.

use std::sync::Arc;

use chrono::{Duration, Utc};

use crate::graph::Fetch;
use crate::graph::models::*;

pub fn enabled() -> bool {
    std::env::var("GCM_DEMO").is_ok_and(|value| value == "1")
}

fn ago(days: i64) -> Option<chrono::DateTime<Utc>> {
    Some(Utc::now() - Duration::days(days))
}

pub fn organization() -> Organization {
    Organization {
        id: "8f3a91d2-0000-4c1a-9e77-demo00000001".into(),
        display_name: Some("Contoso Demonstration".into()),
        tenant_type: Some("AAD".into()),
        country_letter_code: Some("GB".into()),
        created_date_time: ago(1460),
        verified_domains: vec![
            VerifiedDomain {
                name: Some("contoso.co.uk".into()),
                is_default: Some(true),
                is_initial: Some(false),
            },
            VerifiedDomain {
                name: Some("contoso.onmicrosoft.com".into()),
                is_default: Some(false),
                is_initial: Some(true),
            },
        ],
        assigned_plans: vec![
            AssignedPlan {
                service: Some("exchange".into()),
                service_plan_id: None,
                capability_status: Some("Enabled".into()),
            },
            AssignedPlan {
                service: Some("TeamspaceAPI".into()),
                service_plan_id: None,
                capability_status: Some("Enabled".into()),
            },
            // Keep the tenant summary consistent with the managed devices
            // below, unless GCM_DEMO_NO_INTUNE is deliberately turning them off.
            AssignedPlan {
                service: Some("SCO".into()),
                service_plan_id: None,
                capability_status: Some(
                    if std::env::var("GCM_DEMO_NO_INTUNE").is_ok_and(|v| v == "1") {
                        "Deleted"
                    } else {
                        "Enabled"
                    }
                    .into(),
                ),
            },
        ],
    }
}

pub fn users() -> Arc<Vec<User>> {
    let people = [
        ("Aisha Rahman", "aisha.rahman", "Finance Director", "Finance", true),
        ("Ben Okafor", "ben.okafor", "Service Desk Analyst", "IT", true),
        ("Chloe Duval", "chloe.duval", "Marketing Manager", "Marketing", true),
        ("Dmitri Sokolov", "dmitri.sokolov", "Solutions Architect", "IT", true),
        ("Elena Marsh", "elena.marsh", "HR Business Partner", "People", false),
        ("Farid Haddad", "farid.haddad", "Financial Analyst", "Finance", true),
        ("Grace Lin", "grace.lin", "Chief Technology Officer", "Executive", true),
        ("Hamish Reid", "hamish.reid", "Network Engineer", "IT", true),
        ("Ingrid Sollberger", "ingrid.sollberger", "General Counsel", "Legal", true),
        ("Jonah Whitfield", "jonah.whitfield", "Sales Executive", "Sales", false),
        ("Kiara Mensah", "kiara.mensah", "Product Designer", "Product", true),
        ("Liam Byrne", "liam.byrne", "Security Analyst", "IT", true),
    ];

    let users = people
        .iter()
        .enumerate()
        .map(|(index, (name, alias, title, department, enabled))| User {
            id: format!("user-{index:04}"),
            display_name: Some((*name).into()),
            user_principal_name: Some(format!("{alias}@contoso.co.uk")),
            mail: Some(format!("{alias}@contoso.co.uk")),
            job_title: Some((*title).into()),
            department: Some((*department).into()),
            office_location: Some(if index % 2 == 0 { "London" } else { "Leeds" }.into()),
            mobile_phone: Some(format!("+44 7700 9000{index:02}")),
            account_enabled: Some(*enabled),
            user_type: Some(if index == 9 { "Guest" } else { "Member" }.into()),
            created_date_time: ago(900 - index as i64 * 40),
            last_password_change_date_time: ago(index as i64 * 11 + 3),
            on_premises_sync_enabled: Some(index % 3 == 0),
            on_premises_sam_account_name: (index % 3 == 0).then(|| (*alias).to_string()),
            usage_location: Some("GB".into()),
            assigned_licenses: if *enabled {
                vec![AssignedLicense {
                    sku_id: Some("sku-spe-e3".into()),
                    disabled_plans: vec![],
                }]
            } else {
                vec![]
            },
            business_phones: vec!["+44 20 7946 0000".into()],
            proxy_addresses: vec![
                format!("SMTP:{alias}@contoso.co.uk"),
                format!("smtp:{alias}@contoso.onmicrosoft.com"),
            ],
        })
        .collect();

    Arc::new(users)
}

pub fn groups() -> Arc<Vec<Group>> {
    let specs = [
        ("All Company", "Unified", true, false, None),
        ("Finance Team", "Unified", true, false, None),
        ("IT Administrators", "", false, true, None),
        ("London Office", "DynamicMembership", false, true, Some("(user.city -eq \"London\")")),
        ("Sales Distribution", "", true, false, None),
        ("Security Operations", "", false, true, None),
        ("Project Falcon", "Unified", true, false, None),
        ("Licensed Users", "DynamicMembership", false, true, Some("(user.assignedPlans -any (assignedPlan.capabilityStatus -eq \"Enabled\"))")),
    ];

    let groups = specs
        .iter()
        .enumerate()
        .map(|(index, (name, kind, mail, security, rule))| Group {
            id: format!("group-{index:04}"),
            display_name: Some((*name).into()),
            description: Some(format!("{name} — demonstration group")),
            mail: mail.then(|| {
                format!("{}@contoso.co.uk", name.to_lowercase().replace(' ', "."))
            }),
            mail_nickname: Some(name.to_lowercase().replace(' ', ".")),
            mail_enabled: Some(*mail),
            security_enabled: Some(*security),
            visibility: Some(if *mail { "Private" } else { "" }.into()),
            created_date_time: ago(700 - index as i64 * 50),
            membership_rule: rule.map(String::from),
            membership_rule_processing_state: rule.map(|_| "On".to_string()),
            on_premises_sync_enabled: Some(index == 2),
            is_assignable_to_role: Some(index == 2),
            group_types: if kind.is_empty() {
                vec![]
            } else {
                vec![(*kind).into()]
            },
        })
        .collect();

    Arc::new(groups)
}

pub fn roles() -> Arc<Vec<DirectoryRole>> {
    let specs = [
        ("Global Administrator", "Can manage all aspects of the tenant."),
        ("User Administrator", "Can manage users and groups."),
        ("Helpdesk Administrator", "Can reset passwords for non-administrators."),
        ("Intune Administrator", "Can manage all aspects of Microsoft Intune."),
        ("Security Reader", "Can read security information and reports."),
        ("Billing Administrator", "Can manage subscriptions and invoices."),
    ];

    Arc::new(
        specs
            .iter()
            .enumerate()
            .map(|(index, (name, description))| DirectoryRole {
                id: format!("role-{index:04}"),
                display_name: Some((*name).into()),
                description: Some((*description).into()),
                role_template_id: Some(format!("template-{index:04}")),
            })
            .collect(),
    )
}

pub fn devices() -> Arc<Vec<Device>> {
    let specs = [
        ("LON-LT-0041", "Windows", "10.0.26100.2314", "AzureAd", "Dell Inc.", "Latitude 7440", true),
        ("LON-LT-0042", "Windows", "10.0.22631.4317", "ServerAd", "Dell Inc.", "Latitude 5540", true),
        ("LDS-DT-0007", "Windows", "10.0.26100.2314", "ServerAd", "HP", "EliteDesk 800", false),
        ("Grace-MBP", "MacMDM", "15.1.0", "Workplace", "Apple Inc.", "MacBook Pro 14-inch", true),
        ("Liam-iPhone", "IPhone", "18.1.1", "Workplace", "Apple Inc.", "iPhone 15", true),
        ("LON-SRV-0001", "Windows", "10.0.20348.2849", "ServerAd", "Lenovo", "ThinkSystem SR650", false),
        ("Kiara-Surface", "Windows", "10.0.26100.2314", "AzureAd", "Microsoft Corporation", "Surface Laptop 6", true),
    ];

    Arc::new(
        specs
            .iter()
            .enumerate()
            .map(
                |(index, (name, os, version, trust, manufacturer, model, compliant))| Device {
                    id: format!("device-{index:04}"),
                    device_id: Some(format!("aaaaaaaa-0000-0000-0000-{index:012}")),
                    display_name: Some((*name).into()),
                    operating_system: Some((*os).into()),
                    operating_system_version: Some((*version).into()),
                    trust_type: Some((*trust).into()),
                    profile_type: Some("RegisteredDevice".into()),
                    manufacturer: Some((*manufacturer).into()),
                    model: Some((*model).into()),
                    is_compliant: Some(*compliant),
                    is_managed: Some(true),
                    account_enabled: Some(true),
                    approximate_last_sign_in_date_time: ago(index as i64),
                    registration_date_time: ago(400 - index as i64 * 30),
                    on_premises_sync_enabled: Some(*trust == "ServerAd"),
                },
            )
            .collect(),
    )
}

/// Set `GCM_DEMO_NO_INTUNE=1` to exercise the unavailable-feature path.
pub fn managed_devices() -> Fetch<Arc<Vec<ManagedDevice>>> {
    if unavailable("GCM_DEMO_NO_INTUNE") {
        return Fetch::Unavailable(
            "This tenant does not expose Intune managed devices. Either Intune is not \
             licensed here, or the app registration has not been granted \
             DeviceManagementManagedDevices.Read.All.\n\n403 — Forbidden: Tenant is not \
             licensed for Microsoft Intune."
                .into(),
        );
    }

    let specs = [
        ("LON-LT-0041", "aisha.rahman", "Windows", "10.0.26100.2314", "compliant", "mdm"),
        ("LON-LT-0042", "ben.okafor", "Windows", "10.0.22631.4317", "noncompliant", "configurationManagerClientMdm"),
        ("Grace-MBP", "grace.lin", "macOS", "15.1.0", "compliant", "mdm"),
        ("Liam-iPhone", "liam.byrne", "iOS", "18.1.1", "inGracePeriod", "mdm"),
        ("Kiara-Surface", "kiara.mensah", "Windows", "10.0.26100.2314", "compliant", "mdm"),
    ];

    Fetch::Ready(Arc::new(
        specs
            .iter()
            .enumerate()
            .map(
                |(index, (name, alias, os, version, compliance, agent))| ManagedDevice {
                    id: format!("managed-{index:04}"),
                    device_name: Some((*name).into()),
                    managed_device_owner_type: Some("company".into()),
                    operating_system: Some((*os).into()),
                    os_version: Some((*version).into()),
                    compliance_state: Some((*compliance).into()),
                    management_agent: Some((*agent).into()),
                    enrolled_date_time: ago(300 - index as i64 * 20),
                    last_sync_date_time: ago(index as i64),
                    user_principal_name: Some(format!("{alias}@contoso.co.uk")),
                    model: Some("Demonstration hardware".into()),
                    manufacturer: Some("Contoso".into()),
                    serial_number: Some(format!("SN-{index:08}")),
                    imei: (*os == "iOS").then(|| "350000000000001".to_string()),
                    is_encrypted: Some(true),
                    is_supervised: Some(*os == "iOS"),
                    jail_broken: Some("False".into()),
                    device_enrollment_type: Some("windowsAzureADJoin".into()),
                    total_storage_space_in_bytes: Some(512 * 1_073_741_824),
                    free_storage_space_in_bytes: Some((180 + index as i64 * 20) * 1_073_741_824),
                },
            )
            .collect(),
    ))
}

pub fn licenses() -> Arc<Vec<SubscribedSku>> {
    let specs = [
        ("SPE_E3", "sku-spe-e3", 250, 231),
        ("SPE_E5", "sku-spe-e5", 40, 40),
        ("POWER_BI_PRO", "sku-pbi-pro", 60, 43),
        ("PROJECTPROFESSIONAL", "sku-project", 15, 16),
        ("EXCHANGEARCHIVE_ADDON", "sku-archive", 100, 12),
        ("SOME_UNMAPPED_SKU", "sku-unknown", 25, 3),
    ];

    Arc::new(
        specs
            .iter()
            .enumerate()
            .map(|(index, (part, sku_id, total, consumed))| SubscribedSku {
                id: format!("tenant_{sku_id}"),
                sku_id: Some((*sku_id).into()),
                sku_part_number: Some((*part).into()),
                applies_to: Some("User".into()),
                capability_status: Some("Enabled".into()),
                consumed_units: Some(*consumed),
                prepaid_units: Some(PrepaidUnits {
                    enabled: Some(*total),
                    suspended: Some(0),
                    warning: Some(if index == 2 { 5 } else { 0 }),
                }),
                service_plans: vec![
                    ServicePlan {
                        service_plan_id: Some("plan-exchange".into()),
                        service_plan_name: Some("EXCHANGE_S_ENTERPRISE".into()),
                        provisioning_status: Some("Success".into()),
                        applies_to: Some("User".into()),
                    },
                    ServicePlan {
                        service_plan_id: Some("plan-sharepoint".into()),
                        service_plan_name: Some("SHAREPOINTENTERPRISE".into()),
                        provisioning_status: Some("Success".into()),
                        applies_to: Some("User".into()),
                    },
                    ServicePlan {
                        service_plan_id: Some("plan-audit".into()),
                        service_plan_name: Some("EXCHANGE_ANALYTICS".into()),
                        provisioning_status: Some("Success".into()),
                        applies_to: Some("Company".into()),
                    },
                ],
            })
            .collect(),
    )
}

/// Set `GCM_DEMO_NO_TEAMS=1` to exercise the unavailable-feature path.
pub fn teams() -> Fetch<Arc<Vec<Team>>> {
    if unavailable("GCM_DEMO_NO_TEAMS") {
        return Fetch::Unavailable(
            "This tenant does not expose Microsoft Teams. Either Teams is not licensed \
             here, or the app registration has not been granted Team.ReadBasic.All.\n\n\
             403 — Forbidden: Insufficient privileges to complete the operation."
                .into(),
        );
    }

    let specs = [
        ("All Company", "public", false),
        ("Finance Team", "private", false),
        ("Project Falcon", "private", false),
        ("Project Kestrel", "private", true),
        ("Service Desk", "public", false),
    ];

    Fetch::Ready(Arc::new(
        specs
            .iter()
            .enumerate()
            .map(|(index, (name, visibility, archived))| Team {
                id: format!("team-{index:04}"),
                display_name: Some((*name).into()),
                description: Some(format!("{name} — demonstration team")),
                visibility: Some((*visibility).into()),
                is_archived: Some(*archived),
                ..Default::default()
            })
            .collect(),
    ))
}

/// Full settings for one team, as `GET /teams/{id}` would return them.
pub fn team_detail(team: &Team) -> (Fetch<Team>, Arc<Vec<Channel>>) {
    let channels = ["General", "Planning", "Deployments", "Random"];
    let channels: Vec<Channel> = channels
        .iter()
        .enumerate()
        .map(|(index, name)| Channel {
            id: format!("{}-channel-{index:04}", team.id),
            display_name: Some((*name).into()),
            description: (index == 1).then(|| "Sprint planning and retros".to_string()),
            membership_type: Some(if index == 2 { "private" } else { "standard" }.into()),
            email: Some(format!(
                "{}@contoso.co.uk",
                name.to_lowercase().replace(' ', ".")
            )),
            created_date_time: ago(500 - index as i64 * 30),
        })
        .collect();

    let full = Team {
        classification: Some("Internal".into()),
        specialization: Some("none".into()),
        created_date_time: ago(600),
        web_url: Some(format!("https://teams.microsoft.com/l/team/{}", team.id)),
        member_settings: Some(TeamMemberSettings {
            allow_create_update_channels: Some(true),
            allow_delete_channels: Some(false),
            allow_add_remove_apps: Some(true),
            allow_create_update_remove_tabs: Some(true),
            allow_create_update_remove_connectors: Some(false),
        }),
        guest_settings: Some(TeamGuestSettings {
            allow_create_update_channels: Some(false),
            allow_delete_channels: Some(false),
        }),
        messaging_settings: Some(TeamMessagingSettings {
            allow_user_edit_messages: Some(true),
            allow_user_delete_messages: Some(true),
            allow_owner_delete_messages: Some(true),
            allow_team_mentions: Some(true),
            allow_channel_mentions: Some(true),
        }),
        fun_settings: Some(TeamFunSettings {
            allow_giphy: Some(true),
            giphy_content_rating: Some("moderate".into()),
            allow_stickers_and_memes: Some(true),
            allow_custom_memes: Some(false),
        }),
        ..team.clone()
    };

    (Fetch::Ready(full), Arc::new(channels))
}

/// Set `GCM_DEMO_NO_EXCHANGE=1` to exercise the unavailable-feature path.
pub fn mailboxes() -> Fetch<Arc<Vec<Mailbox>>> {
    if unavailable("GCM_DEMO_NO_EXCHANGE") {
        return Fetch::Unavailable(
            "This tenant does not expose the mailbox usage report. Either Exchange \
             Online is not licensed here, or the app registration has not been granted \
             Reports.Read.All.\n\n403 — Forbidden: Insufficient privileges."
                .into(),
        );
    }

    const GIGABYTE: i64 = 1_073_741_824;
    let people = [
        ("Aisha Rahman", "aisha.rahman", 94, 21_403),
        ("Ben Okafor", "ben.okafor", 31, 8_120),
        ("Chloe Duval", "chloe.duval", 68, 15_902),
        ("Dmitri Sokolov", "dmitri.sokolov", 12, 3_004),
        ("Elena Marsh", "elena.marsh", 3, 210),
        ("Grace Lin", "grace.lin", 99, 44_781),
    ];

    let mut mailboxes: Vec<Mailbox> = people
        .iter()
        .enumerate()
        .map(|(index, (name, alias, percent, items))| {
            let quota = 100 * GIGABYTE;
            Mailbox {
                user_principal_name: format!("{alias}@contoso.co.uk"),
                display_name: (*name).into(),
                is_deleted: false,
                created: ago(900 - index as i64 * 40).map(|dt| dt.date_naive()),
                // A mailbox nobody has ever opened has no last activity at all,
                // which is a case the pane has to render.
                last_activity: (index != 4).then(|| {
                    (chrono::Utc::now() - Duration::days(index as i64)).date_naive()
                }),
                item_count: *items,
                storage_used: quota / 100 * *percent,
                issue_warning_quota: quota / 100 * 90,
                prohibit_send_quota: quota / 100 * 98,
                prohibit_send_receive_quota: quota,
                deleted_item_count: *items / 20,
                deleted_item_size: GIGABYTE / 2,
                has_archive: Some(index % 2 == 0),
            }
        })
        .collect();

    // Fullest first, matching what the real client does with the report.
    mailboxes.sort_by(|a, b| b.usage_fraction().total_cmp(&a.usage_fraction()));
    Fetch::Ready(Arc::new(mailboxes))
}

/// Mailbox settings for one mailbox, with an out-of-office on the busiest one.
pub fn mailbox_settings(upn: &str) -> Fetch<MailboxSettings> {
    if upn.starts_with("dmitri") {
        // Not every mailbox is readable with a delegated sign-in, and the pane
        // has to say so gracefully.
        return Fetch::Unavailable(
            "These mailbox settings are not readable with the current sign-in.\n\n\
             403 — ErrorAccessDenied: Access is denied. Check credentials and try again."
                .into(),
        );
    }

    let on = upn.starts_with("elena");
    Fetch::Ready(MailboxSettings {
        time_zone: Some("GMT Standard Time".into()),
        date_format: Some("dd/MM/yyyy".into()),
        time_format: Some("HH:mm".into()),
        language: Some(LocaleInfo {
            locale: Some("en-GB".into()),
            display_name: Some("English (United Kingdom)".into()),
        }),
        user_purpose: Some(serde_json::json!("user")),
        automatic_replies_setting: Some(AutomaticReplies {
            status: Some(if on { "alwaysEnabled" } else { "disabled" }.into()),
            external_audience: Some("all".into()),
            scheduled_start_date_time: None,
            scheduled_end_date_time: None,
            internal_reply_message: on.then(|| {
                "<html><body><p>On parental leave until March.<br>Please contact \
                 the People team.</p></body></html>"
                    .to_string()
            }),
            external_reply_message: on.then(|| {
                "<html><body><p>On leave. Please contact people@contoso.co.uk.</p>\
                 </body></html>"
                    .to_string()
            }),
        }),
    })
}

/// Set `GCM_DEMO_NO_AUDIT=1` to exercise the unavailable-feature path, which is
/// the common case on a tenant without Entra ID P1.
pub fn sign_ins() -> Fetch<Arc<Vec<SignIn>>> {
    if unavailable("GCM_DEMO_NO_AUDIT") {
        return Fetch::Unavailable(
            "This tenant does not expose the sign-in log. It needs Microsoft Entra ID \
             P1 or P2, the app registration needs AuditLog.Read.All, and the signed-in \
             account needs a role that can read reports.\n\n403 — \
             Authentication_RequestFromUnsupportedUserRole: Neither tenant is B2C or \
             tenant doesn't have premium license"
                .into(),
        );
    }

    let specs = [
        ("Aisha Rahman", "aisha.rahman", "Microsoft Teams", 0, "none", "London"),
        ("Ben Okafor", "ben.okafor", "Office 365 Exchange Online", 0, "none", "Leeds"),
        ("Grace Lin", "grace.lin", "Azure Portal", 50126, "none", "London"),
        ("Jonah Whitfield", "jonah.whitfield", "Microsoft Graph", 53003, "atRisk", "Lagos"),
        ("Liam Byrne", "liam.byrne", "Windows Sign In", 0, "none", "London"),
        ("Chloe Duval", "chloe.duval", "SharePoint Online", 0, "remediated", "Leeds"),
        ("Grace Lin", "grace.lin", "Azure Portal", 0, "none", "London"),
        ("Elena Marsh", "elena.marsh", "Microsoft Teams", 50058, "none", "Manchester"),
    ];

    Fetch::Ready(Arc::new(
        specs
            .iter()
            .enumerate()
            .map(|(index, (name, alias, app, error, risk, city))| SignIn {
                id: format!("signin-{index:04}"),
                created_date_time: Some(
                    chrono::Utc::now() - Duration::hours(index as i64 * 3 + 1),
                ),
                user_display_name: Some((*name).into()),
                user_principal_name: Some(format!("{alias}@contoso.co.uk")),
                app_display_name: Some((*app).into()),
                resource_display_name: Some("Microsoft Graph".into()),
                ip_address: Some(format!("203.0.113.{}", 10 + index)),
                client_app_used: Some("Browser".into()),
                correlation_id: Some(format!("cccccccc-0000-0000-0000-{index:012}")),
                conditional_access_status: Some(
                    if *error == 0 { "success" } else { "failure" }.into(),
                ),
                is_interactive: Some(true),
                risk_state: Some((*risk).into()),
                risk_level_during_sign_in: Some(
                    if *risk == "atRisk" { "medium" } else { "none" }.into(),
                ),
                risk_detail: Some("none".into()),
                status: Some(SignInStatus {
                    error_code: Some(*error),
                    failure_reason: (*error != 0).then(|| {
                        match error {
                            50126 => "Invalid username or password.",
                            53003 => "Access has been blocked by Conditional Access policies.",
                            _ => "The user did not complete multi-factor authentication.",
                        }
                        .to_string()
                    }),
                    additional_details: None,
                }),
                device_detail: Some(SignInDevice {
                    device_id: Some(format!("aaaaaaaa-0000-0000-0000-{index:012}")),
                    display_name: Some(format!("LON-LT-{index:04}")),
                    operating_system: Some("Windows 10".into()),
                    browser: Some("Edge 131.0".into()),
                    is_compliant: Some(*error == 0),
                    is_managed: Some(true),
                    trust_type: Some("AzureAd".into()),
                }),
                location: Some(SignInLocation {
                    city: Some((*city).into()),
                    state: Some("England".into()),
                    country_or_region: Some("GB".into()),
                }),
            })
            .collect(),
    ))
}

pub fn audits() -> Fetch<Arc<Vec<DirectoryAudit>>> {
    if unavailable("GCM_DEMO_NO_AUDIT") {
        return Fetch::Unavailable(
            "This tenant does not expose the directory audit log. The app registration \
             needs AuditLog.Read.All, and the signed-in account needs a role that can \
             read reports.\n\n403 — Authorization_RequestDenied: Insufficient \
             privileges to complete the operation."
                .into(),
        );
    }

    let specs = [
        ("Add member to group", "GroupManagement", "Finance Team", "success"),
        ("Update user", "UserManagement", "Ben Okafor", "success"),
        ("Reset user password", "UserManagement", "Chloe Duval", "success"),
        ("Add member to role", "RoleManagement", "Liam Byrne", "success"),
        ("Delete group", "GroupManagement", "Old Project", "success"),
        ("Update device", "DeviceManagement", "LON-LT-0042", "failure"),
        ("Disable account", "UserManagement", "Jonah Whitfield", "success"),
    ];

    Fetch::Ready(Arc::new(
        specs
            .iter()
            .enumerate()
            .map(|(index, (activity, category, target, result))| DirectoryAudit {
                id: format!("audit-{index:04}"),
                activity_date_time: Some(
                    chrono::Utc::now() - Duration::hours(index as i64 * 5 + 2),
                ),
                activity_display_name: Some((*activity).into()),
                category: Some((*category).into()),
                correlation_id: Some(format!("dddddddd-0000-0000-0000-{index:012}")),
                result: Some((*result).into()),
                result_reason: (*result == "failure")
                    .then(|| "Insufficient privileges to complete the operation".to_string()),
                logged_by_service: Some("Core Directory".into()),
                operation_type: Some(if activity.starts_with("Add") {
                    "Add"
                } else {
                    "Update"
                }
                .into()),
                // Alternating between a person and an application, because both
                // shapes appear in a real log and the pane renders them
                // differently.
                initiated_by: Some(AuditInitiator {
                    user: (index % 3 != 2).then(|| AuditUser {
                        id: Some("user-0000".into()),
                        display_name: Some("Aisha Rahman".into()),
                        user_principal_name: Some("aisha.rahman@contoso.co.uk".into()),
                        ip_address: Some("203.0.113.10".into()),
                    }),
                    app: (index % 3 == 2).then(|| AuditApp {
                        app_id: Some("00000003-0000-0000-c000-000000000000".into()),
                        display_name: Some("Microsoft Approval Management".into()),
                        service_principal_name: None,
                    }),
                }),
                target_resources: vec![AuditTargetResource {
                    id: Some(format!("object-{index:04}")),
                    display_name: Some((*target).into()),
                    resource_type: Some(if category.starts_with("Group") {
                        "Group"
                    } else {
                        "User"
                    }
                    .into()),
                    user_principal_name: None,
                    modified_properties: vec![AuditModifiedProperty {
                        display_name: Some("AccountEnabled".into()),
                        old_value: Some("\"true\"".into()),
                        new_value: Some("\"false\"".into()),
                    }],
                }],
                additional_details: vec![AuditKeyValue {
                    key: Some("User-Agent".into()),
                    value: Some(concat!("gcm/", env!("CARGO_PKG_VERSION")).into()),
                }],
            })
            .collect(),
    ))
}

/// A synthetic `[mariadb]` section, so the export dialog can be opened and read
/// without a database to point it at.
///
/// The export itself is simulated rather than attempted — see
/// `App::simulate_database_export`. What this exercises is the part that is
/// hard to get right and easy to get wrong: whether the dialog names the right
/// tables, counts the right rows, and says clearly enough that it is about to
/// replace them.
pub fn mariadb() -> crate::config::MariaDb {
    crate::config::MariaDb {
        host: "db.contoso.internal".into(),
        port: 3306,
        user: "gcm_export".into(),
        database: "m365".into(),
        table_prefix: "gcm_".into(),
        require_tls: true,
    }
}

/// Whether a `GCM_DEMO_NO_*` switch is set.
fn unavailable(variable: &str) -> bool {
    std::env::var(variable).is_ok_and(|value| value == "1")
}

pub fn members(count: usize, seed: usize) -> Arc<Vec<DirectoryMember>> {
    let names = [
        "Aisha Rahman",
        "Ben Okafor",
        "Chloe Duval",
        "Dmitri Sokolov",
        "Grace Lin",
        "Hamish Reid",
        "Kiara Mensah",
        "Liam Byrne",
    ];
    Arc::new(
        (0..count)
            .map(|index| {
                let name = names[(index + seed) % names.len()];
                DirectoryMember {
                    id: format!("member-{seed:02}-{index:04}"),
                    display_name: Some(name.into()),
                    user_principal_name: Some(format!(
                        "{}@contoso.co.uk",
                        name.to_lowercase().replace(' ', ".")
                    )),
                    odata_type: Some("#microsoft.graph.user".into()),
                }
            })
            .collect(),
    )
}
