//! Configuration storage in the Windows registry.
//!
//! On Windows the settings that live in `config.ini` elsewhere live under
//! `HKEY_CURRENT_USER\Software\gcm` instead, one subkey per section and one
//! value per setting. The two logs are unaffected: `error.log` and
//! `actions.log` stay files in gcm's own directory on every platform, because
//! an append-only diagnostic and an audit trail are not things the registry
//! stores well, and "send me the folder" has to keep working.
//!
//! ## Why the registry rather than a file
//!
//! Because it can be deployed. A file beside the user's profile has to be
//! copied to every machine by hand; a registry key can be pushed by Group
//! Policy preference to a whole OU. That is also why `HKEY_LOCAL_MACHINE` is
//! read as a fallback layer beneath `HKEY_CURRENT_USER`: an administrator
//! sets the tenant and client ID machine-wide, and an individual value can
//! still be overridden per user without the policy having to know.
//!
//! ## How it is read
//!
//! Not with a bespoke deserializer. Each value is rendered back into the same
//! INI text [`crate::config::parse`] already accepts, and handed to the
//! existing parser. That is deliberate: every `#[serde(default)]`, every
//! `Option` section and the whole of `validate` then behave identically on
//! both platforms by construction, rather than by two implementations
//! agreeing. It also means [`export_ini`] can hand somebody a support ticket's
//! worth of configuration in the format the documentation already describes.
//!
//! ## What is not stored here
//!
//! No secrets — the same rule the file has. The MariaDB password and the LDAP
//! bind password are asked for once per session and held in memory. HKCU is
//! ACLed to the user, but "protected by an ACL" is a weaker promise than "not
//! written down", and the second one is the one gcm makes.

//! ## Why half of this compiles everywhere
//!
//! Only the calls that touch the registry are Windows-only. The schema, the
//! INI rendering and the escaping are ordinary code, and keeping them
//! platform-independent is what lets `cargo test` cover them on a Mac — the
//! alternative is a schema that can only be checked by the one runner in the
//! matrix that also has the least reason to be looked at.

#[cfg(windows)]
use anyhow::{Context, Result};
#[cfg(windows)]
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

/// Where the configuration lives, under both hives.
pub const KEY_PATH: &str = r"Software\gcm";

/// The key as an error message should name it, so somebody can paste it
/// straight into regedit's address bar.
pub fn key_label() -> String {
    format!(r"HKEY_CURRENT_USER\{KEY_PATH}")
}

/// How a setting is rendered back into INI text.
///
/// The registry is untyped as far as gcm is concerned — an administrator
/// writing these by hand or by policy may reasonably use `REG_SZ` for a number
/// — so this says what the value *means*, and reading is permissive about how
/// it was actually stored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Flag,
}

struct Field {
    name: &'static str,
    kind: Kind,
}

const fn text(name: &'static str) -> Field {
    Field {
        name,
        kind: Kind::Text,
    }
}

const fn number(name: &'static str) -> Field {
    Field {
        name,
        kind: Kind::Number,
    }
}

const fn flag(name: &'static str) -> Field {
    Field {
        name,
        kind: Kind::Flag,
    }
}

/// Every section gcm understands, and the settings in it.
///
/// This mirrors the structs in [`crate::config`]. A setting absent from here is
/// simply not read from the registry, so adding one to the config there means
/// adding it here too — the test at the bottom of this file is what stops the
/// two drifting apart silently.
const SECTIONS: &[(&str, &[Field])] = &[
    ("application", &[text("client"), text("tenant")]),
    ("cloud", &[text("authority"), text("graph")]),
    (
        "query",
        &[
            number("page_size"),
            number("max_objects"),
            number("log_days"),
            number("log_records"),
        ],
    ),
    (
        "mariadb",
        &[
            text("host"),
            number("port"),
            text("user"),
            text("database"),
            text("table_prefix"),
            flag("require_tls"),
        ],
    ),
    (
        "directory",
        &[
            text("host"),
            number("port"),
            text("base_dn"),
            text("bind_dn"),
            text("auth"),
            flag("tls"),
            flag("start_tls"),
        ],
    ),
];

/// Read one setting, preferring the user's own value over a machine-wide one.
///
/// Returns the value already rendered as INI, so the caller does not have to
/// know how it was stored. A value present but unreadable as its declared kind
/// is treated as absent rather than failing the whole load: one malformed
/// `REG_BINARY` written by hand should not stop the console starting.
#[cfg(windows)]
fn read_field(section: &str, field: &Field) -> Option<String> {
    let path = format!(r"{KEY_PATH}\{section}");
    for hive in [CURRENT_USER, LOCAL_MACHINE] {
        let Ok(key) = hive.open(&path) else {
            continue;
        };
        if let Some(rendered) = read_from(&key, field) {
            return Some(rendered);
        }
    }
    None
}

/// Render one value from an open key, accepting either the natural registry
/// type or a string spelling of it.
#[cfg(windows)]
fn read_from(key: &Key, field: &Field) -> Option<String> {
    match field.kind {
        Kind::Text => key.get_string(field.name).ok().map(|value| quote(&value)),
        // REG_DWORD is the natural choice, but a policy template or a hand
        // edit may well have used REG_SZ, and refusing that would be a
        // gratuitous way to fail.
        Kind::Number => {
            if let Ok(value) = key.get_u64(field.name) {
                return Some(value.to_string());
            }
            let raw = key.get_string(field.name).ok()?;
            let parsed: u64 = raw.trim().parse().ok()?;
            Some(parsed.to_string())
        }
        // Windows has no boolean type, so a flag is a DWORD 0/1 by convention
        // — but "true"/"false" is what the file form uses, and somebody
        // migrating by hand will type that.
        Kind::Flag => {
            if let Ok(value) = key.get_u64(field.name) {
                return Some(if value == 0 { "false" } else { "true" }.into());
            }
            let raw = key.get_string(field.name).ok()?;
            match raw.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Some("true".into()),
                "false" | "no" | "0" => Some("false".into()),
                _ => None,
            }
        }
    }
}

/// Quote a string as a TOML basic string.
///
/// This matters more than it looks: `base_dn` and `bind_dn` are full of
/// backslashes (`CORP\svc-gcm`, and DNs with escaped commas), and a backslash
/// is an escape character in a TOML basic string. Emitting one unescaped turns
/// `CORP\svc-gcm` into a parse error at best, and silently into something else
/// at worst.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Anything else in the C0 range has no literal form in a basic
            // string and has to go out as \u00XX.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render everything found in the registry as INI text.
///
/// `None` when there is no configuration to speak of — specifically when the
/// `[application]` section yielded nothing, since a client and tenant are the
/// two settings without which gcm cannot do anything at all. That is the
/// signal the caller uses to decide between migrating a `config.ini` and
/// seeding a fresh key.
#[cfg(windows)]
pub fn read() -> Option<String> {
    render(read_field)
}

/// Assemble INI text from whatever `lookup` can find.
///
/// Separated from the registry itself so the shape of the output — which
/// sections appear, which are suppressed, and what makes a configuration
/// count as absent — can be tested without a registry to read.
fn render(lookup: impl Fn(&str, &Field) -> Option<String>) -> Option<String> {
    let mut rendered = String::new();
    let mut has_application = false;

    for (section, fields) in SECTIONS {
        let mut body = String::new();
        for field in *fields {
            if let Some(value) = lookup(section, field) {
                body.push_str(&format!("{} = {}\n", field.name, value));
            }
        }
        // An absent section is not the same as an empty one: `[mariadb]` and
        // `[directory]` are `Option`s whose presence is what turns the feature
        // on, so emitting an empty header would fail validation rather than
        // leaving the feature off.
        if body.is_empty() {
            continue;
        }
        if *section == "application" {
            has_application = true;
        }
        rendered.push_str(&format!("[{section}]\n{body}\n"));
    }

    has_application.then_some(rendered)
}

/// The configuration as it is safe to hand to somebody else.
///
/// The file form of this is pasteable into a support ticket, and that is a
/// property [`crate::config`] goes out of its way to keep — nothing in it is a
/// secret. A registry key is not pasteable, so this renders the same content
/// back into the documented format rather than losing the property to the
/// change in storage.
#[cfg(windows)]
pub fn export_ini() -> String {
    let body = read().unwrap_or_default();
    format!(
        "; gcm configuration, read from {}\n\
         ; This is the same content the registry holds, in the file format the\n\
         ; README describes. It contains no passwords.\n\n{body}",
        key_label()
    )
}

/// Write one section's worth of settings under HKCU.
#[cfg(windows)]
fn write_section(section: &str, values: &[(&str, Value)]) -> Result<()> {
    let path = format!(r"{KEY_PATH}\{section}");
    let key = CURRENT_USER
        .create(&path)
        .with_context(|| format!(r"creating HKEY_CURRENT_USER\{path}"))?;
    for (name, value) in values {
        match value {
            Value::Text(text) => key.set_string(name, text),
            Value::Number(number) => key.set_u32(name, *number),
            Value::Flag(flag) => key.set_u32(name, u32::from(*flag)),
        }
        .with_context(|| format!(r"writing {name} under HKEY_CURRENT_USER\{path}"))?;
    }
    Ok(())
}

/// A value on its way into the registry.
#[cfg(windows)]
enum Value {
    Text(String),
    Number(u32),
    Flag(bool),
}

/// Copy a configuration that was loaded from `config.ini` into the registry.
///
/// Runs once, when a registry key does not yet exist but a file does — an
/// upgrade from a version that stored its settings in a file. The file is left
/// exactly where it is: it holds nothing secret, it is the only copy of these
/// settings until this succeeds, and deleting somebody's configuration as a
/// side effect of an upgrade is not a thing to do.
///
/// Sections that were absent from the file stay absent, so a cloud-only tenant
/// does not acquire an empty `[directory]` key it then has to be told to
/// ignore. Defaulted values *are* written out explicitly, which is the one
/// place this is not a faithful copy — an unset `page_size` becomes a written
/// 999. That is deliberate: a policy-managed key that shows every setting it
/// controls is easier to reason about than one with invisible defaults.
#[cfg(windows)]
pub fn migrate(config: &crate::config::Config) -> Result<()> {
    write_section(
        "application",
        &[
            ("client", Value::Text(config.application.client.clone())),
            ("tenant", Value::Text(config.application.tenant.clone())),
        ],
    )?;
    write_section(
        "cloud",
        &[
            ("authority", Value::Text(config.cloud.authority.clone())),
            ("graph", Value::Text(config.cloud.graph.clone())),
        ],
    )?;
    write_section(
        "query",
        &[
            ("page_size", Value::Number(config.query.page_size)),
            ("max_objects", Value::Number(config.query.max_objects)),
            ("log_days", Value::Number(config.query.log_days)),
            ("log_records", Value::Number(config.query.log_records)),
        ],
    )?;
    if let Some(mariadb) = &config.mariadb {
        write_section(
            "mariadb",
            &[
                ("host", Value::Text(mariadb.host.clone())),
                ("port", Value::Number(u32::from(mariadb.port))),
                ("user", Value::Text(mariadb.user.clone())),
                ("database", Value::Text(mariadb.database.clone())),
                ("table_prefix", Value::Text(mariadb.table_prefix.clone())),
                ("require_tls", Value::Flag(mariadb.require_tls)),
            ],
        )?;
    }
    if let Some(directory) = &config.directory {
        write_section(
            "directory",
            &[
                ("host", Value::Text(directory.host.clone())),
                ("port", Value::Number(u32::from(directory.port))),
                ("base_dn", Value::Text(directory.base_dn.clone())),
                ("bind_dn", Value::Text(directory.bind_dn.clone())),
                ("tls", Value::Flag(directory.tls)),
                ("start_tls", Value::Flag(directory.start_tls)),
            ],
        )?;
    }
    Ok(())
}

/// Create the key with the placeholders a fresh install needs filling in.
///
/// The equivalent of the template file written elsewhere. Only the settings
/// somebody must supply, plus the query limits, are seeded — `[mariadb]` and
/// `[directory]` are left absent, because creating them would turn both
/// features on with empty hosts and fail validation on the next start.
#[cfg(windows)]
pub fn seed_placeholders() -> Result<()> {
    write_section(
        "application",
        &[
            ("client", Value::Text("CLIENT_ID_HERE".into())),
            ("tenant", Value::Text("TENANT_ID_HERE".into())),
        ],
    )?;
    write_section(
        "query",
        &[
            ("page_size", Value::Number(999)),
            ("max_objects", Value::Number(0)),
            ("log_days", Value::Number(7)),
            ("log_records", Value::Number(500)),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backslashes_in_a_dn_survive_the_round_trip() {
        // The failure this exists to stop: `CORP\svc-gcm` emitted with a bare
        // backslash is either a parse error or, worse, quietly something else.
        let rendered = format!(
            "[directory]\nhost = \"dc01\"\nbase_dn = {}\nbind_dn = {}\n",
            quote("DC=corp,DC=contoso,DC=com"),
            quote(r"CORP\svc-gcm"),
        );
        let parsed: toml::Value = toml::from_str(&rendered).expect("should parse");
        let directory = &parsed["directory"];
        assert_eq!(directory["bind_dn"].as_str(), Some(r"CORP\svc-gcm"));
        assert_eq!(
            directory["base_dn"].as_str(),
            Some("DC=corp,DC=contoso,DC=com")
        );
    }

    #[test]
    fn quotes_and_control_characters_cannot_break_out_of_the_value() {
        let rendered = format!("[application]\nclient = {}\n", quote("a\"b\\c\td"));
        let parsed: toml::Value = toml::from_str(&rendered).expect("should parse");
        assert_eq!(parsed["application"]["client"].as_str(), Some("a\"b\\c\td"));
    }

    /// A stand-in for the registry with every setting present and plausible.
    fn every_field(_section: &str, field: &Field) -> Option<String> {
        Some(match field.kind {
            // Real values for the two that have a shape validation checks, so
            // this test fails on a rename rather than on a malformed DN.
            Kind::Text if field.name == "base_dn" => quote("DC=corp,DC=contoso,DC=com"),
            Kind::Text if field.name == "bind_dn" => quote(r"CORP\svc-gcm"),
            // An enum rather than free text, and one whose other value is only
            // valid on Windows — so "simple" is the one this can assert with
            // on every platform.
            Kind::Text if field.name == "auth" => quote("simple"),
            Kind::Text => quote("x"),
            Kind::Number => "1".into(),
            Kind::Flag => "true".into(),
        })
    }

    #[test]
    fn every_setting_here_is_one_the_config_parser_accepts() {
        // The guard against drift. A setting renamed in config.rs but not here
        // would be read from the registry, rendered into INI, and then quietly
        // ignored by serde as an unknown key — the operator would set it and
        // watch it have no effect.
        let rendered = render(every_field).expect("a full registry should produce a config");
        let config = crate::config::parse(&rendered).unwrap_or_else(|err| {
            panic!("registry schema does not match config.rs: {err}\n{rendered}")
        });

        config
            .validate()
            .expect("a fully-populated registry should also pass validation");
        assert!(config.mariadb.is_some(), "[mariadb] should have been read");
        assert!(
            config.directory.is_some(),
            "[directory] should have been read"
        );
        // Proof the values arrived intact rather than the sections merely parsing.
        assert_eq!(config.client_id(), "x");
        assert_eq!(
            config.directory.as_ref().unwrap().bind_dn,
            r"CORP\svc-gcm"
        );
    }

    #[test]
    fn an_absent_optional_section_stays_absent() {
        // Emitting a bare `[mariadb]` header would turn the export on with an
        // empty host and fail validation, rather than leaving it off.
        let rendered = render(|section, field| {
            (section == "application" || section == "query").then(|| every_field(section, field))?
        })
        .expect("application is present, so there is a config");

        assert!(!rendered.contains("[mariadb]"), "got: {rendered}");
        assert!(!rendered.contains("[directory]"), "got: {rendered}");

        let config = crate::config::parse(&rendered).expect("should parse");
        assert!(config.mariadb.is_none());
        assert!(config.directory.is_none());
        config.validate().expect("a cloud-only tenant is valid");
    }

    #[test]
    fn no_application_section_means_no_configuration_at_all() {
        // This is the signal to migrate a config.ini or seed placeholders. A
        // registry holding only query limits is not a configuration somebody
        // can sign in with, and treating it as one would bail with a confusing
        // "client is not set" instead of setting the install up.
        assert!(
            render(|section, field| (section == "query").then(|| every_field(section, field))?)
                .is_none(),
            "only [query] present is not a configuration"
        );
        assert!(render(|_, _| None).is_none(), "an empty registry");
    }
}
