# Changelog

Notable changes to gcm, newest first.

The section for a version is what the release workflow publishes as that
release's notes, and what the console shows in its own "an update is available"
dialog — so it is written for the person deciding whether to update, not for
someone reading the diff.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
gcm uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [1.6.0] - 2026-08-07

### Added

- A changelog. Release notes and the update dialog now say what actually
  changed, instead of only how to install it.
- **Self-update on macOS.** Previously **Update now** could only open the
  release page there, leaving the new `.app` to be dragged over by hand; it now
  downloads the release and replaces the running bundle, as Windows already
  did. The new bundle is staged beside the old one and moved into place with
  two renames, so an update that fails part-way leaves a working application
  either way — and because gcm downloads it rather than a browser, the
  replacement is not quarantined and opens without the right-click → **Open**
  that the first install needed.

- **Active Directory permissions now pass through on Windows.** gcm binds to a
  domain controller as the signed-in Windows account over Kerberos, so the DC
  evaluates every read and every change against that operator and whatever has
  been delegated to them — rather than against one shared service account that
  flattens every operator into the same rights. There is no password to store
  or type, and no `bind_dn` to set. Configured as `auth` in the `[directory]`
  section, defaulting to `integrated` on Windows and `simple` elsewhere; an
  explicit `integrated` off Windows is refused rather than quietly downgraded.
- **Changes to Active Directory**, behind the same write mode, the same
  confirmation and the same audit log as tenant changes. A selected account or
  computer can be enabled, disabled, unlocked, renamed in its attributes, given
  a new password, or deleted. Two gates apply rather than one: gcm's write mode,
  and Active Directory's own access check — which under integrated
  authentication is the operator's own delegation, and whose refusals are
  reported as AD's decision rather than gcm's. A password reset over an
  unencrypted connection is refused before the password reaches the wire.
- `actions.log` entries now carry a `system` field — `microsoft-365` or
  `active-directory` — so on-premises changes can be found on their own, and
  an on-premises entry names the Windows account the DC actually authorised
  rather than the Entra account gcm signed in with.

### Changed

- **On Windows, configuration now lives in the registry**, under
  `HKEY_CURRENT_USER\Software\gcm`, rather than in `config.ini` — a registry key
  can be deployed by Group Policy and a file in a user profile cannot.
  `HKEY_LOCAL_MACHINE\Software\gcm` is read as a fallback layer beneath it, so
  an administrator can set the tenant and client ID machine-wide while leaving
  individual settings overridable per user. An existing `config.ini` is copied
  across on first run and left in place, so upgrading needs no attention.
  macOS and Linux are unchanged, and the token cache, `error.log` and
  `actions.log` stay files on every platform.
- The failure screen offers **Copy the configuration** on Windows, which renders
  the registry contents back as INI text — a registry key cannot be pasted into
  a support ticket, and that the configuration holds no secrets is a property
  worth keeping usable.

## [1.5.0] - 2026-08-06

### Added

- **Read-only on-premises Active Directory.** A Directory node reads users and
  computers from a domain controller over LDAP, and — the more useful half —
  joins what it finds onto the existing Users pane. The OU an account lives in,
  its `userAccountControl` flags, when its password was really last set, and
  which of its groups never reached the cloud are all on the far side of the
  sync, and are usually the reason somebody went looking. Two things worth
  calling out: a disabled AD account whose tenant shadow is still enabled, and
  that `pwdLastSet` is not Entra's `lastPasswordChangeDateTime`.
- The AD views join the export list, so CSV, JSON and the MariaDB export pick
  them up as `ad_users` and `ad_computers`.
- A right-click menu on every list view. Four views previously opened an empty
  popup, which reads as broken rather than restricted. Copy row, Tick for bulk,
  Tick all shown, Export as CSV/JSON and Refresh are now offered everywhere and
  are never disabled — the write gate has no business gating a copy.
- A second toolbar row carrying what can be done to the selected row, or to the
  whole ticked set.

### Changed

- The toolbar's primary button follows the node it is on: New user… on Users,
  New group… on Groups, Export… where nothing can be created. It previously
  read "New user…" on all eleven nodes, naming the wrong object on nine.
- `Ctrl+N` on a node where nothing can be created now says so, rather than
  opening the create-user form and jumping to Users.

### Fixed

- **Sign out now signs out.** It deleted the cached token and nothing else: the
  in-memory tokens survived, so the console carried on reading the tenant, and
  the re-sign-in reissued silently from the browser's session cookie and
  brought the same account straight back. It now drops both, resets whether
  writes are available, and asks Entra for the account picker.
- A search stopped short by a domain controller's `sizeLimit` or `adminLimit`
  returned a partial list as though it were the whole domain. The closing
  result code is now checked.
- `start_tls = true` with `tls = false` silently connected in the clear while
  reading as though TLS had been asked for. Now refused.
- Cancelling the AD bind dialog left the view showing an ordinary "No items.",
  a false statement about a domain that was never read.
- Synced users all reported "no matching account found" in demo mode, which is
  indistinguishable from a genuinely absent object.

## [1.3.0] - 2026-08-06

### Added

- **Self-update.** gcm checks GitHub releases at startup and offers to apply
  the update. On Windows it replaces the running install via a detached
  relauncher; on macOS it opens the release page, because replacing a running
  `.app` bundle in place is a different and riskier problem.
- A right-click menu on the scope tree's nodes.

### Fixed

- The New Group button always offered "New user…" and created a user.
- The MariaDB export had no connect timeout, so an unreachable host hung
  indefinitely with nothing written and no error. It now gives up at 15s.
- CSV export had no defence against formula injection: a directory value
  starting with `=`, `+`, `-` or `@` — settable through self-service profile
  edits, not just by an operator — would execute when the file was opened in
  Excel. Cells are now defused.
- The VirusTotal step computed its hash with `sha256sum`, which does not exist
  on the macOS runner. It would have published a link with no hash in it while
  reporting success.

## [1.2.0] - 2026-08-06

### Added

- **MariaDB export** (`Ctrl+Shift+D`), writing every loaded view into one table
  per collection. No password on disk: the `[mariadb]` section carries only
  what is safe to paste into a support ticket, and gcm asks for the password
  once per session. Tables are built as staging and swapped in with
  `RENAME TABLE`, so a dashboard reading `gcm_users` sees either the whole
  previous export or the whole new one — never a table mid-refill. Unlike the
  file exports it is unfiltered, because a table quietly narrowed by whatever
  was in the filter box twenty minutes ago would trap every query that later
  joins against it.

## [1.1.0] - 2026-08-06

### Added

- User creation.
- Sign-in and directory audit log views.
- Exchange and Teams.
- A diagnostic log, separate from the audit trail, recording what went wrong
  rather than what was changed.

## [1.0.0] - 2026-08-06

### Added

- First release. An MMC-style console for Microsoft 365: a scope tree, list
  views, and a details pane over Users, Groups, Devices, Licenses and roles.
- CSV and JSON export, and CSV import with a preview.
- Read-only by default, with a write gate that must be armed deliberately.
- A release workflow building a signed `.app` bundle on macOS and bundling the
  VC++ runtime on Windows.

[Unreleased]: https://github.com/mediaswing/gcm/compare/v1.6.0...HEAD
[1.6.0]: https://github.com/mediaswing/gcm/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/mediaswing/gcm/compare/v1.3.0...v1.5.0
[1.3.0]: https://github.com/mediaswing/gcm/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/mediaswing/gcm/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/mediaswing/gcm/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/mediaswing/gcm/releases/tag/v1.0.0
