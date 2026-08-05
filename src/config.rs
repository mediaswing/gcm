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
//!
//! **Nothing in this file is a secret, and that is a property worth keeping.**
//! The database export was the first thing that wanted to break it; instead of
//! storing a password here, gcm asks for one per session and holds it in
//! memory. The file therefore stays safe to paste into a support ticket, which
//! is exactly what somebody does when a tenant will not load.

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
    // Sign-in and directory audit logs. The sign-in log additionally needs
    // Entra ID P1 on the tenant and a reporting role on the signed-in account,
    // neither of which a scope can supply — hence the graceful degradation.
    "AuditLog.Read.All",
    // Teams. Team.ReadBasic.All lists them; TeamSettings.Read.All is what
    // fills in the settings and archived state on an individual team.
    "Team.ReadBasic.All",
    "TeamSettings.Read.All",
    "Channel.ReadBasic.All",
    // The mailbox usage report, which is the only way Graph will enumerate
    // mailboxes at all.
    "Reports.Read.All",
    "MailboxSettings.Read",
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
    // Archiving and unarchiving a team.
    "TeamSettings.ReadWrite.All",
    // Setting somebody's automatic replies.
    "MailboxSettings.ReadWrite",
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
    /// Where the database export writes. Absent when the feature is not used,
    /// which is the normal case.
    pub mariadb: Option<MariaDb>,
}

/// Connection details for the MariaDB export.
///
/// Deliberately no password. Everything here is the sort of thing that can sit
/// in a config file and be pasted into a support ticket — a hostname, a
/// username, a schema name. The password is asked for once per session and kept
/// only in memory, which is what lets this file go on holding no secret at all.
#[derive(Debug, Clone, Deserialize)]
pub struct MariaDb {
    pub host: String,
    #[serde(default = "default_mysql_port")]
    pub port: u16,
    pub user: String,
    /// The schema to write into. It must already exist; gcm creates tables, not
    /// databases, because creating a database is not a thing an export should
    /// decide to do on its own.
    pub database: String,
    /// Prefix for the tables gcm writes, so its output cannot collide with
    /// anything already in the schema.
    #[serde(default = "default_table_prefix")]
    pub table_prefix: String,
    /// Require TLS to the database. On by default: this connection carries the
    /// whole directory, and a password.
    #[serde(default = "default_true")]
    pub require_tls: bool,
}

fn default_mysql_port() -> u16 {
    3306
}

fn default_table_prefix() -> String {
    "gcm_".into()
}

fn default_true() -> bool {
    true
}

impl MariaDb {
    /// The connection URL for `mysql_async`.
    ///
    /// Takes the password rather than storing it, so there is no field anywhere
    /// on this struct that must not be printed. Never logged, never shown, and
    /// never put in an error message — see [`Self::describe`] for the form that
    /// is safe to display.
    pub fn url(&self, password: &str) -> String {
        format!(
            "mysql://{}:{}@{}:{}/{}",
            urlencoding::encode(&self.user),
            urlencoding::encode(password),
            self.host,
            self.port,
            urlencoding::encode(&self.database)
        )
    }

    /// The connection as it is safe to print: no password, ever.
    pub fn describe(&self) -> String {
        format!(
            "{}@{}:{}/{}",
            self.user, self.host, self.port, self.database
        )
    }

    /// Full name of the table a view is written to.
    pub fn table_for(&self, stem: &str) -> String {
        format!("{}{stem}", self.table_prefix)
    }

    fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            bail!("`host` is not set in the [mariadb] section of {}", config_path().display());
        }
        if self.user.trim().is_empty() {
            bail!("`user` is not set in the [mariadb] section of {}", config_path().display());
        }
        if self.database.trim().is_empty() {
            bail!(
                "`database` is not set in the [mariadb] section of {}",
                config_path().display()
            );
        }
        // A prefix is what keeps gcm's tables from colliding with whatever else
        // lives in the schema, so an empty one is refused rather than silently
        // letting an export drop a table called `users`.
        if self.table_prefix.trim().is_empty() {
            bail!(
                "`table_prefix` in the [mariadb] section of {} must not be empty — it is \
                 what stops an export from overwriting a table it did not create",
                config_path().display()
            );
        }
        if !self
            .table_prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            bail!("`table_prefix` may contain only letters, digits and underscores");
        }
        Ok(())
    }
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
    /// How far back the sign-in and audit log views reach, in days.
    ///
    /// Separate from `max_objects` because the logs need a bound that the
    /// directory collections do not: an unfiltered call to the sign-in log
    /// times out on a busy tenant, and "no limit" is not an option there.
    #[serde(default = "default_log_days")]
    pub log_days: u32,
    /// Most log entries to hold, across all pages.
    #[serde(default = "default_log_records")]
    pub log_records: u32,
}

fn default_page_size() -> u32 {
    999
}

/// A week: long enough to cover "what happened over the weekend", short enough
/// that the query returns promptly.
fn default_log_days() -> u32 {
    7
}

/// Enough to scan for something unusual without holding a busy tenant's entire
/// week of sign-ins in memory.
fn default_log_records() -> u32 {
    500
}

impl Default for Query {
    fn default() -> Self {
        Self {
            page_size: default_page_size(),
            max_objects: 0,
            log_days: default_log_days(),
            log_records: default_log_records(),
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

    /// How far back the log views reach. Entra keeps at most 30 days of
    /// sign-ins without a P2 licence, so asking for more just returns less.
    pub fn log_days(&self) -> u32 {
        self.query.log_days.clamp(1, 30)
    }

    /// Ceiling on log entries held in memory.
    pub fn log_records(&self) -> usize {
        self.query.log_records.max(1) as usize
    }

    /// Page size for the log endpoints, which cap at 1000 rather than 999 and
    /// should never ask for more than the ceiling will keep.
    pub fn log_page_size(&self) -> u32 {
        self.query.log_records.clamp(1, 1000)
    }

    fn validate(&self) -> Result<()> {
        let path = config_path();
        if is_placeholder(&self.application.client) {
            bail!("`client` is not set in {}", path.display());
        }
        if is_placeholder(&self.application.tenant) {
            bail!("`tenant` is not set in {}", path.display());
        }
        if let Some(mariadb) = &self.mariadb {
            mariadb.validate()?;
        }
        Ok(())
    }
}

/// Make the config file readable only by its owner.
///
/// The file holds no secret — that is the point of prompting for the database
/// password rather than storing it — so this is hygiene rather than protection.
/// It costs nothing, it matches the token cache and the two logs beside it, and
/// it means the whole directory can be described with one sentence.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

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
;   DeviceManagementManagedDevices.Read.All, AuditLog.Read.All,
;   Team.ReadBasic.All, TeamSettings.Read.All, Channel.ReadBasic.All,
;   Reports.Read.All, MailboxSettings.Read
;
; Anything not granted simply shows as unavailable in that view; gcm still runs.
;
; Then fill in the two IDs below and restart gcm.

[application]
client = "CLIENT_ID_HERE"
tenant = "TENANT_ID_HERE"

; Objects fetched per Graph request (max 999), and a safety valve for very large
; tenants. max_objects = 0 means fetch everything.
;
; The sign-in and audit log views get their own limits: they are far larger than
; any directory collection, so "everything" is never the right answer there.
; Entra keeps at most 30 days of sign-ins without an Entra ID P2 licence.
[query]
page_size = 999
max_objects = 0
log_days = 7
log_records = 500

; Optional. Uncomment to enable "Export to MariaDB", which writes every loaded
; view into one table per collection, replacing what was there before.
;
; There is deliberately no password here. gcm asks for it once per session and
; keeps it only in memory, so this file stays safe to share or paste into a
; support ticket. The database must already exist; gcm creates tables in it.
;
; [mariadb]
; host = "db.contoso.internal"
; port = 3306
; user = "gcm_export"
; database = "m365"
; table_prefix = "gcm_"
; require_tls = true

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
    // Written owner-only from the outset, so a password added to it later is
    // never briefly world-readable.
    restrict_permissions(path);
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
    fn log_limits_have_workable_defaults() {
        let raw = "[application]\nclient = \"abc\"\ntenant = \"def\"\n";
        let config: Config = parse(raw).expect("should parse");
        assert_eq!(config.log_days(), 7);
        assert_eq!(config.log_records(), 500);
        assert_eq!(config.log_page_size(), 500);
    }

    #[test]
    fn clamps_log_limits_to_what_graph_allows() {
        let raw = r#"
[application]
client = "abc"
tenant = "def"

[query]
log_days = 400
log_records = 100000
"#;
        let config: Config = parse(raw).expect("should parse");
        // Entra keeps 30 days at most, and the log endpoints cap a page at 1000.
        assert_eq!(config.log_days(), 30);
        assert_eq!(config.log_page_size(), 1000);
        // The overall ceiling is ours to honour, so it is not clamped down.
        assert_eq!(config.log_records(), 100_000);
    }

    #[test]
    fn a_zero_log_limit_never_asks_for_zero_records() {
        let raw = r#"
[application]
client = "abc"
tenant = "def"

[query]
log_days = 0
log_records = 0
"#;
        let config: Config = parse(raw).expect("should parse");
        assert_eq!(config.log_days(), 1);
        assert_eq!(config.log_records(), 1);
        assert_eq!(config.log_page_size(), 1);
    }

    #[test]
    fn every_write_scope_is_additional_to_the_read_set() {
        // The two lists are concatenated at sign-in; a duplicate would make the
        // consent prompt list the same permission twice.
        for scope in WRITE_SCOPES {
            assert!(
                !READ_SCOPES.contains(scope),
                "{scope} appears in both scope lists"
            );
        }
        assert!(scopes(false).split(' ').count() == READ_SCOPES.len());
        assert!(scopes(true).contains("User.ReadWrite.All"));
        assert!(!scopes(false).contains("ReadWrite"));
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
