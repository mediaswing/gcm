//! An LDAP client for on-premises Active Directory.
//!
//! The shape mirrors [`crate::graph`] deliberately, because the console upstream
//! of both cannot tell them apart: paged reads of whole collections, a ceiling
//! on how much is fetched, and a distinction between "this failed" and "this
//! domain does not offer that" so the UI can explain an empty view rather than
//! showing one.
//!
//! Three things differ enough from the Graph client to be worth stating.
//!
//! * **Nothing here is long-lived.** A connection is opened, bound, used and
//!   dropped inside one call. That is not an optimisation — it is what lets a
//!   bind password travel with the request and be forgotten afterwards,
//!   instead of the worker holding a credential for the life of the process.
//! * **Paging is a control, not a link.** AD's simple paged-results control
//!   carries an opaque cookie in the same place `@odata.nextLink` would carry a
//!   URL. `ldap3`'s [`PagedResults`] adapter hides the difference.
//! * **Who gcm binds as is a configuration decision with real consequences.**
//!   Under `auth = "integrated"` on Windows the bind uses the operator's own
//!   Kerberos ticket, so every read and every write is evaluated by the DC
//!   against *their* account and whatever has been delegated to it. Under a
//!   simple bind everything carries the service account's rights instead, and
//!   the console cannot tell one operator from another.
//!
//! Writes live in [`actions`], behind the same write gate in
//! [`crate::worker`] that governs the tenant — and behind Active Directory's
//! own access check, which is the one that actually distinguishes between
//! operators.

pub mod actions;
pub mod models;

use std::collections::HashSet;
use std::sync::Once;
use std::time::Duration;

use anyhow::{Result, anyhow};
use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
use ldap3::{Ldap, LdapConnAsync, LdapConnSettings, LdapError, Mod, Scope, SearchEntry};

use crate::config::Directory;
use crate::mariadb::Secret;
use actions::DirectoryAction;
use models::{AdComputer, AdUser, Attributes};

/// How long to wait for a DC to answer before giving up.
///
/// A domain controller that is reachable answers in milliseconds; one that is
/// not usually does not refuse the connection but drops it, which without a
/// timeout of our own reads to the operator as a hang.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// LDAP result code 49: the bind DN or password is wrong.
const RC_INVALID_CREDENTIALS: u32 = 49;
/// LDAP result code 32: the search base does not exist.
const RC_NO_SUCH_OBJECT: u32 = 32;
/// LDAP result code 8: the DC refuses a plaintext simple bind.
const RC_STRONGER_AUTH_REQUIRED: u32 = 8;
/// LDAP result code 10: part of the subtree lives in another partition.
/// Expected, and not a failure, because gcm does not chase referrals.
const RC_REFERRAL: u32 = 10;
/// LDAP result code 4: the DC's size limit stopped the search early.
const RC_SIZE_LIMIT: u32 = 4;
/// LDAP result code 11: the DC's own administrative limit stopped it early.
const RC_ADMIN_LIMIT: u32 = 11;
/// LDAP result code 50: the bound account may not change this object. Under
/// integrated authentication this is the operator's own delegation talking.
const RC_INSUFFICIENT_ACCESS: u32 = 50;
/// LDAP result code 53: AD declines the operation — a password that fails
/// policy, or an attribute that cannot be written this way.
const RC_UNWILLING_TO_PERFORM: u32 = 53;
/// LDAP result code 19: the value breaks a constraint on the attribute.
const RC_CONSTRAINT_VIOLATION: u32 = 19;
/// LDAP result code 68: an object of that name is already there.
const RC_ALREADY_EXISTS: u32 = 68;

/// Install a rustls crypto provider, once per process.
///
/// `ldap3` builds its TLS config with `ClientConfig::builder()`, which reads a
/// process-wide default provider — and never installs one. reqwest and
/// mysql_async both select their provider explicitly, so nothing else in gcm
/// installs it either, and the first LDAPS connection would panic inside a
/// dependency with no useful message. Doing it here, before any connection is
/// attempted, is the whole reason `rustls` is a direct dependency.
///
/// A failure means something else installed one first, which is a perfectly
/// good outcome.
fn ensure_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Turn an LDAP failure into something an administrator can act on.
///
/// The raw errors are accurate and useless: a wrong password is
/// `LDAP operation result: rc=49`, and a DC that only accepts LDAPS answers a
/// bare `rc=8`. Each of these has exactly one likely cause and one obvious fix,
/// so they are named here rather than left for the operator to look up.
fn explain(settings: &Directory, error: LdapError) -> anyhow::Error {
    match &error {
        LdapError::LdapResult { result } => match result.rc {
            // Under integrated authentication there is no password to have got
            // wrong, so the advice has to be different: rc=49 there means the
            // ticket was refused, and the usual cause is that `host` is not the
            // name the DC holds a service principal for.
            RC_INVALID_CREDENTIALS if settings.uses_integrated_auth() => anyhow!(
                "{} refused the signed-in Windows account. Check that this machine is \
                 joined to the domain, that you are logged in with a domain account, \
                 and that `host` is the domain controller's real FQDN — Kerberos builds \
                 its service principal from that name, so an IP address or a CNAME is \
                 refused here even though it would resolve.",
                settings.host
            ),
            RC_INVALID_CREDENTIALS => anyhow!(
                "{} rejected the credentials for {}. Check bind_dn and the password — \
                 AD reports a wrong password and an unknown account identically.",
                settings.host,
                settings.bind_dn
            ),
            // The one that matters once writes exist. This is AD's own access
            // check refusing, not gcm's write gate — so the fix is a
            // delegation on the object, not anything in the console.
            RC_INSUFFICIENT_ACCESS => anyhow!(
                "{} refused that change: {} does not have permission on the object. \
                 This is Active Directory's own access check, not gcm's — the account \
                 needs the right delegated on that object or the OU containing it.",
                settings.host,
                settings.describe()
            ),
            // AD refuses a password write on an unencrypted connection, and
            // also refuses one that does not meet the domain's complexity or
            // history policy, with the same code.
            RC_UNWILLING_TO_PERFORM => anyhow!(
                "{} was unwilling to perform that change. For a password reset this \
                 usually means the new password does not meet the domain's complexity, \
                 length or history policy; for other changes, that the attribute cannot \
                 be set this way.",
                settings.host
            ),
            RC_CONSTRAINT_VIOLATION => anyhow!(
                "{} rejected the value as violating a constraint on that attribute — \
                 for a password, the domain's minimum age or history policy.",
                settings.host
            ),
            RC_ALREADY_EXISTS => anyhow!(
                "{} already has an object with that name in the target location.",
                settings.host
            ),
            RC_NO_SUCH_OBJECT => anyhow!(
                "{} has no object at {}. Check base_dn in the [directory] section.",
                settings.host,
                settings.base_dn
            ),
            RC_STRONGER_AUTH_REQUIRED => anyhow!(
                "{} refuses a simple bind over an unencrypted connection. Set tls = true \
                 (LDAPS on port 636) or start_tls = true in the [directory] section.",
                settings.host
            ),
            // Reported rather than swallowed: the DC returns the entries it
            // reached before stopping, so a truncated read is indistinguishable
            // from a complete one unless somebody says so.
            RC_SIZE_LIMIT | RC_ADMIN_LIMIT => anyhow!(
                "{} stopped the search before it finished (LDAP error {}), so the \
                 result would have been incomplete. Narrow base_dn to a smaller OU, or \
                 set max_objects in the [query] section to read a bounded number \
                 deliberately.",
                settings.host,
                result.rc
            ),
            rc => {
                let text = result.text.trim();
                if text.is_empty() {
                    anyhow!("{} returned LDAP error {rc}", settings.host)
                } else {
                    anyhow!("{} returned LDAP error {rc}: {text}", settings.host)
                }
            }
        },
        LdapError::Timeout { .. } => anyhow!(
            "{}:{} did not answer within {} seconds. Check the host and that {} is \
             reachable from here.",
            settings.host,
            settings.port,
            CONNECT_TIMEOUT.as_secs(),
            settings.transport()
        ),
        LdapError::Io { source } => anyhow!(
            "could not reach {}:{} — {source}. Check the host, the port, and whether a \
             firewall permits {} from here.",
            settings.host,
            settings.port,
            settings.transport()
        ),
        LdapError::Rustls { .. } | LdapError::DNSName { .. } => anyhow!(
            "the TLS handshake with {}:{} failed — {error}. A domain controller usually \
             presents a certificate from an internal CA, which must be trusted by this \
             machine's certificate store for LDAPS to succeed.",
            settings.host,
            settings.port
        ),
        _ => anyhow!("{error}"),
    }
}

/// Authenticate an open connection, by whichever method the configuration asks
/// for.
///
/// The integrated path is what makes a Directory view show the operator their
/// own permissions rather than a service account's: SSPI binds with the
/// Kerberos ticket Windows already issued them at logon, so the DC evaluates
/// every read and every write against their account and its delegations.
async fn bind(ldap: &mut Ldap, settings: &Directory, password: Option<&Secret>) -> Result<()> {
    if settings.uses_integrated_auth() {
        #[cfg(windows)]
        {
            // The SPN is built from this name, so it has to be the DC's real
            // FQDN — an IP address or a CNAME produces a ticket request for a
            // principal the KDC has never heard of. `explain` says so.
            ldap.sasl_gssapi_bind(&settings.host)
                .await
                .map_err(|err| explain(settings, err))?
                .success()
                .map_err(|err| explain(settings, err))?;
            return Ok(());
        }
        // Unreachable in practice: `Directory::validate` refuses integrated
        // authentication off Windows rather than letting it get this far.
        // Kept so the branch is a stated refusal rather than a silent
        // downgrade to a simple bind with an empty DN.
        #[cfg(not(windows))]
        return Err(anyhow!(
            "integrated authentication is only available on Windows"
        ));
    }

    let password = password.ok_or_else(|| {
        anyhow!(
            "no bind password was supplied for {}, and this connection is not using \
             integrated authentication",
            settings.host
        )
    })?;

    ldap.simple_bind(&settings.bind_dn, password.expose())
        .await
        .map_err(|err| explain(settings, err))?
        .success()
        .map_err(|err| explain(settings, err))?;
    Ok(())
}

/// An open, bound connection to a domain controller.
///
/// Holds no credential: the password is consumed by [`Self::connect`] and the
/// handle that survives it is already authenticated.
struct Connection {
    ldap: Ldap,
    settings: Directory,
}

impl Connection {
    /// Open a connection and bind.
    ///
    /// `password` is `None` under integrated authentication, where there is no
    /// credential to carry: SSPI uses the ticket the operator already has.
    async fn connect(settings: &Directory, password: Option<&Secret>) -> Result<Self> {
        ensure_crypto_provider();

        let conn_settings = LdapConnSettings::new()
            .set_conn_timeout(CONNECT_TIMEOUT)
            .set_starttls(settings.tls && settings.start_tls);

        let (conn, mut ldap) =
            LdapConnAsync::with_settings(conn_settings, &settings.url())
                .await
                .map_err(|err| explain(settings, err))?;

        // The connection half has to be driven for the handle half to make any
        // progress; without this every await below hangs rather than failing.
        ldap3::drive!(conn);

        bind(&mut ldap, settings, password).await?;

        Ok(Self {
            ldap,
            settings: settings.clone(),
        })
    }

    /// Walk a paged subtree search, mapping each entry as it arrives.
    ///
    /// `max_objects` is the same safety valve the Graph client applies, for the
    /// same reason: a forest root with eighty thousand computer accounts should
    /// paint promptly rather than completely. Stopping early abandons the
    /// search rather than draining it, so the DC stops working on a result
    /// nobody is going to read.
    async fn search<T>(
        &mut self,
        filter: &str,
        attrs: &[&str],
        page_size: i32,
        max_objects: u32,
        map: impl Fn(&Attributes) -> T,
    ) -> Result<Vec<T>> {
        let adapters: Vec<Box<dyn Adapter<_, _>>> = vec![
            Box::new(EntriesOnly::new()),
            Box::new(PagedResults::new(page_size)),
        ];

        let mut stream = self
            .ldap
            .streaming_search_with(
                adapters,
                &self.settings.base_dn,
                Scope::Subtree,
                filter,
                attrs.to_vec(),
            )
            .await
            .map_err(|err| explain(&self.settings, err))?;

        let ceiling = if max_objects == 0 {
            usize::MAX
        } else {
            max_objects as usize
        };

        let mut results = Vec::new();
        while let Some(entry) = stream
            .next()
            .await
            .map_err(|err| explain(&self.settings, err))?
        {
            let entry = SearchEntry::construct(entry);
            let attributes = Attributes::new(entry.dn, entry.attrs, entry.bin_attrs);
            results.push(map(&attributes));

            if results.len() >= ceiling {
                break;
            }
        }

        // Whether *we* stopped it. A deliberate stop abandons the search, so
        // the code it finishes with says nothing worth reporting.
        let truncated = results.len() >= ceiling;
        let outcome = stream.finish().await;

        // Otherwise the closing result code matters, and discarding it is how a
        // console ends up presenting half a domain as the whole of it. A DC cuts
        // a search short with `sizeLimitExceeded` or `adminLimitExceeded` and
        // still returns every entry it managed first — so the entries look
        // perfectly normal, and nothing says the list is incomplete.
        //
        // `referral` is the exception: referral chasing is off, so a search
        // crossing a partition boundary is expected to report one, and
        // `EntriesOnly` has already dropped the references themselves.
        if !truncated && outcome.rc != 0 && outcome.rc != RC_REFERRAL {
            return Err(explain(
                &self.settings,
                LdapError::LdapResult { result: outcome },
            ));
        }

        Ok(results)
    }

    /// Read one integer attribute from a single object.
    ///
    /// A base-scope search rather than a subtree one: the DN is already known
    /// exactly, and searching beneath it would be both slower and wrong.
    async fn read_u32(&mut self, dn: &str, attribute: &str) -> Result<Option<u32>> {
        let (entries, outcome) = self
            .ldap
            .search(dn, Scope::Base, "(objectClass=*)", vec![attribute])
            .await
            .map_err(|err| explain(&self.settings, err))?
            .success()
            .map_err(|err| explain(&self.settings, err))?;
        let _ = outcome;

        let Some(entry) = entries.into_iter().next() else {
            return Ok(None);
        };
        let entry = SearchEntry::construct(entry);
        Ok(entry
            .attrs
            .get(attribute)
            .and_then(|values| values.first())
            .and_then(|value| value.parse().ok()))
    }

    /// Apply one modification, translating the result code on the way back.
    async fn modify(&mut self, dn: &str, mods: Vec<Mod<Vec<u8>>>) -> Result<()> {
        self.ldap
            .modify(dn, mods)
            .await
            .map_err(|err| explain(&self.settings, err))?
            .success()
            .map_err(|err| explain(&self.settings, err))?;
        Ok(())
    }

    /// Carry out one change.
    async fn apply(&mut self, action: &DirectoryAction) -> Result<()> {
        match action {
            DirectoryAction::SetEnabled { dn, enabled, .. } => {
                // Read-modify-write, because userAccountControl is a bitfield
                // holding a dozen unrelated flags. Writing a bare 0x2 would
                // clear NORMAL_ACCOUNT along with everything else and leave an
                // account that fails to authenticate for reasons that take a
                // long afternoon to find.
                let current = self
                    .read_u32(dn, "userAccountControl")
                    .await?
                    .ok_or_else(|| {
                        anyhow!("{dn} has no userAccountControl, so it is not an account")
                    })?;
                let updated = actions::account_control_for(current, *enabled);
                if updated == current {
                    // Already in the requested state. Writing it anyway would
                    // succeed and log a change that did not happen.
                    return Ok(());
                }
                self.modify(dn, vec![replace("userAccountControl", updated.to_string())])
                    .await
            }
            DirectoryAction::Unlock { dn, .. } => {
                // 0 is what AD defines as "not locked out". The attribute is
                // never removed; a missing lockoutTime is not the same thing.
                self.modify(dn, vec![replace("lockoutTime", "0")]).await
            }
            DirectoryAction::ResetPassword {
                dn,
                password,
                must_change,
                ..
            } => {
                let mut mods = vec![Mod::Replace(
                    b"unicodePwd".to_vec(),
                    HashSet::from([actions::encode_password(password.expose())]),
                )];
                if *must_change {
                    // 0 means "must change at next logon". -1 would mean "set
                    // it now"; anything else is refused.
                    mods.push(replace("pwdLastSet", "0"));
                }
                self.modify(dn, mods).await
            }
            DirectoryAction::UpdateUser { dn, patch, .. } => {
                let mods = patch.modifications();
                if mods.is_empty() {
                    return Ok(());
                }
                self.modify(dn, mods).await
            }
            DirectoryAction::Delete { dn, .. } => {
                self.ldap
                    .delete(dn)
                    .await
                    .map_err(|err| explain(&self.settings, err))?
                    .success()
                    .map_err(|err| explain(&self.settings, err))?;
                Ok(())
            }
        }
    }

    async fn unbind(mut self) {
        let _ = self.ldap.unbind().await;
    }
}

/// A `Replace` of one textual attribute with one value.
fn replace(attribute: &str, value: impl AsRef<str>) -> Mod<Vec<u8>> {
    Mod::Replace(
        attribute.as_bytes().to_vec(),
        HashSet::from([value.as_ref().as_bytes().to_vec()]),
    )
}

/// Who the domain controller will evaluate a change against.
///
/// Under a simple bind this is the configured service account, and every
/// operator's changes look identical. Under integrated authentication it is
/// the signed-in Windows account — which is the whole reason that mode exists,
/// and so is the name the audit log has to carry. `USERDOMAIN` and `USERNAME`
/// are what Windows itself sets for the interactive session, and they produce
/// exactly the `DOMAIN\user` form an administrator will search AD's own logs
/// for.
pub fn acting_identity(settings: &Directory) -> String {
    if !settings.uses_integrated_auth() {
        return settings.bind_dn.clone();
    }

    match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(user)) if !domain.is_empty() && !user.is_empty() => {
            format!("{domain}\\{user}")
        }
        (_, Ok(user)) if !user.is_empty() => user,
        // Better to say the identity is unknown than to name the wrong one.
        _ => "the signed-in Windows account".into(),
    }
}

/// Carry out one change against the domain controller.
///
/// The write gate in [`crate::worker`] decides whether this is called at all.
/// What happens once it is called is Active Directory's decision: under
/// integrated authentication the connection carries the operator's own
/// identity, so their delegations are what determine whether the DC accepts
/// it.
pub async fn apply(
    settings: &Directory,
    password: Option<&Secret>,
    action: &DirectoryAction,
) -> Result<()> {
    // Refused here rather than by the DC. AD rejects `unicodePwd` on an
    // unencrypted connection — but only after the password has already
    // travelled over it, which is precisely the thing worth preventing.
    if action.carries_a_password() && !settings.tls {
        return Err(anyhow!(
            "refusing to set a password over an unencrypted connection to {}. Set \
             tls = true (LDAPS on port 636) or start_tls = true in the [directory] \
             section; Active Directory will not accept a password reset any other way.",
            settings.host
        ));
    }

    let mut connection = Connection::connect(settings, password).await?;
    let outcome = connection.apply(action).await;
    connection.unbind().await;
    outcome
}

/// Read every user account under the configured base DN.
///
/// Sorted by display name, as the Graph collections are. A DC returns entries
/// in whatever order it walked its index, which differs between two DCs in the
/// same domain — so without this the same view would be ordered differently
/// depending on which one answered.
pub async fn users(
    settings: &Directory,
    password: Option<&Secret>,
    page_size: i32,
    max_objects: u32,
) -> Result<Vec<AdUser>> {
    let mut connection = Connection::connect(settings, password).await?;
    let users = connection
        .search(
            AdUser::FILTER,
            AdUser::ATTRS,
            page_size,
            max_objects,
            AdUser::from_attributes,
        )
        .await;
    connection.unbind().await;

    let mut users = users?;
    users.sort_by_key(|user| user.name().to_lowercase());
    Ok(users)
}

/// Read every computer account under the configured base DN.
pub async fn computers(
    settings: &Directory,
    password: Option<&Secret>,
    page_size: i32,
    max_objects: u32,
) -> Result<Vec<AdComputer>> {
    let mut connection = Connection::connect(settings, password).await?;
    let computers = connection
        .search(
            AdComputer::FILTER,
            AdComputer::ATTRS,
            page_size,
            max_objects,
            AdComputer::from_attributes,
        )
        .await;
    connection.unbind().await;

    let mut computers = computers?;
    computers.sort_by_key(|computer| computer.name().to_lowercase());
    Ok(computers)
}

/// Verify that a bind DN and password work, without reading anything.
///
/// The bind dialog calls this before the password is accepted for the session,
/// so a typo is reported against the field that caused it rather than surfacing
/// later as an empty Users view.
pub async fn verify(settings: &Directory, password: Option<&Secret>) -> Result<()> {
    let connection = Connection::connect(settings, password).await?;
    connection.unbind().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldap3::result::LdapResult;

    fn settings() -> Directory {
        Directory {
            host: "dc01.corp.contoso.com".into(),
            port: 636,
            base_dn: "DC=corp,DC=contoso,DC=com".into(),
            bind_dn: "CORP\\svc-gcm".into(),
            auth: crate::config::DirectoryAuth::Simple,
            tls: true,
            start_tls: false,
        }
    }

    fn result(rc: u32) -> LdapError {
        LdapError::LdapResult {
            result: LdapResult {
                rc,
                matched: String::new(),
                text: String::new(),
                refs: Vec::new(),
                ctrls: Vec::new(),
            },
        }
    }

    fn integrated() -> Directory {
        Directory {
            auth: crate::config::DirectoryAuth::Integrated,
            bind_dn: String::new(),
            ..settings()
        }
    }

    #[test]
    fn a_refused_ticket_does_not_advise_checking_a_password() {
        // Under integrated authentication there is no password and no
        // bind_dn, so the simple-bind advice would send somebody looking for
        // settings that are deliberately empty. The real causes are a
        // non-domain-joined machine or a host that is not the DC's FQDN.
        let message = format!("{:#}", explain(&integrated(), result(RC_INVALID_CREDENTIALS)));
        assert!(!message.contains("bind_dn"), "got: {message}");
        assert!(!message.contains("password"), "got: {message}");
        assert!(message.contains("FQDN"), "got: {message}");
        assert!(message.contains("domain"), "got: {message}");
    }

    #[test]
    fn a_permission_failure_is_blamed_on_ad_rather_than_on_gcm() {
        // rc=50 is the whole point of integrated authentication: the operator
        // is being refused by Active Directory's own access check, and no
        // amount of arming write mode in gcm will change that. The message has
        // to say so, or the next thing they do is look in the wrong place.
        let message = format!("{:#}", explain(&integrated(), result(RC_INSUFFICIENT_ACCESS)));
        assert!(message.contains("Active Directory's own"), "got: {message}");
        assert!(message.contains("delegated"), "got: {message}");
        // And it names who was refused, which under integrated auth is the
        // signed-in account rather than a bind DN.
        assert!(message.contains("signed-in Windows account"), "got: {message}");
    }

    #[test]
    fn a_simple_bind_still_gets_the_simple_bind_advice() {
        // The two messages must not converge: a service-account setup has a
        // bind_dn and a password that genuinely might be wrong.
        let message = format!("{:#}", explain(&settings(), result(RC_INVALID_CREDENTIALS)));
        assert!(message.contains("bind_dn"), "got: {message}");
        assert!(message.contains("password"), "got: {message}");
    }

    #[test]
    fn bad_credentials_name_the_setting_to_fix() {
        let message = format!("{:#}", explain(&settings(), result(RC_INVALID_CREDENTIALS)));
        assert!(message.contains("bind_dn"), "got: {message}");
        assert!(message.contains("CORP\\svc-gcm"), "got: {message}");
    }

    #[test]
    fn a_missing_base_names_the_base_that_was_tried() {
        // rc=32 against a base DN is nearly always a typo in the config, and
        // the fastest way to see it is to be shown what was sent.
        let message = format!("{:#}", explain(&settings(), result(RC_NO_SUCH_OBJECT)));
        assert!(message.contains("DC=corp,DC=contoso,DC=com"), "got: {message}");
        assert!(message.contains("base_dn"), "got: {message}");
    }

    #[test]
    fn a_refused_plaintext_bind_suggests_turning_tls_on() {
        // rc=8 is what every hardened DC answers to a simple bind on port 389,
        // and it is unintelligible without this translation.
        let message = format!("{:#}", explain(&settings(), result(RC_STRONGER_AUTH_REQUIRED)));
        assert!(message.contains("tls = true"), "got: {message}");
        assert!(message.contains("start_tls"), "got: {message}");
    }

    #[test]
    fn a_truncated_search_says_the_result_was_incomplete() {
        // The failure mode this guards: the DC returns every entry it reached
        // before stopping, so silence here reads as a complete domain.
        for rc in [RC_SIZE_LIMIT, RC_ADMIN_LIMIT] {
            let message = format!("{:#}", explain(&settings(), result(rc)));
            assert!(message.contains("incomplete"), "rc={rc} got: {message}");
            assert!(message.contains("max_objects"), "rc={rc} got: {message}");
        }
    }

    #[test]
    fn an_unrecognised_code_is_still_reported_with_its_number() {
        // 51 (busy) is deliberately one gcm does not translate. Codes that
        // have their own message — 49, 50, 53 and the rest above — are tested
        // for that message instead.
        let message = format!("{:#}", explain(&settings(), result(51)));
        assert!(message.contains("51"), "got: {message}");
    }

    #[test]
    fn the_url_scheme_follows_the_transport() {
        let mut config = settings();
        assert_eq!(config.url(), "ldaps://dc01.corp.contoso.com:636");
        assert_eq!(config.transport(), "LDAPS");

        // StartTLS begins in the clear and upgrades, so it is an ldap:// URL
        // even though the session ends up encrypted.
        config.start_tls = true;
        config.port = 389;
        assert_eq!(config.url(), "ldap://dc01.corp.contoso.com:389");
        assert_eq!(config.transport(), "StartTLS");

        config.tls = false;
        config.start_tls = false;
        assert_eq!(config.url(), "ldap://dc01.corp.contoso.com:389");
        assert_eq!(config.transport(), "unencrypted");
    }

    #[test]
    fn a_password_reset_is_refused_before_it_reaches_an_unencrypted_wire() {
        // The ordering is the whole point. Active Directory also refuses this,
        // but only after the password has already crossed the network in the
        // clear — so gcm has to refuse first. The host here is unroutable, and
        // the test passing quickly is itself evidence that nothing was
        // connected to.
        let mut plaintext = settings();
        plaintext.tls = false;
        plaintext.start_tls = false;
        plaintext.port = 389;

        let reset = DirectoryAction::ResetPassword {
            dn: "CN=Jane Smith,OU=Staff,DC=corp,DC=contoso,DC=com".into(),
            name: "Jane Smith".into(),
            password: Secret::new("Pa55w0rd!".into()),
            must_change: true,
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let err = runtime
            .block_on(apply(&plaintext, Some(&Secret::new("x".into())), &reset))
            .expect_err("a password must not be sent unencrypted");

        let message = format!("{err:#}");
        assert!(message.contains("unencrypted"), "got: {message}");
        assert!(message.contains("tls = true"), "got: {message}");
        // And the password itself is never in the message.
        assert!(!message.contains("Pa55w0rd"), "got: {message}");
    }

    #[test]
    fn changes_that_carry_no_password_are_not_blocked_by_the_tls_check() {
        // The guard is specific to `unicodePwd`; an unlock over a plaintext
        // connection is a bad idea but not this function's decision to refuse,
        // and blocking it here would be a surprise.
        let unlock = DirectoryAction::Unlock {
            dn: "CN=a,DC=b".into(),
            name: "a".into(),
        };
        assert!(!unlock.carries_a_password());
    }

    #[test]
    fn installing_the_crypto_provider_twice_is_harmless() {
        // It runs once per process behind a `Once`, but the guarantee that
        // matters is that a second install attempt cannot panic — something
        // else in the dependency tree may get there first.
        ensure_crypto_provider();
        ensure_crypto_provider();
    }
}
