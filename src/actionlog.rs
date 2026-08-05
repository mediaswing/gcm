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
    action: String,
    target_id: &'a str,
    target_name: &'a str,
    severity: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
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

/// Record that an action is about to be attempted.
pub fn record_attempt(action: &Action, actor: Option<&str>) {
    append(&Entry {
        timestamp: Utc::now().to_rfc3339(),
        phase: "attempt",
        actor: actor.unwrap_or("unknown"),
        action: action.label(),
        target_id: action.target_id(),
        target_name: action.target_name(),
        severity: severity_name(action.severity()),
        result: None,
        error: None,
    });
}

/// Record how an action turned out.
pub fn record_outcome(action: &Action, actor: Option<&str>, result: &Result<(), String>) {
    let (outcome, error) = match result {
        Ok(()) => ("succeeded", None),
        Err(message) => ("failed", Some(message.as_str())),
    };

    append(&Entry {
        timestamp: Utc::now().to_rfc3339(),
        phase: "outcome",
        actor: actor.unwrap_or("unknown"),
        action: action.label(),
        target_id: action.target_id(),
        target_name: action.target_name(),
        severity: severity_name(action.severity()),
        result: Some(outcome),
        error,
    });
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
