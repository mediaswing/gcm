//! Changes gcm makes to on-premises Active Directory.
//!
//! The counterpart to [`crate::graph::actions`], and deliberately the same
//! shape: an action is an inert value describing what to do, carrying its own
//! label, severity and target, and it is executed somewhere else. That is what
//! lets [`crate::worker`] gate, log and report on-premises changes with the
//! same code path it already uses for the tenant, rather than growing a second
//! set of rules that could drift from the first.
//!
//! ## Two gates, not one
//!
//! A tenant write passes exactly one check: gcm's own write mode. Graph will
//! do whatever the app registration's scopes allow, so that gate is the only
//! thing standing between a button and a change.
//!
//! An on-premises write passes two. gcm's write gate is the first, and Active
//! Directory's own access check is the second — and under integrated
//! authentication that second one is evaluated against the operator's account,
//! not a service account's. This is the point of the arrangement: an operator
//! delegated password resets over one OU can reset passwords in that OU and
//! nothing else, and gcm does not have to know anything about that. A refusal
//! comes back as `rc=50` and is explained as AD's decision rather than gcm's,
//! because the fix is a delegation rather than anything in the console.
//!
//! ## What is deliberately absent
//!
//! No object creation. Creating a user in AD means choosing an OU, an RDN, a
//! sAMAccountName, a UPN suffix and an initial password, and getting any of
//! them wrong leaves an object that has to be cleaned up by hand. The console
//! creates users in the tenant, where those decisions are far smaller.
//!
//! No group membership editing and no moving between OUs either, for a duller
//! reason: both need somewhere to pick the target from, and gcm reads only
//! users and computers from a domain controller — there is no AD group
//! collection and no OU tree to choose from. Adding those is a larger feature
//! than the write path itself, and a half-built picker is worse than none.

use std::collections::HashSet;

use ldap3::Mod;

use crate::graph::actions::Severity;
use crate::mariadb::Secret;

/// The `ACCOUNTDISABLE` flag in `userAccountControl`.
pub const UAC_ACCOUNTDISABLE: u32 = 0x0002;

/// Attributes an operator may edit on an on-premises account.
///
/// Every field is `Option`: `None` leaves the attribute alone, `Some("")`
/// clears it. Those are genuinely different instructions, and collapsing them
/// would make "clear this person's telephone number" impossible to express.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdUserPatch {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub department: Option<String>,
    pub company: Option<String>,
    pub office: Option<String>,
    pub telephone: Option<String>,
    pub mobile: Option<String>,
    pub mail: Option<String>,
    pub employee_id: Option<String>,
}

impl AdUserPatch {
    /// The LDAP attribute name for each field that was set.
    fn entries(&self) -> Vec<(&'static str, &String)> {
        let mut out = Vec::new();
        for (attr, value) in [
            ("displayName", &self.display_name),
            ("description", &self.description),
            ("title", &self.title),
            ("department", &self.department),
            ("company", &self.company),
            ("physicalDeliveryOfficeName", &self.office),
            ("telephoneNumber", &self.telephone),
            ("mobile", &self.mobile),
            ("mail", &self.mail),
            ("employeeID", &self.employee_id),
        ] {
            if let Some(value) = value {
                out.push((attr, value));
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// The modifications this patch becomes.
    ///
    /// A cleared value is a `Replace` with no values, which is how LDAP
    /// removes an attribute — Active Directory has no concept of an attribute
    /// present but empty, so writing `""` would be rejected rather than
    /// clearing it.
    pub fn modifications(&self) -> Vec<Mod<Vec<u8>>> {
        self.entries()
            .into_iter()
            .map(|(attr, value)| {
                let mut values = HashSet::new();
                if !value.trim().is_empty() {
                    values.insert(value.as_bytes().to_vec());
                }
                Mod::Replace(attr.as_bytes().to_vec(), values)
            })
            .collect()
    }
}

/// One change to an on-premises object.
#[derive(Debug, Clone)]
pub enum DirectoryAction {
    /// Flip the `ACCOUNTDISABLE` bit in `userAccountControl`.
    SetEnabled {
        dn: String,
        name: String,
        enabled: bool,
    },
    /// Clear `lockoutTime`, releasing an account locked out by failed
    /// sign-ins. Writing 0 is what AD defines as "not locked out"; the
    /// attribute is never deleted.
    Unlock { dn: String, name: String },
    /// Administratively set `unicodePwd`.
    ResetPassword {
        dn: String,
        name: String,
        password: Secret,
        /// Force a change at next sign-in, by setting `pwdLastSet` to 0.
        must_change: bool,
    },
    /// Write a handful of ordinary attributes.
    UpdateUser {
        dn: String,
        name: String,
        patch: Box<AdUserPatch>,
    },
    /// Delete an object outright.
    Delete { dn: String, name: String },
}

impl DirectoryAction {
    /// What the menus, the confirmation and the audit log call this.
    pub fn label(&self) -> String {
        match self {
            Self::SetEnabled { name, enabled, .. } => {
                let verb = if *enabled { "Enable" } else { "Disable" };
                format!("{verb} {name} in Active Directory")
            }
            Self::Unlock { name, .. } => format!("Unlock {name} in Active Directory"),
            Self::ResetPassword { name, .. } => {
                format!("Reset the on-premises password for {name}")
            }
            Self::UpdateUser { name, .. } => format!("Update {name} in Active Directory"),
            Self::Delete { name, .. } => format!("Delete {name} from Active Directory"),
        }
    }

    /// How much care this warrants before it runs.
    ///
    /// Matched to the tenant equivalents rather than invented separately: a
    /// deletion is destructive on both sides of the sync, and an operator
    /// should not find the same decision guarded differently depending on
    /// which pane they are looking at.
    pub fn severity(&self) -> Severity {
        match self {
            // Reversible and low-impact: an unlock restores the state the
            // account was in before somebody mistyped their password.
            Self::Unlock { .. } => Severity::Safe,
            Self::UpdateUser { .. } => Severity::Safe,
            Self::SetEnabled { .. } | Self::ResetPassword { .. } => Severity::Caution,
            // A deleted AD object takes its SID with it. Restoring one from
            // the Recycle Bin is a different job, and needs it to have been
            // enabled beforehand.
            Self::Delete { .. } => Severity::Destructive,
        }
    }

    /// The object this acts on. The DN is the only stable identifier a DC
    /// offers for a write, and it is what the audit log should record.
    pub fn target_id(&self) -> &str {
        match self {
            Self::SetEnabled { dn, .. }
            | Self::Unlock { dn, .. }
            | Self::ResetPassword { dn, .. }
            | Self::UpdateUser { dn, .. }
            | Self::Delete { dn, .. } => dn,
        }
    }

    pub fn target_name(&self) -> &str {
        match self {
            Self::SetEnabled { name, .. }
            | Self::Unlock { name, .. }
            | Self::ResetPassword { name, .. }
            | Self::UpdateUser { name, .. }
            | Self::Delete { name, .. } => name,
        }
    }

    /// What the operator should understand before approving this.
    ///
    /// Only where the consequence is not obvious from the label. "Disable
    /// account" explains itself; that a disabled on-premises account will
    /// propagate to the tenant at the next sync — and that the tenant copy may
    /// stay enabled until it does — does not.
    pub fn consequence(&self) -> Option<&'static str> {
        match self {
            Self::SetEnabled { enabled: false, .. } => Some(
                "The account is disabled in Active Directory immediately. The tenant \
                 copy stays as it is until the next directory sync, so the two will \
                 disagree until then.",
            ),
            Self::ResetPassword { .. } => Some(
                "The password is replaced immediately. If password hash sync is in \
                 use, the tenant copy follows at the next sync rather than at once.",
            ),
            Self::Delete { .. } => Some(
                "The object is removed along with its SID and group memberships. \
                 Recovering it needs the AD Recycle Bin, which has to have been \
                 enabled beforehand — and a resynced replacement is a different \
                 account as far as the tenant is concerned.",
            ),
            _ => None,
        }
    }

    /// Whether this action puts a password on the wire.
    ///
    /// Active Directory refuses `unicodePwd` on an unencrypted connection, but
    /// gcm refuses first — the failure would otherwise be a bare `rc=53` after
    /// the password had already been sent in the clear.
    pub fn carries_a_password(&self) -> bool {
        matches!(self, Self::ResetPassword { .. })
    }
}

/// Encode a password the way Active Directory requires `unicodePwd` to be
/// written: wrapped in double quotes, then UTF-16 little-endian.
///
/// Neither half is optional and neither is guessable. Writing the plain UTF-8
/// bytes is refused with a constraint violation that says nothing about why.
pub fn encode_password(password: &str) -> Vec<u8> {
    format!("\"{password}\"")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// `userAccountControl` with the disable bit set or cleared.
pub fn account_control_for(current: u32, enabled: bool) -> u32 {
    if enabled {
        current & !UAC_ACCOUNTDISABLE
    } else {
        current | UAC_ACCOUNTDISABLE
    }
}

/// The parent DN of a distinguished name — everything after the first
/// unescaped comma.
///
/// Escaping matters: `CN=Smith\, John,OU=Staff,DC=corp` has a comma inside its
/// RDN, and splitting on the first comma of the string would produce nonsense.
#[allow(dead_code, reason = "the OU move that uses these is not built yet")]
pub fn parent_dn(dn: &str) -> Option<&str> {
    let bytes = dn.as_bytes();
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'\\' if !escaped => escaped = true,
            b',' if !escaped => return Some(dn[index + 1..].trim_start()),
            _ => escaped = false,
        }
    }
    None
}

/// The leading RDN of a distinguished name — `CN=Jane Smith` from
/// `CN=Jane Smith,OU=Staff,DC=corp,DC=contoso,DC=com`.
///
/// Needed because a move keeps the object's own name and changes only its
/// parent, and `modifydn` takes the two separately.
#[allow(dead_code, reason = "the OU move that uses these is not built yet")]
pub fn leading_rdn(dn: &str) -> &str {
    match parent_dn(dn) {
        Some(parent) => {
            let cut = dn.len() - parent.len();
            dn[..cut].trim_end().trim_end_matches(',')
        }
        None => dn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_quoted_and_utf16_little_endian() {
        // Both halves are required by AD and neither is guessable from the
        // error it returns when they are missing.
        let encoded = encode_password("Pa55w0rd");
        assert_eq!(&encoded[..2], &[b'"', 0], "must start with a quote");
        assert_eq!(&encoded[encoded.len() - 2..], &[b'"', 0], "and end with one");
        assert_eq!(
            encoded.len(),
            ("Pa55w0rd".len() + 2) * 2,
            "one UTF-16 code unit per character, quotes included"
        );
        // Little-endian: the high byte of an ASCII character is the zero.
        assert_eq!(&encoded[2..4], &[b'P', 0]);
    }

    #[test]
    fn a_non_ascii_password_survives_encoding() {
        let encoded = encode_password("café☕");
        // é is one UTF-16 unit, ☕ is one, so 5 characters plus two quotes.
        assert_eq!(encoded.len(), 7 * 2);
    }

    #[test]
    fn the_disable_bit_is_flipped_without_disturbing_the_others() {
        // 0x0200 is NORMAL_ACCOUNT and must survive. Writing a bare 0x2
        // instead of a masked value is the classic way to turn an account into
        // something that cannot log in for reasons nobody can find.
        let normal = 0x0200;
        assert_eq!(account_control_for(normal, false), 0x0202);
        assert_eq!(account_control_for(0x0202, true), 0x0200);
        // Already in the requested state: unchanged rather than toggled.
        assert_eq!(account_control_for(0x0202, false), 0x0202);
        assert_eq!(account_control_for(0x0200, true), 0x0200);
        // Unrelated flags are preserved. 0x10000 is DONT_EXPIRE_PASSWORD.
        assert_eq!(account_control_for(0x10200, false), 0x10202);
    }

    #[test]
    fn a_dn_is_split_on_the_first_unescaped_comma() {
        assert_eq!(
            parent_dn("CN=Jane Smith,OU=Staff,DC=corp,DC=contoso,DC=com"),
            Some("OU=Staff,DC=corp,DC=contoso,DC=com")
        );
        assert_eq!(leading_rdn("CN=Jane Smith,OU=Staff,DC=corp"), "CN=Jane Smith");

        // The case that breaks a naive split: a comma inside the RDN itself,
        // which is how AD writes "Surname, Forename" display names.
        assert_eq!(
            parent_dn(r"CN=Smith\, John,OU=Staff,DC=corp"),
            Some("OU=Staff,DC=corp")
        );
        assert_eq!(leading_rdn(r"CN=Smith\, John,OU=Staff,DC=corp"), r"CN=Smith\, John");

        // A naming context has no parent.
        assert_eq!(parent_dn("DC=corp"), None);
        assert_eq!(leading_rdn("DC=corp"), "DC=corp");
    }

    #[test]
    fn clearing_an_attribute_is_told_apart_from_leaving_it_alone() {
        // The distinction the whole `Option<String>` shape exists for.
        let untouched = AdUserPatch::default();
        assert!(untouched.is_empty());
        assert!(untouched.modifications().is_empty());

        let cleared = AdUserPatch {
            telephone: Some(String::new()),
            ..Default::default()
        };
        let mods = cleared.modifications();
        assert_eq!(mods.len(), 1);
        match &mods[0] {
            // No values at all is how LDAP deletes an attribute; AD will not
            // store one that is present and empty.
            Mod::Replace(attr, values) => {
                assert_eq!(attr, b"telephoneNumber");
                assert!(values.is_empty(), "clearing must send no values");
            }
            other => panic!("expected a Replace, got {other:?}"),
        }

        let set = AdUserPatch {
            title: Some("Head of Fish".into()),
            ..Default::default()
        };
        match &set.modifications()[0] {
            Mod::Replace(attr, values) => {
                assert_eq!(attr, b"title");
                assert!(values.contains(b"Head of Fish".as_slice()));
            }
            other => panic!("expected a Replace, got {other:?}"),
        }
    }

    #[test]
    fn a_deletion_is_the_only_destructive_change() {
        let user = |dn: &str| dn.to_string();
        assert_eq!(
            DirectoryAction::Delete {
                dn: user("CN=a,DC=b"),
                name: "a".into()
            }
            .severity(),
            Severity::Destructive
        );
        assert_eq!(
            DirectoryAction::Unlock {
                dn: user("CN=a,DC=b"),
                name: "a".into()
            }
            .severity(),
            Severity::Safe
        );
        assert_eq!(
            DirectoryAction::ResetPassword {
                dn: user("CN=a,DC=b"),
                name: "a".into(),
                password: Secret::new("x".into()),
                must_change: true,
            }
            .severity(),
            Severity::Caution
        );
    }

    #[test]
    fn only_a_password_reset_needs_an_encrypted_connection() {
        let reset = DirectoryAction::ResetPassword {
            dn: "CN=a,DC=b".into(),
            name: "a".into(),
            password: Secret::new("x".into()),
            must_change: false,
        };
        assert!(reset.carries_a_password());
        assert!(
            !DirectoryAction::Unlock {
                dn: "CN=a,DC=b".into(),
                name: "a".into()
            }
            .carries_a_password()
        );
    }

    #[test]
    fn a_password_never_appears_in_a_debug_rendering() {
        // The action travels inside a `Command`, which derives Debug. This is
        // the same guarantee `Secret` gives the database password, checked
        // here because a reset carries one through the identical path.
        let reset = DirectoryAction::ResetPassword {
            dn: "CN=a,DC=b".into(),
            name: "a".into(),
            password: Secret::new("hunter2".into()),
            must_change: false,
        };
        let rendered = format!("{reset:?}");
        assert!(!rendered.contains("hunter2"), "got: {rendered}");
        assert!(rendered.contains("redacted"), "got: {rendered}");
        // And the label, which is shown on screen and written to the log.
        assert!(!reset.label().contains("hunter2"));
    }
}
