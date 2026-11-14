//! Huawei HTTP session: one cookie-holding client, the single-use CSRF token queue and the
//! SCRAM login (spec §5.1–§5.3).
//!
//! Never logs or formats the password, tokens, nonces or proofs. Login never retries on its
//! own: one failed password attempt is returned to the caller so a loop can't lock the admin
//! account. (A refused token on the challenge is resent once; it precedes any credential check.)

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use quick_xml::escape::escape;
use reqwest::header::{HeaderMap, CONTENT_TYPE};
use reqwest::{Client, StatusCode, Url};

use super::scram::ScramClient;
use super::xml;
use crate::{ApiErrorKind, Credentials, LoginGate, RouterError, RouterIdentity, SessionState};

const GET_TIMEOUT: Duration = Duration::from_secs(5);
const POST_TIMEOUT: Duration = Duration::from_secs(10);
const POST_CONTENT_TYPE: &str = "application/x-www-form-urlencoded; charset=UTF-8";
const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

const TOKEN_HEADER: &str = "__RequestVerificationToken";
const TOKEN_HEADER_ONE: &str = "__RequestVerificationTokenone";
const TOKEN_HEADER_TWO: &str = "__RequestVerificationTokentwo";

/// The web UI page. The H122 issues its `SessionID` cookie only here; API tokens fetched
/// without that cookie are refused with 125003.
const PATH_WEB_PAGE: &str = "/";
const PATH_TOKEN: &str = "/api/webserver/token";
const PATH_STATE_LOGIN: &str = "/api/user/state-login";
const PATH_CHALLENGE: &str = "/api/user/challenge_login";
const PATH_AUTH: &str = "/api/user/authentication_login";

/// PBKDF2 iterations accepted from the router (1000 observed). Below the floor, a device at the
/// router's address could ask for a proof that's cheap to guess the password from.
const MIN_ITERATIONS: u32 = 1_000;
const MAX_ITERATIONS: u32 = 1_000_000;

pub struct HuaweiSession {
    /// Replaced by `forget`, which drops the cookie jar. Read through `client()`; the lock is
    /// never held across an await.
    client: RwLock<Client>,
    base: Url,
    tokens: Mutex<VecDeque<String>>,
}

impl fmt::Debug for HuaweiSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HuaweiSession")
            .field("base", &self.base.as_str())
            .field("queued_tokens", &self.queued_tokens())
            .finish_non_exhaustive()
    }
}

fn map_transport(e: reqwest::Error) -> RouterError {
    if e.is_timeout() {
        RouterError::Timeout
    } else {
        RouterError::Unreachable
    }
}

/// Login-specific mapping of router error codes (spec §5.3 step 7). Any other router error
/// during the login exchange is `LoginRejected`, which the actor never retries on its own: an
/// unrecognised refusal must not turn into a loop that locks the admin account.
fn map_login_error(e: RouterError) -> RouterError {
    match e {
        RouterError::Api { kind, code } => match kind {
            ApiErrorKind::BadCredentials => RouterError::BadCredentials,
            ApiErrorKind::AlreadyLoggedIn => RouterError::SessionTakenOver,
            ApiErrorKind::TooManyAttempts => RouterError::LockedOut { wait_s: 0 },
            _ => RouterError::LoginRejected { code },
        },
        other => other,
    }
}

/// Mapping for every request outside the login exchange: "no rights" (100003) is what the
/// router answers once our login is gone (expired, or replaced by the web UI), so it is a
/// session loss and leads to a re-login. So is a token error that survives `post`'s retry, or
/// any token error on a GET (the H122 answers data GETs with 125002 once the session expires).
fn map_data_error(e: RouterError) -> RouterError {
    match e {
        RouterError::Api {
            kind: ApiErrorKind::NoRights | ApiErrorKind::Token,
            code,
        } => RouterError::SessionLost { code: Some(code) },
        other => other,
    }
}

/// One cookie-holding client per session, never following redirects or using a proxy.
fn build_client() -> Result<Client, RouterError> {
    Client::builder()
        .cookie_store(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| RouterError::Validation {
            message: "HTTP client setup failed".to_string(),
        })
}

pub(crate) fn wrap_request(inner_xml: &str) -> String {
    format!("{XML_HEADER}<request>{inner_xml}</request>")
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    // HeaderMap lookups are case-insensitive.
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Tokens carried by a response, in queue order: `one`/`two` if present, otherwise the
/// `#`-joined pool in `__RequestVerificationToken`.
fn harvest_tokens(headers: &HeaderMap) -> Vec<String> {
    let one = header_value(headers, TOKEN_HEADER_ONE);
    let two = header_value(headers, TOKEN_HEADER_TWO);
    if one.is_some() || two.is_some() {
        return one.into_iter().chain(two).map(str::to_string).collect();
    }
    header_value(headers, TOKEN_HEADER)
        .map(|pool| {
            pool.split('#')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Checks the body for `<error>` first (whatever the status), then the HTTP status.
fn check_response(status: StatusCode, body: &str) -> Result<(), RouterError> {
    xml::check_error(body)?;
    if !status.is_success() {
        return Err(RouterError::Parse {
            message: format!("unexpected HTTP status {}", status.as_u16()),
        });
    }
    Ok(())
}

struct RawResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

/// Far more than any real response, and small enough that a device on the network can't fill the
/// PC's memory with one.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

impl HuaweiSession {
    pub fn new(base_url: &str) -> Result<Self, RouterError> {
        let invalid = || RouterError::Validation {
            message: "invalid router address".to_string(),
        };
        let base = Url::parse(base_url).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(invalid());
        }
        Ok(Self {
            client: RwLock::new(build_client()?),
            base,
            tokens: Mutex::new(VecDeque::new()),
        })
    }

    /// The current client (a cheap handle to the shared one).
    fn client(&self) -> Client {
        self.client
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Forgets the session: the queued tokens and the cookie jar (with the `SessionID`), so
    /// the next request starts as a stranger to the router. Sends nothing. A client that
    /// can't be rebuilt keeps the old one, whose session the router has ended anyway.
    pub fn forget(&self) {
        self.flush_tokens();
        if let Ok(fresh) = build_client() {
            *self.client.write().unwrap_or_else(|p| p.into_inner()) = fresh;
        }
    }

    /// Number of unused CSRF tokens in the queue.
    pub fn queued_tokens(&self) -> usize {
        self.lock_tokens().len()
    }

    fn lock_tokens(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        // A poisoned queue only ever holds strings; keep using it.
        self.tokens.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn flush_tokens(&self) {
        self.lock_tokens().clear();
    }

    fn push_tokens(&self, headers: &HeaderMap) {
        let fresh = harvest_tokens(headers);
        if !fresh.is_empty() {
            self.lock_tokens().extend(fresh);
        }
    }

    fn url(&self, path: &str) -> Result<Url, RouterError> {
        self.base.join(path).map_err(|_| RouterError::Validation {
            message: "invalid request path".to_string(),
        })
    }

    fn huawei_headers(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("X-Requested-With", "XMLHttpRequest")
            .header("_ResponseSource", "Broswer")
    }

    async fn read(resp: reqwest::Response) -> Result<RawResponse, RouterError> {
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = Self::read_bytes(resp).await?;
        Ok(RawResponse {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// The body as sent, up to `MAX_RESPONSE_BYTES`.
    async fn read_bytes(mut resp: reqwest::Response) -> Result<Vec<u8>, RouterError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(map_transport)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_RESPONSE_BYTES {
                return Err(RouterError::Parse {
                    message: "the router's response is too large".to_string(),
                });
            }
        }
        Ok(bytes)
    }

    /// A web UI file as a browser loads it: a plain GET with the session cookie, without the API
    /// headers or token, never following a redirect. The bytes come back as sent (a gzip body
    /// stays packed) with no `<error>` check. A redirect is the router sending us to its login
    /// page (`page_scripts::PAGE_NOT_LOGGED_IN`); 404 is `PAGE_NOT_FOUND`. For the read-only
    /// page-script saver only; the caller checks the path.
    pub async fn get_page(&self, path: &str) -> Result<Vec<u8>, RouterError> {
        let url = self.url(path)?;
        let resp = self
            .client()
            .get(url)
            .timeout(GET_TIMEOUT)
            .send()
            .await
            .map_err(map_transport)?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(RouterError::Validation {
                message: crate::page_scripts::PAGE_NOT_LOGGED_IN.to_string(),
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(RouterError::Validation {
                message: crate::page_scripts::PAGE_NOT_FOUND.to_string(),
            });
        }
        if !status.is_success() {
            return Err(RouterError::Parse {
                message: format!("unexpected HTTP status {}", status.as_u16()),
            });
        }
        Self::read_bytes(resp).await
    }

    /// GET with the Huawei headers; the body is checked for `<error>`. "No rights" (100003)
    /// is `SessionLost`.
    pub async fn get(&self, path: &str) -> Result<String, RouterError> {
        self.get_raw(path).await.map_err(map_data_error)
    }

    /// `get` with its own timeout, for a call the router takes long to answer (a manual network
    /// search). Mapped like `get`.
    pub async fn get_with_timeout(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<String, RouterError> {
        self.get_raw_with(path, timeout)
            .await
            .map_err(map_data_error)
    }

    /// `get` for a page a router may not have (the privacy check's reads: two answered on the
    /// H122 on 2026-11-13, `wlan/host-list` hasn't been asked for). An HTTP error status is the likeliest answer from a router without
    /// the page, so it comes back as an API error with the status as its code, whatever the body
    /// holds: "not supported" for 404 and a plain refusal for any other. Otherwise as `get`.
    pub async fn get_optional(&self, path: &str) -> Result<String, RouterError> {
        let url = self.url(path)?;
        let resp = Self::huawei_headers(self.client().get(url))
            .timeout(GET_TIMEOUT)
            .send()
            .await
            .map_err(map_transport)?;
        let raw = Self::read(resp).await?;
        if !raw.status.is_success() {
            let kind = if raw.status == StatusCode::NOT_FOUND {
                ApiErrorKind::NotSupported
            } else {
                ApiErrorKind::Unknown
            };
            return Err(RouterError::Api {
                code: u32::from(raw.status.as_u16()),
                kind,
            });
        }
        check_response(raw.status, &raw.body).map_err(map_data_error)?;
        Ok(raw.body)
    }

    /// GET without the session-loss mapping (login exchange and token fetches).
    async fn get_raw(&self, path: &str) -> Result<String, RouterError> {
        self.get_raw_with(path, GET_TIMEOUT).await
    }

    async fn get_raw_with(&self, path: &str, timeout: Duration) -> Result<String, RouterError> {
        let url = self.url(path)?;
        let resp = Self::huawei_headers(self.client().get(url))
            .timeout(timeout)
            .send()
            .await
            .map_err(map_transport)?;
        let raw = Self::read(resp).await?;
        check_response(raw.status, &raw.body)?;
        Ok(raw.body)
    }

    /// Loads the web page so the router issues a `SessionID` cookie, as the browser does before
    /// any API call. A valid cookie is kept by the router; an unknown one is replaced. Only a
    /// transport failure is an error: the page body and status don't matter.
    async fn open_web_session(&self) -> Result<(), RouterError> {
        let url = self.url(PATH_WEB_PAGE)?;
        let resp = self
            .client()
            .get(url)
            .timeout(GET_TIMEOUT)
            .send()
            .await
            .map_err(map_transport)?;
        Self::read(resp).await.map(|_| ())
    }

    /// Takes the next token, or fetches one from `/api/webserver/token` if the queue is empty.
    async fn take_token(&self) -> Result<String, RouterError> {
        let queued = self.lock_tokens().pop_front();
        if let Some(t) = queued {
            return Ok(t);
        }
        let body = self.get_raw(PATH_TOKEN).await?;
        let (_, m) = xml::flat(&body, &["token"])?;
        let token = xml::get(&m, "token");
        if token.is_empty() || !token.is_ascii() {
            return Err(RouterError::TokenExhausted);
        }
        // The last 32 characters are the usable token.
        Ok(token[token.len().saturating_sub(32)..].to_string())
    }

    /// Sends one POST with a fresh token. Doesn't harvest or check the body.
    async fn send_post(&self, path: &str, inner_xml: &str) -> Result<RawResponse, RouterError> {
        let url = self.url(path)?;
        let token = self.take_token().await?;
        let resp = Self::huawei_headers(self.client().post(url))
            .header(CONTENT_TYPE, POST_CONTENT_TYPE)
            .header(TOKEN_HEADER, token)
            .timeout(POST_TIMEOUT)
            .body(wrap_request(inner_xml))
            .send()
            .await
            .map_err(map_transport)?;
        Self::read(resp).await
    }

    /// One POST: consumes a token, harvests the response's tokens, checks the body.
    async fn post_once(&self, path: &str, inner_xml: &str) -> Result<String, RouterError> {
        let raw = self.send_post(path, inner_xml).await?;
        self.push_tokens(&raw.headers);
        check_response(raw.status, &raw.body)?;
        Ok(raw.body)
    }

    /// POST `inner_xml` wrapped in `<request>`. On a token error the queue is flushed and the
    /// request is retried once with a fresh token; a second token error is `SessionLost`, and so
    /// is "no rights" (100003).
    pub async fn post(&self, path: &str, inner_xml: &str) -> Result<String, RouterError> {
        self.post_retrying(path, inner_xml)
            .await
            .map_err(map_data_error)
    }

    async fn post_retrying(&self, path: &str, inner_xml: &str) -> Result<String, RouterError> {
        match self.post_once(path, inner_xml).await {
            Err(RouterError::Api {
                kind: ApiErrorKind::Token,
                ..
            }) => {
                self.flush_tokens();
                match self.post_once(path, inner_xml).await {
                    Err(RouterError::Api {
                        kind: ApiErrorKind::Token,
                        code,
                    }) => {
                        self.flush_tokens();
                        Err(RouterError::SessionLost { code: Some(code) })
                    }
                    other => other,
                }
            }
            other => other,
        }
    }

    /// SCRAM login (spec §5.3 steps 1–6, after opening a web session). Makes at most one
    /// password attempt; only the challenge is resent, once, after a refused token.
    pub async fn login(&self, creds: &Credentials) -> Result<(), RouterError> {
        self.login_gated(creds, &LoginGate::Open).await.map(|_| ())
    }

    /// `login` that stops before the proof when `gate` refuses the router it finds. Returns the
    /// router's identity once its signature has verified; `None` if already logged in.
    pub async fn login_gated(
        &self,
        creds: &Credentials,
        gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        self.login_with_scram_gated(creds, ScramClient::new(), gate)
            .await
    }

    /// `login` with a caller-supplied SCRAM client, so tests can fix the nonce.
    #[doc(hidden)]
    pub async fn login_with_scram(
        &self,
        creds: &Credentials,
        scram: ScramClient,
    ) -> Result<(), RouterError> {
        self.login_with_scram_gated(creds, scram, &LoginGate::Open)
            .await
            .map(|_| ())
    }

    #[doc(hidden)]
    pub async fn login_with_scram_gated(
        &self,
        creds: &Credentials,
        scram: ScramClient,
        gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        // 0. Get a web session cookie first, or the router refuses our tokens (125003).
        self.open_web_session().await?;

        // 1. Never attempt a login while the router reports a lockout.
        let state = self.get_raw(PATH_STATE_LOGIN).await?;
        match xml::parse_state_login(&state)? {
            SessionState::Locked { wait_s } => return Err(RouterError::LockedOut { wait_s }),
            SessionState::LoggedIn => return Ok(None),
            SessionState::LoggedOut => {}
        }

        // 2. Tokens from before the login are stale; start from /api/webserver/token.
        self.flush_tokens();

        // 3. Challenge.
        let challenge = format!(
            "<username>{}</username><firstnonce>{}</firstnonce><mode>1</mode>",
            escape(creds.username.as_str()),
            scram.first_nonce()
        );
        let body = match self.post_once(PATH_CHALLENGE, &challenge).await {
            // The token is checked before any credential, so a refusal here (125003 on the
            // first login of a fresh process) is not a failed login: refresh the web session
            // and token once and resend. The proof step is never retried.
            Err(RouterError::Api {
                kind: ApiErrorKind::Token,
                ..
            }) => {
                self.open_web_session().await?;
                self.flush_tokens();
                self.post_once(PATH_CHALLENGE, &challenge).await
            }
            other => other,
        }
        .map_err(map_login_error)?;
        let (_, m) = xml::flat(
            &body,
            &["salt", "servernonce", "iterations", "modeselected"],
        )?;
        let mode = xml::get(&m, "modeselected");
        if !mode.is_empty() && mode != "1" {
            return Err(RouterError::LoginAlgorithmMismatch);
        }
        let iterations: u32 = xml::get(&m, "iterations")
            .parse()
            .map_err(|_| RouterError::LoginAlgorithmMismatch)?;
        if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
            return Err(RouterError::LoginAlgorithmMismatch);
        }

        // Nothing derived from the password has left the app yet: the last point to stop.
        let seen = RouterIdentity::new(xml::get(&m, "salt"), iterations);
        gate.check(&seen)?;

        // 4. Proof.
        let proof = scram.prove(
            creds.password(),
            xml::get(&m, "salt"),
            iterations,
            xml::get(&m, "servernonce"),
        )?;

        // 5. Authentication; tokens are kept only once the server signature verifies.
        let auth = format!(
            "<clientproof>{}</clientproof><finalnonce>{}</finalnonce>",
            proof.client_proof_hex,
            escape(proof.final_nonce.as_str())
        );
        let raw = self.send_post(PATH_AUTH, &auth).await?;
        check_response(raw.status, &raw.body).map_err(map_login_error)?;
        let (_, m) = xml::flat(&raw.body, &["serversignature"])?;
        if let Err(e) = proof.verify_server(xml::get(&m, "serversignature")) {
            self.flush_tokens();
            return Err(e);
        }

        // 6. Post-login tokens replace anything left over.
        self.flush_tokens();
        self.push_tokens(&raw.headers);
        Ok(Some(seen))
    }
}
