//! A read-only LDAP client for on-premises Active Directory.
//!
//! The shape mirrors [`crate::graph`] deliberately, because the console upstream
//! of both cannot tell them apart: paged reads of whole collections, a ceiling
//! on how much is fetched, and a distinction between "this failed" and "this
//! domain does not offer that" so the UI can explain an empty view rather than
//! showing one.
//!
//! Three things differ enough from the Graph client to be worth stating.
//!
//! * **Nothing here is long-lived.** A connection is opened, bound, searched
//!   and dropped inside one call. That is not an optimisation — it is what lets
//!   the bind password travel with the request and be forgotten afterwards,
//!   instead of the worker holding a credential for the life of the process.
//! * **Paging is a control, not a link.** AD's simple paged-results control
//!   carries an opaque cookie in the same place `@odata.nextLink` would carry a
//!   URL. `ldap3`'s [`PagedResults`] adapter hides the difference.
//! * **Everything is read-only.** No `Mod`, no `add`, no `delete`. The write
//!   gate in [`crate::worker`] governs the tenant; this module gives it nothing
//!   to govern.

pub mod models;

use std::sync::Once;
use std::time::Duration;

use anyhow::{Result, anyhow};
use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
use ldap3::{Ldap, LdapConnAsync, LdapConnSettings, LdapError, Scope, SearchEntry};

use crate::config::Directory;
use crate::mariadb::Secret;
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
            RC_INVALID_CREDENTIALS => anyhow!(
                "{} rejected the credentials for {}. Check bind_dn and the password — \
                 AD reports a wrong password and an unknown account identically.",
                settings.host,
                settings.bind_dn
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
    async fn connect(settings: &Directory, password: &Secret) -> Result<Self> {
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

        ldap.simple_bind(&settings.bind_dn, password.expose())
            .await
            .map_err(|err| explain(settings, err))?
            .success()
            .map_err(|err| explain(settings, err))?;

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

    async fn unbind(mut self) {
        let _ = self.ldap.unbind().await;
    }
}

/// Read every user account under the configured base DN.
///
/// Sorted by display name, as the Graph collections are. A DC returns entries
/// in whatever order it walked its index, which differs between two DCs in the
/// same domain — so without this the same view would be ordered differently
/// depending on which one answered.
pub async fn users(
    settings: &Directory,
    password: &Secret,
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
    password: &Secret,
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
pub async fn verify(settings: &Directory, password: &Secret) -> Result<()> {
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
        let message = format!("{:#}", explain(&settings(), result(53)));
        assert!(message.contains("53"), "got: {message}");
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
    fn installing_the_crypto_provider_twice_is_harmless() {
        // It runs once per process behind a `Once`, but the guarantee that
        // matters is that a second install attempt cannot panic — something
        // else in the dependency tree may get there first.
        ensure_crypto_provider();
        ensure_crypto_provider();
    }
}
