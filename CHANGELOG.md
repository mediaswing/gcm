# Changelog

## [Unreleased]

- A debug log, switched on under Settings or with `GCM_DEBUG=1`. It records
  each Graph request with its status, timing and Microsoft request ID, the
  steps of a MariaDB export, background jobs and crashes, in `gcm-debug.log`
  in the data directory. Secrets, tokens and passwords are never written.

## [1.0.0]

First release.

- Sign in to Entra ID as an app registration with a tenant ID, client ID and
  client secret (client-credentials grant), and see which Graph permissions
  the app has been granted.
- Users: search, view, create, edit, enable/disable, reset password, delete,
  and bulk import and export through CSV.
- Groups: search, view members, add and remove members, create and delete.
- Devices: Entra and Intune devices joined into one list, with Intune's sync,
  restart, remote lock, Defender scan, retire, wipe and delete actions.
- Export users, groups, memberships and devices to MariaDB or MySQL.
- Secrets can be remembered in a read-only `~/.gcm-credentials.json`.
- macOS `.app` (ad-hoc signed), Ubuntu `.deb` and Windows `.zip` builds.
