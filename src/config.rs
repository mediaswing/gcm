//! Configuration loaded from the user directory.
//!
//! The file is INI-shaped (`[section]` headers, `key = "value"` pairs), which
//! TOML parses natively — so the familiar layout costs us no extra parser,
//! beyond accepting `;` comments as well as `#`.
//!
//! It deliberately lives in the user's config directory rather than next to the
//! binary. Neither the client ID nor the tenant ID is a secret (a public
//! client's ID travels in plaintext on every authorization request, and the
//! tenant ID is discoverable from any verified domain), but the refresh token
//! cached alongside it *is* a bearer credential — see [`crate::auth`], which
//! writes it 0600. Keeping both in one owner-controlled directory means there is
//! exactly one place to protect, and no chance of committing either to a repo.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::Deserialize;

/// Delegated scopes requested during sign-in.
///
/// `offline_access` is what earns us a refresh token, so the user is not
/// re-prompted on every launch.
///
/// These include write scopes, so the access token is capable of changing the
/// tenant for the whole session. gcm nonetheless starts read-only and requires
/// write mode to be armed explicitly — that gate is enforced in
/// [`crate::worker`], not here. It is an application-level boundary rather than
/// one Entra imposes; see the README for the trade-off that was chosen
/// deliberately over incremental consent.
/// Scopes needed to display the console. Nothing here can change a tenant.
///
/// These are requested on their own if the full set is refused, so a tenant
/// that will not consent to the write permissions still gets a working
/// read-only console rather than a locked door.
pub const READ_SCOPES: &[&str] = &[
    "offline_access",
    "User.Read",
    "User.Read.All",
    "Group.Read.All",
    "GroupMember.Read.All",
    "Directory.Read.All",
    "RoleManagement.Read.Directory",
    "Device.Read.All",
    "DeviceManagementManagedDevices.Read.All",
    "Organization.Read.All",
];

/// Additional scopes needed to change the tenant, requested on top of
/// [`READ_SCOPES`] and only ever exercised while write mode is armed.
///
/// Device writes ride on `Directory.ReadWrite.All` rather than
/// `Device.ReadWrite.All`, which Graph exposes only as an application
/// permission — requesting it delegated is refused outright.
pub const WRITE_SCOPES: &[&str] = &[
    "User.ReadWrite.All",
    "Group.ReadWrite.All",
    "GroupMember.ReadWrite.All",
    "Directory.ReadWrite.All",
    "DeviceManagementManagedDevices.ReadWrite.All",
    // Retire, wipe, remote lock and Autopilot reset live behind this one.
    "DeviceManagementManagedDevices.PrivilegedOperations.All",
    // Administrative password reset.
    "UserAuthenticationMethod.ReadWrite.All",
];

/// The scope string to request, with or without the write permissions.
pub fn scopes(include_writes: bool) -> String {
    let mut all: Vec<&str> = READ_SCOPES.to_vec();
    if include_writes {
        all.extend_from_slice(WRITE_SCOPES);
    }
    all.join(" ")
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Entra app registration details.
    pub application: Application,
    /// Sovereign cloud endpoints. Defaults to worldwide commercial.
    #[serde(default)]
    pub cloud: Cloud,
    /// Paging limits.
    #[serde(default)]
    pub query: Query,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Application {
    /// Application (client) ID of the Entra app registration.
    pub client: String,
    /// Directory (tenant) ID, or a domain such as `contoso.onmicrosoft.com`.
    pub tenant: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Query {
    /// Objects per Graph request. Graph caps most collections at 999.
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Stop paging after this many objects per collection, to keep the first
    /// paint fast in very large tenants. 0 means no limit.
    #[serde(default)]
    pub max_objects: u32,
}

fn default_page_size() -> u32 {
    999
}

impl Default for Query {
    fn default() -> Self {
        Self {
            page_size: default_page_size(),
            max_objects: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cloud {
    /// Authority host, e.g. `https://login.microsoftonline.com`.
    #[serde(default = "default_authority")]
    pub authority: String,
    /// Graph host, e.g. `https://graph.microsoft.com`.
    #[serde(default = "default_graph")]
    pub graph: String,
}

fn default_authority() -> String {
    "https://login.microsoftonline.com".into()
}

fn default_graph() -> String {
    "https://graph.microsoft.com".into()
}

impl Default for Cloud {
    fn default() -> Self {
        Self {
            authority: default_authority(),
            graph: default_graph(),
        }
    }
}

impl Config {
    pub fn client_id(&self) -> &str {
        &self.application.client
    }

    pub fn tenant_id(&self) -> &str {
        &self.application.tenant
    }

    /// Base URL for the tenant's OAuth endpoints.
    pub fn authority_url(&self) -> String {
        format!(
            "{}/{}",
            self.cloud.authority.trim_end_matches('/'),
            self.application.tenant
        )
    }

    /// Base URL for Graph v1.0.
    pub fn graph_url(&self) -> String {
        format!("{}/v1.0", self.cloud.graph.trim_end_matches('/'))
    }

    /// Page size clamped to what Graph will actually honour.
    pub fn page_size(&self) -> u32 {
        self.query.page_size.clamp(1, 999)
    }

    fn validate(&self) -> Result<()> {
        let path = config_path();
        if is_placeholder(&self.application.client) {
            bail!("`client` is not set in {}", path.display());
        }
        if is_placeholder(&self.application.tenant) {
            bail!("`tenant` is not set in {}", path.display());
        }
        Ok(())
    }
}

/// True when a value is blank or still carries the shipped placeholder text.
fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value.eq_ignore_ascii_case("CLIENT_ID_HERE")
        || value.eq_ignore_ascii_case("TENANT_ID_HERE")
        || value.starts_with("00000000-0000")
}

/// `~/Library/Application Support/gcm` on macOS, `~/.config/gcm` on Linux,
/// `%APPDATA%\gcm` on Windows.
pub fn config_dir() -> PathBuf {
    ProjectDirs::from("", "", "gcm")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".gcm"))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.ini")
}

const TEMPLATE: &str = r#"; Graphical Cloud Manager (gcm)
;
; Register a public client application in Entra ID, turn on "Allow public client
; flows", and grant these delegated Microsoft Graph permissions:
;   User.Read.All, Group.Read.All, GroupMember.Read.All, Directory.Read.All,
;   RoleManagement.Read.Directory, Device.Read.All, Organization.Read.All,
;   DeviceManagementManagedDevices.Read.All
;
; Then fill in the two IDs below and restart gcm.

[application]
client = "CLIENT_ID_HERE"
tenant = "TENANT_ID_HERE"

; Objects fetched per Graph request (max 999), and a safety valve for very large
; tenants. max_objects = 0 means fetch everything.
[query]
page_size = 999
max_objects = 0

; Sovereign clouds only — omit this section for worldwide commercial M365.
; [cloud]
; authority = "https://login.microsoftonline.us"
; graph = "https://graph.microsoft.us"
"#;

/// Load the config, writing a template first if none exists.
///
/// The error text names the exact file to edit — this is the first thing anyone
/// hits on a fresh install, so a bare parse failure would be a poor greeting.
pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        write_template(&path)?;
        bail!(
            "No configuration found, so a template was created at:\n\n    {}\n\n\
             Fill in `client` and `tenant`, then restart gcm.",
            path.display()
        );
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config: Config = parse(&raw).with_context(|| format!("parsing {}", path.display()))?;
    config.validate()?;
    Ok(config)
}

/// Parse the INI-shaped configuration.
///
/// TOML only recognises `#` as a comment marker, but the `;` form is what
/// people reach for in a file named `.ini` — and rejecting it produces a
/// baffling "key with no value" error. Normalise whole-line `;` comments first
/// so both conventions work.
fn parse(raw: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(&normalise_comments(raw))
}

/// Rewrite lines whose first non-whitespace character is `;` into TOML
/// comments. Only whole-line comments are converted: a trailing `;` could
/// legitimately sit inside a quoted value, and silently mangling a client ID
/// would be far worse than not supporting inline comments.
fn normalise_comments(raw: &str) -> String {
    raw.lines()
        .map(|line| {
            if line.trim_start().starts_with(';') {
                let indent = line.len() - line.trim_start().len();
                format!("{}#{}", &line[..indent], &line.trim_start()[1..])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_template(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_minimal_ini_shape() {
        let raw = r#"
[application]
client = "abc"
tenant = "contoso.onmicrosoft.com"
"#;
        let config: Config = parse(raw).expect("should parse");
        assert_eq!(config.client_id(), "abc");
        assert_eq!(config.tenant_id(), "contoso.onmicrosoft.com");
        // Omitted sections fall back to worldwide commercial defaults.
        assert_eq!(config.graph_url(), "https://graph.microsoft.com/v1.0");
        assert_eq!(
            config.authority_url(),
            "https://login.microsoftonline.com/contoso.onmicrosoft.com"
        );
        assert_eq!(config.page_size(), 999);
    }

    #[test]
    fn the_shipped_template_parses() {
        let config: Config = parse(TEMPLATE).expect("template should parse");
        // ...but is rejected until the placeholders are replaced.
        assert!(config.validate().is_err());
    }

    #[test]
    fn honours_sovereign_cloud_overrides() {
        let raw = r#"
[application]
client = "abc"
tenant = "def"

[cloud]
authority = "https://login.microsoftonline.us/"
graph = "https://graph.microsoft.us/"
"#;
        let config: Config = parse(raw).expect("should parse");
        // Trailing slashes must not produce a doubled separator.
        assert_eq!(config.authority_url(), "https://login.microsoftonline.us/def");
        assert_eq!(config.graph_url(), "https://graph.microsoft.us/v1.0");
    }

    #[test]
    fn clamps_out_of_range_page_size() {
        let raw = r#"
[application]
client = "abc"
tenant = "def"

[query]
page_size = 5000
"#;
        let config: Config = parse(raw).expect("should parse");
        assert_eq!(config.page_size(), 999);
    }

    #[test]
    fn accepts_both_comment_markers() {
        let raw = r#"
; an INI-style comment
# a TOML-style comment
[application]
client = "abc"
  ; an indented comment
tenant = "def"
"#;
        let config = parse(raw).expect("both comment styles should parse");
        assert_eq!(config.client_id(), "abc");
        assert_eq!(config.tenant_id(), "def");
    }

    #[test]
    fn semicolons_inside_values_survive() {
        // Only whole-line comments are rewritten, so a value containing a
        // semicolon must come through untouched.
        let raw = "[application]\nclient = \"a;b\"\ntenant = \"def\"\n";
        let config = parse(raw).expect("should parse");
        assert_eq!(config.client_id(), "a;b");
    }

    #[test]
    fn recognises_placeholders() {
        assert!(is_placeholder("CLIENT_ID_HERE"));
        assert!(is_placeholder("  "));
        assert!(is_placeholder("00000000-0000-0000-0000-000000000000"));
        assert!(!is_placeholder("9f4a1c7e-1234-5678-9abc-def012345678"));
    }
}
