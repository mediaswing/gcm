//! A local record of every write gcm attempts.
//!
//! Entra keeps its own audit log, but it records the app registration as the
//! actor — every change gcm makes looks identical to every other. This file is
//! the only place that says *this console, on this machine, issued that wipe*.
//!
//! One JSON object per line, appended and flushed immediately. The attempt is
//! written before the call goes out and the outcome is appended after, so an
//! action that never returns — a crash, a power cut mid-wipe — still leaves
//! evidence that it was issued. A log that only records successes is precisely
//! useless in the situation you need it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;

use crate::config::config_dir;
use crate::graph::actions::{Action, Severity};
use crate::ldap::actions::DirectoryAction;

pub fn log_path() -> PathBuf {
    config_dir().join("actions.log")
}

#[derive(Debug, Serialize)]
struct Entry<'a> {
    timestamp: String,
    /// `attempt` or `outcome`.
    phase: &'a str,
    /// Signed-in account that issued it.
    actor: &'a str,
    /// Which directory was changed — `microsoft-365` or `active-directory`.
    ///
    /// Worth a field of its own rather than being inferred from the action
    /// name: the two have separate blast radiuses and separate people
    /// answerable for them, and "did anything touch the on-premises domain
    /// last Tuesday" should be one grep rather than a list of action names
    /// somebody has to keep up to date.
    system: &'a str,
    action: String,
    target_id: &'a str,
    target_name: &'a str,
    severity: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// The directory an action changed.
const SYSTEM_TENANT: &str = "microsoft-365";
const SYSTEM_DIRECTORY: &str = "active-directory";

/// Everything the log needs to know about an action, whichever kind it is.
///
/// The two action types have nothing in common structurally, but they have
/// exactly these four things in common semantically — so the log is written
/// once against this rather than twice against them.
struct Recorded<'a> {
    system: &'a str,
    action: String,
    target_id: &'a str,
    target_name: &'a str,
    severity: Severity,
}

impl<'a> Recorded<'a> {
    fn tenant(action: &'a Action) -> Self {
        Self {
            system: SYSTEM_TENANT,
            action: action.label(),
            target_id: action.target_id(),
            target_name: action.target_name(),
            severity: action.severity(),
        }
    }

    fn directory(action: &'a DirectoryAction) -> Self {
        Self {
            system: SYSTEM_DIRECTORY,
            action: action.label(),
            target_id: action.target_id(),
            target_name: action.target_name(),
            severity: action.severity(),
        }
    }

    fn attempt(&self, actor: Option<&str>) {
        append(&Entry {
            timestamp: Utc::now().to_rfc3339(),
            phase: "attempt",
            actor: actor.unwrap_or("unknown"),
            system: self.system,
            action: self.action.clone(),
            target_id: self.target_id,
            target_name: self.target_name,
            severity: severity_name(self.severity),
            result: None,
            error: None,
        });
    }

    fn outcome(&self, actor: Option<&str>, result: &Result<(), String>) {
        let (outcome, error) = match result {
            Ok(()) => ("succeeded", None),
            Err(message) => ("failed", Some(message.as_str())),
        };

        append(&Entry {
            timestamp: Utc::now().to_rfc3339(),
            phase: "outcome",
            actor: actor.unwrap_or("unknown"),
            system: self.system,
            action: self.action.clone(),
            target_id: self.target_id,
            target_name: self.target_name,
            severity: severity_name(self.severity),
            result: Some(outcome),
            error,
        });
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Safe => "safe",
        Severity::Caution => "caution",
        Severity::Destructive => "destructive",
    }
}

fn append(entry: &Entry<'_>) {
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };

    restrict_permissions(&path);
    // Flushed rather than buffered: an unflushed audit line helps nobody.
    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

/// The log names who changed what in the tenant, so it is kept owner-only like
/// the token cache beside it.
#[cfg(unix)]
fn restrict_permissions(path: &PathBuf) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &PathBuf) {}

/// Record that a tenant change is about to be attempted.
pub fn record_attempt(action: &Action, actor: Option<&str>) {
    Recorded::tenant(action).attempt(actor);
}

/// Record how a tenant change turned out.
pub fn record_outcome(action: &Action, actor: Option<&str>, result: &Result<(), String>) {
    Recorded::tenant(action).outcome(actor, result);
}

/// Record that an on-premises change is about to be attempted.
///
/// The same two-phase treatment as a tenant write, for the same reason: a
/// password reset that never returns still has to leave evidence that it was
/// issued.
pub fn record_directory_attempt(action: &DirectoryAction, actor: Option<&str>) {
    Recorded::directory(action).attempt(actor);
}

/// Record how an on-premises change turned out.
pub fn record_directory_outcome(
    action: &DirectoryAction,
    actor: Option<&str>,
    result: &Result<(), String>,
) {
    Recorded::directory(action).outcome(actor, result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::actions::DeviceOp;

    fn wipe() -> Action {
        Action::ManagedDevice {
            id: "device-1".into(),
            name: "LON-LT-0041".into(),
            op: DeviceOp::Wipe,
        }
    }

    #[test]
    fn attempt_entries_serialise_with_the_expected_shape() {
        let action = wipe();
        let entry = Entry {
            timestamp: "2026-08-05T12:00:00Z".into(),
            phase: "attempt",
            actor: "admin@contoso.co.uk",
            system: SYSTEM_TENANT,
            action: action.label(),
            target_id: action.target_id(),
            target_name: action.target_name(),
            severity: severity_name(action.severity()),
            result: None,
            error: None,
        };

        let json = serde_json::to_string(&entry).expect("should serialise");
        assert!(json.contains("\"phase\":\"attempt\""));
        assert!(json.contains("\"severity\":\"destructive\""));
        assert!(json.contains("Wipe LON-LT-0041"));
        // Absent fields are omitted rather than written as null.
        assert!(!json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn failures_carry_their_reason() {
        let action = wipe();
        let entry = Entry {
            timestamp: "2026-08-05T12:00:01Z".into(),
            phase: "outcome",
            actor: "admin@contoso.co.uk",
            system: SYSTEM_TENANT,
            action: action.label(),
            target_id: action.target_id(),
            target_name: action.target_name(),
            severity: severity_name(action.severity()),
            result: Some("failed"),
            error: Some("403 — Forbidden"),
        };

        let json = serde_json::to_string(&entry).expect("should serialise");
        assert!(json.contains("\"result\":\"failed\""));
        assert!(json.contains("403"));
    }

    #[test]
    fn every_line_is_valid_standalone_json() {
        // The format's whole value is being greppable and machine-readable one
        // line at a time.
        let action = wipe();
        let entry = Entry {
            timestamp: Utc::now().to_rfc3339(),
            phase: "outcome",
            actor: "a@b.com",
            system: SYSTEM_TENANT,
            action: action.label(),
            target_id: action.target_id(),
            target_name: action.target_name(),
            severity: severity_name(action.severity()),
            result: Some("succeeded"),
            error: None,
        };

        let line = serde_json::to_string(&entry).expect("should serialise");
        assert!(!line.contains('\n'));
        serde_json::from_str::<serde_json::Value>(&line).expect("each line must parse");
    }
}
