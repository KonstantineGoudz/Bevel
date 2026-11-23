//! Self-service signup, proxied through the trusted sim.
//!
//! Rauthy has no SMTP configured, so its open-registration page cannot deliver
//! the password-setup email. Instead the game posts here; the sim (which holds
//! a Rauthy admin API key server-side, never shipped to clients) creates the
//! user via the admin API, sets the password, and marks the email verified.
//! The client then logs straight in with the password grant.
//!
//! Served on `DUNGEON_SIGNUP_ADDR` (default `0.0.0.0:6565`), exposed publicly
//! as `https://play.conman.biz/signup` (same origin as the game, so WASM
//! needs no CORS). Optional invite gate via `DUNGEON_SIGNUP_CODE`.

use std::net::SocketAddr;

use axum::{
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub name: String,
    pub password: String,
    pub invite: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignupResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct SignupConfig {
    /// Rauthy base URL reachable from the sim, e.g. `http://rauthy:6080`.
    pub rauthy_internal: String,
    /// Full `name$secret` API key pair (goes after `API-Key `).
    pub api_key: Option<String>,
    /// Optional invite code; when set (non-empty), requests must match it.
    pub invite_code: Option<String>,
    /// OIDC client id + secret for the password/refresh grants. Held here so
    /// a **confidential** client works without shipping the secret in the
    /// game binary. Optional: when absent the client talks to Rauthy directly
    /// (only viable for a public client).
    pub client_id: String,
    pub client_secret: Option<String>,
    pub bind_addr: SocketAddr,
}

impl SignupConfig {
    pub fn from_env() -> Self {
        // Rauthy API keys are `name$secret`, and compose interpolates `$` in
        // env_file values — the .env therefore escapes it as `$$`. Undo that
        // so both the escaped and the literal form work.
        let api_key = std::env::var("RAUTHY_API_KEY")
            .or_else(|_| std::env::var("DUNGEON_RAUTHY_API_KEY"))
            .ok()
            .map(|s| s.trim().replace("$$", "$"))
            .filter(|s| !s.is_empty());
        let invite_code = std::env::var("DUNGEON_SIGNUP_CODE")
            .ok()
            .filter(|s| !s.is_empty());
        let client_secret = std::env::var("RAUTHY_CLIENT_SECRET")
            .ok()
            .map(|s| s.trim().replace("$$", "$"))
            .filter(|s| !s.is_empty());
        let bind_addr: SocketAddr = std::env::var("DUNGEON_SIGNUP_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:6565".to_string())
            .parse()
            .expect("DUNGEON_SIGNUP_ADDR must be a socket addr");
        Self {
            rauthy_internal: std::env::var("DUNGEON_RAUTHY_INTERNAL")
                .unwrap_or_else(|_| "http://rauthy:6080".to_string()),
            api_key,
            invite_code,
            client_id: std::env::var("DUNGEON_RAUTHY_CLIENT_ID")
                .unwrap_or_else(|_| "party-dungeon".to_string()),
            client_secret,
            bind_addr,
        }
    }
}

pub fn validate_signup(req: &SignupRequest, invite_code: Option<&str>) -> Result<String, String> {
    let email = req.email.trim().to_lowercase();
    if !email.contains('@') || email.len() < 3 || email.len() > 254 {
        return Err("enter a valid email address".to_string());
    }
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err("pick a character name".to_string());
    }
    if name.chars().count() > 24 {
        return Err("character name must be 24 characters or less".to_string());
    }
    // Mirror Rauthy's password policy so a rejected password never leaves a
    // half-created account behind (see `provision_user`).
    if req.password.len() < 8 {
        return Err("password must be at least 8 characters".to_string());
    }
    if req.password.len() > 256 {
        return Err("password must be 256 characters or less".to_string());
    }
    if !req.password.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("password needs at least one uppercase letter".to_string());
    }
    if !req.password.chars().any(|c| c.is_ascii_lowercase()) {
        return Err("password needs at least one lowercase letter".to_string());
    }
    if !req.password.chars().any(|c| c.is_ascii_digit()) {
        return Err("password needs at least one digit".to_string());
    }
    if !req
        .password
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !c.is_whitespace())
    {
        return Err("password needs at least one special character".to_string());
    }
    if let Some(code) = invite_code {
        let given = req.invite.as_deref().unwrap_or("").trim();
        if given != code {
            return Err("invalid invite code".to_string());
        }
    }
    Ok(email)
}

fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    if let Ok(val) = serde_json::from_slice::<T>(body) {
        return Ok(val);
    }
    if let Ok(val) = serde_urlencoded::from_bytes::<T>(body) {
        return Ok(val);
    }
    serde_json::from_slice::<T>(body).map_err(|e| format!("invalid request payload: {e}"))
}

async fn signup_handler(
    axum::extract::State(cfg): axum::extract::State<SignupConfig>,
    body: axum::body::Bytes,
) -> (StatusCode, Json<SignupResponse>) {
    let req: SignupRequest = match parse_body(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SignupResponse {
                    ok: false,
                    error: Some(e),
                }),
            );
        }
    };
    let email = match validate_signup(&req, cfg.invite_code.as_deref()) {
        Ok(email) => email,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SignupResponse {
                    ok: false,
                    error: Some(e),
                }),
            )
        }
    };
    let name = req.name.trim().to_string();
    let Some(_api_key) = cfg.api_key.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SignupResponse {
                ok: false,
                error: Some("signup is not enabled on this server".to_string()),
            }),
        );
    };
    match provision_user(&cfg, &email, &name, &req.password).await {
        Ok(()) => (
            StatusCode::OK,
            Json(SignupResponse {
                ok: true,
                error: None,
            }),
        ),
        Err(e) => {
            // Don't leak internals; Rauthy already returns safe messages.
            eprintln!("signup failed for {email}: {e}");
            (
                StatusCode::BAD_GATEWAY,
                Json(SignupResponse {
                    ok: false,
                    error: Some(e),
                }),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct RauthyError {
    #[serde(default)]
    message: Option<String>,
}

fn rauthy_error(status: u16, body: &str) -> String {
    let raw = serde_json::from_str::<RauthyError>(body)
        .ok()
        .and_then(|e| e.message)
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("identity service rejected the request ({status})"));
    // Map Rauthy's internal wording to something a player can act on.
    let lower = raw.to_ascii_lowercase();
    let friendly = if lower.contains("resource temporarily unavailable") {
        "login is temporarily throttled — wait a moment and try again"
    } else if lower.contains("invalid user credentials") {
        "invalid email or password"
    } else if lower.contains("no rows returned") {
        "invalid email or password"
    } else if lower.contains("user is not enabled") || lower.contains("disabled") {
        "this account is disabled"
    } else if lower.contains("rate") || lower.contains("too many") {
        "too many attempts — wait a moment and try again"
    } else {
        return raw.chars().take(200).collect();
    };
    friendly.to_string()
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.1 client (internal network only)
// ---------------------------------------------------------------------------
//
// The sim talks to Rauthy over the compose network with tiny JSON/form
// payloads. `reqwest` stalled on connect here (30s while `curl` in the same
// container answered in ~1ms), so this uses a plain blocking socket on
// `spawn_blocking`: no resolver/TLS/proxy machinery, nothing to stall on.
// Only `http://host:port/path` is supported — which is all we need.

struct HttpResponse {
    status: u16,
    body: String,
}

fn http_request_blocking(
    method: &str,
    url: &str,
    content_type: &str,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> Result<HttpResponse, String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported url (http only): {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = authority
        .to_socket_addrs()
        .map_err(|e| format!("resolve {authority}: {e}"))?
        .next()
        .ok_or_else(|| format!("resolve {authority}: no addresses"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("connect {authority}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| format!("socket: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("socket: {e}"))?;

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccept: application/json\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|e| format!("send: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed response (no header terminator)".to_string())?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "malformed status line".to_string())?;
    // `Connection: close` means no chunking on these endpoints; strip any
    // chunked framing defensively so JSON parsing never sees size lines.
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        let mut out = String::new();
        let mut rest = body;
        while let Some(nl) = rest.find("\r\n") {
            let Ok(size) = usize::from_str_radix(rest[..nl].trim(), 16) else {
                break;
            };
            if size == 0 {
                break;
            }
            let start = nl + 2;
            let Some(end) = rest.get(start..start + size) else {
                break;
            };
            out.push_str(end);
            rest = &rest[start + size..];
            rest = rest.strip_prefix("\r\n").unwrap_or(rest);
        }
        out
    } else {
        body.to_string()
    };
    Ok(HttpResponse { status, body })
}

/// Run [`http_request_blocking`] off the async workers.
async fn http_request(
    method: &'static str,
    url: String,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
) -> Result<HttpResponse, String> {
    tokio::task::spawn_blocking(move || {
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        http_request_blocking(method, &url, content_type, &header_refs, &body)
    })
    .await
    .map_err(|e| format!("http task: {e}"))?
}

/// Create the user via the Rauthy admin API and set its password + verified
/// flag, so it can log straight in with the password grant.
async fn provision_user(
    cfg: &SignupConfig,
    email: &str,
    name: &str,
    password: &str,
) -> Result<(), String> {
    let auth = format!("API-Key {}", cfg.api_key.as_deref().unwrap_or_default());
    let base = cfg.rauthy_internal.trim_end_matches('/');

    // 1. Create the user (admin API).
    let create_body = serde_json::to_vec(&serde_json::json!({
        "email": email,
        "given_name": name,
        "language": "en",
        "roles": [],
    }))
    .map_err(|e| format!("encode request: {e}"))?;
    let created = http_request(
        "POST",
        format!("{base}/auth/v1/users"),
        "application/json",
        vec![("Authorization".to_string(), auth.clone())],
        create_body,
    )
    .await?;
    if !(200..300).contains(&created.status) {
        return Err(rauthy_error(created.status, &created.body));
    }
    let parsed: serde_json::Value = serde_json::from_str(&created.body)
        .map_err(|e| format!("account creation failed: {e}"))?;
    let id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "account creation failed: no user id returned".to_string())?
        .to_string();

    // 2. Set the password + verify the email. No SMTP is configured, so the
    // magic-link verification flow is unavailable.
    let update_body = serde_json::to_vec(&serde_json::json!({
        "email": email,
        "given_name": name,
        "roles": [],
        "enabled": true,
        "email_verified": true,
        "password": password,
    }))
    .map_err(|e| format!("encode request: {e}"))?;
    let updated = http_request(
        "PUT",
        format!("{base}/auth/v1/users/{id}"),
        "application/json",
        vec![("Authorization".to_string(), auth.clone())],
        update_body,
    )
    .await?;
    if !(200..300).contains(&updated.status) {
        // Don't strand a password-less account holding the email: delete it
        // so the player can simply retry.
        let _ = http_request(
            "DELETE",
            format!("{base}/auth/v1/users/{id}"),
            "application/json",
            vec![("Authorization".to_string(), auth)],
            Vec::new(),
        )
        .await;
        return Err(rauthy_error(updated.status, &updated.body));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Login / refresh proxy
// ---------------------------------------------------------------------------
//
// The game binary cannot hold an OIDC client secret. When the Rauthy client is
// **confidential**, the sim performs the password / refresh grant on the
// client's behalf and returns only the tokens. When no secret is configured
// this 501s and the client falls back to calling Rauthy directly (public
// client).

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenPayload {
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Tokens are nested under `token` (explicit shape; `flatten` + `Option`
    /// deserializes unreliably on the client side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<TokenPayload>,
}

/// The player's address: `X-Forwarded-For` (set by the reverse proxy) when
/// present, else the direct peer (local/dev or same-host browser).
fn client_ip(headers: &axum::http::HeaderMap, peer: std::net::SocketAddr) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|list| list.split(',').next())
        .map(str::trim)
        .filter(|ip| !ip.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| peer.ip().to_string())
}

fn login_err(msg: impl Into<String>) -> (StatusCode, Json<LoginResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(LoginResponse {
            ok: false,
            error: Some(msg.into()),
            token: None,
        }),
    )
}

/// Exchange form params against Rauthy's token endpoint using the configured
/// confidential client credentials.
async fn token_grant(
    cfg: &SignupConfig,
    grant: &[(&str, String)],
    client_ip: Option<&str>,
) -> Result<TokenPayload, String> {
    let Some(secret) = cfg.client_secret.as_deref() else {
        return Err("login proxy is not enabled on this server".to_string());
    };
    // Form-encode. `url::form_urlencoded` would be another dep; these values
    // are emails/tokens/opaque secrets, so percent-encode everything non-safe.
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    let mut pairs: Vec<(String, String)> = grant
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    pairs.push(("client_id".to_string(), cfg.client_id.clone()));
    pairs.push(("client_secret".to_string(), secret.to_string()));
    let body = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes();
    let base = cfg.rauthy_internal.trim_end_matches('/');
    // Every player reaches Rauthy through this one proxy IP, and Rauthy's
    // `login_delay` throttles per source IP. Forward the real player address
    // (Rauthy trusts the compose network) so one player's failures don't slow
    // everyone down.
    let headers = match client_ip {
        Some(ip) if !ip.is_empty() => vec![("X-Forwarded-For".to_string(), ip.to_string())],
        _ => Vec::new(),
    };
    let resp = http_request(
        "POST",
        format!("{base}/auth/v1/oidc/token"),
        "application/x-www-form-urlencoded",
        headers,
        body,
    )
    .await?;
    if !(200..300).contains(&resp.status) {
        return Err(rauthy_error(resp.status, &resp.body));
    }
    serde_json::from_str::<TokenPayload>(&resp.body)
        .map_err(|e| format!("bad token response: {e}"))
}

async fn login_handler(
    axum::extract::State(cfg): axum::extract::State<SignupConfig>,
    headers: axum::http::HeaderMap,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: axum::body::Bytes,
) -> (StatusCode, Json<LoginResponse>) {
    let client_ip = client_ip(&headers, peer);
    let req: LoginRequest = match parse_body(&body) {
        Ok(r) => r,
        Err(e) => return login_err(e),
    };
    if req.email.trim().is_empty() || req.password.is_empty() {
        return login_err("enter your email and password");
    }
    match token_grant(
        &cfg,
        &[
            ("grant_type", "password".to_string()),
            ("username", req.email.trim().to_string()),
            ("password", req.password),
            (
                "scope",
                "openid profile email offline_access".to_string(),
            ),
        ],
        Some(&client_ip),
    )
    .await
    {
        Ok(token) => (
            StatusCode::OK,
            Json(LoginResponse {
                ok: true,
                error: None,
                token: Some(token),
            }),
        ),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(LoginResponse {
                ok: false,
                error: Some(e),
                token: None,
            }),
        ),
    }
}

async fn refresh_handler(
    axum::extract::State(cfg): axum::extract::State<SignupConfig>,
    headers: axum::http::HeaderMap,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: axum::body::Bytes,
) -> (StatusCode, Json<LoginResponse>) {
    let client_ip = client_ip(&headers, peer);
    let req: RefreshRequest = match parse_body(&body) {
        Ok(r) => r,
        Err(e) => return login_err(e),
    };
    if req.refresh_token.trim().is_empty() {
        return login_err("missing refresh token");
    }
    match token_grant(
        &cfg,
        &[
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", req.refresh_token),
        ],
        Some(&client_ip),
    )
    .await
    {
        Ok(token) => (
            StatusCode::OK,
            Json(LoginResponse {
                ok: true,
                error: None,
                token: Some(token),
            }),
        ),
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(LoginResponse {
                ok: false,
                error: Some(e),
                token: None,
            }),
        ),
    }
}

/// Which cross-origin requests the auth API answers.
///
/// In production the game page and this API share an origin (`play.conman.biz`
/// → NPM → sim), so no CORS is involved at all. Local development is the one
/// case that needs it: the page comes from `serve-web.py` on `:6082` while this
/// API listens on `:6565`, and without these headers the browser fails every
/// login with "Failed to fetch". Only loopback origins are allowed (any port),
/// and no credentials are accepted, so a random website still cannot read
/// responses from this API.
fn dev_cors_origin(origin: Option<&str>) -> Option<&str> {
    let origin = origin?;
    let loopback = origin
        .strip_prefix("http://localhost")
        .or_else(|| origin.strip_prefix("http://127.0.0.1"))
        .or_else(|| origin.strip_prefix("http://[::1]"))?;
    // Nothing (bare host) or a port, never a path.
    if loopback.is_empty() || loopback.starts_with(':') && !loopback.contains('/') {
        Some(origin)
    } else {
        None
    }
}

/// Adds the CORS headers described by [`dev_cors_origin`] and answers preflight
/// requests. See the note there: this exists for local dev only.
async fn dev_cors(request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let allowed = dev_cors_origin(origin.as_deref()).map(str::to_string);
    let preflight = request.method() == Method::OPTIONS;

    let mut response = match &allowed {
        Some(_) if preflight => StatusCode::NO_CONTENT.into_response(),
        _ => next.run(request).await,
    };
    if let Some(origin) = allowed {
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&origin) {
            headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            headers.insert(header::VARY, HeaderValue::from_static("origin"));
        }
        if preflight {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("POST, OPTIONS"),
            );
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("content-type"),
            );
            headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        }
    }
    response
}

/// Serve `POST /signup`, `/login`, `/refresh`. Runs for the life of the
/// process.
pub async fn serve(cfg: SignupConfig) -> anyhow::Result<()> {
    let proxy = cfg.client_secret.is_some();
    let app = Router::new()
        .route("/signup", post(signup_handler))
        .route("/login", post(login_handler))
        .route("/refresh", post(refresh_handler))
        .layer(axum::middleware::from_fn(dev_cors))
        .with_state(cfg.clone());
    eprintln!(
        "auth api: listening on {} (signup {}, login proxy {})",
        cfg.bind_addr,
        if cfg.api_key.is_some() { "on" } else { "off" },
        if proxy { "on" } else { "off" }
    );
    let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(email: &str, name: &str, password: &str, invite: Option<&str>) -> SignupRequest {
        SignupRequest {
            email: email.to_string(),
            name: name.to_string(),
            password: password.to_string(),
            invite: invite.map(str::to_string),
        }
    }

    #[test]
    fn local_dev_origins_are_allowed_and_the_rest_are_not() {
        assert_eq!(
            dev_cors_origin(Some("http://localhost:6082")),
            Some("http://localhost:6082")
        );
        assert_eq!(
            dev_cors_origin(Some("http://127.0.0.1:6082")),
            Some("http://127.0.0.1:6082")
        );
        assert_eq!(
            dev_cors_origin(Some("http://[::1]:6082")),
            Some("http://[::1]:6082")
        );
        assert_eq!(dev_cors_origin(Some("http://localhost")), Some("http://localhost"));
        // Production is same-origin, so it never needs CORS headers.
        assert_eq!(dev_cors_origin(Some("https://play.conman.biz")), None);
        assert_eq!(dev_cors_origin(Some("http://evil.example")), None);
        // Prefix tricks must not pass.
        assert_eq!(dev_cors_origin(Some("http://localhost.evil.example")), None);
        assert_eq!(dev_cors_origin(None), None);
    }

    #[test]
    fn valid_signup_passes() {
        let r = req("a@b.cd", "Hero", "S3cure!pass", None);
        assert_eq!(
            validate_signup(&r, None).as_deref(),
            Ok("a@b.cd")
        );
    }

    #[test]
    fn email_is_lowercased_and_trimmed() {
        let r = req("  A@B.CD ", "Hero", "S3cure!pass", None);
        assert_eq!(validate_signup(&r, None).as_deref(), Ok("a@b.cd"));
    }

    #[test]
    fn bad_emails_rejected() {
        for email in ["", "no-at-sign", "a@", "@b", "   "] {
            assert!(validate_signup(&req(email, "Hero", "S3cure!pass", None), None).is_err());
        }
    }

    #[test]
    fn name_rules() {
        assert!(validate_signup(&req("a@b.cd", "", "S3cure!pass", None), None).is_err());
        assert!(validate_signup(&req("a@b.cd", "  ", "S3cure!pass", None), None).is_err());
        assert!(validate_signup(
            &req("a@b.cd", &"x".repeat(25), "S3cure!pass", None),
            None
        )
        .is_err());
        assert!(validate_signup(
            &req("a@b.cd", &"x".repeat(24), "S3cure!pass", None),
            None
        )
        .is_ok());
    }

    #[test]
    fn password_minimum() {
        assert!(validate_signup(&req("a@b.cd", "Hero", "Sh0rt!", None), None).is_err());
        assert!(validate_signup(&req("a@b.cd", "Hero", "Longenough!1", None), None).is_ok());
    }

    #[test]
    fn password_policy_matches_rauthy() {
        let ok = |pw: &str| validate_signup(&req("a@b.cd", "Hero", pw, None), None).is_ok();
        assert!(ok("S3cure!pass"));
        assert!(!ok("lowercase1!"), "needs an uppercase letter");
        assert!(!ok("UPPERCASE1!"), "needs a lowercase letter");
        assert!(!ok("NoDigits!!"), "needs a digit");
        assert!(!ok("NoSpecial1a"), "needs a special character");
        assert!(!ok("Sh0rt!"), "needs 8+ characters");
    }

    #[test]
    fn invite_gate() {
        let r = |invite: Option<&str>| req("a@b.cd", "Hero", "Longenough!1", invite);
        // No code configured: anything goes.
        assert!(validate_signup(&r(None), None).is_ok());
        assert!(validate_signup(&r(Some("zzz")), None).is_ok());
        // Code configured: must match exactly.
        assert!(validate_signup(&r(Some("alpha")), Some("alpha")).is_ok());
        assert!(validate_signup(&r(None), Some("alpha")).is_err());
        assert!(validate_signup(&r(Some("beta")), Some("alpha")).is_err());
    }

    #[test]
    fn parse_body_accepts_json_and_urlencoded() {
        let json_bytes = b"{\"email\":\"hero@test.com\",\"password\":\"Pass123!\"}";
        let parsed: LoginRequest = parse_body(json_bytes).expect("parses json");
        assert_eq!(parsed.email, "hero@test.com");
        assert_eq!(parsed.password, "Pass123!");

        let form_bytes = b"email=hero%40test.com&password=Pass123!";
        let parsed_form: LoginRequest = parse_body(form_bytes).expect("parses form");
        assert_eq!(parsed_form.email, "hero@test.com");
        assert_eq!(parsed_form.password, "Pass123!");

        let invalid = b"";
        assert!(parse_body::<LoginRequest>(invalid).is_err());
    }

    /// The login/refresh response shape the game client expects.
    #[test]
    fn login_response_nests_tokens() {
        let resp = LoginResponse {
            ok: true,
            error: None,
            token: Some(TokenPayload {
                id_token: "id".into(),
                access_token: "at".into(),
                refresh_token: Some("rt".into()),
                expires_in: Some(3600),
            }),
        };
        let json = serde_json::to_value(&resp).expect("serializes");
        assert_eq!(json["ok"], true);
        assert_eq!(json["token"]["id_token"], "id");
        assert_eq!(json["token"]["expires_in"], 3600);
        assert!(json.get("id_token").is_none(), "tokens must not be flattened");
        // Errors carry no token object.
        let err = serde_json::to_value(LoginResponse {
            ok: false,
            error: Some("invalid email or password".into()),
            token: None,
        })
        .expect("serializes");
        assert!(err.get("token").is_none());
    }

    /// The HTTP helper must reject anything it cannot prove is plain HTTP,
    /// and must surface connection failures instead of hanging.
    #[test]
    fn http_helper_rejects_unsupported_urls() {
        assert!(http_request_blocking("POST", "https://rauthy:6080/x", "application/json", &[], b"{}").is_err());
        assert!(http_request_blocking("POST", "rauthy:6080/x", "application/json", &[], b"{}").is_err());
        // Closed port on loopback: fails fast, no panic.
        let err = http_request_blocking(
            "POST",
            "http://127.0.0.1:1/none",
            "application/json",
            &[],
            b"{}",
        );
        assert!(err.is_err());
    }
}
