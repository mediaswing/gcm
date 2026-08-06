//! Checking GitHub for a newer gcm, and — on Windows — replacing the running
//! install with it.
//!
//! Windows allows renaming or deleting a file that is still mapped in by a
//! running process (the loader opens it with `FILE_SHARE_DELETE`), but it will
//! not let anything overwrite that file's *contents* while it is running.
//! `gcm.exe` cannot replace itself, so the update this process cannot apply is
//! handed to a small detached helper: it waits for this process to exit,
//! copies the new build over the install directory, and relaunches it.
//! Nothing here touches gcm's own configuration — that lives in the user's
//! config directory, not beside the binary — so the copy only ever replaces
//! what the release zip ships.
//!
//! macOS ships as a `.app` bundle rather than a loose binary. Replacing one in
//! place while it is running, and re-signing it, is a different and riskier
//! problem this module does not attempt — there, "update" means opening the
//! release page and letting the operator drag the new bundle over, the same
//! as the first install.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const REPO: &str = "mediaswing/gcm";

/// A release newer than the one running, worth telling the operator about.
#[derive(Debug, Clone)]
pub struct Release {
    /// Without the leading `v`, e.g. `1.3.0`.
    pub version: String,
    pub notes: String,
    pub html_url: String,
    asset_url: String,
    asset_name: String,
    /// True on Windows, where [`apply`] can replace the install itself. False
    /// elsewhere, where the dialog offers the release page instead.
    pub can_self_update: bool,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The asset name this platform's release workflow publishes, without the
/// `.zip`. `None` on a platform the release workflow does not build for.
fn asset_stem() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("gcm-windows-x86_64")
    } else if cfg!(target_os = "macos") {
        Some("gcm-macos-aarch64")
    } else {
        None
    }
}

/// Ask GitHub for the latest release and return it if it is both newer than
/// this build and has shipped an asset for this platform.
///
/// Failures here (no network, GitHub unreachable, an unexpected response
/// shape) are the caller's to decide whether to surface — a start-up check
/// that cannot reach GitHub is not something worth interrupting anyone over.
pub async fn check(http: &reqwest::Client) -> Result<Option<Release>> {
    let Some(stem) = asset_stem() else {
        return Ok(None);
    };

    let response = http
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("checking for an update")?;

    if !response.status().is_success() {
        bail!(
            "GitHub returned {} while checking for updates",
            response.status()
        );
    }

    let release: GhRelease = response
        .json()
        .await
        .context("reading the release list")?;

    if release.draft || release.prerelease {
        return Ok(None);
    }

    let version = release.tag_name.trim_start_matches('v');
    if !is_newer(version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    let asset_name = format!("{stem}.zip");
    // A release still mid-build, with its tag pushed but not every platform's
    // asset uploaded yet, is not an update to offer — only the next poll
    // needs to see it.
    let Some(asset) = release.assets.iter().find(|a| a.name == asset_name) else {
        return Ok(None);
    };

    Ok(Some(Release {
        version: version.to_string(),
        notes: release.body.unwrap_or_default(),
        html_url: release.html_url,
        asset_url: asset.browser_download_url.clone(),
        asset_name: asset.name.clone(),
        can_self_update: cfg!(target_os = "windows"),
    }))
}

/// Plain `major.minor.patch` comparison; gcm's own versions never carry
/// anything else, and a scheme mismatch is treated as "not newer" rather than
/// guessed at.
fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some((major, minor, patch))
    }
    match (parts(candidate), parts(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Download the release asset and, on Windows, replace the running install.
///
/// Returns only on failure. Success hands off to the relauncher and ends the
/// process from inside this function — there is no "and then" for the caller
/// to run.
#[cfg(windows)]
pub async fn apply(http: &reqwest::Client, release: &Release) -> Result<()> {
    let bytes = http
        .get(&release.asset_url)
        .send()
        .await
        .context("downloading the update")?
        .error_for_status()
        .context("downloading the update")?
        .bytes()
        .await
        .context("reading the downloaded update")?;

    let staging = std::env::temp_dir().join(format!("gcm-update-{}", release.version));
    if staging.exists() {
        // Left over from a previous attempt at this same version; the fresh
        // extraction below is the only copy that should exist.
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(&staging).context("creating a staging directory")?;

    extract_zip(&bytes, &staging).context("unpacking the update")?;

    // The zip's own top-level folder, e.g. `gcm-windows-x86_64`, exactly as
    // the release workflow named it when it ran `Compress-Archive`.
    let stem = release
        .asset_name
        .strip_suffix(".zip")
        .unwrap_or(&release.asset_name);
    let payload = staging.join(stem);
    if !payload.join("gcm.exe").is_file() {
        bail!("the downloaded update did not contain gcm.exe");
    }

    let current_exe = std::env::current_exe().context("locating the running executable")?;
    let install_dir = current_exe
        .parent()
        .context("the running executable has no parent directory")?
        .to_path_buf();

    spawn_relauncher(&payload, &install_dir, &current_exe)?;
    Ok(())
}

#[cfg(not(windows))]
pub async fn apply(_http: &reqwest::Client, _release: &Release) -> Result<()> {
    bail!("gcm can only apply updates automatically on Windows")
}

#[cfg(windows)]
fn extract_zip(bytes: &[u8], into: &std::path::Path) -> Result<()> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).context("reading the update archive")?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name` refuses a path that would escape `into` — a `..`
        // segment or an absolute path — rather than extracting it anyway.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let dest = into.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

/// Write a batch script that waits for this process to exit, copies the new
/// build over the install directory, relaunches it, and deletes itself, then
/// spawn it detached so it survives this process ending.
#[cfg(windows)]
fn spawn_relauncher(
    payload: &std::path::Path,
    install_dir: &std::path::Path,
    exe: &std::path::Path,
) -> Result<()> {
    use std::os::windows::process::CommandExt;

    let pid = std::process::id();
    let script_dir = payload.parent().unwrap_or(payload);
    let script_path = script_dir.join("apply-update.bat");

    // Waits for this PID to vanish from the process list rather than for a
    // fixed delay: on a machine under load, or with write mode mid-flight
    // when the update was requested, a short sleep would race the exit and
    // copy over a still-open file.
    let script = format!(
        r#"@echo off
setlocal
set "PID={pid}"
:wait
tasklist /FI "PID eq %PID%" /NH | find "%PID%" >nul
if not errorlevel 1 (
  ping -n 2 127.0.0.1 >nul
  goto wait
)
robocopy "{src}" "{dest}" /E /IS /IT /R:5 /W:1 >nul
start "" "{exe}"
rmdir /s /q "{src}"
del "%~f0"
"#,
        pid = pid,
        src = payload.display(),
        dest = install_dir.display(),
        exe = exe.display(),
    );

    std::fs::write(&script_path, script).context("writing the update helper script")?;

    // DETACHED_PROCESS so the helper outlives this process rather than being
    // torn down with its console; CREATE_NO_WINDOW because a batch script
    // flashing a console open is not part of the experience.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new("cmd")
        .args(["/C", &script_path.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .context("starting the update helper")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_version_is_newer() {
        assert!(is_newer("1.3.0", "1.2.0"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(is_newer("1.2.1", "1.2.0"));
    }

    #[test]
    fn an_equal_or_lower_version_is_not_newer() {
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
    }

    #[test]
    fn an_unparsable_version_is_never_newer() {
        // A tag that does not fit gcm's own major.minor.patch scheme is
        // treated as "nothing to offer" rather than guessed at.
        assert!(!is_newer("1.2", "1.2.0"));
        assert!(!is_newer("1.2.0-beta", "1.2.0"));
        assert!(!is_newer("not-a-version", "1.2.0"));
    }
}
