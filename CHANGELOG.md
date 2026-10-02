# Changelog

## [1.2.0]

- A debug log, switched on under Settings or with `GCM_DEBUG=1`. It records
  each Graph request with its status, timing and Microsoft request ID, the
  steps of a MariaDB export, background jobs and crashes, in `gcm-debug.log`
  in the data directory. Secrets, tokens and passwords are never written.
- Graph requests that are throttled (429) are sent again after the wait
  Graph asks for, as are reads and updates that meet a busy service (503,
  504). Large imports and exports no longer fail on the first one.
- A group deleted while the members export is running is skipped, instead
  of failing the whole export.
- The MariaDB export files rows under the tenant's GUID, taken from the
  sign-in, so signing in by a domain name no longer makes a second copy of
  the tenant. Rows exported earlier under a domain name are left as they
  are; delete them by hand if you no longer want them.
- `gcm_devices.display_name` is NULL for a device with no name, rather than
  "(no name)".
- Signing in or out waits while a CSV import is running, and the last
  import's results, with any generated passwords, survive a sign-out.
- Editing the Connection boxes while a sign-in is under way no longer
  saves the edited values, or remembers the secret against them.
- Deleting a device from Entra ID keeps its Intune record in the list.
- A failure to load a user's groups or a group's members is shown for that
  user or group, not for whichever one is selected when it arrives.
- Sign-in errors in the status bar leave out Microsoft's trace and
  correlation IDs, which are in the debug log instead.
- The macOS release package is built for Apple silicon only
  (`gcm-<version>-macos-aarch64.zip`) rather than as a universal binary.
  `gcm-package --universal` still builds one for Intel Macs too.
- A release's notes on GitHub are its section of this changelog.
- The README now says what Windows does with the privacy of the files the
  app writes, rather than claiming they are readable only by you.
