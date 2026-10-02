# Changelog

## [1.2.0]

- A Licensing tab: every subscription with its assigned, available and
  total counts, who holds each one (directly or through a group), assigning
  by sign-in name, and removing a direct assignment. Needs
  `LicenseAssignment.ReadWrite.All`, or `User.ReadWrite.All` and
  `Organization.Read.All`.
- A user's licences in their details on the Users tab, and a Licences…
  dialog to tick and untick products for them.
- A Logs tab with the sign-in log and the directory audit log, for the last
  hour up to the last 30 days, filtered by user and to failures, with full
  details of each entry and export to CSV. Needs `AuditLog.Read.All`; sign-ins
  also need Entra ID P1 or P2 in the tenant.
- A Sign-ins button in a user's details that opens their sign-ins.
- The workflow's actions run on Node.js 24.
- The VirusTotal reports are linked from the release notes.
