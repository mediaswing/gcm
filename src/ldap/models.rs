//! Active Directory objects, and the conversions that make them legible.
//!
//! LDAP hands back a `HashMap<String, Vec<String>>`: every attribute is a list,
//! every value is text, and nothing is typed. Three of those untyped values
//! carry most of what an administrator actually wants to know, and none of them
//! can be read literally:
//!
//! * `userAccountControl` is a bitmask. "Disabled" is bit 2 of an integer that
//!   also encodes password policy, delegation and account category.
//! * `pwdLastSet`, `lastLogonTimestamp` and `accountExpires` are Windows
//!   FILETIMEs — 100-nanosecond ticks since 1601 — with two sentinel values
//!   that mean "never", one of which is the largest one a signed 64-bit integer
//!   holds.
//! * `whenCreated` is an LDAP generalized time, which is a different format
//!   again, in the same object.
//!
//! Everything below exists to turn those into the types the rest of gcm already
//! displays, so the AD views and the Entra views can share a details pane.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};

// ---- userAccountControl ----------------------------------------------------

/// The `userAccountControl` bits gcm reports on.
///
/// Not the full set — the flag list in the details pane is meant to be read,
/// not audited, so it names the ones that change how an account behaves and
/// leaves out the ones that only restate its category.
pub mod uac {
    pub const ACCOUNT_DISABLED: u32 = 0x0000_0002;
    pub const HOMEDIR_REQUIRED: u32 = 0x0000_0008;
    // 0x10 is LOCKOUT. Deliberately absent: AD computes it per-DC and does not
    // replicate it reliably, so gcm reads `lockoutTime` instead — see
    // `AdUser::is_locked_out`.
    pub const PASSWD_NOTREQD: u32 = 0x0000_0020;
    pub const PASSWD_CANT_CHANGE: u32 = 0x0000_0040;
    // The account-category bits. `describe_uac` deliberately says nothing about
    // these — "this is a normal user account" is not a finding — but they are
    // kept because a reader checking a raw `userAccountControl` value against
    // this list should not conclude the low bits are unaccounted for. Only
    // SERVER_TRUST is read in anger, to tell a DC from a member server.
    #[allow(dead_code)]
    pub const NORMAL_ACCOUNT: u32 = 0x0000_0200;
    #[allow(dead_code)]
    pub const WORKSTATION_TRUST: u32 = 0x0000_1000;
    pub const SERVER_TRUST: u32 = 0x0000_2000;
    pub const DONT_EXPIRE_PASSWORD: u32 = 0x0001_0000;
    pub const SMARTCARD_REQUIRED: u32 = 0x0004_0000;
    pub const TRUSTED_FOR_DELEGATION: u32 = 0x0008_0000;
    pub const NOT_DELEGATED: u32 = 0x0010_0000;
    pub const USE_DES_KEY_ONLY: u32 = 0x0020_0000;
    pub const DONT_REQ_PREAUTH: u32 = 0x0040_0000;
    pub const PASSWORD_EXPIRED: u32 = 0x0080_0000;
    pub const TRUSTED_TO_AUTH_FOR_DELEGATION: u32 = 0x0100_0000;
}

/// Flags worth naming, in the order they are shown.
///
/// Ordered by how much they should worry the reader rather than by bit value:
/// the four that weaken authentication come first, because the reason somebody
/// opens this pane is usually to find out whether one of them is set.
const NOTABLE_FLAGS: &[(u32, &str)] = &[
    (uac::PASSWD_NOTREQD, "Password not required"),
    (uac::DONT_REQ_PREAUTH, "Kerberos pre-authentication not required"),
    (uac::USE_DES_KEY_ONLY, "DES keys only"),
    (uac::TRUSTED_FOR_DELEGATION, "Trusted for delegation"),
    (
        uac::TRUSTED_TO_AUTH_FOR_DELEGATION,
        "Trusted to authenticate for delegation",
    ),
    (uac::DONT_EXPIRE_PASSWORD, "Password never expires"),
    (uac::PASSWORD_EXPIRED, "Password expired"),
    (uac::SMARTCARD_REQUIRED, "Smart card required"),
    (uac::PASSWD_CANT_CHANGE, "Cannot change password"),
    (uac::NOT_DELEGATED, "Account is sensitive and cannot be delegated"),
    (uac::HOMEDIR_REQUIRED, "Home directory required"),
];

/// The notable flags set in a `userAccountControl` value.
pub fn describe_uac(value: u32) -> Vec<&'static str> {
    NOTABLE_FLAGS
        .iter()
        .filter(|(bit, _)| value & bit != 0)
        .map(|(_, label)| *label)
        .collect()
}

// ---- Time ------------------------------------------------------------------

/// Seconds between the FILETIME epoch (1601-01-01) and the Unix epoch.
const FILETIME_EPOCH_OFFSET: i64 = 11_644_473_600;

/// 100-nanosecond ticks per second.
const FILETIME_TICKS_PER_SEC: i64 = 10_000_000;

/// Convert a Windows FILETIME attribute to a timestamp.
///
/// Returns `None` for both "never" sentinels. AD writes `0` for an account that
/// has never logged on and `9223372036854775807` — `i64::MAX` — for one that
/// never expires, and a caller that took either literally would render dates in
/// 1601 and 30828 respectively. Both mean "no value", so both become `None`.
pub fn from_filetime(raw: &str) -> Option<DateTime<Utc>> {
    let ticks: i64 = raw.trim().parse().ok()?;
    if ticks <= 0 || ticks == i64::MAX {
        return None;
    }
    let seconds = ticks / FILETIME_TICKS_PER_SEC - FILETIME_EPOCH_OFFSET;
    let nanos = (ticks % FILETIME_TICKS_PER_SEC) as u32 * 100;
    Utc.timestamp_opt(seconds, nanos).single()
}

/// Convert an LDAP generalized time — `20240115093000.0Z` — to a timestamp.
///
/// `whenCreated` and `whenChanged` use this rather than FILETIME, in the same
/// object as attributes that do. The fractional part is always `.0` from a DC
/// but is optional in the standard, so it is parsed both ways.
pub fn from_generalized_time(raw: &str) -> Option<DateTime<Utc>> {
    let trimmed = raw.trim();
    let base = trimmed
        .split_once('.')
        .map(|(head, _)| head)
        .unwrap_or_else(|| trimmed.trim_end_matches('Z'));
    chrono::NaiveDateTime::parse_from_str(base, "%Y%m%d%H%M%S")
        .ok()
        .map(|naive| naive.and_utc())
}

// ---- Distinguished names ---------------------------------------------------

/// Split a DN into its relative components, honouring `\,` escapes.
///
/// A comma inside a value is escaped rather than quoted in AD — a user called
/// `Smith\, John` is not unusual — so a plain `split(',')` would produce two
/// broken components and misreport the OU of exactly the accounts whose names
/// are most awkward.
pub fn split_dn(dn: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in dn.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            current.push(ch);
            escaped = true;
        } else if ch == ',' {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// The value half of an RDN: `OU=Finance` becomes `Finance`.
fn rdn_value(component: &str) -> String {
    component
        .split_once('=')
        .map(|(_, value)| value.trim())
        .unwrap_or(component)
        .replace("\\,", ",")
}

/// The canonical name of a DN, as `Active Directory Users and Computers` shows
/// it: `corp.contoso.com/Users/Finance/Aisha Rahman`.
///
/// This is the form worth showing. A DN read left to right is the object's path
/// backwards, which is why nobody can find an OU from one at a glance.
pub fn canonical_name(dn: &str) -> String {
    let components = split_dn(dn);
    let (domain, path): (Vec<&String>, Vec<&String>) = components
        .iter()
        .partition(|component| component.to_ascii_uppercase().starts_with("DC="));

    let domain = domain
        .iter()
        .map(|component| rdn_value(component))
        .collect::<Vec<_>>()
        .join(".");

    // The path runs leaf-first in a DN and root-first in a canonical name.
    let mut path: Vec<String> = path.iter().map(|component| rdn_value(component)).collect();
    path.reverse();

    if domain.is_empty() {
        path.join("/")
    } else if path.is_empty() {
        domain
    } else {
        format!("{domain}/{}", path.join("/"))
    }
}

/// The canonical name of whatever contains this object — its OU path, without
/// the object itself.
pub fn container_of(dn: &str) -> String {
    let components = split_dn(dn);
    if components.len() <= 1 {
        return canonical_name(dn);
    }
    canonical_name(&components[1..].join(","))
}

// ---- Binary attributes -----------------------------------------------------

/// Format an `objectGUID` the way Windows tooling prints it.
///
/// The first three fields are little-endian and the last two are big-endian, in
/// the same 16 bytes. Reading it straight through produces a GUID that matches
/// nothing an operator can search for, so the byte order is undone here.
pub fn format_guid(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 16 {
        return None;
    }
    Some(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{}",
        bytes[3],
        bytes[2],
        bytes[1],
        bytes[0],
        bytes[5],
        bytes[4],
        bytes[7],
        bytes[6],
        bytes[8],
        bytes[9],
        bytes[10..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}

/// Format an `objectSid` as `S-1-5-21-…`.
///
/// The sub-authority count is a byte, the identifier authority is six bytes
/// big-endian, and every sub-authority is four bytes little-endian. Worth
/// carrying because the SID is what appears in a file server's ACL when the
/// account behind it has been deleted.
pub fn format_sid(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 8 {
        return None;
    }
    let revision = bytes[0];
    let sub_authority_count = bytes[1] as usize;
    if bytes.len() < 8 + sub_authority_count * 4 {
        return None;
    }

    let authority = bytes[2..8]
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | *byte as u64);

    let mut sid = format!("S-{revision}-{authority}");
    for index in 0..sub_authority_count {
        let start = 8 + index * 4;
        let value = u32::from_le_bytes([
            bytes[start],
            bytes[start + 1],
            bytes[start + 2],
            bytes[start + 3],
        ]);
        sid.push_str(&format!("-{value}"));
    }
    Some(sid)
}

// ---- Attribute access ------------------------------------------------------

/// One entry's attributes, with the lookups the mappers need.
///
/// LDAP attribute names are case-insensitive and a DC does not promise to echo
/// back the case that was asked for, so every lookup folds case rather than
/// trusting the request.
pub struct Attributes {
    pub dn: String,
    text: HashMap<String, Vec<String>>,
    binary: HashMap<String, Vec<Vec<u8>>>,
}

impl Attributes {
    pub fn new(
        dn: String,
        text: HashMap<String, Vec<String>>,
        binary: HashMap<String, Vec<Vec<u8>>>,
    ) -> Self {
        Self {
            dn,
            text: text
                .into_iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value))
                .collect(),
            binary: binary
                .into_iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), value))
                .collect(),
        }
    }

    /// The first value of an attribute, if it has one that is not blank.
    pub fn one(&self, name: &str) -> Option<String> {
        self.text
            .get(&name.to_ascii_lowercase())?
            .iter()
            .find(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_string())
    }

    /// Every value of a multi-valued attribute.
    pub fn many(&self, name: &str) -> Vec<String> {
        self.text
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// The raw bytes of a binary attribute.
    ///
    /// `ldap3` sorts values into the text or the binary map depending on
    /// whether they parse as UTF-8, which is a property of the value rather
    /// than of the attribute — a GUID whose bytes happen to be valid UTF-8
    /// lands in the text map. Both are checked, so those do not silently go
    /// missing.
    pub fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        let key = name.to_ascii_lowercase();
        if let Some(values) = self.binary.get(&key)
            && let Some(first) = values.first()
        {
            return Some(first.clone());
        }
        self.text
            .get(&key)?
            .first()
            .map(|value| value.as_bytes().to_vec())
    }

    pub fn integer(&self, name: &str) -> Option<u32> {
        self.one(name)?.parse().ok()
    }

    pub fn filetime(&self, name: &str) -> Option<DateTime<Utc>> {
        from_filetime(&self.one(name)?)
    }

    pub fn generalized(&self, name: &str) -> Option<DateTime<Utc>> {
        from_generalized_time(&self.one(name)?)
    }
}

// ---- Users -----------------------------------------------------------------

/// A user account in Active Directory.
#[derive(Debug, Clone, Default)]
pub struct AdUser {
    pub dn: String,
    pub sam_account_name: Option<String>,
    pub user_principal_name: Option<String>,
    pub display_name: Option<String>,
    pub mail: Option<String>,
    pub title: Option<String>,
    pub department: Option<String>,
    pub company: Option<String>,
    pub office: Option<String>,
    pub telephone: Option<String>,
    pub mobile: Option<String>,
    pub description: Option<String>,
    pub employee_id: Option<String>,
    /// The manager's DN, shown as a canonical name.
    pub manager: Option<String>,
    pub object_guid: Option<String>,
    pub object_sid: Option<String>,
    pub user_account_control: Option<u32>,
    pub pwd_last_set: Option<DateTime<Utc>>,
    pub account_expires: Option<DateTime<Utc>>,
    pub last_logon: Option<DateTime<Utc>>,
    pub lockout_time: Option<DateTime<Utc>>,
    pub when_created: Option<DateTime<Utc>>,
    pub when_changed: Option<DateTime<Utc>>,
    /// DNs of the groups this account is a direct member of.
    pub member_of: Vec<String>,
}

impl AdUser {
    /// Attributes gcm asks a DC for. Named explicitly rather than requesting
    /// everything, because `*` on a large domain returns megabytes of
    /// replication metadata nobody displays.
    pub const ATTRS: &'static [&'static str] = &[
        "distinguishedName",
        "sAMAccountName",
        "userPrincipalName",
        "displayName",
        "mail",
        "title",
        "department",
        "company",
        "physicalDeliveryOfficeName",
        "telephoneNumber",
        "mobile",
        "description",
        "employeeID",
        "manager",
        "objectGUID",
        "objectSid",
        "userAccountControl",
        "pwdLastSet",
        "accountExpires",
        "lastLogonTimestamp",
        "lockoutTime",
        "whenCreated",
        "whenChanged",
        "memberOf",
    ];

    /// Only enabled and disabled user accounts — not contacts, not computers,
    /// and not the `krbtgt` and trust accounts that share the `user` class.
    pub const FILTER: &'static str =
        "(&(objectCategory=person)(objectClass=user)(!(sAMAccountType=805306370)))";

    pub fn from_attributes(attrs: &Attributes) -> Self {
        Self {
            dn: attrs
                .one("distinguishedName")
                .unwrap_or_else(|| attrs.dn.clone()),
            sam_account_name: attrs.one("sAMAccountName"),
            user_principal_name: attrs.one("userPrincipalName"),
            display_name: attrs.one("displayName"),
            mail: attrs.one("mail"),
            title: attrs.one("title"),
            department: attrs.one("department"),
            company: attrs.one("company"),
            office: attrs.one("physicalDeliveryOfficeName"),
            telephone: attrs.one("telephoneNumber"),
            mobile: attrs.one("mobile"),
            description: attrs.one("description"),
            employee_id: attrs.one("employeeID"),
            manager: attrs.one("manager"),
            object_guid: attrs.bytes("objectGUID").as_deref().and_then(format_guid),
            object_sid: attrs.bytes("objectSid").as_deref().and_then(format_sid),
            user_account_control: attrs.integer("userAccountControl"),
            pwd_last_set: attrs.filetime("pwdLastSet"),
            account_expires: attrs.filetime("accountExpires"),
            last_logon: attrs.filetime("lastLogonTimestamp"),
            lockout_time: attrs.filetime("lockoutTime"),
            when_created: attrs.generalized("whenCreated"),
            when_changed: attrs.generalized("whenChanged"),
            member_of: attrs.many("memberOf"),
        }
    }

    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.sam_account_name.as_deref())
            .or(self.user_principal_name.as_deref())
            .unwrap_or("(unnamed)")
    }

    pub fn sam(&self) -> &str {
        self.sam_account_name.as_deref().unwrap_or("—")
    }

    pub fn upn(&self) -> &str {
        self.user_principal_name.as_deref().unwrap_or("—")
    }

    /// True when bit 2 of `userAccountControl` is set. An account with no
    /// `userAccountControl` at all is treated as enabled, which is what a DC
    /// means by omitting it.
    pub fn is_disabled(&self) -> bool {
        self.user_account_control
            .is_some_and(|value| value & uac::ACCOUNT_DISABLED != 0)
    }

    /// True while the account is locked out.
    ///
    /// Read from `lockoutTime` rather than the `LOCKOUT` bit of
    /// `userAccountControl`, which AD does not reliably maintain — the bit is
    /// computed per-DC and is routinely stale on a replica.
    pub fn is_locked_out(&self) -> bool {
        self.lockout_time.is_some()
    }

    pub fn status(&self) -> &'static str {
        if self.is_disabled() {
            "Disabled"
        } else if self.is_locked_out() {
            "Locked out"
        } else if self.is_expired() {
            "Expired"
        } else {
            "Enabled"
        }
    }

    /// True when `accountExpires` is in the past. Distinct from disabled: an
    /// expired account is still enabled, and still refuses every logon.
    pub fn is_expired(&self) -> bool {
        self.account_expires
            .is_some_and(|expiry| expiry < Utc::now())
    }

    pub fn password_never_expires(&self) -> bool {
        self.user_account_control
            .is_some_and(|value| value & uac::DONT_EXPIRE_PASSWORD != 0)
    }

    /// The notable `userAccountControl` flags, for the details pane.
    pub fn flags(&self) -> Vec<&'static str> {
        self.user_account_control.map(describe_uac).unwrap_or_default()
    }

    /// The OU this account sits in, as a canonical path.
    pub fn ou(&self) -> String {
        container_of(&self.dn)
    }

    /// The names of the groups this account belongs to, without their paths.
    pub fn group_names(&self) -> Vec<String> {
        self.member_of
            .iter()
            .map(|dn| {
                split_dn(dn)
                    .first()
                    .map(|rdn| rdn_value(rdn))
                    .unwrap_or_else(|| dn.clone())
            })
            .collect()
    }

    /// The manager's display name, taken from their DN.
    pub fn manager_name(&self) -> Option<String> {
        let dn = self.manager.as_ref()?;
        split_dn(dn).first().map(|rdn| rdn_value(rdn))
    }
}

// ---- Computers -------------------------------------------------------------

/// A computer account in Active Directory.
#[derive(Debug, Clone, Default)]
pub struct AdComputer {
    pub dn: String,
    pub name: Option<String>,
    pub sam_account_name: Option<String>,
    pub dns_host_name: Option<String>,
    pub operating_system: Option<String>,
    pub operating_system_version: Option<String>,
    pub description: Option<String>,
    pub managed_by: Option<String>,
    pub object_guid: Option<String>,
    pub user_account_control: Option<u32>,
    pub last_logon: Option<DateTime<Utc>>,
    pub when_created: Option<DateTime<Utc>>,
}

impl AdComputer {
    pub const ATTRS: &'static [&'static str] = &[
        "distinguishedName",
        "cn",
        "sAMAccountName",
        "dNSHostName",
        "operatingSystem",
        "operatingSystemVersion",
        "description",
        "managedBy",
        "objectGUID",
        "userAccountControl",
        "lastLogonTimestamp",
        "whenCreated",
    ];

    pub const FILTER: &'static str = "(objectCategory=computer)";

    pub fn from_attributes(attrs: &Attributes) -> Self {
        Self {
            dn: attrs
                .one("distinguishedName")
                .unwrap_or_else(|| attrs.dn.clone()),
            name: attrs.one("cn"),
            sam_account_name: attrs.one("sAMAccountName"),
            dns_host_name: attrs.one("dNSHostName"),
            operating_system: attrs.one("operatingSystem"),
            operating_system_version: attrs.one("operatingSystemVersion"),
            description: attrs.one("description"),
            managed_by: attrs.one("managedBy"),
            object_guid: attrs.bytes("objectGUID").as_deref().and_then(format_guid),
            user_account_control: attrs.integer("userAccountControl"),
            last_logon: attrs.filetime("lastLogonTimestamp"),
            when_created: attrs.generalized("whenCreated"),
        }
    }

    pub fn name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.sam_account_name.as_deref())
            .unwrap_or("(unnamed)")
    }

    /// Operating system and version as one string, since neither is much use
    /// alone — "Windows Server 2022 Standard" without a build, or a build
    /// without a name.
    pub fn os_display(&self) -> String {
        match (&self.operating_system, &self.operating_system_version) {
            (Some(os), Some(version)) => format!("{os} ({version})"),
            (Some(os), None) => os.clone(),
            (None, Some(version)) => version.clone(),
            (None, None) => "—".into(),
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.user_account_control
            .is_some_and(|value| value & uac::ACCOUNT_DISABLED != 0)
    }

    pub fn status(&self) -> &'static str {
        if self.is_disabled() { "Disabled" } else { "Enabled" }
    }

    /// Domain controllers hold a server trust account rather than a workstation
    /// one, which is the only way to tell them apart from a member server here.
    pub fn is_domain_controller(&self) -> bool {
        self.user_account_control
            .is_some_and(|value| value & uac::SERVER_TRUST != 0)
    }

    pub fn role(&self) -> &'static str {
        if self.is_domain_controller() {
            "Domain controller"
        } else {
            "Member"
        }
    }

    pub fn ou(&self) -> String {
        container_of(&self.dn)
    }

    /// The display name of whoever owns this machine, taken from the
    /// `managedBy` DN.
    pub fn managed_by_name(&self) -> Option<String> {
        let dn = self.managed_by.as_ref()?;
        split_dn(dn).first().map(|rdn| rdn_value(rdn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filetime_becomes_a_timestamp() {
        // 2024-01-15 09:30:00 UTC, as a DC would write it.
        let value = from_filetime("133497846000000000").expect("a real time converts");
        assert_eq!(value.format("%Y-%m-%d %H:%M").to_string(), "2024-01-15 09:30");
    }

    #[test]
    fn both_never_sentinels_are_absent_rather_than_ancient() {
        // Taken literally these render as 1601 and 30828, which is how a
        // console ends up claiming an account expired four centuries ago.
        assert!(from_filetime("0").is_none(), "0 means never logged on");
        assert!(
            from_filetime("9223372036854775807").is_none(),
            "i64::MAX means never expires"
        );
        assert!(from_filetime("-1").is_none());
        assert!(from_filetime("").is_none());
        assert!(from_filetime("not a number").is_none());
    }

    #[test]
    fn generalized_time_parses_with_and_without_a_fraction() {
        let with = from_generalized_time("20240115093000.0Z").expect("the DC's form");
        let without = from_generalized_time("20240115093000Z").expect("the bare form");
        assert_eq!(with, without);
        assert_eq!(with.format("%Y-%m-%d %H:%M").to_string(), "2024-01-15 09:30");
    }

    #[test]
    fn a_dn_splits_around_escaped_commas() {
        // The account most likely to be misfiled is the one whose name
        // contains the separator.
        let parts = split_dn("CN=Smith\\, John,OU=Finance,DC=corp,DC=contoso,DC=com");
        assert_eq!(parts.len(), 5, "the escaped comma must not split the RDN");
        assert_eq!(parts[0], "CN=Smith\\, John");
        assert_eq!(parts[1], "OU=Finance");
    }

    #[test]
    fn a_canonical_name_reads_root_first() {
        assert_eq!(
            canonical_name("CN=Aisha Rahman,OU=Finance,OU=Users,DC=corp,DC=contoso,DC=com"),
            "corp.contoso.com/Users/Finance/Aisha Rahman"
        );
    }

    #[test]
    fn a_container_drops_the_leaf() {
        assert_eq!(
            container_of("CN=Aisha Rahman,OU=Finance,DC=corp,DC=contoso,DC=com"),
            "corp.contoso.com/Finance"
        );
        // An object sitting directly in the domain root has no OU to name.
        assert_eq!(container_of("CN=Guest,DC=corp,DC=contoso,DC=com"), "corp.contoso.com");
    }

    #[test]
    fn an_escaped_comma_survives_into_the_canonical_name() {
        assert_eq!(
            canonical_name("CN=Smith\\, John,OU=Finance,DC=corp,DC=contoso,DC=com"),
            "corp.contoso.com/Finance/Smith, John"
        );
    }

    #[test]
    fn a_guid_is_unscrambled_into_windows_byte_order() {
        // The first three fields are little-endian and the last two are not;
        // reading straight through gives a GUID that matches nothing.
        let bytes: Vec<u8> = vec![
            0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        assert_eq!(
            format_guid(&bytes).expect("16 bytes is a GUID"),
            "76543210-ba98-fedc-0123-456789abcdef"
        );
        assert!(format_guid(&[0u8; 8]).is_none(), "a short value is not a GUID");
    }

    #[test]
    fn a_sid_renders_in_the_form_an_acl_shows() {
        // S-1-5-21-1004336348-1177238915-682003330-512 — Domain Admins.
        let mut bytes = vec![1, 5, 0, 0, 0, 0, 0, 5];
        for value in [21u32, 1004336348, 1177238915, 682003330, 512] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            format_sid(&bytes).expect("a well-formed SID"),
            "S-1-5-21-1004336348-1177238915-682003330-512"
        );
        assert!(format_sid(&[1, 5, 0]).is_none(), "a truncated SID is refused");
    }

    #[test]
    fn disabled_is_a_bit_rather_than_a_field() {
        let mut user = AdUser {
            user_account_control: Some(uac::NORMAL_ACCOUNT),
            ..Default::default()
        };
        assert!(!user.is_disabled());
        assert_eq!(user.status(), "Enabled");

        user.user_account_control = Some(uac::NORMAL_ACCOUNT | uac::ACCOUNT_DISABLED);
        assert!(user.is_disabled());
        assert_eq!(user.status(), "Disabled");
    }

    #[test]
    fn an_absent_uac_reads_as_enabled() {
        // Omitting the attribute is how a DC says "nothing unusual", not
        // "unknown" — defaulting to disabled would libel every such account.
        let user = AdUser::default();
        assert!(!user.is_disabled());
        assert_eq!(user.status(), "Enabled");
    }

    #[test]
    fn lockout_and_expiry_are_reported_separately_from_disabled() {
        let locked = AdUser {
            user_account_control: Some(uac::NORMAL_ACCOUNT),
            lockout_time: Some(Utc::now()),
            ..Default::default()
        };
        assert_eq!(locked.status(), "Locked out");

        let expired = AdUser {
            user_account_control: Some(uac::NORMAL_ACCOUNT),
            account_expires: Some(Utc::now() - chrono::Duration::days(1)),
            ..Default::default()
        };
        assert_eq!(expired.status(), "Expired", "an expired account is still enabled");

        let future = AdUser {
            user_account_control: Some(uac::NORMAL_ACCOUNT),
            account_expires: Some(Utc::now() + chrono::Duration::days(30)),
            ..Default::default()
        };
        assert_eq!(future.status(), "Enabled", "a future expiry has not happened yet");
    }

    #[test]
    fn the_flag_list_names_only_what_is_set() {
        let flags = describe_uac(uac::NORMAL_ACCOUNT | uac::DONT_EXPIRE_PASSWORD);
        assert_eq!(flags, vec!["Password never expires"]);
        assert!(
            describe_uac(uac::NORMAL_ACCOUNT).is_empty(),
            "an ordinary account has nothing worth flagging"
        );
    }

    #[test]
    fn the_risky_flags_are_listed_before_the_administrative_ones() {
        // The order is the point: somebody scanning this pane should meet
        // "password not required" before "home directory required".
        let flags = describe_uac(
            uac::PASSWD_NOTREQD | uac::HOMEDIR_REQUIRED | uac::DONT_EXPIRE_PASSWORD,
        );
        assert_eq!(
            flags,
            vec![
                "Password not required",
                "Password never expires",
                "Home directory required"
            ]
        );
    }

    #[test]
    fn group_membership_is_shown_by_name_rather_than_by_dn() {
        let user = AdUser {
            member_of: vec![
                "CN=Finance Team,OU=Groups,DC=corp,DC=contoso,DC=com".into(),
                "CN=VPN Users,OU=Groups,DC=corp,DC=contoso,DC=com".into(),
            ],
            ..Default::default()
        };
        assert_eq!(user.group_names(), vec!["Finance Team", "VPN Users"]);
    }

    #[test]
    fn a_domain_controller_is_told_apart_by_its_trust_account() {
        let dc = AdComputer {
            user_account_control: Some(uac::SERVER_TRUST),
            ..Default::default()
        };
        let member = AdComputer {
            user_account_control: Some(uac::WORKSTATION_TRUST),
            ..Default::default()
        };
        assert_eq!(dc.role(), "Domain controller");
        assert_eq!(member.role(), "Member");
    }

    #[test]
    fn attribute_lookup_ignores_the_case_the_dc_replied_in() {
        // A DC may echo back any case; asking for the case we sent is a bug
        // that only shows up against one vendor's server.
        let mut text = HashMap::new();
        text.insert("SAMAccountName".to_string(), vec!["arahman".to_string()]);
        let attrs = Attributes::new("CN=x".into(), text, HashMap::new());
        assert_eq!(attrs.one("sAMAccountName").as_deref(), Some("arahman"));
        assert_eq!(attrs.one("samaccountname").as_deref(), Some("arahman"));
    }

    #[test]
    fn a_blank_attribute_value_reads_as_absent() {
        let mut text = HashMap::new();
        text.insert("title".to_string(), vec!["   ".to_string()]);
        let attrs = Attributes::new("CN=x".into(), text, HashMap::new());
        assert!(attrs.one("title").is_none());
    }
}
