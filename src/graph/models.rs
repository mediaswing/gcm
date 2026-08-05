//! Deserialized Microsoft Graph resources.
//!
//! Every field beyond the identifier is optional. Graph omits properties the
//! caller lacks permission for, and omits most properties entirely unless they
//! are named in `$select`, so `Option` is the honest representation.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Formats a timestamp for display, or `—` when absent.
pub fn fmt_date(value: &Option<DateTime<Utc>>) -> String {
    match value {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "—".into(),
    }
}

/// Formats an optional string for display, or `—` when absent or blank.
pub fn fmt_opt(value: &Option<String>) -> String {
    match value {
        Some(s) if !s.trim().is_empty() => s.clone(),
        _ => "—".into(),
    }
}

/// Formats a tri-state boolean: Graph distinguishes false from unknown.
pub fn fmt_bool(value: &Option<bool>) -> String {
    match value {
        Some(true) => "Yes".into(),
        Some(false) => "No".into(),
        None => "—".into(),
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub display_name: Option<String>,
    pub user_principal_name: Option<String>,
    pub mail: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    pub office_location: Option<String>,
    pub mobile_phone: Option<String>,
    pub account_enabled: Option<bool>,
    pub user_type: Option<String>,
    pub created_date_time: Option<DateTime<Utc>>,
    pub last_password_change_date_time: Option<DateTime<Utc>>,
    pub on_premises_sync_enabled: Option<bool>,
    pub on_premises_sam_account_name: Option<String>,
    pub usage_location: Option<String>,
    #[serde(default)]
    pub assigned_licenses: Vec<AssignedLicense>,
    #[serde(default)]
    pub business_phones: Vec<String>,
    #[serde(default)]
    pub proxy_addresses: Vec<String>,
}

impl User {
    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.user_principal_name.as_deref())
            .unwrap_or("(unnamed)")
    }

    pub fn upn(&self) -> &str {
        self.user_principal_name.as_deref().unwrap_or("—")
    }

    pub fn status(&self) -> &'static str {
        match self.account_enabled {
            Some(true) => "Enabled",
            Some(false) => "Disabled",
            None => "—",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssignedLicense {
    pub sku_id: Option<String>,
    #[serde(default)]
    pub disabled_plans: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub mail: Option<String>,
    pub mail_nickname: Option<String>,
    pub mail_enabled: Option<bool>,
    pub security_enabled: Option<bool>,
    pub visibility: Option<String>,
    pub created_date_time: Option<DateTime<Utc>>,
    pub membership_rule: Option<String>,
    pub membership_rule_processing_state: Option<String>,
    pub on_premises_sync_enabled: Option<bool>,
    pub is_assignable_to_role: Option<bool>,
    #[serde(default)]
    pub group_types: Vec<String>,
}

impl Group {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed)")
    }

    /// The classification an admin actually thinks in: Microsoft 365,
    /// Distribution, Mail-enabled security, or Security.
    pub fn kind(&self) -> &'static str {
        let unified = self.group_types.iter().any(|t| t == "Unified");
        match (
            unified,
            self.mail_enabled.unwrap_or(false),
            self.security_enabled.unwrap_or(false),
        ) {
            (true, _, _) => "Microsoft 365",
            (false, true, true) => "Mail-enabled security",
            (false, true, false) => "Distribution",
            (false, false, _) => "Security",
        }
    }

    /// Dynamic groups carry a membership rule; everything else is assigned.
    pub fn membership(&self) -> &'static str {
        if self.group_types.iter().any(|t| t.starts_with("Dynamic")) {
            "Dynamic"
        } else {
            "Assigned"
        }
    }

    pub fn source(&self) -> &'static str {
        if self.on_premises_sync_enabled.unwrap_or(false) {
            "Windows Server AD"
        } else {
            "Cloud"
        }
    }
}

/// A directory role. Roles are listed alongside groups because administrators
/// reason about both as "things that grant a person something".
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRole {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub role_template_id: Option<String>,
}

impl DirectoryRole {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed role)")
    }
}

/// A member of a group or role. Graph returns a heterogeneous collection, so
/// the OData type tells us whether this is a user, group, device, or service
/// principal.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryMember {
    /// Kept so a future drill-through can resolve the member; not displayed.
    #[allow(dead_code)]
    pub id: String,
    pub display_name: Option<String>,
    pub user_principal_name: Option<String>,
    #[serde(rename = "@odata.type")]
    pub odata_type: Option<String>,
}

impl DirectoryMember {
    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.user_principal_name.as_deref())
            .unwrap_or("(unnamed)")
    }

    /// `#microsoft.graph.user` becomes `User`.
    pub fn kind(&self) -> String {
        let raw = self
            .odata_type
            .as_deref()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("");
        match raw {
            "user" => "User".into(),
            "group" => "Group".into(),
            "device" => "Device".into(),
            "servicePrincipal" => "Service principal".into(),
            "orgContact" => "Contact".into(),
            "" => "Member".into(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => "Member".into(),
                }
            }
        }
    }
}

/// An Entra ID registered/joined device.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub device_id: Option<String>,
    pub display_name: Option<String>,
    pub operating_system: Option<String>,
    pub operating_system_version: Option<String>,
    pub trust_type: Option<String>,
    pub profile_type: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub is_compliant: Option<bool>,
    pub is_managed: Option<bool>,
    pub account_enabled: Option<bool>,
    pub approximate_last_sign_in_date_time: Option<DateTime<Utc>>,
    pub registration_date_time: Option<DateTime<Utc>>,
    pub on_premises_sync_enabled: Option<bool>,
}

impl Device {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed device)")
    }

    /// Entra reports join type in a machine-readable form; translate to the
    /// wording used in the Entra portal.
    pub fn join_type(&self) -> &'static str {
        match self.trust_type.as_deref() {
            Some("AzureAd") => "Entra joined",
            Some("ServerAd") => "Hybrid joined",
            Some("Workplace") => "Entra registered",
            _ => "—",
        }
    }

    pub fn os_display(&self) -> String {
        match (&self.operating_system, &self.operating_system_version) {
            (Some(os), Some(version)) => format!("{os} {version}"),
            (Some(os), None) => os.clone(),
            _ => "—".into(),
        }
    }
}

/// An Intune-managed device. Only present when the tenant licenses Intune.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDevice {
    pub id: String,
    pub device_name: Option<String>,
    pub managed_device_owner_type: Option<String>,
    pub operating_system: Option<String>,
    pub os_version: Option<String>,
    pub compliance_state: Option<String>,
    pub management_agent: Option<String>,
    pub enrolled_date_time: Option<DateTime<Utc>>,
    pub last_sync_date_time: Option<DateTime<Utc>>,
    pub user_principal_name: Option<String>,
    pub model: Option<String>,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub imei: Option<String>,
    pub is_encrypted: Option<bool>,
    pub is_supervised: Option<bool>,
    pub jail_broken: Option<String>,
    pub device_enrollment_type: Option<String>,
    pub total_storage_space_in_bytes: Option<i64>,
    pub free_storage_space_in_bytes: Option<i64>,
}

impl ManagedDevice {
    pub fn name(&self) -> &str {
        self.device_name.as_deref().unwrap_or("(unnamed device)")
    }

    pub fn os_display(&self) -> String {
        match (&self.operating_system, &self.os_version) {
            (Some(os), Some(version)) => format!("{os} {version}"),
            (Some(os), None) => os.clone(),
            _ => "—".into(),
        }
    }

    /// Intune's `managementAgent` values are terse; expand the common ones.
    pub fn agent_display(&self) -> String {
        match self.management_agent.as_deref() {
            Some("mdm") => "MDM".into(),
            Some("eas") => "Exchange ActiveSync".into(),
            Some("easMdm") => "EAS + MDM".into(),
            Some("configurationManagerClient") => "Configuration Manager".into(),
            Some("configurationManagerClientMdm") => "Co-managed".into(),
            Some("intuneClient") => "Intune client".into(),
            Some(other) => other.into(),
            None => "—".into(),
        }
    }

    pub fn compliance_display(&self) -> String {
        match self.compliance_state.as_deref() {
            Some("compliant") => "Compliant".into(),
            Some("noncompliant") => "Not compliant".into(),
            Some("inGracePeriod") => "In grace period".into(),
            Some("configManager") => "Managed by ConfigMgr".into(),
            Some("conflict") => "Conflict".into(),
            Some("error") => "Error".into(),
            Some("unknown") | None => "Unknown".into(),
            Some(other) => other.into(),
        }
    }

    pub fn storage_display(&self) -> String {
        match (self.free_storage_space_in_bytes, self.total_storage_space_in_bytes) {
            (Some(free), Some(total)) if total > 0 => {
                format!("{} free of {}", gigabytes(free), gigabytes(total))
            }
            _ => "—".into(),
        }
    }
}

fn gigabytes(bytes: i64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
}

/// A subscribed SKU — one purchased product with a seat count.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubscribedSku {
    /// Composite `tenantId_skuId`. The details pane shows `sku_id` instead,
    /// which is the identifier used when assigning licences.
    #[allow(dead_code)]
    pub id: String,
    pub sku_id: Option<String>,
    pub sku_part_number: Option<String>,
    pub applies_to: Option<String>,
    pub capability_status: Option<String>,
    pub consumed_units: Option<i64>,
    pub prepaid_units: Option<PrepaidUnits>,
    #[serde(default)]
    pub service_plans: Vec<ServicePlan>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PrepaidUnits {
    pub enabled: Option<i64>,
    pub suspended: Option<i64>,
    pub warning: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServicePlan {
    /// Retained to mirror the Graph resource; the pane keys off the name.
    #[allow(dead_code)]
    pub service_plan_id: Option<String>,
    pub service_plan_name: Option<String>,
    pub provisioning_status: Option<String>,
    pub applies_to: Option<String>,
}

impl SubscribedSku {
    pub fn part_number(&self) -> &str {
        self.sku_part_number.as_deref().unwrap_or("—")
    }

    /// Friendly product name where we know it, otherwise the raw part number.
    pub fn display_name(&self) -> String {
        crate::graph::skus::friendly_name(self.part_number())
    }

    pub fn total_seats(&self) -> i64 {
        self.prepaid_units
            .as_ref()
            .and_then(|u| u.enabled)
            .unwrap_or(0)
    }

    pub fn consumed(&self) -> i64 {
        self.consumed_units.unwrap_or(0)
    }

    pub fn available(&self) -> i64 {
        (self.total_seats() - self.consumed()).max(0)
    }

    /// Fraction of purchased seats in use, for the usage bar.
    pub fn usage_fraction(&self) -> f32 {
        let total = self.total_seats();
        if total <= 0 {
            return 0.0;
        }
        (self.consumed() as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// Tenant details, shown in the console root.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: String,
    pub display_name: Option<String>,
    pub tenant_type: Option<String>,
    pub country_letter_code: Option<String>,
    pub created_date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub verified_domains: Vec<VerifiedDomain>,
    #[serde(default)]
    pub assigned_plans: Vec<AssignedPlan>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedDomain {
    pub name: Option<String>,
    pub is_default: Option<bool>,
    /// True for the `*.onmicrosoft.com` domain created with the tenant.
    #[allow(dead_code)]
    pub is_initial: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssignedPlan {
    pub service: Option<String>,
    /// Retained to mirror the Graph resource; Intune detection keys off
    /// `service` because plan GUIDs change between offerings.
    #[allow(dead_code)]
    pub service_plan_id: Option<String>,
    pub capability_status: Option<String>,
}

impl Organization {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed tenant)")
    }

    pub fn default_domain(&self) -> String {
        self.verified_domains
            .iter()
            .find(|d| d.is_default.unwrap_or(false))
            .or_else(|| self.verified_domains.first())
            .and_then(|d| d.name.clone())
            .unwrap_or_else(|| "—".into())
    }

    /// Whether the tenant has any Intune service plan provisioned. Used to
    /// decide between "no devices" and "Intune is not enabled here".
    pub fn has_intune(&self) -> bool {
        self.assigned_plans.iter().any(|plan| {
            let service = plan.service.as_deref().unwrap_or("");
            let enabled = matches!(plan.capability_status.as_deref(), Some("Enabled"));
            enabled && (service == "SCO" || service.eq_ignore_ascii_case("Intune"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_with(types: &[&str], mail: bool, security: bool) -> Group {
        Group {
            group_types: types.iter().map(|s| s.to_string()).collect(),
            mail_enabled: Some(mail),
            security_enabled: Some(security),
            ..Default::default()
        }
    }

    #[test]
    fn classifies_group_kinds() {
        assert_eq!(group_with(&["Unified"], true, false).kind(), "Microsoft 365");
        assert_eq!(group_with(&[], false, true).kind(), "Security");
        assert_eq!(group_with(&[], true, false).kind(), "Distribution");
        assert_eq!(
            group_with(&[], true, true).kind(),
            "Mail-enabled security"
        );
    }

    #[test]
    fn detects_dynamic_membership() {
        let dynamic = group_with(&["DynamicMembership"], false, true);
        assert_eq!(dynamic.membership(), "Dynamic");
        assert_eq!(group_with(&[], false, true).membership(), "Assigned");
    }

    #[test]
    fn translates_device_trust_type() {
        let device = Device {
            trust_type: Some("ServerAd".into()),
            ..Default::default()
        };
        assert_eq!(device.join_type(), "Hybrid joined");
    }

    #[test]
    fn sku_seat_math_never_goes_negative() {
        // Over-assignment happens in trials; the UI must not show -3 available.
        let sku = SubscribedSku {
            consumed_units: Some(12),
            prepaid_units: Some(PrepaidUnits {
                enabled: Some(10),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(sku.available(), 0);
        assert_eq!(sku.usage_fraction(), 1.0);
    }

    #[test]
    fn sku_usage_handles_zero_seats() {
        let sku = SubscribedSku::default();
        assert_eq!(sku.usage_fraction(), 0.0);
        assert_eq!(sku.available(), 0);
    }

    #[test]
    fn detects_intune_service_plan() {
        let org = Organization {
            assigned_plans: vec![AssignedPlan {
                service: Some("SCO".into()),
                capability_status: Some("Enabled".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(org.has_intune());

        let deleted = Organization {
            assigned_plans: vec![AssignedPlan {
                service: Some("SCO".into()),
                capability_status: Some("Deleted".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!deleted.has_intune());
    }

    #[test]
    fn member_kind_from_odata_type() {
        let member = DirectoryMember {
            odata_type: Some("#microsoft.graph.servicePrincipal".into()),
            ..Default::default()
        };
        assert_eq!(member.kind(), "Service principal");
    }
}
