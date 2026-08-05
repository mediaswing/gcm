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
    if std::env::var("GCM_DEMO_NO_INTUNE").is_ok_and(|value| value == "1") {
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
