//! A diagnostic log of everything that went wrong.
//!
//! [`crate::actionlog`] answers "what did this console change?" — it is an
//! audit trail, and it records only writes. This file answers the different
//! question somebody asks when the console is misbehaving: *what actually
//! happened?* Failed sign-ins, collections that would not load, features the
//! tenant refused, and every failed write, in the order they occurred.
//!
//! The two are kept apart deliberately. Mixing diagnostics into the audit trail
//! would bury the handful of lines that say who deleted what under a running
//! commentary of throttling and permission errors.
//!
//! ## Where it lives
//!
//! Alongside `config.ini` and `actions.log` in gcm's own directory, rather than
//! loose in the home directory: it is gcm's file, it belongs with gcm's other
//! files, and there is then exactly one folder to send when somebody asks for
//! diagnostics. The path is shown in the console — under Keyboard help, and on
//! the failure screen — because a log nobody can find is not a diagnostic.
//!
//! ## What it does not contain
//!
//! No access tokens, no refresh tokens, and no passwords. Those pass through
//! the same code paths as everything logged here, so the rule is that this
//! module is only ever handed an already-rendered error message, never a
//! request body — see [`redact`], which is the belt to that braces.
//!
//! Plain text rather than JSON, one line per event: this file gets read by a
//! person, usually in a hurry, quite possibly over somebody's shoulder.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;

use crate::config::config_dir;

/// Trim the file once it passes this, so a console left running for a month
/// with a broken permission cannot fill a disk.
const MAX_BYTES: u64 = 512 * 1024;

/// How much of the file to keep when trimming. Keeping the tail rather than the
/// head is the point — the most recent failures are the ones being diagnosed.
const KEEP_BYTES: usize = 128 * 1024;

/// Serialises writers, so two threads reporting at once cannot interleave
/// halfway through a line. The worker thread and the UI thread both log.
static LOCK: Mutex<()> = Mutex::new(());

pub fn log_path() -> PathBuf {
    config_dir().join("error.log")
}

/// How serious an entry is. Written in the line so the file can be grepped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Something failed and the operator will have noticed.
    Error,
    /// Something was refused or degraded but the console carried on — a tenant
    /// without Intune, a collection that would not load.
    Warn,
    /// A milestone worth having for context around the lines that matter:
    /// startup, sign-in, write mode being armed.
    Info,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
        }
    }
}

/// Append one line: timestamp, level, the area it came from, and the message.
///
/// Never fails and never panics. A console that crashed while trying to write
/// its own error log would be a poor advertisement, and there is nothing useful
/// to do about a log that cannot be written anyway.
pub fn log(level: Level, area: &str, message: &str) {
    let line = format!(
        "{} {} [{area}] {}",
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ"),
        level.tag(),
        // Newlines would break the one-event-per-line contract that makes the
        // file greppable; Graph error bodies routinely contain them.
        redact(message).replace('\n', " ⏎ ")
    );

    let Ok(_guard) = LOCK.lock() else {
        return;
    };

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    trim_if_large(&path);

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    restrict_permissions(&path);
    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

pub fn error(area: &str, message: &str) {
    log(Level::Error, area, message);
}

pub fn warn(area: &str, message: &str) {
    log(Level::Warn, area, message);
}

pub fn info(area: &str, message: &str) {
    log(Level::Info, area, message);
}

/// Last-ditch scrub for anything token-shaped that reached a message by
/// mistake.
///
/// Nothing here is *supposed* to carry a credential — callers pass rendered
/// error text, not request bodies. But a diagnostic file is exactly the thing
/// somebody emails to a vendor, and an access token in it would be a real
/// incident. So the shapes that matter are matched and cut, and the cost of
/// occasionally redacting something harmless is accepted.
fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());

    for word in message.split_inclusive(char::is_whitespace) {
        let trimmed = word.trim_end();
        let looks_like_a_token = trimmed.starts_with("eyJ")
            || trimmed.len() > 200 && !trimmed.contains(' ');
        if looks_like_a_token {
            out.push_str("[redacted]");
            // Keep whatever whitespace followed, so the line still reads.
            out.push_str(&word[trimmed.len()..]);
        } else {
            out.push_str(word);
        }
    }

    out
}

/// Keep the file bounded, preserving the most recent entries.
fn trim_if_large(path: &PathBuf) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() <= MAX_BYTES {
        return;
    }

    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };
    // Cut at a line boundary so the file never opens mid-entry.
    let tail = contents
        .char_indices()
        .nth(contents.chars().count().saturating_sub(KEEP_BYTES))
        .map(|(index, _)| &contents[index..])
        .unwrap_or(&contents);
    let tail = match tail.find('\n') {
        Some(newline) => &tail[newline + 1..],
        None => tail,
    };

    let _ = fs::write(
        path,
        format!("--- earlier entries trimmed to keep this file small ---\n{tail}"),
    );
}

/// The log names accounts, tenants and object ids, so it is kept owner-only
/// like the token cache and the audit trail beside it.
#[cfg(unix)]
fn restrict_permissions(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_written_at_a_fixed_width() {
        // So that the [area] column lines up when the file is read by eye.
        let widths: Vec<usize> = [Level::Error, Level::Warn, Level::Info]
            .iter()
            .map(|level| level.tag().len())
            .collect();
        assert_eq!(widths, vec![5, 5, 5]);
    }

    #[test]
    fn a_jwt_never_reaches_the_file() {
        let message = "sign-in failed with token \
                       eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.payload.signature here";
        let redacted = redact(message);
        assert!(!redacted.contains("eyJ0eXAi"));
        assert!(redacted.contains("[redacted]"));
        // The rest of the sentence has to survive, or the line says nothing.
        assert!(redacted.contains("sign-in failed with token"));
        assert!(redacted.contains("here"));
    }

    #[test]
    fn an_improbably_long_opaque_word_is_redacted() {
        // Refresh tokens are not JWTs and have no recognisable prefix; length
        // with no spaces is the only signal left.
        let secret = "M.C107_BAY".to_string() + &"a".repeat(400);
        let redacted = redact(&format!("refresh failed: {secret}"));
        assert!(!redacted.contains(&secret));
        assert!(redacted.contains("refresh failed:"));
    }

    #[test]
    fn ordinary_messages_pass_through_untouched() {
        let message = "403 — Forbidden: Tenant is not licensed for Microsoft Intune";
        assert_eq!(redact(message), message);
    }

    #[test]
    fn a_message_keeps_its_words_and_spacing() {
        assert_eq!(redact("a b  c"), "a b  c");
        assert_eq!(redact(""), "");
    }
}
