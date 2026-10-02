# Graphical Cloud Manager

A desktop app for managing Microsoft Entra ID: users, groups, and devices,
including Intune's remote actions. It can import and export users as CSV, and
copy the whole directory to a MariaDB server. Written in Rust, with
[egui](https://github.com/emilk/egui) for the interface. It runs on macOS,
Windows and Linux.

The program is called `gcm` on disk and in your terminal. The window title
says **Graphical Cloud Manager**.

## Signing in

The app signs in as an **app registration** using the OAuth 2.0
client-credentials grant. You give it a tenant ID, a client ID and a client
secret, and it gets an app-only token straight from Microsoft. There is no
browser, no device code and no signed-in user. It can do exactly what the
registration's *application* permissions allow, and nothing more.

### Creating the app registration

1. In the [Entra admin centre](https://entra.microsoft.com), go to
   **Identity → Applications → App registrations → New registration**. Give
   it a name and leave the redirect URI empty.
2. On its **Overview** page, copy the **Application (client) ID** and the
   **Directory (tenant) ID**.
3. Under **Certificates & secrets → Client secrets**, add a secret and copy
   its **Value**. You copy the Value, not the Secret ID, and it is shown only
   once.
4. Under **API permissions → Add a permission → Microsoft Graph →
   Application permissions**, add the permissions below. Then press **Grant
   admin consent**.

| Permission | Used for |
| --- | --- |
| `User.ReadWrite.All` | Listing, creating, editing, enabling, disabling and deleting users |
| `User-PasswordProfile.ReadWrite.All` | Resetting passwords |
| `Group.ReadWrite.All` | Listing, creating and deleting groups |
| `GroupMember.ReadWrite.All` | Adding and removing group members |
| `Device.ReadWrite.All` | Listing, enabling, disabling and deleting Entra devices |
| `DeviceManagementManagedDevices.ReadWrite.All` | Listing Intune managed devices |
| `DeviceManagementManagedDevices.PrivilegedOperations.All` | Intune actions: sync, restart, lock, Defender scan, retire, wipe |
| `Organization.Read.All` | Showing the tenant's name (optional) |

For a read-only setup, grant the `.Read.All` versions of these instead. The
lists will load, and any change you try will be refused with a message saying
so. After you sign in, the **Connection** tab shows which of these
permissions the token actually carries.

Two limits come from Entra itself, not from this app. An app with
`User.ReadWrite.All` cannot reset the password of, or delete, a user who
holds an admin role, unless the app has been given a suitable directory role
too. And users synchronised from on-premises Active Directory have to be
changed there.

### Where things are kept

The tenant ID, the client ID and the MariaDB server details are saved in
`config.json` in the app's data directory:

- macOS: `~/Library/Application Support/GraphicalCloudManager`
- Linux: `~/.local/share/GraphicalCloudManager`
- Windows: `%APPDATA%\GraphicalCloudManager`

Secrets never go in that file. If you tick **Remember the secret**, the
client secret is saved in `.gcm-credentials.json` in your home directory, and
the app signs in by itself at the next start. The MariaDB password can be
remembered in the same file. On macOS and Linux the file is set to mode
`0400`, read-only and readable only by you. On Windows it gets the read-only
attribute, and who can read it is decided by your user profile's own
permissions, which normally admit only you and administrators. The app makes
it writable just long enough to update it, and deletes it once nothing is
left in it.

It is plain JSON, so you can also write it yourself, for example to set up a
machine without typing anything into the window:

```json
{
  "tenant_id": "00000000-0000-0000-0000-000000000000",
  "client_id": "00000000-0000-0000-0000-000000000000",
  "client_secret": "the secret's Value",
  "mariadb_password": "optional"
}
```

```sh
chmod 400 ~/.gcm-credentials.json
```

If the app has no settings saved yet, it takes the tenant and client IDs from
this file and signs in straight away. The client secret is only ever sent for
the tenant and client named next to it. Anyone who can read the file can sign
in as the app registration, so treat it like a password. When a secret is
rotated in Entra, sign in once with the new value and the file is updated.

## The window

Six tabs run down the left-hand side, the same layout as
[watchspend](https://github.com/mediaswing/watchspend). The status bar along
the bottom shows which tenant you are signed in to, what the app is doing,
and the result of the last action. Every Graph and MariaDB call runs in the
background, so the window never freezes.

**Connection** holds the sign-in form. After you sign in, it lists the
permissions the token carries.

**Users** lists every user in the tenant. You can search by name, sign-in
name, mail, department or job title. Selecting a user opens a details panel
with their properties and group memberships. From there you can **Edit**
the profile, **Enable** or **Disable** the account, **Reset password** (a
strong password is generated for you), or **Delete** the user. A deleted user
can be restored from the admin centre for 30 days. **New user** opens a
creation form.

The toolbar also has the bulk tools:

- **Import CSV…** reads a file of users and shows each row before anything
  is created. Rows with problems are marked and skipped. When the import
  finishes, a summary lists any failures, and **Save results…** writes a CSV
  with each row's outcome and new object ID.
- **Export CSV…** writes the users currently shown, so a search narrows the
  export too.
- **Save CSV template…** writes an import file with the expected header and
  one example row.

**Groups** lists every group with its type (Microsoft 365, Security,
Mail-enabled security, Distribution) and whether membership is assigned or
dynamic. Selecting a group shows its members. You can add a user by sign-in
name or remove a member. Dynamic groups are read-only, because their members
come from a rule. **New group** creates a Security or Microsoft 365 group.

**Devices** joins Entra devices with their Intune records on the Entra device
ID, so each physical device appears once. You can filter to all devices, those
managed by Intune, those not in Intune, or those not compliant. Selecting a
device shows both halves. A managed device offers the Intune actions:

| Action | What it does |
| --- | --- |
| Sync | Asks the device to check in now |
| Restart | Restarts the device |
| Remote lock | Locks the screen (iOS, iPadOS, Android, macOS) |
| Defender quick / full scan | Runs a Microsoft Defender scan (Windows) |
| Retire | Removes company data and stops managing the device |
| Wipe | Factory-resets the device |
| Delete from Intune | Removes the Intune record only |

Restart, lock, retire, wipe and delete each ask for confirmation first. The
Entra half can be enabled, disabled, or deleted from Entra ID.

If the tenant has no Intune licence, or the app lacks the Intune permission,
the Entra devices are still shown, with a note saying why the Intune devices
are missing.

**Export** copies the directory into a MariaDB or MySQL server; see below.

**Settings** chooses light, dark, or following the system, and turns the
debug log on or off.

## The debug log

When something goes wrong, tick **Write a debug log** under **Settings** and
try again. The app then writes `gcm-debug.log` in its data directory (see
[Where things are kept](#where-things-are-kept)); **Show the log file**
opens it in the file manager. To log a single run from the very start, set
`GCM_DEBUG=1` instead:

```sh
GCM_DEBUG=1 gcm
```

Each request to Microsoft Graph is logged with its method, path, status, how
long it took and Microsoft's request ID, which Microsoft support will ask for.
So are each step of a MariaDB export, every background job, and any crash.
The client secret, access tokens, passwords and request bodies are never
written to it. Paths and messages can include object IDs, sign-in names and
group names, so look through the file before sharing it.

On macOS and Linux the file is readable only by you; on Windows it takes the
permissions of your user profile. Once it passes 5 MB, it is moved to
`gcm-debug.log.1` the next time logging starts, replacing any older one.

Warnings still go to the terminal as before, and `RUST_LOG` controls that as
usual.

## CSV import format

The first row is a header. Columns are matched by name, ignoring case, spaces,
underscores and hyphens, so `userPrincipalName`, `User Principal Name` and
`user_principal_name` all work. Columns the app does not know are ignored.
That means a file from **Export CSV…** can be edited and imported again.

| Column | Required | Notes |
| --- | --- | --- |
| `displayName` | yes | |
| `userPrincipalName` | yes | `name@yourdomain`, on a domain verified in the tenant |
| `password` | no | Left blank, a 16-character password is generated and written to the results file |
| `mailNickname` | no | Defaults to the part of the sign-in name before the `@` |
| `givenName`, `surname`, `jobTitle`, `department`, `officeLocation`, `mobilePhone` | no | |
| `usageLocation` | no | Two-letter country code such as `GB`. Needed before a licence can be assigned |
| `accountEnabled` | no | `true`/`false`, `yes`/`no` or `1`/`0`. Defaults to `true` |
| `forceChangePasswordNextSignIn` | no | Same values. Defaults to `true` |

The results file can contain passwords. On macOS and Linux it is written
readable only by you. On Windows it takes the permissions of the folder you
save it in, so save it inside your own user folder rather than a shared one.
Either way, keep it somewhere safe and delete it once the passwords have been
handed over.

## Exporting to MariaDB

Enter the server's details and press **Test connection**. Then choose what to
export and press **Export to MariaDB**. The export reads fresh from Graph
rather than from what the tabs have loaded, so the database matches the tenant
as it is at that moment.

The four tables are created if they do not exist. Each one is written in a
single transaction, so a failure part-way leaves that table as it was.

| Table | Contents | Key |
| --- | --- | --- |
| `gcm_users` | One row per user | `tenant_id, id` |
| `gcm_groups` | One row per group | `tenant_id, id` |
| `gcm_group_members` | One row per direct membership | `tenant_id, group_id, member_id` |
| `gcm_devices` | One row per device, Entra and Intune joined | `tenant_id, row_key` |

Every row carries the tenant ID, so several tenants can share one database.
Running the export again updates rows in place. Ticking **Mirror** also
removes rows for objects that are no longer in the tenant. Times are stored in
UTC, and `exported_at` records when each row was last written. The full
`CREATE TABLE` statements are in the **Table definitions** section at the
bottom of the Export tab.

The account needs `CREATE`, `SELECT`, `INSERT`, `UPDATE` and `DELETE` on the
database:

```sql
CREATE DATABASE gcm CHARACTER SET utf8mb4;
CREATE USER 'gcm'@'%' IDENTIFIED BY 'a-strong-password';
GRANT CREATE, SELECT, INSERT, UPDATE, DELETE ON gcm.* TO 'gcm'@'%';
```

TLS is optional. If your server uses a certificate from a private authority,
you can point the app at that authority's `.pem` file.

## Building

You need a recent stable Rust toolchain (edition 2024).

```sh
cargo run --release
```

On Linux, install the window-system headers first:

```sh
sudo apt install build-essential pkg-config perl make \
    libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

### Packages

The packages are built by `gcm-package`, a second binary in this crate. Its
code for each platform is in `src/package/`: `macos.rs`, `ubuntu.rs` and
`windows.rs`. It builds the package for the platform it runs on, and writes
it to `dist/`. GitHub Actions runs the same command (see
`.github/workflows/build.yml`), and publishes the packages when a `v*` tag is
pushed. The release notes are that version's section of `CHANGELOG.md`, so
before tagging `v1.1.0`, rename `## [Unreleased]` to `## [1.1.0]`. A tag
with no section of its own fails the release rather than publishing it
without notes.

#### macOS: an ad-hoc signed `.app`

```sh
cargo run --release --bin gcm-package                # this Mac's architecture
cargo run --release --bin gcm-package -- --universal # Apple silicon and Intel together
```

This writes `dist/Graphical Cloud Manager.app` and a zip of it. The bundle is
signed ad hoc (`codesign --sign -`), which is enough for it to run on Apple
silicon. It is not notarised, so the first time a downloaded copy is opened,
macOS will refuse. To allow it, go to **System Settings → Privacy &
Security**, scroll down, and press **Open Anyway**. To give the app an icon,
put an `AppIcon.icns` in `packaging/macos/` before building.

The macOS package published with each release is built for Apple silicon
only, as `gcm-<version>-macos-aarch64.zip`. It will not run on an Intel Mac;
build one there with the first command above, or anywhere with `--universal`.

#### Ubuntu: a `.deb`

```sh
sudo apt install dpkg-dev
cargo run --release --bin gcm-package
sudo apt install ./dist/gcm_*.deb
```

This installs `/usr/bin/gcm` and a **Graphical Cloud Manager** entry in the
applications menu. File dialogs use the desktop portal
(`xdg-desktop-portal`), which is present on a standard Ubuntu desktop. To give
the menu entry an icon, put a 256×256 `gcm.png` in `packaging/ubuntu/`.

#### Windows: a `.zip`

```sh
cargo run --release --bin gcm-package
```

This writes `dist\gcm-<version>-windows-x86_64.zip`, holding `gcm.exe` and
the licences.

### Tests

```sh
cargo test
```

## Licence

MIT. See [`LICENSE`](LICENSE). The bundled Ubuntu Bold font is under the
[Ubuntu Font Licence](assets/fonts/UBUNTU-FONT-LICENCE-1.0.txt).
