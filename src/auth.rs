//! OAuth 2.0 authorization code flow with PKCE, against Entra ID.
//!
//! The config carries only a client ID and a tenant ID — there is no client
//! secret, and a public client cannot keep one. PKCE takes its place: the
//! authorization code is redeemable only by whoever generated the verifier.
//!
//! Sign-in happens in the operator's own browser, which redirects back to a
//! loopback listener on `127.0.0.1`. That matters for more than convenience:
//! because the browser runs on this machine, Entra sees the real device and
//! Conditional Access policies keyed on device state can evaluate it. The
//! device code flow, which this replaced, cannot — the browser typing the code
//! and the application receiving the token are different devices as far as
//! Entra is concerned, so device-based policies reject it outright.
//!
//! The resulting refresh token is cached on disk so subsequent launches sign in
//! silently.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use urlencoding::encode;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::{Config, config_dir, scopes};

/// Refresh a little before actual expiry so a long list call cannot straddle
/// the boundary and 401 halfway through paging.
const EXPIRY_SKEW: i64 = 120;

/// Progress messages emitted while acquiring a token.
#[derive(Debug, Clone)]
pub enum AuthProgress {
    /// A cached refresh token is being redeemed; no user action needed.
    Silent,
    /// The browser has been opened and we are waiting for it to come back. The
    /// URL is carried so the UI can offer it when the browser did not open.
    AwaitingBrowser { url: String },
}

#[derive(Debug, Clone)]
pub struct Token {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: Option<String>,
    /// UPN of the signed-in account, decoded from the ID token when present.
    pub account: Option<String>,
    /// Whether this token carries the write scopes.
    pub writes: bool,
}

impl Token {
    fn is_valid(&self) -> bool {
        Utc::now() + ChronoDuration::seconds(EXPIRY_SKEW) < self.expires_at
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Persisted between runs. Only the refresh token is worth keeping; access
/// tokens expire in about an hour.
#[derive(Debug, Serialize, Deserialize)]
struct CachedToken {
    refresh_token: String,
    #[serde(default)]
    account: Option<String>,
}

fn cache_path() -> PathBuf {
    config_dir().join("token.json")
}

fn save_cache(token: &Token) {
    let Some(refresh_token) = token.refresh_token.clone() else {
        return;
    };
    let cached = CachedToken {
        refresh_token,
        account: token.account.clone(),
    };
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&cached)
        && fs::write(&path, json).is_ok()
    {
        restrict_permissions(&path);
    }
}

/// Make the token cache readable only by its owner. The refresh token is a
/// bearer credential for the whole tenant's read surface.
#[cfg(unix)]
fn restrict_permissions(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &PathBuf) {}

fn load_cache() -> Option<CachedToken> {
    let raw = fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Delete the cached refresh token.
///
/// Deliberately private. Signing out means [`Authenticator::forget`] — deleting
/// this file alone leaves the tokens in memory intact and the session fully
/// usable, which is exactly the bug that made Sign out appear to do nothing.
fn clear_cache() {
    let _ = fs::remove_file(cache_path());
}

/// Extract the `upn` (or `preferred_username`) claim from an ID token.
///
/// This is display-only — we never make a trust decision on it, so decoding the
/// payload without signature verification is fine.
fn account_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("upn")
        .or_else(|| claims.get("preferred_username"))
        .or_else(|| claims.get("email"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut lookup = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }

    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Base64url without padding, as OAuth requires.
fn base64url_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let indices = [n >> 18, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        // Emit only the characters the input actually filled.
        for index in indices.iter().take(chunk.len() + 1) {
            out.push(TABLE[*index as usize] as char);
        }
    }
    out
}

/// A random URL-safe string, for the PKCE verifier and the state parameter.
fn random_urlsafe(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    if getrandom::fill(&mut buffer).is_err() {
        // Falling back to a predictable value would defeat PKCE entirely, so
        // return empty and let the exchange fail loudly instead.
        return String::new();
    }
    base64url_encode(&buffer)
}

/// The S256 challenge derived from a PKCE verifier.
fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url_encode(&digest)
}

/// Pull one parameter out of a query string.
fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| urlencoding::decode(value).ok().map(|v| v.into_owned()))?
    })
}

/// The page the browser lands on once Entra redirects back.
fn landing_page(title: &str, detail: &str) -> String {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"font-family:-apple-system,Segoe UI,sans-serif;padding:3rem;color:#222\">\
         <h2>{title}</h2><p>{detail}</p></body></html>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Wait for Entra to redirect the browser back with an authorization code.
///
/// Rejects any response whose `state` does not match the one we sent, which is
/// what stops a third party from feeding us a code of their choosing.
async fn await_redirect(listener: &TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .context("accepting the sign-in redirect")?;

        // The request line carries everything we need; 8 KiB is far more than
        // any redirect requires and bounds what a stray client can send.
        let mut buffer = vec![0u8; 8192];
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();

        let Some(target) = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
        else {
            continue;
        };
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");

        if let Some(error) = query_param(query, "error") {
            let description = query_param(query, "error_description").unwrap_or_default();
            let page = landing_page(
                "Sign-in failed",
                "You can close this tab and return to Graphical Cloud Manager.",
            );
            let _ = stream.write_all(page.as_bytes()).await;
            let _ = stream.shutdown().await;
            bail!("{}", describe(&error, Some(&description)));
        }

        let Some(code) = query_param(query, "code") else {
            // Browsers ask for /favicon.ico and similar; ignore and keep waiting.
            let page = landing_page("Waiting for sign-in", "Nothing to see here yet.");
            let _ = stream.write_all(page.as_bytes()).await;
            let _ = stream.shutdown().await;
            continue;
        };

        let state = query_param(query, "state").unwrap_or_default();
        if state != expected_state {
            let page = landing_page(
                "Sign-in rejected",
                "The response did not match this sign-in attempt.",
            );
            let _ = stream.write_all(page.as_bytes()).await;
            let _ = stream.shutdown().await;
            bail!("the sign-in response did not match this request (state mismatch)");
        }

        let page = landing_page(
            "Signed in",
            "You can close this tab and return to Graphical Cloud Manager.",
        );
        let _ = stream.write_all(page.as_bytes()).await;
        let _ = stream.shutdown().await;
        return Ok(code);
    }
}

/// Whether an Entra failure means "this tenant will not grant these
/// permissions", as opposed to something transient.
///
/// Adding a scope the tenant has not consented to, or that requires an admin
/// who has not approved it, fails here — and used to take the whole sign-in
/// down with it. Recognising the shape lets us retry read-only.
fn is_permission_refusal(text: &str) -> bool {
    let lowered = text.to_lowercase();
    // Deliberately narrow. A broad match would quietly drop the console to
    // read-only on failures that have nothing to do with scopes — Conditional
    // Access, app assignment — hiding the real cause behind a second sign-in.
    [
        "aadsts65001", // no consent recorded for these permissions
        "aadsts90094", // admin consent required
        "aadsts70011", // the scope value is not valid
        "aadsts65005", // permission not configured on the registration
        "invalid_scope",
        "consent_required",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// Plain-English guidance for the Entra failures a first-time setup actually
/// hits.
///
/// Entra's own text describes the protocol violation rather than the mistake —
/// `AADSTS7000218` reads as "the request body must contain 'client_secret'",
/// which invites exactly the wrong fix, since a desktop app must not carry one.
/// Matching on the AADSTS code lets the sign-in screen name the setting to
/// change instead.
fn hint_for(description: &str) -> Option<&'static str> {
    const HINTS: &[(&str, &str)] = &[
        (
            "AADSTS7000218",
            "The app registration is set up as a confidential client, so Entra is \
             demanding a secret. gcm is a desktop app and deliberately holds no \
             secret.\n\nFix: App registrations → your app → Authentication → \
             Advanced settings → turn on \"Allow public client flows\", then save.",
        ),
        (
            "AADSTS700016",
            "The tenant has no application with this client ID.\n\nCheck `client` \
             in your config, and that the registration lives in the tenant named \
             by `tenant`.",
        ),
        (
            "AADSTS90002",
            "Entra does not recognise this tenant.\n\nCheck `tenant` in your \
             config — it takes a directory GUID or a verified domain such as \
             contoso.onmicrosoft.com.",
        ),
        (
            "AADSTS65001",
            "Nobody has consented to the permissions gcm asks for.\n\nFix: App \
             registrations → your app → API permissions → Grant admin consent.",
        ),
        (
            "AADSTS50020",
            "The account you signed in with belongs to a different tenant than the \
             one in your config.",
        ),
        (
            "AADSTS50076",
            "Multi-factor authentication is required for this account, and the \
             device code flow could not satisfy it.",
        ),
        (
            "AADSTS53003",
            "A Conditional Access policy blocked this sign-in. A policy requiring a \
             compliant or hybrid-joined device will reject the device code flow.",
        ),
        (
            "AADSTS900144",
            "Entra rejected the request as malformed. If you edited the `[cloud]` \
             section, check the authority URL.",
        ),
    ];

    HINTS
        .iter()
        .find(|(code, _)| description.contains(code))
        .map(|(_, hint)| *hint)
}

/// Render an Entra error, appending guidance when we recognise the code.
fn describe(error: &str, description: Option<&str>) -> String {
    let description = description.unwrap_or_default();
    let base = if description.is_empty() {
        error.to_string()
    } else {
        format!("{error}: {description}")
    };

    match hint_for(description) {
        Some(hint) => format!("{base}\n\n— {hint}"),
        None => base,
    }
}

/// Turn a non-success token response into a described error.
///
/// Reads the body rather than relying on the status alone: Entra puts the
/// AADSTS code only in the body, so `error_for_status` would discard the one
/// piece of information worth showing.
async fn error_from(response: reqwest::Response, context: &str) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    match serde_json::from_str::<ErrorResponse>(&body) {
        Ok(err) => anyhow!(
            "{}",
            describe(&err.error, err.error_description.as_deref())
        ),
        Err(_) => {
            let excerpt: String = body.chars().take(300).collect();
            anyhow!("{context} failed ({}): {excerpt}", status.as_u16())
        }
    }
}

/// Holds credentials for the session and hands out fresh access tokens.
pub struct Authenticator {
    config: Config,
    http: reqwest::Client,
    token: Option<Token>,
    /// Whether the tenant granted the write scopes. False means the console
    /// runs read-only and write mode cannot be armed at all.
    writes_available: bool,
    /// Set by [`Self::forget`], and the reason Sign out is worth pressing.
    ///
    /// Without it Entra re-issues silently from the browser's own session
    /// cookie: the redirect completes before anyone can read it, and the
    /// operator lands back in the same account having apparently done nothing.
    /// See [`Self::auth_code_flow`].
    force_account_picker: bool,
}

impl Authenticator {
    pub fn new(config: Config, http: reqwest::Client) -> Self {
        Self {
            config,
            http,
            token: None,
            writes_available: true,
            force_account_picker: false,
        }
    }

    /// Forget the signed-in identity completely, and ask who they are next time.
    ///
    /// Three things, all of which Sign out needs and only the first of which it
    /// used to do:
    ///
    /// * the refresh token cached on disk, so a later launch does not resume;
    /// * the tokens held **in memory**, without which the console kept a fully
    ///   working session — every read still succeeded, and a cancelled or
    ///   failed re-sign-in left it that way;
    /// * a flag asking Entra for the account picker, because otherwise the
    ///   browser's existing session signs the same person straight back in.
    pub fn forget(&mut self) {
        clear_cache();
        self.token = None;
        // The next sign-in decides this afresh; carrying the old answer over
        // would let a read-only session poison a subsequent privileged one.
        self.writes_available = true;
        self.force_account_picker = true;
    }

    /// Whether this session may attempt writes at all.
    pub fn writes_available(&self) -> bool {
        self.writes_available
    }

    /// UPN of the signed-in account, once known.
    pub fn account(&self) -> Option<String> {
        self.token.as_ref().and_then(|t| t.account.clone())
    }

    /// Sign in, preferring a cached refresh token over prompting the user.
    ///
    /// `progress` receives either [`AuthProgress::Silent`] or the device code
    /// the user needs to type.
    pub async fn sign_in(&mut self, progress: mpsc::UnboundedSender<AuthProgress>) -> Result<()> {
        if let Some(cached) = load_cache() {
            let _ = progress.send(AuthProgress::Silent);
            match self.redeem_refresh_token(&cached.refresh_token).await {
                Ok(mut token) => {
                    // Entra omits the id_token on refresh unless openid scope is
                    // re-requested; fall back to the cached account name.
                    if token.account.is_none() {
                        token.account = cached.account.clone();
                    }
                    // A cached token predates this run, so believe what it
                    // actually carries rather than what we hoped to request.
                    self.writes_available = token.writes;
                    save_cache(&token);
                    self.token = Some(token);
                    return Ok(());
                }
                Err(_) => {
                    // Refresh token revoked, expired, or scopes changed. Fall
                    // through to a fresh interactive sign-in.
                    clear_cache();
                }
            }
        }

        // Ask for the write scopes first. If the tenant refuses them, fall back
        // to read-only rather than failing sign-in altogether — a console that
        // cannot change anything is far more use than one that will not open.
        match self.auth_code_flow(progress.clone(), true).await {
            Ok(token) => {
                save_cache(&token);
                self.token = Some(token);
                self.force_account_picker = false;
                Ok(())
            }
            Err(err) if is_permission_refusal(&format!("{err:#}")) => {
                self.writes_available = false;
                let token = self.auth_code_flow(progress, false).await.context(
                    "the tenant refused the write permissions, and the read-only \
                     retry also failed",
                )?;
                save_cache(&token);
                self.token = Some(token);
                self.force_account_picker = false;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Build the authorize URL.
    ///
    /// Split out from the flow because the one thing here that is easy to get
    /// wrong — whether Entra is asked for the account picker — is otherwise
    /// only observable by watching a browser.
    fn authorize_url(
        &self,
        redirect_uri: &str,
        challenge: &str,
        state: &str,
        include_writes: bool,
    ) -> String {
        let authority = self.config.authority_url();
        format!(
            "{authority}/oauth2/v2.0/authorize\
             ?client_id={client_id}\
             &response_type=code\
             &redirect_uri={redirect}\
             &response_mode=query\
             &scope={scope}\
             &state={state}\
             &code_challenge={challenge}\
             &code_challenge_method=S256{prompt}",
            client_id = encode(self.config.client_id()),
            redirect = encode(redirect_uri),
            scope = encode(&scopes(include_writes)),
            state = encode(state),
            challenge = encode(challenge),
            // Only after a deliberate sign-out. On a first run, or when a
            // refresh token has simply expired, silent single sign-on is the
            // desirable behaviour and prompting would be a regression.
            prompt = if self.force_account_picker {
                "&prompt=select_account"
            } else {
                ""
            },
        )
    }

    /// A currently-valid access token, refreshing transparently if needed.
    pub async fn access_token(&mut self) -> Result<String> {
        if let Some(token) = &self.token
            && token.is_valid()
        {
            return Ok(token.access_token.clone());
        }

        let refresh_token = self
            .token
            .as_ref()
            .and_then(|t| t.refresh_token.clone())
            .ok_or_else(|| anyhow!("not signed in"))?;

        let mut token = self.redeem_refresh_token(&refresh_token).await?;
        if token.account.is_none() {
            token.account = self.token.as_ref().and_then(|t| t.account.clone());
        }
        save_cache(&token);
        let access = token.access_token.clone();
        self.token = Some(token);
        Ok(access)
    }

    /// Authorization code flow with PKCE, through the operator's own browser.
    ///
    /// The browser runs on this machine, so Entra sees the real device and any
    /// Conditional Access policy keyed on device state can evaluate it. The
    /// device code flow could not do that — the browser typing the code and the
    /// application receiving the token are different devices as far as Entra is
    /// concerned, which is why device-based policies reject it outright.
    ///
    /// PKCE replaces the client secret a public client cannot keep: the code is
    /// only redeemable by whoever generated the verifier.
    async fn auth_code_flow(
        &self,
        progress: mpsc::UnboundedSender<AuthProgress>,
        include_writes: bool,
    ) -> Result<Token> {
        let authority = self.config.authority_url();

        // Port 0 lets the OS choose, so two copies of gcm cannot collide. Entra
        // permits any port on a registered `http://localhost` redirect.
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("opening a local port to receive the sign-in redirect")?;
        let port = listener
            .local_addr()
            .context("reading the local redirect port")?
            .port();
        let redirect_uri = format!("http://localhost:{port}");

        let verifier = random_urlsafe(64);
        let challenge = pkce_challenge(&verifier);
        let state = random_urlsafe(24);

        let url = self.authorize_url(&redirect_uri, &challenge, &state, include_writes);

        // Opening the browser can fail silently on a headless session, so the
        // URL is handed to the UI either way.
        let _ = open::that_detached(&url);
        let _ = progress.send(AuthProgress::AwaitingBrowser { url: url.clone() });

        let code = tokio::time::timeout(
            Duration::from_secs(300),
            await_redirect(&listener, &state),
        )
        .await
        .context("timed out waiting for the browser to complete sign-in")??;

        let response = self
            .http
            .post(format!("{authority}/oauth2/v2.0/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.config.client_id()),
                ("code", code.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("code_verifier", verifier.as_str()),
            ])
            .send()
            .await
            .context("exchanging the authorization code")?;

        if !response.status().is_success() {
            return Err(error_from(response, "exchanging the authorization code").await);
        }

        let body: TokenResponse = response.json().await.context("parsing token response")?;
        Ok(self.to_token(body, include_writes))
    }

    async fn redeem_refresh_token(&self, refresh_token: &str) -> Result<Token> {
        let response = self
            .http
            .post(format!("{}/oauth2/v2.0/token", self.config.authority_url()))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.config.client_id()),
                ("refresh_token", refresh_token),
                ("scope", scopes(self.writes_available).as_str()),
            ])
            .send()
            .await
            .context("redeeming refresh token")?;

        if !response.status().is_success() {
            return Err(error_from(response, "redeeming the refresh token").await);
        }

        let body: TokenResponse = response.json().await.context("parsing token response")?;
        Ok(self.to_token(body, self.writes_available))
    }

    fn to_token(&self, body: TokenResponse, writes: bool) -> Token {
        Token {
            access_token: body.access_token,
            expires_at: Utc::now() + ChronoDuration::seconds(body.expires_in),
            refresh_token: body.refresh_token,
            account: body.id_token.as_deref().and_then(account_from_id_token),
            writes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authenticator() -> Authenticator {
        let config = Config {
            application: crate::config::Application {
                client: "test-client".into(),
                tenant: "contoso.onmicrosoft.com".into(),
            },
            cloud: Default::default(),
            query: Default::default(),
            mariadb: None,
        };
        Authenticator::new(config, reqwest::Client::new())
    }

    #[test]
    fn an_ordinary_sign_in_does_not_force_the_account_picker() {
        // Silent single sign-on is the right behaviour on a first run and after
        // a refresh token simply expires; prompting there would be a
        // regression, not a fix.
        let auth = authenticator();
        let url = auth.authorize_url("http://localhost:1234", "challenge", "state", true);
        assert!(!url.contains("prompt="), "got: {url}");
    }

    #[test]
    fn signing_out_forces_the_account_picker() {
        // Without this Entra reissues from the browser's own session cookie and
        // the operator lands back in the same account, which is precisely why
        // Sign out looked like it did nothing.
        let mut auth = authenticator();
        auth.forget();
        let url = auth.authorize_url("http://localhost:1234", "challenge", "state", true);
        assert!(url.contains("&prompt=select_account"), "got: {url}");
    }

    #[test]
    fn signing_out_drops_the_token_held_in_memory() {
        // Deleting the cache file alone left a fully working session: every
        // read still succeeded, and a cancelled re-sign-in left it that way.
        let mut auth = authenticator();
        auth.token = Some(Token {
            access_token: "secret".into(),
            refresh_token: Some("secret".into()),
            expires_at: Utc::now() + ChronoDuration::hours(1),
            account: Some("someone@contoso.co.uk".into()),
            writes: true,
        });
        assert!(auth.account().is_some());

        auth.forget();

        assert!(auth.token.is_none(), "the in-memory token must not survive");
        assert!(auth.account().is_none(), "the console must forget who it was");
    }

    #[test]
    fn the_authorize_url_still_carries_pkce_and_state() {
        // The prompt parameter is appended to the end of the format string, so
        // this guards against it having eaten the parameter before it.
        let url = authenticator().authorize_url("http://localhost:1234", "chal", "st", true);
        assert!(url.contains("code_challenge=chal"), "got: {url}");
        assert!(url.contains("code_challenge_method=S256"), "got: {url}");
        assert!(url.contains("&state=st"), "got: {url}");
        assert!(url.contains("response_type=code"), "got: {url}");
    }

    #[test]
    fn decodes_base64url_without_padding() {
        // {"upn":"a@b.com"}
        let encoded = "eyJ1cG4iOiJhQGIuY29tIn0";
        let decoded = base64url_decode(encoded).expect("should decode");
        assert_eq!(decoded, br#"{"upn":"a@b.com"}"#);
    }

    #[test]
    fn extracts_upn_from_id_token() {
        let token = "header.eyJ1cG4iOiJhQGIuY29tIn0.signature";
        assert_eq!(account_from_id_token(token).as_deref(), Some("a@b.com"));
    }

    #[test]
    fn falls_back_to_preferred_username() {
        // {"preferred_username":"c@d.com"}
        let token = "h.eyJwcmVmZXJyZWRfdXNlcm5hbWUiOiJjQGQuY29tIn0.s";
        assert_eq!(account_from_id_token(token).as_deref(), Some("c@d.com"));
    }

    #[test]
    fn rejects_malformed_id_token() {
        assert_eq!(account_from_id_token("not-a-jwt"), None);
    }

    #[test]
    fn base64url_encoding_is_unpadded_and_url_safe() {
        assert_eq!(base64url_encode(b""), "");
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
        // No padding, and none of the characters that would need escaping.
        let encoded = base64url_encode(&[251, 255, 190]);
        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn encode_and_decode_round_trip() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let encoded = base64url_encode(&bytes);
        assert_eq!(base64url_decode(&encoded).expect("should decode"), bytes);
    }

    /// The worked example from RFC 7636 appendix B, which pins the challenge
    /// derivation against the spec rather than against our own arithmetic.
    #[test]
    fn pkce_challenge_matches_the_rfc_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifiers_are_long_and_unpredictable() {
        let a = random_urlsafe(64);
        let b = random_urlsafe(64);
        // RFC 7636 requires 43..=128 characters.
        assert!(a.len() >= 43 && a.len() <= 128, "length was {}", a.len());
        assert_ne!(a, b);
    }

    #[test]
    fn query_parameters_are_decoded() {
        let query = "code=abc%2Fdef&state=xyz";
        assert_eq!(query_param(query, "code").as_deref(), Some("abc/def"));
        assert_eq!(query_param(query, "state").as_deref(), Some("xyz"));
        assert_eq!(query_param(query, "missing"), None);
    }

    #[test]
    fn conditional_access_failures_are_not_mistaken_for_consent() {
        // 530035 is a Conditional Access block. Treating it as a scope refusal
        // would silently drop the console to read-only and hide the real cause.
        assert!(!is_permission_refusal("AADSTS530035: blocked by policy"));
        assert!(!is_permission_refusal("AADSTS53003: blocked by Conditional Access"));
        // Nor app assignment, which a read-only retry would fail at too.
        assert!(!is_permission_refusal("AADSTS50105: not assigned to a role"));
        assert!(is_permission_refusal("AADSTS65001: no consent recorded"));
        assert!(is_permission_refusal("AADSTS70011: scope is not valid"));
    }

    #[test]
    fn explains_the_confidential_client_error() {
        // The failure a first-time setup almost always hits.
        let description = "AADSTS7000218: The request body must contain the following \
                           parameter: 'client_assertion' or 'client_secret'.";
        let message = describe("invalid_client", Some(description));

        assert!(message.contains("Allow public client flows"));
        // The raw Entra text is kept — the trace IDs in it are what support asks for.
        assert!(message.contains("AADSTS7000218"));
        // And it must not send the user off to add a secret.
        assert!(!message.contains("add a client secret"));
    }

    #[test]
    fn explains_a_wrong_tenant() {
        let message = describe("invalid_request", Some("AADSTS90002: Tenant not found"));
        assert!(message.contains("verified domain"));
    }

    #[test]
    fn unrecognised_errors_pass_through_unchanged() {
        let message = describe("weird_error", Some("AADSTS99999: Something new"));
        assert_eq!(message, "weird_error: AADSTS99999: Something new");
    }

    #[test]
    fn copes_with_a_missing_description() {
        assert_eq!(describe("invalid_grant", None), "invalid_grant");
    }

    #[test]
    fn every_hint_matches_only_its_own_code() {
        // A substring collision would attach the wrong advice to a real failure.
        let codes = [
            "AADSTS7000218",
            "AADSTS700016",
            "AADSTS90002",
            "AADSTS65001",
            "AADSTS50020",
            "AADSTS50076",
            "AADSTS53003",
            "AADSTS900144",
        ];
        for code in codes {
            let hint = hint_for(code).unwrap_or_else(|| panic!("{code} has no hint"));
            for other in codes {
                if other != code {
                    assert_ne!(hint, hint_for(other).unwrap(), "{code} collides with {other}");
                }
            }
        }
    }
}
