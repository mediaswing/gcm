//! A small Microsoft Graph client that signs in as the app registration itself.
//!
//! The OAuth 2.0 client-credentials grant: the tenant ID, client ID and client
//! secret are exchanged directly for an app-only token, with no browser, no
//! device code and no signed-in user. What the app can do is therefore exactly
//! the *application* permissions granted to the registration (with admin
//! consent) — [`REQUIRED_ROLES`] lists them, and the Connection tab shows
//! which of them the token actually carries.
//!
//! The client is cheap to clone and safe to share between threads; every pane
//! hands a clone to a [`crate::task::Task`]. The token is cached and fetched
//! again a few minutes before it expires.

pub mod devices;
pub mod groups;
pub mod models;
pub mod users;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use ureq::Agent;

pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const LOGIN: &str = "https://login.microsoftonline.com";
const TIMEOUT: Duration = Duration::from_secs(60);
/// Fetch a new token this long before the old one runs out, so a request is
/// never sent with one that expires on the way.
const TOKEN_MARGIN: Duration = Duration::from_secs(300);

/// The application permissions this app uses, and what each one is for.
pub const REQUIRED_ROLES: &[(&str, &str)] = &[
    ("User.ReadWrite.All", "List, create, update and delete users"),
    ("User-PasswordProfile.ReadWrite.All", "Reset user passwords"),
    ("Group.ReadWrite.All", "List, create and delete groups"),
    ("GroupMember.ReadWrite.All", "Add and remove group members"),
    ("Device.ReadWrite.All", "List, enable, disable and delete Entra devices"),
    (
        "DeviceManagementManagedDevices.ReadWrite.All",
        "List Intune managed devices",
    ),
    (
        "DeviceManagementManagedDevices.PrivilegedOperations.All",
        "Intune actions: sync, restart, lock, scan, retire, wipe",
    ),
    ("Organization.Read.All", "Show the tenant's name"),
];

#[derive(Clone)]
pub struct Credentials {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<hidden>")
            .finish()
    }
}

struct Token {
    bearer: String,
    expires: Instant,
}

struct Inner {
    agent: Agent,
    credentials: Credentials,
    token: Mutex<Option<Token>>,
}

#[derive(Clone)]
pub struct Graph {
    inner: Arc<Inner>,
}

/// What a successful sign-in found out.
#[derive(Clone, Debug)]
pub struct Session {
    /// The tenant's display name, when the app may read it.
    pub organisation: Option<String>,
    /// The application permissions in the token's `roles` claim.
    pub roles: Vec<String>,
}

pub type Result<T> = std::result::Result<T, String>;

impl Graph {
    pub fn new(credentials: Credentials) -> Self {
        // Graph's error bodies carry the only useful explanation of what went
        // wrong, so a 4xx is read like any other response rather than turned
        // into a bare status code.
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("gcm/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            inner: Arc::new(Inner {
                agent,
                credentials,
                token: Mutex::new(None),
            }),
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.inner.credentials.tenant_id
    }

    /// Sign in, and find out what the sign-in is good for.
    pub fn sign_in(&self) -> Result<Session> {
        let bearer = self.bearer()?;
        let roles = roles_in(&bearer);
        // Optional: without Organization.Read.All this is a 403, and that is
        // no reason to refuse to connect.
        let organisation = self
            .get("/organization?$select=displayName")
            .ok()
            .and_then(|v| {
                v["value"][0]["displayName"]
                    .as_str()
                    .map(str::to_owned)
            });
        Ok(Session {
            organisation,
            roles,
        })
    }

    /// A valid access token, from the cache or freshly fetched.
    fn bearer(&self) -> Result<String> {
        let mut slot = self.inner.token.lock().map_err(|_| "token cache poisoned")?;
        if let Some(token) = slot.as_ref()
            && token.expires > Instant::now() + TOKEN_MARGIN
        {
            return Ok(token.bearer.clone());
        }

        let c = &self.inner.credentials;
        if c.tenant_id.trim().is_empty() || c.client_id.trim().is_empty() {
            return Err("Enter a tenant ID and a client ID first.".to_owned());
        }
        if c.client_secret.is_empty() {
            return Err("Enter the client secret first.".to_owned());
        }

        // The tenant becomes part of the sign-in URL, so a `/`, `?` or `#`
        // typed into the box would point the secret at a different endpoint.
        let tenant = c.tenant_id.trim();
        if !tenant
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.'))
        {
            return Err("The tenant ID is a GUID or a domain such as contoso.onmicrosoft.com.".to_owned());
        }
        let url = format!("{LOGIN}/{tenant}/oauth2/v2.0/token");
        let mut response = self
            .inner
            .agent
            .post(&url)
            .send_form([
                ("grant_type", "client_credentials"),
                ("client_id", c.client_id.trim()),
                ("client_secret", c.client_secret.as_str()),
                ("scope", "https://graph.microsoft.com/.default"),
            ])
            .map_err(|e| format!("Could not reach Microsoft sign-in: {e}"))?;
        let status = response.status();
        let body: Value = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("Unreadable answer from Microsoft sign-in: {e}"))?;

        if !status.is_success() {
            // AADSTS messages are long, and the first line says it all.
            let description = body["error_description"]
                .as_str()
                .or_else(|| body["error"].as_str())
                .unwrap_or("unknown error");
            let first = description.lines().next().unwrap_or(description);
            return Err(format!("Sign-in refused: {first}"));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }
        let parsed: TokenResponse = serde_json::from_value(body)
            .map_err(|e| format!("Unexpected answer from Microsoft sign-in: {e}"))?;
        let bearer = parsed.access_token;
        *slot = Some(Token {
            bearer: bearer.clone(),
            expires: Instant::now() + Duration::from_secs(parsed.expires_in),
        });
        Ok(bearer)
    }

    /// A path under [`GRAPH`], or a full URL such as an `@odata.nextLink`.
    /// A full URL has to be Graph's own: the bearer token goes with every
    /// request, and it must never be handed to another host.
    fn url(path: &str) -> Result<String> {
        if path.starts_with('/') {
            Ok(format!("{GRAPH}{path}"))
        } else if path.starts_with(GRAPH)
            && matches!(path.as_bytes().get(GRAPH.len()), Some(b'/' | b'?'))
        {
            Ok(path.to_owned())
        } else {
            Err(format!("Refusing to send the access token outside Microsoft Graph: {path}"))
        }
    }

    /// Send a request and read the answer, turning a Graph error into its
    /// message. `None` for the many calls that answer 202 or 204 with nothing.
    fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>> {
        let url = Self::url(path)?;
        let auth = format!("Bearer {}", self.bearer()?);
        let agent = &self.inner.agent;

        let sent = match (method, body) {
            ("GET", _) => agent.get(&url).header("Authorization", &auth).call(),
            ("DELETE", _) => agent.delete(&url).header("Authorization", &auth).call(),
            ("POST", Some(body)) => agent
                .post(&url)
                .header("Authorization", &auth)
                .send_json(body),
            ("POST", None) => agent
                .post(&url)
                .header("Authorization", &auth)
                .send_empty(),
            ("PATCH", Some(body)) => agent
                .patch(&url)
                .header("Authorization", &auth)
                .send_json(body),
            _ => return Err(format!("unsupported request {method} {path}")),
        };
        let mut response = sent.map_err(|e| format!("Could not reach Microsoft Graph: {e}"))?;
        let status = response.status();
        let text = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| format!("Unreadable answer from Microsoft Graph: {e}"))?;

        if !status.is_success() {
            return Err(graph_error(status.as_u16(), &text));
        }
        if text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("Unexpected answer from Microsoft Graph: {e}"))
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.send("GET", path, None)?
            .ok_or_else(|| "Microsoft Graph sent an empty answer.".to_owned())
    }

    /// Every page of a collection, following `@odata.nextLink` to the end.
    pub fn get_all<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let mut items = Vec::new();
        let mut next = Some(path.to_owned());
        while let Some(page_url) = next.take() {
            let mut page = self.get(&page_url)?;
            if let Value::Array(values) = page["value"].take() {
                for value in values {
                    items.push(
                        serde_json::from_value(value)
                            .map_err(|e| format!("Unexpected item from Microsoft Graph: {e}"))?,
                    );
                }
            }
            next = page["@odata.nextLink"].as_str().map(str::to_owned);
        }
        Ok(items)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Option<Value>> {
        self.send("POST", path, Some(body))
    }

    pub fn post_empty(&self, path: &str) -> Result<()> {
        self.send("POST", path, None).map(drop)
    }

    pub fn patch(&self, path: &str, body: &Value) -> Result<()> {
        self.send("PATCH", path, Some(body)).map(drop)
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        self.send("DELETE", path, None).map(drop)
    }
}

/// Graph's `{"error": {"code": …, "message": …}}`, as a sentence.
fn graph_error(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().map(|v| &v["error"]);
    let code = error.and_then(|e| e["code"].as_str()).unwrap_or("");
    let message = error.and_then(|e| e["message"].as_str()).unwrap_or("");
    let hint = match status {
        401 => " Sign in again on the Connection tab.",
        403 => " The app registration may be missing a permission, or admin consent.",
        _ => "",
    };
    match (code, message) {
        ("", "") => format!("Microsoft Graph answered {status}.{hint}"),
        (code, "") => format!("Microsoft Graph answered {status} ({code}).{hint}"),
        (_, message) => format!("{message}{hint}"),
    }
}

/// The `roles` claim of an access token, which is where an app-only token
/// lists the application permissions it was granted. The token is only read,
/// never verified: it came straight from Microsoft over TLS, and nothing is
/// decided on it except what to show.
fn roles_in(token: &str) -> Vec<String> {
    let Some(payload) = token.split('.').nth(1) else {
        return Vec::new();
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return Vec::new();
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&bytes) else {
        return Vec::new();
    };
    let mut roles: Vec<String> = claims["roles"]
        .as_array()
        .map(|r| r.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    roles.sort();
    roles
}

/// Escape a value for use inside single quotes in an OData filter or key.
pub fn odata_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// Percent-encode a query-string value. Graph filters are full of spaces and
/// quotes, and a UPN can hold `+`, `#` and `&`.
pub fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'\'') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_read_from_the_token_payload() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"roles":["User.ReadWrite.All","Device.ReadWrite.All"]}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            roles_in(&token),
            vec!["Device.ReadWrite.All", "User.ReadWrite.All"]
        );
        assert!(roles_in("not a token").is_empty());
    }

    #[test]
    fn the_token_only_goes_to_graph() {
        assert_eq!(Graph::url("/users").unwrap(), format!("{GRAPH}/users"));
        let next = format!("{GRAPH}/users?$skiptoken=abc");
        assert_eq!(Graph::url(&next).unwrap(), next);
        assert!(Graph::url("https://graph.microsoft.com.evil.example/v1.0/users").is_err());
        assert!(Graph::url("https://evil.example/users").is_err());
        assert!(Graph::url("users").is_err());
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(
            encode_query("userPrincipalName eq 'a+b@x.com'"),
            "userPrincipalName%20eq%20'a%2Bb%40x.com'"
        );
    }

    #[test]
    fn graph_errors_become_their_message() {
        let body = r#"{"error":{"code":"Request_BadRequest","message":"Another object with the same value for property userPrincipalName already exists."}}"#;
        assert_eq!(
            graph_error(400, body),
            "Another object with the same value for property userPrincipalName already exists."
        );
        assert!(graph_error(403, "").contains("permission"));
    }
}
