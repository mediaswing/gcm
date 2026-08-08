# gcm — Graphical Cloud Manager

A directory administration console for the desktop, written in Rust, covering
both halves of a hybrid estate.

**Microsoft 365** — users, groups and directory roles, Entra and Intune-managed
devices, licence consumption, Exchange mailboxes, Teams, and the sign-in and
audit logs. **On-premises Active Directory** — users and computers read straight
from a domain controller, with each synced account joined to its on-premises
object so the OU it lives in, its `userAccountControl` flags and when its
password was really last set sit on the same property sheet as the cloud half.

All of it in the shape of a classic MMC snap-in: a scope tree on the left, a
result list in the middle, a property sheet on the right.

Everything in it is reachable from the keyboard. It opens **read-only every
time**, and changing anything requires deliberately arming write mode — see
[Making changes](#making-changes).

Changes to Active Directory carry a second gate that tenant changes do not: on
Windows gcm binds as the signed-in operator, so the domain controller enforces
whatever has been delegated to *them*, rather than a shared service account's
rights standing in for everybody.

```
┌──────────────┬───────────────────────────────┬──────────────────┐
│ CONSOLE      │ Users              12 items   │ DETAILS          │
│ Console Root │ ┌───────────────────────────┐ │ Aisha Rahman     │
│   Users      │ │ Aisha Rahman  Finance  ✓  │ │ Sign-in name …   │
│ ▾ Groups     │ │ Ben Okafor    IT       ✓  │ │ Account  Enabled │
│     Roles    │ │ Chloe Duval   Marketing✓  │ │ ORGANISATION     │
│ ▾ Devices    │ │ …                         │ │ Job title …      │
│     Managed  │ └───────────────────────────┘ │ MEMBER OF …      │
│   Licenses   │                               │                  │
│   Exchange   │                               │                  │
│   Teams      │                               │                  │
│ ▾ Monitoring │                               │                  │
│     Audit    │                               │                  │
└──────────────┴───────────────────────────────┴──────────────────┘
  Ready                    Contoso · Focus: Results · F1 shortcuts
```

## What it shows

| View | Contents |
| --- | --- |
| **Users** | Every user, with sign-in name, job title, department, enabled state and member/guest type. Details add contact fields, on-premises sync origin, assigned licences resolved to product names, email aliases, and group and role membership. |
| **Groups** | Security, Microsoft 365, distribution and mail-enabled security groups, classified the way the Entra portal words it, with assigned/dynamic membership and cloud/AD origin. Details add the dynamic membership rule, owners and members. |
| **Directory Roles** | Roles activated in the tenant and who holds each one. Listed under Groups because both answer "what does this person get?" |
| **Entra Devices** | Registered and joined devices, with join type (Entra joined, hybrid joined, Entra registered), compliance, and last sign-in. |
| **Managed Devices** | Intune enrolments: compliance state, management agent, ownership, hardware, encryption and supervision, storage and last check-in. Degrades to a clear explanation when the tenant has no Intune — see below. |
| **Licenses** | Every subscribed SKU with a friendly product name, seats purchased, assigned and available, a usage bar, and the service plans inside each SKU. Over-assignment is called out rather than shown as a negative number. |
| **Exchange** | Every mailbox with size against quota, item count and last activity, fullest first. Mailboxes newer than the daily usage report are listed too, from the directory, marked as not yet reported. Details add all three quota thresholds, archive state, deleted-item volume, and the mailbox's time zone, language and automatic replies. |
| **Teams** | Every team with visibility and archived state. Details add channels with their addresses, what members and guests are allowed to do, messaging and Giphy settings, and a link straight into the Teams client. |
| **Sign-in Logs** | Recent sign-ins with the user, application, outcome, risk state and location. Details add the failure code and reason, the device and its compliance, Conditional Access outcome, and the correlation ID to quote at Microsoft support. |
| **Audit Logs** | Recent directory changes: what happened, in which category, who did it, to what, and whether it worked — with the before-and-after values of each modified property. |
| **AD Users** | User accounts read straight from an on-premises domain controller: SAM account, UPN, status and OU. Details decode `userAccountControl` into readable flags, and add the real `pwdLastSet`, last logon, account expiry, lockout state, object GUID and SID, and group membership. Optional — see [On-premises Active Directory](#on-premises-active-directory). |
| **AD Computers** | Computer accounts with DNS name, operating system, whether the machine is a domain controller or a member, and its OU. |

### Where the mailbox list comes from

Two places, because neither is sufficient alone.

Graph has no collection that lists mailboxes; `/users` knows about accounts, not
about the mailboxes behind them. The Exchange view therefore reads the
`getMailboxUsageDetail` report, which is the only v1.0 endpoint that enumerates
them — and which happens to carry exactly the numbers somebody opens Exchange to
check. It arrives as CSV rather than JSON, addressed by column name rather than
position, because Microsoft has changed the column set before.

That report is compiled once a day, which used to mean a mailbox created this
morning was simply missing. So accounts holding an **enabled Exchange service
plan** are folded in from `/users` as well — a live signal, and one that costs
nothing, since the Users collection is already loaded. A mailbox created minutes
ago therefore appears straight away, marked **Not yet reported** where its size,
quota and activity would be.

The two halves miss different things, which is why the view unions them rather
than choosing:

| | Covers | Misses |
| --- | --- | --- |
| **The usage report** | Every number the view shows; shared, room and equipment mailboxes, which hold no licence | Anything created since it was last compiled |
| **`/users`** | Mailboxes created since then | Every figure — Graph exposes no mailbox size or quota at all; unlicensed mailboxes |

Rows the report supplied always win, so a mailbox known to both is the reported
one, numbers and all. Unreported rows sort below every reported one — the view
orders by fullness, and a row with no figures is not an empty mailbox.

Two more consequences worth knowing. **The figures stay up to a day stale**;
this fixes "where is the mailbox I just made?", not "is this mailbox full right
now". And a tenant with *Reports → Display concealed user information* switched
on returns anonymised identifiers instead of names — there is then no UPN to
join on, so the union is skipped entirely rather than appending every mailbox a
second time under its real name. gcm says so in the details pane rather than
leaving you to wonder why every mailbox looks like a serial number.

### What Graph cannot do to Exchange

Mailbox permissions, forwarding rules, litigation hold, retention policies and
transport rules are Exchange Online PowerShell, not Graph. They are not here,
and no amount of consent will make them appear. What Graph *does* expose —
mailbox size, quota, activity and automatic replies — is what this view offers.

### When the tenant does not have something

Several views depend on things a tenant may simply not have: Intune, Exchange,
Teams, or Entra ID P1 for the sign-in log. All of them answer `403` — sometimes
`404`, occasionally a `400` whose text mentions a licence — and gcm treats every
one as a state to report rather than an error to raise.

Each such view explains itself in three layers: a dry one-line headline, the
plain-English fix beneath it, and the exact response Graph gave at the bottom.
The headline is allowed to be funny; the two lines under it are not, and the
rule that keeps them apart is enforced by a test. The rest of the console
carries on regardless — every collection loads independently, so one refusal
never blanks the window.

## Requirements

- Rust 1.85 or newer (the crate is on the 2024 edition).
- An Entra ID app registration in the tenant you want to read.

## Setting up the app registration

In the Entra admin centre, under **App registrations → New registration**:

1. Give it any name.
2. Open **Authentication**:
   - **Add a platform → Mobile and desktop applications**, and add the redirect
     URI `http://localhost` — no port. Not the *Web* platform, which implies a
     confidential client and would want a secret.
   - Turn on **Allow public client flows**. gcm holds no client secret, so this
     is required.
3. Under **API permissions**, add these **delegated** Microsoft Graph
   permissions, then grant admin consent.

   Read — needed to display anything:

   | Permission | Needed for |
   | --- | --- |
   | `User.Read.All` | Users |
   | `Group.Read.All` | Groups |
   | `GroupMember.Read.All` | Group members and owners |
   | `Directory.Read.All` | Directory objects generally |
   | `RoleManagement.Read.Directory` | Directory roles |
   | `Device.Read.All` | Entra devices |
   | `DeviceManagementManagedDevices.Read.All` | Intune devices |
   | `Organization.Read.All` | Tenant summary and licences |
   | `AuditLog.Read.All` | Sign-in and audit logs |
   | `Team.ReadBasic.All` | The list of teams |
   | `TeamSettings.Read.All` | An individual team's settings and archived state |
   | `Channel.ReadBasic.All` | A team's channels |
   | `Reports.Read.All` | Mailbox sizes, quotas and activity, via the usage report |
   | `MailboxSettings.Read` | Automatic replies and mailbox preferences |

   Anything you leave out simply shows as unavailable in that view; gcm still
   runs, and says which permission is missing.

   Write — only needed if you want gcm to change anything. Omit them and it
   runs read-only, which it will tell you at sign-in:

   | Permission | Needed for |
   | --- | --- |
   | `User.ReadWrite.All` | Editing, enabling and deleting users |
   | `Group.ReadWrite.All` | Creating, editing and deleting groups |
   | `GroupMember.ReadWrite.All` | Group membership and ownership |
   | `Directory.ReadWrite.All` | Licence assignment, Entra device writes |
   | `DeviceManagementManagedDevices.ReadWrite.All` | Intune device changes |
   | `DeviceManagementManagedDevices.PrivilegedOperations.All` | Retire, wipe, remote lock, Autopilot reset |
   | `UserAuthenticationMethod.ReadWrite.All` | Password reset |
   | `TeamSettings.ReadWrite.All` | Archiving and restoring a team |
   | `MailboxSettings.ReadWrite` | Setting somebody's automatic replies |

   Creating users rides on `User.ReadWrite.All`; deleting a team rides on
   `Group.ReadWrite.All`, since a team is deleted by deleting its group.

   > **Delegated access to other people's mailboxes.** `MailboxSettings.Read`
   > delegated reaches your own mailbox and any you have been granted rights
   > over — not, in general, every mailbox in the tenant. The mailbox *list*
   > comes from the usage report and is unaffected; the per-mailbox settings
   > pane is the part that may be refused, and it says so when it is. Reading
   > every mailbox needs the `MailboxSettings.Read` **application** permission,
   > which is a different app registration shape from the one described here.

4. Copy the **Application (client) ID** and **Directory (tenant) ID**.

## Configuration

Run gcm once. It writes a commented template — or, on Windows, a registry key —
and tells you where:

| Platform | Where |
| --- | --- |
| macOS | `~/Library/Application Support/gcm/config.ini` |
| Linux | `~/.config/gcm/config.ini` |
| Windows | `HKEY_CURRENT_USER\Software\gcm` |

Fill in the two IDs and restart:

```ini
[application]
client = "9f4a1c7e-1234-5678-9abc-def012345678"
tenant = "contoso.onmicrosoft.com"     ; a domain or a GUID both work
```

Optional sections:

```ini
[query]
page_size  = 999   ; objects per Graph request (max 999)
max_objects = 0    ; stop after this many per collection; 0 means no limit

[cloud]            ; sovereign clouds only
authority = "https://login.microsoftonline.us"
graph     = "https://graph.microsoft.us"

[directory]        ; on-premises Active Directory; see below
host    = "dc01.corp.contoso.com"
port    = 636
base_dn = "DC=corp,DC=contoso,DC=com"
bind_dn = "CORP\\svc-gcm"
tls     = true
```

The file is INI-shaped and parsed as TOML, so both `;` and `#` introduce a
comment. `config.ini.example` in this repo is a placeholder copy, safe to
commit.

### On Windows: the registry

Windows keeps the same settings in the registry rather than in a file, because
a registry key can be deployed by Group Policy and a file in a user profile
cannot. Each section above becomes a subkey, and each setting a value:

```
HKEY_CURRENT_USER\Software\gcm
    application\    client       REG_SZ     9f4a1c7e-1234-5678-9abc-def012345678
                    tenant       REG_SZ     contoso.onmicrosoft.com
    query\          page_size    REG_DWORD  999
                    max_objects  REG_DWORD  0
    directory\      host         REG_SZ     dc01.corp.contoso.com
                    port         REG_DWORD  636
                    base_dn      REG_SZ     DC=corp,DC=contoso,DC=com
                    bind_dn      REG_SZ     CORP\svc-gcm
                    tls          REG_DWORD  1
```

Notes:

- **`HKEY_LOCAL_MACHINE\Software\gcm` is read as a fallback**, value by value.
  Set the tenant and client ID machine-wide by policy, and a user can still
  override an individual setting without the policy knowing.
- Numbers and flags are `REG_DWORD` by convention, but `REG_SZ` is accepted for
  both — `"999"`, and `"true"`/`"false"` — so a policy template or a hand edit
  does not have to get the type right.
- **Omit `mariadb` and `directory` entirely** unless you use them. The presence
  of the subkey is what turns the feature on, so an empty one fails validation
  rather than leaving it off.
- **Upgrading from a version that used `config.ini`** needs nothing: the file is
  copied into the registry on first run, and then left where it is.
- A registry key cannot be pasted into a support ticket, so **Copy the
  configuration** (on the failure screen) renders it back as INI text. It
  contains no passwords, the same as the file.

### Where the config lives, and why

Configuration is read from your user directory or the registry, never from
beside the binary.

Neither value in it is a secret. A public client's ID travels in plaintext on
every authorization request, and the tenant ID is discoverable by anyone from
any of your verified domains. Keeping the file out of the repo is still worth
doing — it avoids advertising which tenant you administer — but it is not
protecting a credential.

The credential that *does* matter is the **refresh token**, which gcm caches as
`token.json` beside the config and writes with owner-only (`0600`) permissions.
That token is a bearer credential for your whole read surface until it expires
or is revoked. Both files live in one owner-controlled directory so there is
exactly one place to protect. `.gitignore` covers `config.ini` and `token.json`
in case a working copy ever lands in the tree.

Moving the settings into the registry on Windows changes none of that. The
token cache stays a file, and so do both logs — `error.log` and `actions.log`
are an append-only diagnostic and an audit trail, which is not what the registry
is for, and "send me the gcm folder" has to go on working.

**Sign out** in the toolbar forgets the identity completely: the cached refresh
token on disk, the tokens held in memory, and — via `prompt=select_account` on
the next authorization request — Entra's own opinion about who you are. Without
that last part the browser's existing session signs the same person straight
back in before anyone can read the redirect, which is exactly what made Sign
out appear to do nothing before v1.3.1.

## Signing in

gcm uses the authorization code flow with PKCE. On first launch it opens your
browser, you sign in there, and the browser redirects back to a loopback
listener on `127.0.0.1` — no codes to type.

This needs one redirect URI on the registration: **Authentication → Add a
platform → Mobile and desktop applications → `http://localhost`** (no port).
Entra treats loopback specially and matches any port at runtime, which lets gcm
bind an ephemeral one so two copies cannot collide.

Signing in through your own browser is also what makes Conditional Access work.
The browser runs on this machine, so Entra sees the real device and a policy
requiring a compliant or registered device can evaluate it. The device code flow
cannot do that — the browser typing the code and the application receiving the
token are different devices as far as Entra is concerned, so device-based
policies reject it with `AADSTS530035` and similar.

After the first sign-in it is silent: the cached refresh token is redeemed on
startup. If it has been revoked, the browser flow starts again.

If the tenant declines the **write** permissions, gcm retries read-only rather
than failing, and says so — a console that cannot change anything is far more
use than one that will not open.

### If sign-in fails

gcm translates the Entra error codes that a first-time setup actually hits into
the setting you need to change, and keeps the raw text and trace IDs underneath.

| Code | Meaning |
| --- | --- |
| `AADSTS7000218` | The registration is a *confidential* client, so Entra wants a secret. Turn on **Allow public client flows**. Do not add a secret — a desktop app cannot keep one. |
| `AADSTS700016` | No app with that client ID in this tenant. Check `client`, and that the registration is in the tenant named by `tenant`. |
| `AADSTS90002` | Tenant not recognised. Check `tenant` — a directory GUID or verified domain. |
| `AADSTS65001` | Permissions not consented. Grant admin consent on the registration. |
| `AADSTS50020` | You signed in with an account from a different tenant. |
| `AADSTS53003` | Conditional Access blocked it — commonly a policy demanding a compliant or hybrid-joined device, which device code flow cannot satisfy. |

## Making changes

gcm opens **read-only every time**. Action buttons are visible but disabled, so
you can see what is possible without arming anything.

`Ctrl+Shift+W` (or the **Read-only** button) arms write mode after a
confirmation. While armed, a red **WRITE ENABLED** chip sits in the toolbar and
`WRITE` appears in the status bar. It turns itself off after **15 minutes of
inactivity**, and on every restart — it is never persisted.

Actions are classified by severity, and the classification lives with the action
itself, so a new one cannot be added without declaring how dangerous it is:

| Severity | Examples | Confirmation |
| --- | --- | --- |
| Safe | Sync, restart, rename, edit a field | None |
| Caution | Disable account, licence change, membership change | Dialog |
| Destructive | Delete, retire, wipe, Autopilot reset | Dialog **plus** typing the object's name |

Typed confirmation is not ceremony. It defeats the habit of hammering Enter
through dialogs, and it forces your eye onto *which* object is about to be
destroyed — which is the mistake that actually happens.

### What is enabled so far

| Object | Available now |
| --- | --- |
| User | **Create** · enable/disable · edit job title, department, office, mobile, usage location · reset password · assign/remove licence · add to / remove from group · delete |
| Group | Create · rename and edit description · add/remove members · add/remove owners · delete |
| Entra device | Enable/disable · delete |
| Intune device | Sync · restart · remote lock · rename · Autopilot reset · retire · wipe · delete the enrolment record |
| Team | Archive · restore · delete |
| Mailbox | Turn automatic replies on or off, with separate internal and external messages |

### Creating a user

`Ctrl+N` from the Users node, the **New user…** button in the toolbar, or
right-clicking the Users node in the scope tree. The form
asks for a display name and a sign-in name, splitting the latter into an alias
and a domain chosen from the tenant's *verified* domains — Graph rejects
anything else, and picking from a list cannot be got wrong.

Three details worth calling out:

- **The password is generated, shown once, and never stored.** Same alphabet as
  a password reset, so it survives being read aloud over the phone. Copy it
  before confirming.
- **The alias is checked here, not by Graph.** Entra rejects an invalid sign-in
  name with a `Request_BadRequest` that names neither the property nor the
  offending character, which is a miserable thing to debug from a dialog.
- **Usage location is optional but load-bearing.** A licence cannot be assigned
  without one, so the form asks up front rather than letting the first licence
  assignment fail confusingly.

The account is created disabled if you untick *Can sign in immediately*, which
is the right answer for one prepared ahead of a start date.

### Managing teams

Archive makes a team read-only in the Teams client — nobody can post, nothing is
removed, and it can be restored at any time. gcm also sets the SharePoint site
read-only for members, because a team you cannot post in but whose files you can
still edit is not what anyone means by "archived".

Delete is a different matter. Graph has no endpoint that deletes a team; a team
is deleted by deleting the Microsoft 365 group behind it, which takes the group,
its mailbox, its SharePoint site and every channel conversation with it. The
confirmation says so, and demands the team's name typed out.

Team *membership* is held on that same backing group, so it is edited under
Groups rather than duplicated here.

### The sign-in and audit logs

Both views read a window rather than a collection, and the header says which:
`last 7 days, most recent 500` by default, configurable as `log_days` and
`log_records` under `[query]`.

Both bounds are deliberate. Microsoft's own guidance is that an unfiltered call
to the sign-in log times out on a busy tenant, so a time filter is always
applied. And even one day of sign-ins can run to six figures on a large tenant,
which this console would hold entirely in memory. Entra keeps at most 30 days of
sign-ins without an Entra ID P2 licence, so `log_days` is clamped there.

The sign-in log needs three separate things and will refuse until it has all of
them: Entra ID P1 or P2 on the tenant, `AuditLog.Read.All` on the app
registration, and a reporting role — Reports Reader, Security Reader or Global
Reader — on the *signed-in account*. The last of those catches people out,
because it is not a consent setting.

### Bulk operations

Tick rows with `Space` (which also moves down, so a run ticks in one motion),
`Ctrl+click`, or `Ctrl+A` for everything the filter currently shows. `Ctrl+A`
deliberately does *not* reach past the filter — "select all" should never
include rows you cannot see. A bar appears above the list with the count and the
actions that apply.

Ticks are held as object identities, not row positions, so they survive
filtering: narrow the filter, tick more, widen it again, and the earlier ticks
are still there.

A batch is confirmed **as a unit**, listing every affected object. Approving
twelve deletions one dialog at a time would train exactly the reflex the typed
confirmation exists to defeat. For a batch the phrase is the verb and the count
— `DELETE 12` — because naming one of twelve proves nothing about the other
eleven. The worst severity in a batch governs the whole batch, so one deletion
among ten harmless edits still demands typing.

Items run **sequentially**, not in parallel: parallel writes provoke Graph's
throttling and make partial failure impossible to reason about. A failure does
not abort the run. Everything that failed is listed together at the end, with a
**Copy report** button, because Graph writes are not transactional — a run that
fails at item seven leaves six applied, and knowing which six is the difference
between a recoverable situation and one reconstructed by hand.

Buttons ending in `…` open a form or a picker; the rest act immediately, subject
to confirmation. Dynamic groups do not offer membership editing — Entra
recomputes their membership from the rule, so a hand-made change would simply be
reverted.

Password reset generates a 20-character password, shown once, which the account
must change at next sign-in. The alphabet excludes `0`/`O` and `1`/`l`/`I`,
since temporary passwords get read aloud. gcm never stores it — copy it before
closing the dialog. Note that Graph refuses this against anyone holding a higher
privileged role than you, and it needs `UserAuthenticationMethod.ReadWrite.All`
plus a Helpdesk, User or Global Administrator role.

### The audit log

Every attempted write appends a JSON line to `actions.log` beside the config
(0600), recording timestamp, account, action, target and outcome. The attempt is
written *before* the call and the result after, so an action that never returns
still leaves a trace.

Entra's own audit log records the app registration as the actor, so every change
gcm makes looks identical there. This file is the only record of which console
issued what.

```sh
tail -f ~/Library/Application\ Support/gcm/actions.log | jq .
```

### Exporting to MariaDB

`Ctrl+Shift+D`, or the toolbar button that appears once a `[mariadb]` section
exists. It writes **every loaded view** into one table per collection —
`gcm_users`, `gcm_licenses`, `gcm_sign_ins` and so on — replacing whatever was
there.

```ini
[mariadb]
host = "db.contoso.internal"
port = 3306
user = "gcm_export"
database = "m365"
table_prefix = "gcm_"
require_tls = true
```

There is deliberately **no password in that file.** gcm asks for it once per
session, keeps it in memory, and forgets it on sign-out or exit. That is what
lets `config.ini` go on holding nothing secret, which matters the first time
somebody pastes it into a support ticket. The password is wrapped in a type
whose `Debug` prints `<redacted>`, so it cannot reach the error log even by
accident.

Three things differ from the CSV and JSON exports, all on purpose:

- **Every view, not the current one.** A reporting schema wants the whole tenant
  in one refresh.
- **Every row, not the filtered set.** A file export answers "give me *this
  list*", so the filter belongs in it. A table quietly narrowed by whatever
  somebody typed twenty minutes ago would be a trap for every query that later
  joins against it.
- **Collections the tenant does not offer are skipped**, not written empty. An
  empty `gcm_teams` would read as "this tenant has no teams", which is a
  different and wrong statement.

Column types are inferred from the values being written, so `assigned` arrives
as `BIGINT` and can be summed and `when` arrives as `DATETIME` and can be
ordered. Inference is conservative: one value that does not fit makes the whole
column `TEXT`, because a column that is *nearly* a number is worse than one that
is honestly a string. Values the console shows as `—` are written as `NULL`.

**How a table is replaced.** MariaDB commits implicitly on DDL, so the obvious
"wrap it all in a transaction" is not available. Each table is instead built
beside the live one and swapped in:

```sql
CREATE OR REPLACE TABLE gcm_users__staging (…);
-- batched INSERTs, inside a transaction
RENAME TABLE gcm_users TO gcm_users__old, gcm_users__staging TO gcm_users;
DROP TABLE gcm_users__old;
```

`RENAME TABLE` is atomic, so a dashboard reading `gcm_users` sees either the
whole previous export or the whole new one — never a table mid-refill and never
a missing one.

The database must already exist; gcm creates tables in it, not databases.
`table_prefix` may not be empty, which is what stops an export from dropping a
table it did not create.

### The error log

`actions.log` answers *what did this console change?* — it is an audit trail and
records only writes. `error.log`, beside it, answers the different question you
ask when something is misbehaving: *what actually happened?*

Failed sign-ins, collections that would not load, features the tenant refused,
and every failed write, in the order they occurred, one plain-text line each:

```
2026-08-05T20:14:02.881Z INFO  [startup] gcm 1.2.0 starting on macos
2026-08-05T20:14:06.204Z INFO  [auth] signed in as admin@contoso.co.uk (writes available)
2026-08-05T20:14:09.663Z WARN  [sign-ins] This tenant does not expose the sign-in log… ⏎ 403 — …
2026-08-05T20:15:41.002Z ERROR [action] Delete the group Old Project — 403 — Authorization_RequestDenied
```

Find it next to `config.ini`, or open it from **Keyboard help** (`F1`) — and
from the failure screen, which is where you will want it if gcm cannot start at
all. It is written `0600`, trimmed once it passes 512 KB, and keeps the *most
recent* entries when it trims, since those are the ones being diagnosed.

The two files are kept apart deliberately: mixing diagnostics into the audit
trail would bury the handful of lines saying who deleted what under a running
commentary of throttling and permission errors.

It contains no tokens and no passwords — nothing that logs is ever handed a
request body — and anything token-shaped that reaches a message anyway is
redacted before it is written, because a diagnostic file is exactly the thing
somebody emails to a vendor.

### The limit of the write gate

**All permissions are requested at sign-in**, so the access token is
write-capable for the whole session. The gate is enforced at a single choke
point in the worker rather than scattered across the UI, and it is re-checked at
the moment of execution in case write mode expires while a dialog is open — but
it is an application-level boundary, not one Entra imposes.

The alternative, incremental consent, would make read-only genuinely enforced by
Entra at the cost of a device-code entry each session. That trade was considered
and settled in favour of the simpler flow; switching later is possible.

## Import and export

`Ctrl+E` writes the current view to CSV, `Ctrl+Shift+E` to JSON. Both honour the
active filter and the visible columns, and read through the same accessors the
table renders from — so an exported file cannot quietly differ from what was on
screen.

`Ctrl+I` imports a CSV. Nothing runs on opening it: gcm resolves every row
against the directory and shows a preview of what it *would* do, then hands the
result to the same confirmation and worker gate as any other batch.

Rows it cannot resolve are skipped rather than aborting the file, and the
preview names every one with its reason — a typo in one row must never become a
silent no-op. There is a **Copy skipped rows** button for fixing the source.

Three file shapes are recognised, from the header row. Column names are matched
ignoring case, spaces and underscores, so `User Principal Name`,
`userPrincipalName` and `upn` are the same thing.

**User attributes** — keyed on the user, one row each:

```csv
userPrincipalName,jobTitle,department,officeLocation,mobilePhone,usageLocation
aisha@contoso.co.uk,Finance Director,Finance,London,+44 7700 900001,GB
```

Only the columns present are touched; anything omitted is left alone. An empty
cell **clears** that attribute, which is the only way a spreadsheet can say
"remove this value".

**Group membership** — `action` defaults to `add` if the column is absent, and
`role` may be `member` (default) or `owner`:

```csv
group,member,action,role
Finance Team,ben@contoso.co.uk,add,member
Project Falcon,aisha@contoso.co.uk,remove,owner
```

Dynamic groups are refused with a reason: Entra recomputes their membership from
the rule, so the change would simply be reverted.

**Licences** — the SKU may be a part number, a product name, or a SKU id:

```csv
userPrincipalName,sku,action
ben@contoso.co.uk,SPE_E3,assign
chloe@contoso.co.uk,Power BI Pro,remove
```

Rows that would assign a licence someone already holds, or remove one they do
not, are skipped before they reach Graph, where they would simply fail.

Import covers attributes, membership and licences. It deliberately cannot create
or delete objects, or enable and disable accounts — those stay deliberate,
one-at-a-time or bulk-selected actions in the console.

## On-premises Active Directory

Optional, and off unless configured. Add a `[directory]` section and the console
grows a **Directory** node reading a domain controller over LDAP, alongside
everything it already reads from the tenant.

```ini
[directory]
host      = "dc01.corp.contoso.com"
port      = 636
base_dn   = "DC=corp,DC=contoso,DC=com"
bind_dn   = "CORP\\svc-gcm"           ; a DN, DOMAIN\\user or a UPN all work
tls       = true                     ; LDAPS
start_tls = false                    ; or TLS on port 389, where that is all there is
```

A **read-only** account is sufficient. Nothing in this release writes to Active
Directory — there is no AD action in the menus and none in the code — so the
write gate has nothing to govern here.

There is no password in the file, for the same reason there is none for the
MariaDB export: gcm asks for the bind password the first time you open a
Directory view and keeps it in memory only, so the configuration stays safe to
paste into a support ticket. Signing out forgets it along with everything else.

### What it joins up

The more useful half is not the new node — it is what appears in the **Users**
pane. Graph will tell you an account is synced from AD and then stop. The OU it
lives in, the `userAccountControl` flags on it, when its password was *really*
last set, and which of its groups never made it to the cloud are all on the
other side of the sync, and are usually the reason you were looking.

Selecting a synced user now shows an **On-premises (Active Directory)** section
carrying exactly that, plus two things worth calling out:

- **A disabled AD account whose tenant shadow is still enabled** is flagged. For
  however long the sync takes to notice, a leaver still has a working cloud
  account.
- **Password last set** is AD's `pwdLastSet`, not Entra's
  `lastPasswordChangeDateTime`. For a synced account the latter records when
  password hash sync wrote the cloud credential, which is a different event —
  and where sync is off, it is not an answer at all.

The join is on `sAMAccountName`, matched case-insensitively against the tenant's
`onPremisesSamAccountName`. It is the one identifier both directories agree on,
and the only one that survives a UPN change or a mail-domain move.

### How gcm binds, and why it matters

| `auth` | What happens | Where |
| --- | --- | --- |
| `integrated` | Binds as the **signed-in Windows account** over Kerberos (SSPI) | Windows only, and the default there |
| `simple` | Binds as `bind_dn` with a password typed once per session | Everywhere |

This is the setting that decides *whose* permissions apply. Under
`integrated`, the domain controller evaluates every read and every change
against the operator's own account — so somebody delegated password resets over
one OU can reset passwords in that OU and nothing else, and gcm does not have to
know anything about that arrangement. There is no password to store or type, and
`bind_dn` is not needed.

Under `simple`, everything carries the service account's rights instead, and the
console cannot tell one operator from another.

Two things to get right for `integrated`:

- **`host` must be the DC's real FQDN.** Kerberos builds its service principal
  from that name, so an IP address or a CNAME is refused even though it resolves.
- **The machine must be domain-joined** and the operator signed in with a domain
  account.

### Changing things

Writes need **write mode** armed (`Ctrl+Shift+W`), exactly as tenant changes do,
and they go through the same confirmation and the same `actions.log` — entries
are tagged `"system": "active-directory"` so on-premises changes can be found on
their own.

What is offered, on a selected account or computer:

- Enable or disable
- Unlock (an account locked out by failed sign-ins)
- Reset password, optionally requiring a change at next sign-in
- Edit attributes — display name, description, title, department, company,
  office, telephone, mobile, email, employee ID
- Delete

Two gates apply rather than one. gcm's write mode is the first; **Active
Directory's own access check is the second**, and under `integrated` that one is
evaluated against the operator. A refusal comes back as "does not have
permission on the object" and is AD's decision, not gcm's — the fix is a
delegation, not anything in the console. Entries are offered even where the
operator may turn out not to be delegated, because the console cannot know
without asking the DC, and guessing in the direction of "you cannot do this"
would be worse.

A password reset is refused outright when `tls` is off. Active Directory refuses
it too, but only after the password has crossed the network in the clear.

### Things it will not do

- **No SRV discovery.** `host` names a domain controller explicitly. Finding one
  via `_ldap._tcp.dc._msdcs` would mean shipping a DNS resolver, and naming the
  DC is also the only way to pin reads to a particular replica.
- **No group membership editing, and no moving between OUs.** Both need
  somewhere to pick the target from, and gcm reads only users and computers from
  a DC — there is no AD group collection and no OU tree to choose from.
- **No object creation.** Creating an AD user means choosing an OU, an RDN, a
  `sAMAccountName` and a UPN suffix, and getting any of them wrong leaves an
  object to clean up by hand. gcm creates users in the tenant, where those
  decisions are far smaller.
- **`lastLogonTimestamp`, not `lastLogon`.** It replicates on a delay of up to
  14 days by design. Both the table and the details pane say so, because that is
  precisely the window people use it to judge.
- **One domain.** Searches run from a single `base_dn` on a single DC. An
  account synced from a different domain in the forest simply will not match,
  and the Users pane says that rather than implying the account is missing.

Paging uses AD's simple paged-results control with the same `page_size` and
`max_objects` from `[query]` that Graph uses, clamped to 1000 rather than 999 —
Active Directory's own `MaxPageSize` default, which a DC silently enforces
rather than reporting.


## Keyboard

Press **F1** in the app for the same table. On macOS, `Ctrl` below is `Cmd`.

**Panes**

| Key | Action |
| --- | --- |
| `F6` / `Ctrl+Tab` | Next pane: scope → results → details |
| `Shift+F6` | Previous pane |
| `Esc` | Clear the filter, then return to the scope tree |

**Scope tree**

| Key | Action |
| --- | --- |
| `↑` `↓` | Move between nodes (selection follows) |
| `→` | Expand, or step into the first child |
| `←` | Collapse, or step out to the parent |
| `Enter` | Move focus to the results |
| `Home` / `End` | First or last node |

**Results**

| Key | Action |
| --- | --- |
| `↑` `↓` | Move the selection |
| `PgUp` / `PgDn` | Move by a screenful |
| `Home` / `End` | First or last row |
| `Enter` | Move focus to the details pane |
| `←` | Back to the scope tree |
| `Ctrl+C` | Copy the selected row |

**Jumping**

`Ctrl+0` Console Root · `Ctrl+1` Users · `Ctrl+2` Groups · `Ctrl+3` Directory
Roles · `Ctrl+4` Devices · `Ctrl+5` Managed Devices · `Ctrl+6` Licenses ·
`Ctrl+7` Exchange · `Ctrl+8` Teams · `Ctrl+9` Sign-in Logs

The numbers follow the scope tree top to bottom, so they are a position rather
than something to memorise separately. Jumping to a nested node expands its
parent first, so nothing is unreachable.

**Acting on the selection**

| Key | Action |
| --- | --- |
| `Shift+F10` | Open the actions menu for the selection — the keyboard equivalent of right-clicking |
| `Ctrl+Enter` | The same, for keyboards where F10 is claimed by the window manager |
| `Ctrl+Shift+W` | Turn write mode on or off |
| `Ctrl+N` | Create — a user on the Users node, a group on Groups |
| `Ctrl+Shift+D` | Export every loaded view to MariaDB |

The actions menu, the right-click menu, the toolbar's second row and the details
pane buttons all render from one list, so they cannot offer different things.
The menu opens even while read-only, with the entries that change the tenant
disabled — seeing what is possible should not require arming write mode first.

Every view's menu also carries the things that need no write mode at all: copy
the row, tick rows for a bulk operation, export the view, refresh it. That
matters most on the views Graph will not let anyone change — Directory Roles,
Licenses and both logs — which before v1.3.1 opened a menu with nothing in it,
and so looked broken rather than read-only.

Buttons follow the node. **New user…** appears on Users, **New group…** on
Groups, and on the nodes where nothing can be created the slot carries
**Export…** instead. A second row appears beneath the toolbar when a row is
selected, carrying what can be done to it — or to the whole ticked set.

**Inside a dialog**

| Key | Action |
| --- | --- |
| `↑` `↓` | Move through a list of choices |
| Typing | Filters the choices |
| `Enter` | Choose, save, or confirm — once anything you must type matches |
| `Tab` | Move between fields and buttons |
| `Esc` | Cancel without changing anything |

Pickers open with focus in the filter box and the arrows already driving the
list, so choosing one licence out of ninety is a few letters then `Enter`, not a
long walk with `Tab`.

**Everything else**

| Key | Action |
| --- | --- |
| `Ctrl+F` | Focus the filter box |
| `F5` | Refresh the current view |
| `Ctrl+Shift+R` | Refresh every view |
| `Ctrl+D` | Show or hide the details pane |
| `F1` | Show or hide the shortcut overlay |
| `Tab` | Move between toolbar controls |

Notes on the model: the selected row stays visible in every pane, dimmed when
that pane does not have focus, so `F6` is never disorienting. The focused pane
carries a blue ring. While the filter box has focus, only `Esc` and the pane
keys are intercepted — everything else is typing. Rows are announced to
assistive technology as whole records rather than as loose cells.

## Updating

gcm checks GitHub for a newer release at startup and offers one if it finds it,
showing that version's changelog entry in the dialog. The check is best-effort:
if GitHub cannot be reached, nothing is said about it.

**Update now** downloads the release for your platform and replaces the install
in place, on both Windows and macOS. gcm cannot overwrite itself while running,
so in both cases the work is handed to a small detached helper that waits for
gcm to exit, swaps the build, and relaunches it.

- **Windows** — the new files are copied over the install directory.
- **macOS** — the new `.app` is staged beside the old one and moved into place
  with two renames, so a failure part-way leaves you with a working application
  either way. Because gcm downloads the update itself rather than a browser
  doing it, the replacement is not quarantined and opens without the
  right-click → **Open** that the first install needed.

Nothing in your configuration is touched: it lives in your user directory (or
the registry on Windows), not beside the binary.

Where gcm is not running from an installed application — a `cargo run` build,
or a loose binary — there is no install to replace, so the dialog offers the
release page instead.

## Running

```sh
cargo run --release
```

> On this machine the built binary exits immediately when invoked directly as
> `./target/release/gcm`; launch it through `cargo run --release`.

### Demo mode

To see the console without a tenant — useful for working on the interface or
checking the keyboard model:

```sh
GCM_DEMO=1 cargo run                            # synthetic tenant
GCM_DEMO=1 GCM_DEMO_NO_INTUNE=1 cargo run       # ...without Intune
GCM_DEMO=1 GCM_DEMO_NO_EXCHANGE=1 cargo run     # ...without Exchange
GCM_DEMO=1 GCM_DEMO_NO_TEAMS=1 cargo run        # ...without Teams
GCM_DEMO=1 GCM_DEMO_NO_AUDIT=1 cargo run        # ...without Entra ID P1
GCM_DEMO=1 GCM_DEMO_NO_DIRECTORY=1 cargo run    # ...without an on-premises domain
```

The `NO_*` switches exist so the unavailable-feature paths can be exercised
offline. Otherwise they are only reachable by finding a tenant that genuinely
lacks the thing, which is not a practical way to check that a message reads
well.

Demo mode is `#[cfg(debug_assertions)]` throughout. It does not exist in a
release build and cannot be switched on in one.

### Tests

```sh
cargo test
```

Covers config parsing (including both comment styles and sovereign-cloud
overrides), ID token decoding, group and device classification, licence seat
arithmetic, Graph paging and the unavailable-feature detection, and the scope
tree's flattening and reachability.

## How it is put together

| Module | Responsibility |
| --- | --- |
| `config` | Loads the INI from the user directory; writes the template on first run. |
| `auth` | Authorization code flow with PKCE, refresh-token cache, silent renewal. |
| `graph` | Paged Graph client, resource models, SKU name table, `reports` (the CSV usage reports), `actions` (every mutation, as data). |
| `ldap` | Read-only Active Directory client: LDAPS bind, paged subtree searches, and `models` — the conversions that make `userAccountControl`, Windows FILETIMEs and distinguished names legible. |
| `worker` | Owns the Tokio runtime; the UI talks to it over command/event channels and never blocks or awaits. Also the single choke point every write passes through. |
| `actionlog` | The audit trail: one JSON line per attempted write. |
| `errorlog` | The diagnostic log: everything that failed or was refused. |
| `ui` | The console: `nav` (scope tree), `list` (virtualized results), `details` (property sheet), `forms` and `confirm` (the input and approval halves of an action), `menu` (what can be done to an object, in one place), `keys`, `help`, `quips`, `theme`, and the two credential dialogs `database` and `directory`. |

Nothing upstream of `worker` knows whether a collection came from Graph or from
a domain controller: both arrive as the same events over the same channel, and
both use `Fetch::Unavailable` to explain themselves when a tenant has no Intune
or an estate has no domain. The one place the difference shows is that an AD
read carries a bind password with the request, so the worker never holds one.

The result list is virtualized and builds only the rows on screen, so a tenant
with fifty thousand users costs the same per frame as one with fifty. Group and
role membership, team settings and mailbox settings are all fetched on demand
rather than up front — expanding every group eagerly would be thousands of
requests for data nobody asked for.

Two files exist mainly to stop things drifting apart. `menu` is the single
source for what can be done to an object, so the right-click menu, the details
pane button bar and the keyboard palette cannot offer different things.
`quips` holds every "this is not available here" message, because tone drifts
when it lives in a dozen places, and a file of jokes is easy to read end to end
and ask whether any of them have aged badly.

### The typography

The interface is set in Ubuntu Bold, bundled in `assets/fonts` under the Ubuntu
Font Licence. egui ships only Ubuntu-Light, and `RichText::strong()` recolours
rather than selecting a heavier face, so the bold weight has to arrive as an
actual font file. It goes first in the proportional family's chain, with
everything egui already had left behind it — so a glyph Ubuntu Bold does not
cover still renders instead of becoming a tofu box.

## Limitations

- Long values in narrow columns are clipped; the details pane always shows the
  full value.
- Exchange administration is limited to what Graph exposes: mailbox size, quota,
  activity and automatic replies. Permissions, forwarding, litigation hold and
  transport rules are Exchange Online PowerShell only.
- Mailbox size, quota and activity come from a report compiled daily, so those
  figures can be up to a day stale. The *list* is live — a mailbox created this
  morning appears at once, marked as not yet reported — because Graph exposes
  no mailbox size or quota at any freshness.
- Automatic replies can be switched on and off but not *scheduled*. A schedule
  needs a start and end instant in the mailbox's own time zone, and a console
  that got that subtly wrong would stop answering somebody's mail on the wrong
  day.
- Team membership is edited under Groups, on the Microsoft 365 group behind the
  team, rather than duplicated in the Teams view.
- The log views show a bounded window — seven days and 500 entries by default —
  not the whole log. The header says so.
- The MariaDB export replaces its tables outright; there is no append or upsert
  mode, so it captures current state rather than history.
- Group and role membership lists display the first 200 entries and then say how
  many more there are.
- The SKU name table covers the products found in most commercial tenants;
  anything else falls back to its raw part number, and the details pane says so.
- Licence assignment is read from each user's `assignedLicenses`, so it reflects
  direct and group-inherited licences together without distinguishing them.
- The Active Directory views are read-only, bind with a simple bind rather than
  Kerberos, and read one domain from one named DC. See
  [On-premises Active Directory](#on-premises-active-directory).

## Licence

MIT — see [LICENSE](LICENSE).

The bundled Ubuntu Bold typeface is used under the Ubuntu Font Licence 1.0, a
copy of which sits beside it in
[`assets/fonts`](assets/fonts/UBUNTU-FONT-LICENCE-1.0.txt).
