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
//! macOS ships as a `.app` bundle rather than a loose binary, so the same idea
//! takes a different shape. The helper is a shell script rather than a batch
//! file, and the unit being replaced is a directory rather than a file, which
//! makes the swap safer rather than harder: the new bundle is staged beside the
//! old one and moved into place with two renames, so a failure part-way leaves
//! the operator with a working application either way.
//!
//! Three macOS-specific things are worth knowing.
//!
//! * **The executable bit has to be put back.** A zip carries Unix modes, but
//!   nothing restores them unless the extractor asks — and a `Contents/MacOS`
//!   binary without `+x` is a bundle that cannot launch at all.
//! * **Gatekeeper is less of a problem here, not more.** `com.apple.quarantine`
//!   is applied by the program that downloads a file; gcm downloads this one
//!   itself, so the replacement bundle arrives unquarantined and opens without
//!   the right-click dance the original install needed.
//! * **The ad-hoc signature survives extraction**, because it lives in the
//!   Mach-O and in `_CodeSignature/`, both ordinary files. The helper verifies
//!   it anyway and re-signs ad-hoc if it does not hold, which needs no
//!   certificate.
//!
//! Where gcm is *not* running from a bundle — a `cargo run` build, or a loose
//! binary copied somewhere — there is no install to replace and no sensible
//! guess to make, so [`Release::can_self_update`] is false and the dialog
//! offers the release page instead.

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
    /// True where [`apply`] can replace this install itself. False where it
    /// cannot, and the dialog offers the release page instead — a platform
    /// with no self-update path, or a macOS build not running from a bundle.
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
        can_self_update: can_self_update(),
    }))
}

/// Whether this install is one gcm can replace on its own.
///
/// Decided here rather than at [`apply`] time so the dialog can offer the
/// release page from the outset, instead of promising an update and then
/// failing once the download has already happened.
fn can_self_update() -> bool {
    #[cfg(windows)]
    return true;
    #[cfg(target_os = "macos")]
    return installed_bundle().is_some();
    #[cfg(not(any(windows, target_os = "macos")))]
    return false;
}

/// The `.app` bundle this process is running from.
///
/// `…/Graphical Cloud Manager.app/Contents/MacOS/gcm` yields the `.app`
/// directory. `None` when gcm is not running from a bundle at all — a
/// `cargo run` build, or a loose binary someone copied somewhere. "The install"
/// then has no well-defined meaning, and guessing at one would mean deleting a
/// directory that was never gcm's to delete.
#[cfg(target_os = "macos")]
fn installed_bundle() -> Option<std::path::PathBuf> {
    bundle_containing(&std::env::current_exe().ok()?)
}

/// The `.app` an executable path sits inside, if it sits inside one.
///
/// Split from [`installed_bundle`] only so it can be tested: the whole point
/// is which paths are rejected, and that is not observable through
/// `current_exe`.
#[cfg(target_os = "macos")]
fn bundle_containing(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let shaped_like_a_bundle = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|ext| ext == "app");
    shaped_like_a_bundle.then(|| bundle.to_path_buf())
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

/// Download the release asset and replace the running install with it.
///
/// Returns only on failure. Success hands off to the relauncher and ends the
/// process from inside this function — there is no "and then" for the caller
/// to run.
#[cfg(any(windows, target_os = "macos"))]
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
    // the release workflow named it when it packaged the build.
    let stem = release
        .asset_name
        .strip_suffix(".zip")
        .unwrap_or(&release.asset_name);
    let payload = staging.join(stem);

    #[cfg(windows)]
    {
        if !payload.join("gcm.exe").is_file() {
            bail!("the downloaded update did not contain gcm.exe");
        }

        let current_exe = std::env::current_exe().context("locating the running executable")?;
        let install_dir = current_exe
            .parent()
            .context("the running executable has no parent directory")?
            .to_path_buf();

        spawn_relauncher(&payload, &install_dir, &current_exe)?;
    }

    #[cfg(target_os = "macos")]
    install_macos(&payload, &staging)?;

    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
pub async fn apply(_http: &reqwest::Client, _release: &Release) -> Result<()> {
    bail!("gcm can only apply updates automatically on Windows and macOS")
}

#[cfg(any(windows, target_os = "macos"))]
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

            // Restore the mode the archive recorded. Without this every file
            // comes out 0644, and a `.app` whose `Contents/MacOS` binary is
            // not executable cannot be launched at all — the bundle looks
            // perfectly well-formed and simply does nothing.
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
}

/// Double every `%` in a path so it can sit inside a double-quoted batch
/// string without cmd.exe reading part of it as a `%VAR%` expansion — the one
/// character batch syntax treats specially even inside quotes. Install and
/// temp directories are named by Windows and the user, not by gcm, so a
/// literal `%` is rare but not impossible.
#[cfg(windows)]
fn bat_escape(path: &std::path::Path) -> String {
    path.display().to_string().replace('%', "%%")
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
        src = bat_escape(payload),
        dest = bat_escape(install_dir),
        exe = bat_escape(exe),
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

/// Replace the running `.app` bundle with the one just unpacked.
#[cfg(target_os = "macos")]
fn install_macos(payload: &std::path::Path, staging: &std::path::Path) -> Result<()> {
    let new_bundle = find_app_bundle(payload)
        .context("the downloaded update did not contain an .app bundle")?;

    // Belt to the braces of restoring modes during extraction: an archive
    // built without Unix attributes would leave the binary unexecutable, and
    // the resulting bundle fails to launch with nothing to explain why.
    // `Contents/MacOS` holds nothing but executables, so this is unconditional.
    force_executable(&new_bundle.join("Contents/MacOS"))?;

    let installed = installed_bundle().context(
        "gcm is not running from an .app bundle, so there is no install to replace — \
         install it from the release page instead",
    )?;
    let parent = installed
        .parent()
        .context("the installed bundle has no parent directory")?;

    // Checked before the process exits, so a read-only install directory is
    // reported in the console rather than discovered by a detached script that
    // has no way to tell anybody.
    ensure_writable(parent, &installed)?;

    spawn_relauncher_macos(&new_bundle, &installed, staging)
}

/// The single `.app` directory inside the unpacked payload.
///
/// Found rather than named, so renaming the bundle in the release workflow
/// does not silently break updating.
#[cfg(target_os = "macos")]
fn find_app_bundle(payload: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(payload).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let is_bundle =
            path.is_dir() && path.extension().is_some_and(|ext| ext == "app");
        is_bundle.then_some(path)
    })
}

/// Make everything in a directory owner-executable.
#[cfg(target_os = "macos")]
fn force_executable(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if path.is_file() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("making {} executable", path.display()))?;
        }
    }
    Ok(())
}

/// Refuse early if the install directory cannot be written.
///
/// An application in `/Applications` owned by root is the case this catches:
/// the copy would fail inside the detached helper, long after gcm has exited
/// and with no way left to report it. Probing with a real file is the only
/// honest test — directory mode bits do not account for ACLs, and macOS has
/// plenty of those.
#[cfg(target_os = "macos")]
fn ensure_writable(dir: &std::path::Path, installed: &std::path::Path) -> Result<()> {
    let probe = dir.join(format!(".gcm-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(err) => bail!(
            "gcm cannot write to {} ({err}), so it cannot replace {}. Install the update \
             by hand from the release page, or move gcm somewhere you own.",
            dir.display(),
            installed.display()
        ),
    }
}

/// Quote a path for `sh`.
///
/// Single quotes, because the paths involved routinely contain spaces
/// (`Graphical Cloud Manager.app`) and could contain `$` or a backtick. The
/// closing-and-reopening dance is the only way to get a literal single quote
/// inside a single-quoted word.
#[cfg(target_os = "macos")]
fn sh_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Write a shell script that waits for this process to exit, swaps the new
/// bundle in, relaunches it and cleans up, then spawn it detached so it
/// survives this process ending.
#[cfg(target_os = "macos")]
fn spawn_relauncher_macos(
    new_bundle: &std::path::Path,
    installed: &std::path::Path,
    staging: &std::path::Path,
) -> Result<()> {
    use std::process::Stdio;

    let pid = std::process::id();
    // Beside the staging directory rather than inside it: the script deletes
    // that directory, and `sh` may still be reading itself from disk.
    let script_path = std::env::temp_dir().join(format!("gcm-apply-update-{pid}.sh"));
    let script = relauncher_script(pid, new_bundle, installed, staging);

    std::fs::write(&script_path, script).context("writing the update helper script")?;

    // Detached by virtue of outliving us: a child reparents to launchd when
    // gcm exits, so nothing more is needed than not holding its streams open.
    std::process::Command::new("/bin/sh")
        .arg(&script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the update helper")?;

    Ok(())
}

/// The helper script itself.
///
/// Two renames rather than a copy over the top. `ditto` stages the new bundle
/// alongside the old one, and only then is anything moved — so a failure at
/// any point leaves either the old bundle or the new one in place, never a
/// half-replaced directory. Copying over the top would also leave behind any
/// file the new version had deleted.
///
/// Separated from the spawning so the generated script can be tested; running
/// it is the only way to know the swap actually works.
#[cfg(target_os = "macos")]
fn relauncher_script(
    pid: u32,
    new_bundle: &std::path::Path,
    installed: &std::path::Path,
    staging: &std::path::Path,
) -> String {
    format!(
        r#"#!/bin/sh
set -u

PID={pid}
NEW={new}
DEST={dest}
STAGING={staging}
OLD="$DEST.gcm-update-old"
STAGED="$DEST.gcm-update-new"

# Wait for gcm to exit rather than sleeping a fixed amount: on a loaded
# machine a short sleep would race the exit and copy over a running bundle.
while kill -0 "$PID" 2>/dev/null; do
  sleep 0.3
done

rm -rf "$OLD" "$STAGED"

if ! /usr/bin/ditto "$NEW" "$STAGED"; then
  rm -rf "$STAGED"
  /usr/bin/open "$DEST"
  exit 1
fi

# Downloaded in-process rather than by a browser, so nothing should have
# attached a quarantine attribute — but one would stop the bundle opening.
/usr/bin/xattr -dr com.apple.quarantine "$STAGED" 2>/dev/null

# The ad-hoc signature from the release build normally survives unpacking.
# If it did not, re-sign ad-hoc, which needs no certificate.
if ! /usr/bin/codesign --verify --strict "$STAGED" 2>/dev/null; then
  /usr/bin/codesign --force --sign - "$STAGED" 2>/dev/null
fi

if ! mv "$DEST" "$OLD"; then
  rm -rf "$STAGED"
  /usr/bin/open "$DEST"
  exit 1
fi

if ! mv "$STAGED" "$DEST"; then
  # Put the original back rather than leaving no application at all.
  mv "$OLD" "$DEST"
  rm -rf "$STAGED"
  /usr/bin/open "$DEST"
  exit 1
fi

rm -rf "$OLD" "$STAGING"
/usr/bin/open "$DEST"
rm -f "$0"
"#,
        pid = pid,
        new = sh_quote(new_bundle),
        dest = sh_quote(installed),
        staging = sh_quote(staging),
    )
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

    #[cfg(windows)]
    #[test]
    fn a_percent_in_a_path_cannot_start_a_batch_expansion() {
        // The failure this exists to stop: an install directory like
        // `C:\Users\100% Sure\gcm` would otherwise hand `%PID%`-style syntax
        // to cmd.exe, and `%Sure%` would silently expand to whatever that
        // environment variable holds (usually nothing).
        assert_eq!(
            bat_escape(std::path::Path::new(r"C:\Users\100% Sure\gcm")),
            r"C:\Users\100%% Sure\gcm"
        );
        assert_eq!(
            bat_escape(std::path::Path::new(r"C:\plain\path")),
            r"C:\plain\path"
        );
    }

    #[test]
    fn an_equal_or_lower_version_is_not_newer() {
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::super::*;
        use std::path::{Path, PathBuf};

        #[test]
        fn a_bundled_binary_is_recognised_and_a_loose_one_is_not() {
            // The dangerous direction is the second one. Treating a loose
            // binary as an install would hand the helper a directory to
            // `rm -rf` that gcm never owned — the parent of a `cargo run`
            // build is `target/debug`.
            assert_eq!(
                bundle_containing(Path::new(
                    "/Applications/Graphical Cloud Manager.app/Contents/MacOS/gcm"
                )),
                Some(PathBuf::from("/Applications/Graphical Cloud Manager.app"))
            );

            for loose in [
                "/Users/someone/gcm/target/debug/gcm",
                "/usr/local/bin/gcm",
                // Right depth, wrong shape: these must not be mistaken for a
                // bundle simply because three parents exist.
                "/Applications/NotABundle/Contents/MacOS/gcm",
                "/Applications/Thing.app/Contents/Helpers/gcm",
            ] {
                assert_eq!(
                    bundle_containing(Path::new(loose)),
                    None,
                    "{loose} is not inside an .app bundle"
                );
            }
        }

        #[test]
        fn paths_with_spaces_and_quotes_survive_the_shell() {
            // Every real install hits the space case, since the bundle is
            // called "Graphical Cloud Manager.app".
            assert_eq!(
                sh_quote(Path::new("/Applications/Graphical Cloud Manager.app")),
                "'/Applications/Graphical Cloud Manager.app'"
            );
            // A single quote has to close, escape and reopen, or the rest of
            // the script becomes part of the string.
            assert_eq!(
                sh_quote(Path::new("/Users/o'brien/Apps/gcm.app")),
                r"'/Users/o'\''brien/Apps/gcm.app'"
            );
            // Nothing inside single quotes can start a substitution.
            let quoted = sh_quote(Path::new("/tmp/$(rm -rf ~)/gcm.app"));
            assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
            assert!(!quoted.contains("'$("), "got: {quoted}");
        }

        /// Build a throwaway `.app` whose executable simply records which
        /// build it came from, so a swap can be told from a no-op.
        fn fake_bundle(at: &Path, marker: &str) {
            std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
            std::fs::write(at.join("Contents/Info.plist"), marker).unwrap();
            std::fs::write(at.join("Contents/MacOS/gcm"), marker).unwrap();
        }

        fn marker_of(bundle: &Path) -> String {
            std::fs::read_to_string(bundle.join("Contents/MacOS/gcm")).unwrap()
        }

        #[test]
        fn the_helper_actually_swaps_the_bundle() {
            // The part that cannot be reasoned about from the source: whether
            // the generated shell actually replaces one bundle with another
            // and cleans up after itself. Everything here is real — a real
            // `ditto`, real renames — except the final relaunch, which is
            // neutered so a unit test does not start launching applications.
            let root = std::env::temp_dir().join(format!("gcm-swap-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let staging = root.join("staging");
            std::fs::create_dir_all(&staging).unwrap();

            // A space in the name, because the real bundle has one.
            let installed = root.join("Graphical Cloud Manager.app");
            let incoming = staging.join("payload/Graphical Cloud Manager.app");
            fake_bundle(&installed, "old");
            fake_bundle(&incoming, "new");
            // Present in the old build and gone from the new one: a copy over
            // the top would leave this behind, a swap removes it.
            std::fs::write(installed.join("Contents/stale.txt"), "x").unwrap();

            // A pid that has already exited, so the wait loop falls straight
            // through rather than the test hanging on a live process.
            let mut dead = std::process::Command::new("/usr/bin/true").spawn().unwrap();
            let dead_pid = dead.id();
            dead.wait().unwrap();

            let script = relauncher_script(dead_pid, &incoming, &installed, &staging);
            assert!(
                script.contains("/usr/bin/open"),
                "the relaunch step moved; this test is no longer neutering it"
            );
            let script = script.replace("/usr/bin/open", "/usr/bin/true");

            let script_path = root.join("apply.sh");
            std::fs::write(&script_path, script).unwrap();
            let status = std::process::Command::new("/bin/sh")
                .arg(&script_path)
                .status()
                .unwrap();

            assert!(status.success(), "the helper failed: {status}");
            assert_eq!(marker_of(&installed), "new", "the bundle was not replaced");
            assert!(
                !installed.join("Contents/stale.txt").exists(),
                "a file dropped by the new version survived the swap"
            );
            assert!(!staging.exists(), "the staging directory was left behind");
            assert!(
                !root.join("Graphical Cloud Manager.app.gcm-update-old").exists()
                    && !root.join("Graphical Cloud Manager.app.gcm-update-new").exists(),
                "a temporary bundle was left beside the install"
            );

            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn a_failed_swap_leaves_the_original_application_in_place() {
            // The property that matters more than succeeding: never leaving
            // somebody with no application at all. Here the incoming bundle
            // does not exist, so `ditto` fails at the first step.
            let root = std::env::temp_dir().join(format!("gcm-swap-fail-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();

            let installed = root.join("Graphical Cloud Manager.app");
            fake_bundle(&installed, "old");

            let mut dead = std::process::Command::new("/usr/bin/true").spawn().unwrap();
            let dead_pid = dead.id();
            dead.wait().unwrap();

            let script = relauncher_script(
                dead_pid,
                &root.join("does-not-exist.app"),
                &installed,
                &root.join("staging"),
            )
            .replace("/usr/bin/open", "/usr/bin/true");
            let script_path = root.join("apply.sh");
            std::fs::write(&script_path, script).unwrap();
            let status = std::process::Command::new("/bin/sh")
                .arg(&script_path)
                .status()
                .unwrap();

            assert!(!status.success(), "a failed copy should report failure");
            assert_eq!(
                marker_of(&installed),
                "old",
                "the working application must survive a failed update"
            );

            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn the_executable_bit_survives_extraction() {
            // Without this the whole update lands correctly and the bundle
            // still will not launch, because `Contents/MacOS/gcm` came out
            // 0644 — a failure with nothing on screen to explain it.
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;

            let mut archive = Vec::new();
            {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut archive));
                let executable = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(0o755);
                writer
                    .start_file("payload/Contents/MacOS/gcm", executable)
                    .unwrap();
                writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();

                let ordinary = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(0o644);
                writer
                    .start_file("payload/Contents/Info.plist", ordinary)
                    .unwrap();
                writer.write_all(b"<plist/>").unwrap();
                writer.finish().unwrap();
            }

            let root = std::env::temp_dir().join(format!("gcm-zip-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            extract_zip(&archive, &root).unwrap();

            let mode = |path: &Path| {
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777
            };
            assert_eq!(mode(&root.join("payload/Contents/MacOS/gcm")), 0o755);
            // And an ordinary file is not made executable just because the
            // binary beside it was.
            assert_eq!(mode(&root.join("payload/Contents/Info.plist")), 0o644);

            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn the_app_bundle_is_found_by_shape_rather_than_by_name() {
            let root = std::env::temp_dir().join(format!("gcm-test-payload-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("Some Other Name.app/Contents/MacOS")).unwrap();
            std::fs::write(root.join("README.md"), "not a bundle").unwrap();

            assert_eq!(
                find_app_bundle(&root),
                Some(root.join("Some Other Name.app"))
            );

            // A payload with no bundle at all must report nothing rather than
            // returning some other directory.
            let empty = root.join("empty");
            std::fs::create_dir_all(&empty).unwrap();
            assert_eq!(find_app_bundle(&empty), None);

            let _ = std::fs::remove_dir_all(&root);
        }
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
