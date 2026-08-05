//! Deserialized Microsoft Graph resources.
//!
//! Every field beyond the identifier is optional. Graph omits properties the
//! caller lacks permission for, and omits most properties entirely unless they
//! are named in `$select`, so `Option` is the honest representation.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

/// Formats a timestamp for display, or `—` when absent.
pub fn fmt_date(value: &Option<DateTime<Utc>>) -> String {
    match value {
        Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        None => "—".into(),
    }
}

/// Formats a date with no time component, or `—` when absent.
pub fn fmt_day(value: &Option<NaiveDate>) -> String {
    match value {
        Some(date) => date.format("%Y-%m-%d").to_string(),
        None => "—".into(),
    }
}

/// Bytes as a human-readable size. Mailbox quotas run to tens of gigabytes and
/// item counts to six figures, so the unit has to scale.
pub fn fmt_bytes(bytes: i64) -> String {
    const UNITS: [(&str, f64); 4] = [
        ("TB", 1_099_511_627_776.0),
        ("GB", 1_073_741_824.0),
        ("MB", 1_048_576.0),
        ("KB", 1024.0),
    ];
    let value = bytes as f64;
    for (unit, size) in UNITS {
        if value >= size {
            return format!("{:.1} {unit}", value / size);
        }
    }
    format!("{bytes} B")
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

// ---- Sign-in and audit logs -------------------------------------------------

/// One entry from the Entra sign-in log.
///
/// `/auditLogs/signIns` does not honour `$select`, so the whole resource
/// arrives whether or not it is wanted; the fields left out here are simply
/// ignored during deserialization.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    pub id: String,
    pub created_date_time: Option<DateTime<Utc>>,
    pub user_display_name: Option<String>,
    pub user_principal_name: Option<String>,
    pub app_display_name: Option<String>,
    pub resource_display_name: Option<String>,
    pub ip_address: Option<String>,
    pub client_app_used: Option<String>,
    pub correlation_id: Option<String>,
    pub conditional_access_status: Option<String>,
    pub is_interactive: Option<bool>,
    pub risk_state: Option<String>,
    pub risk_level_during_sign_in: Option<String>,
    pub risk_detail: Option<String>,
    pub status: Option<SignInStatus>,
    pub device_detail: Option<SignInDevice>,
    pub location: Option<SignInLocation>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SignInStatus {
    pub error_code: Option<i64>,
    pub failure_reason: Option<String>,
    pub additional_details: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SignInDevice {
    pub device_id: Option<String>,
    pub display_name: Option<String>,
    pub operating_system: Option<String>,
    pub browser: Option<String>,
    pub is_compliant: Option<bool>,
    pub is_managed: Option<bool>,
    pub trust_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SignInLocation {
    pub city: Option<String>,
    pub state: Option<String>,
    pub country_or_region: Option<String>,
}

impl SignIn {
    pub fn name(&self) -> &str {
        self.user_display_name
            .as_deref()
            .or(self.user_principal_name.as_deref())
            .unwrap_or("(unknown)")
    }

    pub fn upn(&self) -> &str {
        self.user_principal_name.as_deref().unwrap_or("—")
    }

    /// Graph signals success with error code `0`, not with a status string.
    pub fn succeeded(&self) -> bool {
        self.status
            .as_ref()
            .and_then(|status| status.error_code)
            .is_none_or(|code| code == 0)
    }

    pub fn outcome(&self) -> &'static str {
        if self.succeeded() { "Success" } else { "Failure" }
    }

    /// Why a failed sign-in failed, with the error code an admin can look up.
    pub fn failure(&self) -> Option<String> {
        if self.succeeded() {
            return None;
        }
        let status = self.status.as_ref()?;
        let code = status.error_code.unwrap_or(0);
        Some(match status.failure_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => format!("{code}: {reason}"),
            _ => format!("Error {code}"),
        })
    }

    pub fn location_display(&self) -> String {
        let Some(location) = &self.location else {
            return "—".into();
        };
        let parts: Vec<&str> = [
            location.city.as_deref(),
            location.state.as_deref(),
            location.country_or_region.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect();
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(", ")
        }
    }

    pub fn device_display(&self) -> String {
        let Some(device) = &self.device_detail else {
            return "—".into();
        };
        let parts: Vec<&str> = [
            device.display_name.as_deref(),
            device.operating_system.as_deref(),
            device.browser.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect();
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" · ")
        }
    }

    /// Conditional Access outcome in the wording the Entra portal uses.
    pub fn conditional_access(&self) -> &'static str {
        match self.conditional_access_status.as_deref() {
            Some("success") => "Applied",
            Some("failure") => "Failed",
            Some("notApplied") => "Not applied",
            Some("unknownFutureValue") | None => "—",
            Some(_) => "Reported",
        }
    }

    /// True when Entra flagged this sign-in as risky and nobody has cleared it.
    /// Worth surfacing in colour: it is the one column somebody is scanning for.
    pub fn is_risky(&self) -> bool {
        matches!(
            self.risk_state.as_deref(),
            Some("atRisk") | Some("confirmedCompromised")
        )
    }

    pub fn risk_display(&self) -> String {
        match self.risk_state.as_deref() {
            Some("none") | None => "None".into(),
            Some("atRisk") => "At risk".into(),
            Some("confirmedCompromised") => "Compromised".into(),
            Some("confirmedSafe") => "Confirmed safe".into(),
            Some("dismissed") => "Dismissed".into(),
            Some("remediated") => "Remediated".into(),
            Some(other) => other.into(),
        }
    }
}

/// One entry from the Entra directory audit log: who changed what, and whether
/// it worked.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryAudit {
    pub id: String,
    pub activity_date_time: Option<DateTime<Utc>>,
    pub activity_display_name: Option<String>,
    pub category: Option<String>,
    pub correlation_id: Option<String>,
    pub result: Option<String>,
    pub result_reason: Option<String>,
    pub logged_by_service: Option<String>,
    pub operation_type: Option<String>,
    pub initiated_by: Option<AuditInitiator>,
    #[serde(default)]
    pub target_resources: Vec<AuditTargetResource>,
    #[serde(default)]
    pub additional_details: Vec<AuditKeyValue>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditInitiator {
    pub user: Option<AuditUser>,
    pub app: Option<AuditApp>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditUser {
    /// Retained to mirror the Graph resource; the pane shows the UPN, which is
    /// what an administrator can act on.
    #[allow(dead_code)]
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub user_principal_name: Option<String>,
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditApp {
    /// As above: the display name is what identifies an app to a person.
    #[allow(dead_code)]
    pub app_id: Option<String>,
    pub display_name: Option<String>,
    pub service_principal_name: Option<String>,
}

/// An object an audited activity acted on. Graph capitalises `Type`
/// inconsistently between the documented sample and live responses, so both
/// spellings are accepted rather than one of them silently yielding `None`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditTargetResource {
    pub id: Option<String>,
    pub display_name: Option<String>,
    #[serde(rename = "type", alias = "Type")]
    pub resource_type: Option<String>,
    pub user_principal_name: Option<String>,
    #[serde(default)]
    pub modified_properties: Vec<AuditModifiedProperty>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditModifiedProperty {
    pub display_name: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuditKeyValue {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl AuditTargetResource {
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.user_principal_name.clone())
            .or_else(|| self.id.clone())
            .unwrap_or_else(|| "(unnamed)".into())
    }
}

impl DirectoryAudit {
    pub fn activity(&self) -> &str {
        self.activity_display_name
            .as_deref()
            .unwrap_or("(unnamed activity)")
    }

    /// Who did it. A user where there was one, otherwise the application —
    /// "Microsoft Approval Management" is a real and common answer here.
    pub fn actor(&self) -> String {
        let Some(initiated_by) = &self.initiated_by else {
            return "—".into();
        };
        if let Some(user) = &initiated_by.user
            && let Some(name) = user
                .user_principal_name
                .as_deref()
                .or(user.display_name.as_deref())
            && !name.trim().is_empty()
        {
            return name.to_string();
        }
        if let Some(app) = &initiated_by.app
            && let Some(name) = app
                .display_name
                .as_deref()
                .or(app.service_principal_name.as_deref())
            && !name.trim().is_empty()
        {
            return format!("{name} (application)");
        }
        "—".into()
    }

    pub fn actor_ip(&self) -> Option<&str> {
        self.initiated_by
            .as_ref()?
            .user
            .as_ref()?
            .ip_address
            .as_deref()
            .filter(|ip| !ip.trim().is_empty())
    }

    /// What it was done to. Most activities name one object; a bulk membership
    /// change names several.
    pub fn target(&self) -> String {
        if self.target_resources.is_empty() {
            return "—".into();
        }
        let first = self.target_resources[0].name();
        match self.target_resources.len() {
            1 => first,
            n => format!("{first} and {} more", n - 1),
        }
    }

    pub fn result_display(&self) -> String {
        match self.result.as_deref() {
            Some("success") => "Success".into(),
            Some("failure") => "Failure".into(),
            Some("timeout") => "Timeout".into(),
            Some("unknown") | None => "Unknown".into(),
            Some(other) => other.into(),
        }
    }

    pub fn succeeded(&self) -> bool {
        matches!(self.result.as_deref(), Some("success"))
    }
}

// ---- Microsoft Teams --------------------------------------------------------

/// A team. `GET /teams` populates only id, displayName, description and
/// visibility — everything else stays `None` until [`Self`] is refreshed from
/// `GET /teams/{id}`, which is why the details pane fetches on demand.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<String>,
    pub is_archived: Option<bool>,
    pub web_url: Option<String>,
    pub classification: Option<String>,
    pub specialization: Option<String>,
    pub created_date_time: Option<DateTime<Utc>>,
    /// Retained to mirror the Graph resource; it identifies the team to the
    /// Teams service and means nothing to an administrator.
    #[allow(dead_code)]
    pub internal_id: Option<String>,
    pub member_settings: Option<TeamMemberSettings>,
    pub guest_settings: Option<TeamGuestSettings>,
    pub messaging_settings: Option<TeamMessagingSettings>,
    pub fun_settings: Option<TeamFunSettings>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberSettings {
    pub allow_create_update_channels: Option<bool>,
    pub allow_delete_channels: Option<bool>,
    pub allow_add_remove_apps: Option<bool>,
    pub allow_create_update_remove_tabs: Option<bool>,
    pub allow_create_update_remove_connectors: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamGuestSettings {
    pub allow_create_update_channels: Option<bool>,
    pub allow_delete_channels: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamMessagingSettings {
    pub allow_user_edit_messages: Option<bool>,
    pub allow_user_delete_messages: Option<bool>,
    pub allow_owner_delete_messages: Option<bool>,
    pub allow_team_mentions: Option<bool>,
    pub allow_channel_mentions: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamFunSettings {
    pub allow_giphy: Option<bool>,
    pub giphy_content_rating: Option<String>,
    pub allow_stickers_and_memes: Option<bool>,
    pub allow_custom_memes: Option<bool>,
}

impl Team {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed team)")
    }

    pub fn archived(&self) -> bool {
        self.is_archived.unwrap_or(false)
    }

    /// An archived team is read-only in the Teams client, which is the single
    /// most useful thing to know about one at a glance.
    pub fn state(&self) -> &'static str {
        if self.archived() { "Archived" } else { "Active" }
    }

    /// Graph reports visibility lower-cased; the portal capitalises it.
    pub fn visibility_display(&self) -> String {
        match self.visibility.as_deref() {
            Some("public") => "Public".into(),
            Some("private") => "Private".into(),
            Some("hiddenMembership") => "Hidden membership".into(),
            Some(other) if !other.trim().is_empty() => other.into(),
            _ => "—".into(),
        }
    }
}

/// A channel within a team.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    /// Retained to mirror the Graph resource; nothing drills into a channel.
    #[allow(dead_code)]
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub membership_type: Option<String>,
    pub email: Option<String>,
    /// As above.
    #[allow(dead_code)]
    pub created_date_time: Option<DateTime<Utc>>,
}

impl Channel {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or("(unnamed channel)")
    }

    pub fn kind(&self) -> &'static str {
        match self.membership_type.as_deref() {
            Some("private") => "Private",
            Some("shared") => "Shared",
            Some("standard") => "Standard",
            _ => "—",
        }
    }
}

// ---- Exchange Online --------------------------------------------------------

/// One mailbox, from the `getMailboxUsageDetail` report.
///
/// Graph has no "list every mailbox" collection, so this comes out of the usage
/// report instead — which has the happy side effect of carrying the numbers an
/// administrator actually opens Exchange for: size against quota, item count,
/// and when the mailbox was last touched.
#[derive(Debug, Clone, Default)]
pub struct Mailbox {
    pub user_principal_name: String,
    pub display_name: String,
    pub is_deleted: bool,
    pub created: Option<NaiveDate>,
    pub last_activity: Option<NaiveDate>,
    pub item_count: i64,
    pub storage_used: i64,
    pub issue_warning_quota: i64,
    pub prohibit_send_quota: i64,
    pub prohibit_send_receive_quota: i64,
    pub deleted_item_count: i64,
    pub deleted_item_size: i64,
    pub has_archive: Option<bool>,
}

impl Mailbox {
    pub fn name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            self.upn()
        } else {
            &self.display_name
        }
    }

    pub fn upn(&self) -> &str {
        if self.user_principal_name.trim().is_empty() {
            "—"
        } else {
            &self.user_principal_name
        }
    }

    /// The quota that actually stops mail arriving. Falls back to the send
    /// quota, then the warning threshold, since not every tenant sets all three.
    pub fn quota(&self) -> i64 {
        [
            self.prohibit_send_receive_quota,
            self.prohibit_send_quota,
            self.issue_warning_quota,
        ]
        .into_iter()
        .find(|value| *value > 0)
        .unwrap_or(0)
    }

    /// Fraction of the quota consumed, for the usage bar. Mirrors
    /// [`SubscribedSku::usage_fraction`] so the two bars read the same way.
    pub fn usage_fraction(&self) -> f32 {
        let quota = self.quota();
        if quota <= 0 {
            return 0.0;
        }
        (self.storage_used as f32 / quota as f32).clamp(0.0, 1.0)
    }

    pub fn storage_display(&self) -> String {
        match self.quota() {
            0 => fmt_bytes(self.storage_used),
            quota => format!("{} of {}", fmt_bytes(self.storage_used), fmt_bytes(quota)),
        }
    }

    /// True when the tenant has anonymised the report, so the names are GUIDs
    /// rather than people. Worth saying out loud rather than leaving an admin
    /// to wonder why every mailbox looks like a serial number.
    pub fn is_concealed(&self) -> bool {
        !self.user_principal_name.contains('@') && !self.user_principal_name.is_empty()
    }
}

/// Per-mailbox settings, fetched on demand for the selected mailbox.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSettings {
    pub time_zone: Option<String>,
    pub date_format: Option<String>,
    pub time_format: Option<String>,
    pub language: Option<LocaleInfo>,
    pub automatic_replies_setting: Option<AutomaticReplies>,
    /// Graph has returned this both as a bare string and as `{"value": "user"}`
    /// depending on the path used, so it is kept raw and normalised on read.
    pub user_purpose: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocaleInfo {
    pub locale: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticReplies {
    /// `disabled`, `alwaysEnabled` or `scheduled`.
    pub status: Option<String>,
    /// `none`, `contactsOnly` or `all`.
    pub external_audience: Option<String>,
    pub scheduled_start_date_time: Option<GraphDateTime>,
    pub scheduled_end_date_time: Option<GraphDateTime>,
    pub internal_reply_message: Option<String>,
    pub external_reply_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphDateTime {
    pub date_time: Option<String>,
    pub time_zone: Option<String>,
}

impl MailboxSettings {
    pub fn purpose_display(&self) -> String {
        match &self.user_purpose {
            Some(serde_json::Value::String(value)) => value.clone(),
            Some(serde_json::Value::Object(map)) => map
                .get("value")
                .and_then(|value| value.as_str())
                .unwrap_or("—")
                .to_string(),
            _ => "—".into(),
        }
    }

    pub fn language_display(&self) -> String {
        match &self.language {
            Some(locale) => locale
                .display_name
                .clone()
                .or_else(|| locale.locale.clone())
                .unwrap_or_else(|| "—".into()),
            None => "—".into(),
        }
    }
}

impl AutomaticReplies {
    pub fn is_on(&self) -> bool {
        matches!(
            self.status.as_deref(),
            Some("alwaysEnabled") | Some("scheduled")
        )
    }

    pub fn status_display(&self) -> &'static str {
        match self.status.as_deref() {
            Some("alwaysEnabled") => "On",
            Some("scheduled") => "Scheduled",
            Some("disabled") | None => "Off",
            Some(_) => "Unknown",
        }
    }

    pub fn audience_display(&self) -> &'static str {
        match self.external_audience.as_deref() {
            Some("all") => "Everyone outside the organisation",
            Some("contactsOnly") => "External contacts only",
            Some("none") => "Inside the organisation only",
            _ => "—",
        }
    }

    /// The reply body as plain text. Outlook stores it as HTML, and a property
    /// sheet showing raw `<html><body>` markup would be useless.
    pub fn internal_text(&self) -> String {
        strip_html(self.internal_reply_message.as_deref().unwrap_or(""))
    }

    pub fn external_text(&self) -> String {
        strip_html(self.external_reply_message.as_deref().unwrap_or(""))
    }
}

/// Reduce an HTML fragment to readable text.
///
/// Deliberately crude — this renders an out-of-office message into a read-only
/// label, not a browser. Block-level tags become line breaks so paragraphs
/// survive; everything else is dropped, and the handful of entities Outlook
/// actually emits are decoded.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        for inner in chars.by_ref() {
            if inner == '>' {
                break;
            }
            tag.push(inner);
        }
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "p" | "br" | "div" | "tr" | "li" | "h1" | "h2" | "h3"
        ) {
            out.push('\n');
        }
    }

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    // Collapse the run of blank lines that `<html><body><p>` leaves behind.
    decoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Organization {
    /// Whether the tenant has Exchange Online provisioned.
    pub fn has_exchange(&self) -> bool {
        self.has_service(&["exchange"])
    }

    /// Whether the tenant has Microsoft Teams provisioned.
    ///
    /// Teams is licensed under the `TeamspaceAPI` service in `assignedPlans`,
    /// which is not a name anybody would guess; `MicrosoftCommunicationsOnline`
    /// covers the older Skype-derived offerings.
    pub fn has_teams(&self) -> bool {
        self.has_service(&["teamspaceapi", "microsoftcommunicationsonline"])
    }

    fn has_service(&self, names: &[&str]) -> bool {
        self.assigned_plans.iter().any(|plan| {
            let service = plan.service.as_deref().unwrap_or("").to_ascii_lowercase();
            let enabled = matches!(plan.capability_status.as_deref(), Some("Enabled"));
            enabled && names.contains(&service.as_str())
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

    #[test]
    fn detects_exchange_and_teams_plans() {
        let org = Organization {
            assigned_plans: vec![
                AssignedPlan {
                    service: Some("exchange".into()),
                    capability_status: Some("Enabled".into()),
                    ..Default::default()
                },
                AssignedPlan {
                    service: Some("TeamspaceAPI".into()),
                    capability_status: Some("Enabled".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        // The service names are compared case-insensitively; Graph is not
        // consistent about how it capitalises them.
        assert!(org.has_exchange());
        assert!(org.has_teams());
        assert!(!Organization::default().has_exchange());
        assert!(!Organization::default().has_teams());
    }

    #[test]
    fn a_sign_in_with_error_code_zero_succeeded() {
        // Graph reports success as errorCode 0, not as a status string, so
        // reading it the obvious way would mark every success a failure.
        let ok = SignIn {
            status: Some(SignInStatus {
                error_code: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(ok.succeeded());
        assert_eq!(ok.outcome(), "Success");
        assert_eq!(ok.failure(), None);

        let denied = SignIn {
            status: Some(SignInStatus {
                error_code: Some(50126),
                failure_reason: Some("Invalid username or password".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!denied.succeeded());
        assert_eq!(
            denied.failure().as_deref(),
            Some("50126: Invalid username or password")
        );
    }

    #[test]
    fn a_sign_in_without_a_status_is_not_reported_as_failed() {
        assert!(SignIn::default().succeeded());
    }

    #[test]
    fn risky_sign_ins_are_only_the_unresolved_ones() {
        let risky = |state: &str| SignIn {
            risk_state: Some(state.into()),
            ..Default::default()
        };
        assert!(risky("atRisk").is_risky());
        assert!(risky("confirmedCompromised").is_risky());
        // Already dealt with, so it must not keep shouting.
        assert!(!risky("remediated").is_risky());
        assert!(!risky("dismissed").is_risky());
        assert!(!risky("none").is_risky());
    }

    #[test]
    fn audit_actor_falls_back_from_user_to_app() {
        let by_app = DirectoryAudit {
            initiated_by: Some(AuditInitiator {
                user: None,
                app: Some(AuditApp {
                    display_name: Some("Microsoft Approval Management".into()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        assert_eq!(by_app.actor(), "Microsoft Approval Management (application)");

        // A user record whose name came back blank must not win over the app.
        let blank_user = DirectoryAudit {
            initiated_by: Some(AuditInitiator {
                user: Some(AuditUser::default()),
                app: Some(AuditApp {
                    display_name: Some("Some Service".into()),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        assert_eq!(blank_user.actor(), "Some Service (application)");
    }

    #[test]
    fn audit_target_type_parses_either_capitalisation() {
        // The documented sample spells it `Type`; live responses spell it
        // `type`. Accepting only one loses the column on half of all tenants.
        let upper: AuditTargetResource =
            serde_json::from_str(r#"{"Type":"Group","displayName":"Finance"}"#)
                .expect("should parse");
        let lower: AuditTargetResource =
            serde_json::from_str(r#"{"type":"User","displayName":"Aisha"}"#)
                .expect("should parse");
        assert_eq!(upper.resource_type.as_deref(), Some("Group"));
        assert_eq!(lower.resource_type.as_deref(), Some("User"));
    }

    #[test]
    fn audit_summarises_several_targets() {
        let audit = DirectoryAudit {
            target_resources: vec![
                AuditTargetResource {
                    display_name: Some("Finance Team".into()),
                    ..Default::default()
                },
                AuditTargetResource::default(),
                AuditTargetResource::default(),
            ],
            ..Default::default()
        };
        assert_eq!(audit.target(), "Finance Team and 2 more");
        assert_eq!(DirectoryAudit::default().target(), "—");
    }

    #[test]
    fn mailbox_quota_falls_back_through_the_thresholds() {
        // Not every tenant sets all three; the bar must still mean something.
        let only_warning = Mailbox {
            issue_warning_quota: 100,
            storage_used: 50,
            ..Default::default()
        };
        assert_eq!(only_warning.quota(), 100);
        assert_eq!(only_warning.usage_fraction(), 0.5);

        // No quota at all must not divide by zero or claim 100% usage.
        let unlimited = Mailbox {
            storage_used: 50,
            ..Default::default()
        };
        assert_eq!(unlimited.quota(), 0);
        assert_eq!(unlimited.usage_fraction(), 0.0);
    }

    #[test]
    fn mailbox_over_quota_clamps_at_full() {
        let over = Mailbox {
            prohibit_send_receive_quota: 100,
            storage_used: 140,
            ..Default::default()
        };
        assert_eq!(over.usage_fraction(), 1.0);
    }

    #[test]
    fn recognises_a_concealed_report() {
        let concealed = Mailbox {
            user_principal_name: "6EB4C2C1E4B9D2A0".into(),
            ..Default::default()
        };
        let plain = Mailbox {
            user_principal_name: "aisha.rahman@contoso.co.uk".into(),
            ..Default::default()
        };
        assert!(concealed.is_concealed());
        assert!(!plain.is_concealed());
        // An empty name is missing data, not anonymised data.
        assert!(!Mailbox::default().is_concealed());
    }

    #[test]
    fn out_of_office_html_becomes_readable_text() {
        let replies = AutomaticReplies {
            internal_reply_message: Some(
                "<html>\n<body>\n<p>On leave until Monday.<br>\nCall Ben &amp; Chloe.</p>\
                 </body>\n</html>\n"
                    .into(),
            ),
            ..Default::default()
        };
        assert_eq!(
            replies.internal_text(),
            "On leave until Monday.\nCall Ben & Chloe."
        );
    }

    #[test]
    fn out_of_office_status_reads_as_on_or_off() {
        let scheduled = AutomaticReplies {
            status: Some("scheduled".into()),
            ..Default::default()
        };
        assert!(scheduled.is_on());
        assert_eq!(scheduled.status_display(), "Scheduled");
        assert!(!AutomaticReplies::default().is_on());
        assert_eq!(AutomaticReplies::default().status_display(), "Off");
    }

    #[test]
    fn byte_sizes_scale_to_a_sensible_unit() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(50 * 1_073_741_824), "50.0 GB");
    }

    #[test]
    fn user_purpose_parses_both_shapes_graph_returns() {
        let bare: MailboxSettings =
            serde_json::from_str(r#"{"userPurpose":"shared"}"#).expect("should parse");
        let wrapped: MailboxSettings =
            serde_json::from_str(r#"{"userPurpose":{"value":"room"}}"#).expect("should parse");
        assert_eq!(bare.purpose_display(), "shared");
        assert_eq!(wrapped.purpose_display(), "room");
        assert_eq!(MailboxSettings::default().purpose_display(), "—");
    }
}
