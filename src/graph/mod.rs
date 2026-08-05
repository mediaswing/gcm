//! A thin Microsoft Graph client covering the read surface gcm displays.
//!
//! Two behaviours matter more than breadth here:
//!
//! * **Paging.** Graph returns `@odata.nextLink` rather than offsets, so every
//!   collection is walked link by link until exhausted or until the configured
//!   `max_objects` ceiling is reached.
//! * **Graceful degradation.** A tenant without Intune, or an app registration
//!   without the device-management scope, answers `403` on the managed-device
//!   endpoints. That is a normal state to be reported in the UI, not an error to
//!   propagate, so those calls return [`Fetch::Unavailable`].

pub mod actions;
pub mod models;
pub mod skus;

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::auth::Authenticator;
use crate::config::Config;
use models::*;

/// How many times to ride out a 429 or 5xx before giving up.
const MAX_THROTTLE_RETRIES: u32 = 5;

/// Seconds Graph asked us to wait, when it says.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        // Guard against a header that would stall the UI indefinitely.
        .map(|seconds| Duration::from_secs(seconds.min(120)))
}

/// Pull the human-readable part out of Graph's error envelope.
fn describe_error(body: &str) -> String {
    serde_json::from_str::<GraphErrorEnvelope>(body)
        .map(|envelope| {
            let code = envelope.error.code;
            let message = envelope.error.message;
            if code.is_empty() {
                message
            } else {
                format!("{code}: {message}")
            }
        })
        .unwrap_or_else(|_| body.chars().take(300).collect())
}

/// The outcome of fetching a collection that the tenant may not have enabled.
#[derive(Debug, Clone)]
pub enum Fetch<T> {
    Ready(T),
    /// The tenant or app registration does not expose this data. Carries text
    /// suitable for display in the details pane.
    Unavailable(String),
}

/// Graph's error envelope.
#[derive(Debug, Deserialize)]
struct GraphErrorEnvelope {
    error: GraphErrorBody,
}

#[derive(Debug, Deserialize)]
struct GraphErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// A page of an OData collection.
#[derive(Debug, Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

pub struct GraphClient {
    config: Config,
    http: reqwest::Client,
    auth: Authenticator,
}

impl GraphClient {
    pub fn new(config: Config, http: reqwest::Client, auth: Authenticator) -> Self {
        Self { config, http, auth }
    }

    pub fn auth_mut(&mut self) -> &mut Authenticator {
        &mut self.auth
    }

    pub fn account(&self) -> Option<String> {
        self.auth.account()
    }

    /// The `/v1.0` base URL, needed when building `@odata.id` references.
    pub fn graph_base(&self) -> String {
        self.config.graph_url()
    }

    /// GET a single resource.
    async fn get_one<T: DeserializeOwned>(&mut self, path_and_query: &str) -> Result<T> {
        let url = format!("{}{}", self.config.graph_url(), path_and_query);
        let body = self.get_raw(&url).await?;
        serde_json::from_str(&body)
            .with_context(|| format!("parsing response from {path_and_query}"))
    }

    /// GET every page of a collection, honouring the configured ceiling.
    async fn get_all<T: DeserializeOwned>(&mut self, path_and_query: &str) -> Result<Vec<T>> {
        let mut url = format!("{}{}", self.config.graph_url(), path_and_query);
        let ceiling = self.config.query.max_objects as usize;
        let mut collected: Vec<T> = Vec::new();

        loop {
            let body = self.get_raw(&url).await?;
            let page: Page<T> = serde_json::from_str(&body)
                .with_context(|| format!("parsing response from {path_and_query}"))?;
            collected.extend(page.value);

            if ceiling > 0 && collected.len() >= ceiling {
                collected.truncate(ceiling);
                break;
            }

            // nextLink is absolute and already carries the original query
            // string, so it replaces the URL wholesale rather than being appended.
            match page.next_link {
                Some(next) => url = next,
                None => break,
            }
        }

        Ok(collected)
    }

    /// Issue an authenticated GET and return the body, mapping Graph's error
    /// envelope into an `anyhow` error that names the code and message.
    async fn get_raw(&mut self, url: &str) -> Result<String> {
        self.request(reqwest::Method::GET, url, None).await
    }

    /// Issue an authenticated request, retrying when Graph throttles.
    ///
    /// Graph answers `429` with a `Retry-After` header under load, and a bulk
    /// run will reliably provoke it. Retrying here means a batch slows down
    /// rather than failing halfway with some items applied — which is the
    /// difference between an inconvenience and a mess to reconcile by hand.
    async fn request(
        &mut self,
        method: reqwest::Method,
        url: &str,
        body: Option<serde_json::Value>,
    ) -> Result<String> {
        for attempt in 0..MAX_THROTTLE_RETRIES {
            let token = self.auth.access_token().await?;
            let mut builder = self
                .http
                .request(method.clone(), url)
                .bearer_auth(&token)
                // Ask for the richer error text and consistent metadata shape.
                .header("ConsistencyLevel", "eventual");
            if let Some(body) = &body {
                builder = builder.json(body);
            }

            let response = builder
                .send()
                .await
                .with_context(|| format!("requesting {url}"))?;

            let status = response.status();

            if status.as_u16() == 429 || status.is_server_error() {
                let wait = retry_after(&response)
                    // Exponential back-off when Graph does not say how long.
                    .unwrap_or_else(|| Duration::from_secs(2u64.pow(attempt) * 2));
                // Drain the body so the connection can be reused.
                let _ = response.bytes().await;
                tokio::time::sleep(wait).await;
                continue;
            }

            let body = response
                .text()
                .await
                .with_context(|| format!("reading response from {url}"))?;

            if status.is_success() {
                return Ok(body);
            }

            return Err(anyhow!("{} — {}", status.as_u16(), describe_error(&body)));
        }

        Err(anyhow!(
            "Microsoft Graph is throttling this request and did not recover after \
             {MAX_THROTTLE_RETRIES} attempts. Try again shortly."
        ))
    }

    /// Issue a write (PATCH/POST/DELETE), tolerating the empty `204` body that
    /// most Graph mutations return.
    ///
    /// Writes never reach here unless the worker has confirmed write mode is
    /// armed — see [`crate::worker`].
    pub(crate) async fn write(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<()> {
        let url = format!("{}{}", self.config.graph_url(), path);
        self.request(method, &url, body).await?;
        Ok(())
    }

    /// Wrap a collection fetch so that "you do not have this feature" becomes a
    /// reportable state rather than a failure.
    async fn get_all_optional<T: DeserializeOwned>(
        &mut self,
        path_and_query: &str,
        unavailable_hint: &str,
    ) -> Result<Fetch<Vec<T>>> {
        match self.get_all::<T>(path_and_query).await {
            Ok(items) => Ok(Fetch::Ready(items)),
            Err(err) => {
                let text = err.to_string();
                if is_feature_unavailable(&text) {
                    Ok(Fetch::Unavailable(format!("{unavailable_hint}\n\n{text}")))
                } else {
                    Err(err)
                }
            }
        }
    }

    // ---- Tenant -----------------------------------------------------------

    pub async fn organization(&mut self) -> Result<Organization> {
        let page: Page<Organization> = self
            .get_one("/organization?$select=id,displayName,tenantType,countryLetterCode,createdDateTime,verifiedDomains,assignedPlans")
            .await?;
        page.value
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("the tenant returned no organization record"))
    }

    // ---- Users ------------------------------------------------------------

    pub async fn users(&mut self) -> Result<Vec<User>> {
        let select = "id,displayName,userPrincipalName,mail,jobTitle,department,\
                      officeLocation,mobilePhone,businessPhones,accountEnabled,userType,\
                      createdDateTime,lastPasswordChangeDateTime,onPremisesSyncEnabled,\
                      onPremisesSamAccountName,usageLocation,assignedLicenses,proxyAddresses";
        let mut users: Vec<User> = self
            .get_all(&format!(
                "/users?$select={select}&$top={}",
                self.config.page_size()
            ))
            .await?;
        users.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        Ok(users)
    }

    /// Groups and roles a user belongs to, for the user details pane.
    pub async fn user_memberships(&mut self, user_id: &str) -> Result<Vec<DirectoryMember>> {
        self.get_all(&format!(
            "/users/{}/memberOf?$select=id,displayName&$top=999",
            urlencoding::encode(user_id)
        ))
        .await
    }

    // ---- Groups and roles -------------------------------------------------

    pub async fn groups(&mut self) -> Result<Vec<Group>> {
        let select = "id,displayName,description,mail,mailNickname,mailEnabled,\
                      securityEnabled,visibility,createdDateTime,membershipRule,\
                      membershipRuleProcessingState,onPremisesSyncEnabled,\
                      isAssignableToRole,groupTypes";
        let mut groups: Vec<Group> = self
            .get_all(&format!(
                "/groups?$select={select}&$top={}",
                self.config.page_size()
            ))
            .await?;
        groups.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        Ok(groups)
    }

    /// Directory roles that are activated in the tenant.
    ///
    /// Graph only returns activated roles from `/directoryRoles`; the full
    /// catalogue lives at `/directoryRoleTemplates`. Activated roles are what an
    /// administrator cares about, since only those can hold members.
    pub async fn directory_roles(&mut self) -> Result<Vec<DirectoryRole>> {
        let mut roles: Vec<DirectoryRole> = self
            .get_all("/directoryRoles?$select=id,displayName,description,roleTemplateId")
            .await?;
        roles.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        Ok(roles)
    }

    pub async fn group_members(&mut self, group_id: &str) -> Result<Vec<DirectoryMember>> {
        self.get_all(&format!(
            "/groups/{}/members?$select=id,displayName,userPrincipalName&$top=999",
            urlencoding::encode(group_id)
        ))
        .await
    }

    pub async fn group_owners(&mut self, group_id: &str) -> Result<Vec<DirectoryMember>> {
        self.get_all(&format!(
            "/groups/{}/owners?$select=id,displayName,userPrincipalName&$top=999",
            urlencoding::encode(group_id)
        ))
        .await
    }

    pub async fn role_members(&mut self, role_id: &str) -> Result<Vec<DirectoryMember>> {
        self.get_all(&format!(
            "/directoryRoles/{}/members?$select=id,displayName,userPrincipalName&$top=999",
            urlencoding::encode(role_id)
        ))
        .await
    }

    // ---- Devices ----------------------------------------------------------

    pub async fn devices(&mut self) -> Result<Vec<Device>> {
        let select = "id,deviceId,displayName,operatingSystem,operatingSystemVersion,\
                      trustType,profileType,manufacturer,model,isCompliant,isManaged,\
                      accountEnabled,approximateLastSignInDateTime,registrationDateTime,\
                      onPremisesSyncEnabled";
        let mut devices: Vec<Device> = self
            .get_all(&format!(
                "/devices?$select={select}&$top={}",
                self.config.page_size()
            ))
            .await?;
        devices.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        Ok(devices)
    }

    /// Intune-managed devices, or an explanation of why they are unavailable.
    pub async fn managed_devices(&mut self) -> Result<Fetch<Vec<ManagedDevice>>> {
        let select = "id,deviceName,managedDeviceOwnerType,operatingSystem,osVersion,\
                      complianceState,managementAgent,enrolledDateTime,lastSyncDateTime,\
                      userPrincipalName,model,manufacturer,serialNumber,imei,isEncrypted,\
                      isSupervised,jailBroken,deviceEnrollmentType,\
                      totalStorageSpaceInBytes,freeStorageSpaceInBytes";
        let hint = "This tenant does not expose Intune managed devices. Either Intune \
                    is not licensed here, or the app registration has not been granted \
                    DeviceManagementManagedDevices.Read.All.";
        let result = self
            .get_all_optional::<ManagedDevice>(
                &format!("/deviceManagement/managedDevices?$select={select}&$top=999"),
                hint,
            )
            .await?;

        Ok(match result {
            Fetch::Ready(mut devices) => {
                devices.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
                Fetch::Ready(devices)
            }
            other => other,
        })
    }

    // ---- Licences ---------------------------------------------------------

    pub async fn subscribed_skus(&mut self) -> Result<Vec<SubscribedSku>> {
        let mut skus: Vec<SubscribedSku> = self.get_all("/subscribedSkus").await?;
        // Busiest products first — that is what an admin scans for.
        skus.sort_by(|a, b| {
            b.consumed()
                .cmp(&a.consumed())
                .then_with(|| a.display_name().cmp(&b.display_name()))
        });
        Ok(skus)
    }
}

/// Distinguish "the tenant does not have this" from a real failure.
///
/// Graph is inconsistent about which status it returns for an unlicensed
/// workload, so match on the codes and phrasings actually observed rather than
/// on status alone.
fn is_feature_unavailable(error_text: &str) -> bool {
    let lowered = error_text.to_lowercase();
    lowered.starts_with("403")
        || lowered.starts_with("404")
        || lowered.starts_with("501")
        || lowered.contains("forbidden")
        || lowered.contains("accessdenied")
        || lowered.contains("authorization_requestdenied")
        || lowered.contains("not licensed")
        || lowered.contains("tenant is not")
        || lowered.contains("does not have a valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_unlicensed_responses() {
        assert!(is_feature_unavailable(
            "403 — Forbidden: Tenant is not a B2C tenant"
        ));
        assert!(is_feature_unavailable(
            "400 — The tenant is not licensed for Microsoft Intune"
        ));
        assert!(is_feature_unavailable(
            "403 — Authorization_RequestDenied: Insufficient privileges"
        ));
    }

    #[test]
    fn passes_real_failures_through() {
        assert!(!is_feature_unavailable("503 — Service unavailable"));
        assert!(!is_feature_unavailable("429 — Too many requests"));
        assert!(!is_feature_unavailable("500 — Internal server error"));
    }

    #[test]
    fn page_parses_without_a_next_link() {
        let json = r#"{"value":[{"id":"1"}]}"#;
        let page: Page<serde_json::Value> = serde_json::from_str(json).expect("should parse");
        assert_eq!(page.value.len(), 1);
        assert!(page.next_link.is_none());
    }

    #[test]
    fn page_parses_an_empty_collection() {
        let page: Page<serde_json::Value> =
            serde_json::from_str(r#"{"@odata.context":"x"}"#).expect("should parse");
        assert!(page.value.is_empty());
    }

    #[test]
    fn page_captures_the_next_link() {
        let json = r#"{"value":[],"@odata.nextLink":"https://graph.microsoft.com/v1.0/users?$skiptoken=X"}"#;
        let page: Page<serde_json::Value> = serde_json::from_str(json).expect("should parse");
        assert_eq!(
            page.next_link.as_deref(),
            Some("https://graph.microsoft.com/v1.0/users?$skiptoken=X")
        );
    }
}
