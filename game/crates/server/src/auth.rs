//! Auth provider abstraction — swap the token issuer without a refactor.
//!
//! All provider-specific details (JWKS, issuer, audience, Surreal access
//! definition, row predicates) live here. Everything else — `Backend`,
//! `HostNet`, `NetClient`, `schema.surql` generation — talks to the
//! `AuthConfig` trait, not to Firebase/Auth0 directly.
//!
//! Swap via environment:
//! ```sh
//! # Firebase (legacy, still default when DUNGEON_FIREBASE_PROJECT_ID is set)
//! DUNGEON_AUTH_PROVIDER=firebase DUNGEON_FIREBASE_PROJECT_ID=my-proj
//! # Generic OIDC (Auth0, Clerk, Supabase, Okta, …)
//! DUNGEON_AUTH_PROVIDER=oidc DUNGEON_OIDC_ISSUER=https://my-tenant.auth0.com/ DUNGEON_OIDC_AUDIENCE=my-api DUNGEON_OIDC_JWKS_URL=https://my-tenant.auth0.com/.well-known/jwks.json
//! # Local HS256 (tests, dev without external IdP)
//! DUNGEON_AUTH_PROVIDER=hs256 DUNGEON_JWT_SECRET=dev-secret DUNGEON_JWT_ISSUER=my-iss DUNGEON_JWT_AUDIENCE=my-aud
//! ```
//! `DUNGEON_AUTH_PROVIDER` is optional: if unset we infer from the other
//! vars — `DUNGEON_FIREBASE_PROJECT_ID` → firebase, `DUNGEON_OIDC_ISSUER` → oidc,
//! `DUNGEON_JWT_SECRET` → hs256.

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{BackendError, Result};

// ---------------------------------------------------------------------------
// Claims — provider-agnostic, what the database verifies via JWKS.
// ---------------------------------------------------------------------------

/// `aud` can be a single string or an array (Auth0 does both).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AudValue {
    Single(String),
    Multiple(Vec<String>),
}

impl AudValue {
    pub fn contains(&self, aud: &str) -> bool {
        match self {
            AudValue::Single(s) => s == aud,
            AudValue::Multiple(v) => v.iter().any(|s| s == aud),
        }
    }
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            AudValue::Single(s) => vec![s.clone()],
            AudValue::Multiple(v) => v.clone(),
        }
    }
}

fn deserialize_aud<'de, D>(deserializer: D) -> std::result::Result<Option<AudValue>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<AudValue>::deserialize(deserializer)?;
    Ok(opt)
}

/// Normalized claims. Verified inside SurrealDB via JWKS; Rust only decodes
/// for display/routing — authorization lives in row rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthClaims {
    /// Issuer (`iss`). For Firebase: `https://securetoken.google.com/<project>`.
    #[serde(default)]
    pub iss: Option<String>,
    /// Audience (`aud`). String or array.
    #[serde(default, deserialize_with = "deserialize_aud")]
    pub aud: Option<AudValue>,
    /// Subject — the stable player key. Every table's `rid = $token.sub`.
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub exp: Option<u64>,
    #[serde(default)]
    pub iat: Option<u64>,
    /// SurrealDB routing claims added via the IdP's custom-claims hook
    /// (Firebase Admin SDK `setCustomUserClaims`, Auth0 Action, etc.).
    #[serde(default)]
    pub ns: Option<String>,
    #[serde(default)]
    pub db: Option<String>,
    #[serde(default)]
    pub ac: Option<String>,
}

// Backward compat: `FirebaseClaims` was the old name.
pub type FirebaseClaims = AuthClaims;

/// Decode a JWT payload **without verifying its signature**. Useful for
/// showing "signed in as …" or routing; authorization decisions belong to
/// the database and its JWKS check.
pub fn decode_claims_unverified(token: &str) -> Result<AuthClaims> {
    let payload = token.split('.').nth(1).ok_or_else(|| {
        BackendError::Invalid("Malformed token: expected header.payload.signature.".to_string())
    })?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| BackendError::Invalid(format!("Malformed token payload: {e}")))?;
    serde_json::from_slice(&bytes).map_err(BackendError::from)
}

// ---------------------------------------------------------------------------
// Provider abstraction
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum AuthProvider {
    /// Firebase Auth — JWKS is Google's, audience is the project id.
    Firebase { project_id: String },
    /// Rauthy self-hosted — OIDC with local JWKS. `PUB_URL` in rauthy.cfg
    /// must match `issuer` here, and the SurrealDB host must reach
    /// `jwks_url` (use `http://rauthy:6080/...` inside compose, `http://localhost:6080/...` outside).
    Rauthy {
        /// e.g. `http://localhost:6080/auth/v1`
        issuer: String,
        audience: String,
        jwks_url: String,
        #[serde(default = "default_access_name")]
        access_name: String,
    },
    /// Any OIDC-compliant provider (Auth0, Clerk, Supabase Auth, Okta, …).
    Oidc {
        issuer: String,
        audience: String,
        jwks_url: String,
        /// SurrealDB access name. Defaults to `platform_jwt`.
        #[serde(default = "default_access_name")]
        access_name: String,
    },
    /// Local HS256 for tests / dev without an external IdP.
    Hs256 {
        /// Base64 or raw secret for `ALGORITHM HS256 KEY`.
        secret: String,
        #[serde(default)]
        issuer: Option<String>,
        #[serde(default)]
        audience: Option<String>,
        #[serde(default = "default_access_name")]
        access_name: String,
    },
    /// No JWT auth — only service users / dev_login. For pure offline tests.
    None,
}

fn default_access_name() -> String {
    "platform_jwt".to_string()
}

impl AuthProvider {
    pub fn access_name(&self) -> &str {
        match self {
            AuthProvider::Firebase { .. } => "firebase_jwt",
            AuthProvider::Rauthy { access_name, .. } => access_name,
            AuthProvider::Oidc { access_name, .. } => access_name,
            AuthProvider::Hs256 { access_name, .. } => access_name,
            AuthProvider::None => "platform_jwt",
        }
    }

    pub fn jwks_url(&self) -> Option<&str> {
        match self {
            AuthProvider::Firebase { .. } => {
                Some("https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com")
            }
            AuthProvider::Rauthy { jwks_url, .. } => Some(jwks_url),
            AuthProvider::Oidc { jwks_url, .. } => Some(jwks_url),
            AuthProvider::Hs256 { .. } => None,
            AuthProvider::None => None,
        }
    }

    pub fn issuer(&self) -> Option<&str> {
        match self {
            AuthProvider::Firebase { project_id: _ } => None, // constructed dynamically in row_predicates
            AuthProvider::Rauthy { issuer, .. } => Some(issuer),
            AuthProvider::Oidc { issuer, .. } => Some(issuer),
            AuthProvider::Hs256 { issuer, .. } => issuer.as_deref(),
            AuthProvider::None => None,
        }
    }

    pub fn audience(&self) -> Option<&str> {
        match self {
            AuthProvider::Firebase { project_id } => Some(project_id.as_str()),
            AuthProvider::Rauthy { audience, .. } => Some(audience),
            AuthProvider::Oidc { audience, .. } => Some(audience),
            AuthProvider::Hs256 { audience, .. } => audience.as_deref(),
            AuthProvider::None => None,
        }
    }

    /// The full `DEFINE ACCESS` statement for this provider.
    pub fn surreal_access_definition(&self) -> String {
        match self {
            AuthProvider::Firebase { project_id } => {
                format!(
                    "DEFINE ACCESS {} ON DATABASE TYPE JWT URL 'https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com' AUDIENCE '{}';",
                    self.access_name(),
                    escape_single(project_id)
                )
            }
            AuthProvider::Rauthy { jwks_url, audience, .. } => {
                format!(
                    "DEFINE ACCESS {} ON DATABASE TYPE JWT URL '{}' AUDIENCE '{}';",
                    self.access_name(),
                    escape_single(jwks_url),
                    escape_single(audience)
                )
            }
            AuthProvider::Oidc { jwks_url, audience, .. } => {
                format!(
                    "DEFINE ACCESS {} ON DATABASE TYPE JWT URL '{}' AUDIENCE '{}';",
                    self.access_name(),
                    escape_single(jwks_url),
                    escape_single(audience)
                )
            }
            AuthProvider::Hs256 { secret, audience, .. } => {
                let aud_clause = audience
                    .as_deref()
                    .map(|a| format!(" AUDIENCE '{}'", escape_single(a)))
                    .unwrap_or_default();
                format!(
                    "DEFINE ACCESS {} ON DATABASE TYPE JWT ALGORITHM HS256 KEY '{}'{};",
                    self.access_name(),
                    escape_single(secret),
                    aud_clause
                )
            }
            AuthProvider::None => {
                // No JWT access — callers must use service users / dev_login.
                String::new()
            }
        }
    }

    /// Row predicates that every game table's `PERMISSIONS` should include.
    /// For JWT providers this pins `iss`/`aud` to the configured values;
    /// for dev/none it falls back to `$token.sub != NONE`.
    pub fn row_predicates(&self) -> Vec<String> {
        match self {
            AuthProvider::Firebase { project_id } => vec![
                format!("$token.iss == 'https://securetoken.google.com/{}'", escape_single(project_id)),
                format!("$token.aud == '{}'", escape_single(project_id)),
            ],
            AuthProvider::Rauthy { issuer, audience, .. } => vec![
                format!("$token.iss == '{}'", escape_single(issuer)),
                format!("$token.aud == '{}'", escape_single(audience)),
            ],
            AuthProvider::Oidc { issuer, audience, .. } => vec![
                format!("$token.iss == '{}'", escape_single(issuer)),
                format!("$token.aud == '{}'", escape_single(audience)),
            ],
            AuthProvider::Hs256 { issuer, audience, .. } => {
                let mut v = Vec::new();
                if let Some(iss) = issuer {
                    v.push(format!("$token.iss == '{}'", escape_single(iss)));
                }
                if let Some(aud) = audience {
                    v.push(format!("$token.aud == '{}'", escape_single(aud)));
                }
                if v.is_empty() {
                    v.push("$token.sub != NONE".to_string());
                }
                v
            }
            AuthProvider::None => vec!["$token.sub != NONE".to_string()],
        }
    }

    /// Single `WHERE` predicate string joining all row predicates with ` AND `.
    pub fn row_predicate_sql(&self) -> String {
        self.row_predicates().join(" AND ")
    }

    /// Ownership predicate for the player table: you may only write your own
    /// row. Shared tables use `row_predicate_sql()` alone.
    pub fn ownership_predicate(&self) -> String {
        "rid = $token.sub".to_string()
    }
}

fn escape_single(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

// ---------------------------------------------------------------------------
// AuthConfig — the top-level config the rest of the system holds.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthConfig {
    pub provider: AuthProvider,
}

impl AuthConfig {
    pub fn new(provider: AuthProvider) -> Self {
        Self { provider }
    }

    /// Infer from environment. Priority:
    /// 1) `DUNGEON_AUTH_PROVIDER` explicit
    /// 2) `DUNGEON_FIREBASE_PROJECT_ID` → Firebase
    /// 3) `DUNGEON_OIDC_ISSUER` → OIDC
    /// 4) `DUNGEON_JWT_SECRET` / `DUNGEON_DEV_JWT_SECRET` → Hs256
    /// 5) `DUNGEON_AUTH_PROVIDER=none` or fallback to Firebase placeholder (so existing `.env` still works)
    pub fn from_env() -> Result<Self> {
        let explicit = std::env::var("DUNGEON_AUTH_PROVIDER").ok();
        let provider = match explicit.as_deref().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("firebase") => {
                let project_id = std::env::var("DUNGEON_FIREBASE_PROJECT_ID")
                    .or_else(|_| std::env::var("DUNGEON_AUTH_FIREBASE_PROJECT_ID"))
                    .map_err(|_| BackendError::Config("DUNGEON_FIREBASE_PROJECT_ID is not set for firebase provider.".to_string()))?;
                AuthProvider::Firebase { project_id }
            }
            Some("oidc") => {
                let issuer = std::env::var("DUNGEON_OIDC_ISSUER")
                    .or_else(|_| std::env::var("DUNGEON_AUTH_OIDC_ISSUER"))
                    .map_err(|_| BackendError::Config("DUNGEON_OIDC_ISSUER is not set.".to_string()))?;
                let audience = std::env::var("DUNGEON_OIDC_AUDIENCE")
                    .or_else(|_| std::env::var("DUNGEON_AUTH_OIDC_AUDIENCE"))
                    .map_err(|_| BackendError::Config("DUNGEON_OIDC_AUDIENCE is not set.".to_string()))?;
                let jwks_url = std::env::var("DUNGEON_OIDC_JWKS_URL")
                    .or_else(|_| std::env::var("DUNGEON_AUTH_JWKS_URL"))
                    .map_err(|_| BackendError::Config("DUNGEON_OIDC_JWKS_URL is not set.".to_string()))?;
                let access_name = std::env::var("DUNGEON_OIDC_ACCESS_NAME")
                    .unwrap_or_else(|_| default_access_name());
                AuthProvider::Oidc { issuer, audience, jwks_url, access_name }
            }
            Some("rauthy") => {
                // Self-hosted Rauthy — a thin wrapper over OIDC with local defaults.
                // PUB_URL in rauthy.cfg should match issuer; JWKS must be reachable
                // from SurrealDB (use `http://rauthy:6080/...` inside compose).
                let base = std::env::var("DUNGEON_RAUTHY_URL")
                    .or_else(|_| std::env::var("DUNGEON_AUTH_RAUTHY_URL"))
                    .unwrap_or_else(|_| "http://localhost:6080".to_string());
                let base = base.trim_end_matches('/');
                let issuer = std::env::var("DUNGEON_RAUTHY_ISSUER")
                    .or_else(|_| std::env::var("DUNGEON_OIDC_ISSUER"))
                    .unwrap_or_else(|_| format!("{base}/auth/v1"));
                let audience = std::env::var("DUNGEON_RAUTHY_AUDIENCE")
                    .or_else(|_| std::env::var("DUNGEON_OIDC_AUDIENCE"))
                    .unwrap_or_else(|_| "party-dungeon".to_string());
                // Inside compose SurrealDB reaches rauthy at `http://rauthy:6080`.
                let jwks_url = std::env::var("DUNGEON_RAUTHY_JWKS_URL")
                    .or_else(|_| std::env::var("DUNGEON_OIDC_JWKS_URL"))
                    .unwrap_or_else(|_| {
                        if base.contains("localhost") {
                            // When base is localhost, the JWKS that SurrealDB (inside
                            // docker) can reach is at `http://rauthy:6080/...`.
                            "http://rauthy:6080/auth/v1/oidc/jwks".to_string()
                        } else {
                            format!("{base}/auth/v1/oidc/jwks")
                        }
                    });
                let access_name = std::env::var("DUNGEON_RAUTHY_ACCESS_NAME")
                    .or_else(|_| std::env::var("DUNGEON_OIDC_ACCESS_NAME"))
                    .unwrap_or_else(|_| default_access_name());
                AuthProvider::Rauthy { issuer, audience, jwks_url, access_name }
            }
            Some("hs256") | Some("dev") | Some("dev_hs256") => {
                let secret = std::env::var("DUNGEON_JWT_SECRET")
                    .or_else(|_| std::env::var("DUNGEON_DEV_JWT_SECRET"))
                    .or_else(|_| std::env::var("DUNGEON_AUTH_JWT_SECRET"))
                    .map_err(|_| BackendError::Config("DUNGEON_JWT_SECRET is not set for hs256 provider.".to_string()))?;
                let issuer = std::env::var("DUNGEON_JWT_ISSUER").ok()
                    .or_else(|| std::env::var("DUNGEON_AUTH_JWT_ISSUER").ok());
                let audience = std::env::var("DUNGEON_JWT_AUDIENCE").ok()
                    .or_else(|| std::env::var("DUNGEON_AUTH_JWT_AUDIENCE").ok());
                let access_name = std::env::var("DUNGEON_JWT_ACCESS_NAME")
                    .unwrap_or_else(|_| default_access_name());
                AuthProvider::Hs256 { secret, issuer, audience, access_name }
            }
            Some("none") => AuthProvider::None,
            Some(other) => {
                return Err(BackendError::Config(format!("Unknown DUNGEON_AUTH_PROVIDER '{other}'. Expected rauthy|firebase|oidc|hs256|none.")))
            }
            None => {
                // Inference chain
                if let Ok(project_id) = std::env::var("DUNGEON_FIREBASE_PROJECT_ID").or_else(|_| std::env::var("DUNGEON_AUTH_FIREBASE_PROJECT_ID")) {
                    AuthProvider::Firebase { project_id }
                } else if std::env::var("DUNGEON_RAUTHY_URL").is_ok()
                    || std::env::var("DUNGEON_AUTH_RAUTHY_URL").is_ok()
                    || std::env::var("DUNGEON_RAUTHY_ISSUER").is_ok()
                {
                    let base = std::env::var("DUNGEON_RAUTHY_URL")
                        .or_else(|_| std::env::var("DUNGEON_AUTH_RAUTHY_URL"))
                        .unwrap_or_else(|_| "http://localhost:6080".to_string());
                    let base = base.trim_end_matches('/').to_string();
                    let issuer = std::env::var("DUNGEON_RAUTHY_ISSUER")
                        .or_else(|_| std::env::var("DUNGEON_OIDC_ISSUER"))
                        .unwrap_or_else(|_| format!("{base}/auth/v1"));
                    let audience = std::env::var("DUNGEON_RAUTHY_AUDIENCE")
                        .or_else(|_| std::env::var("DUNGEON_OIDC_AUDIENCE"))
                        .unwrap_or_else(|_| "party-dungeon".to_string());
                    let jwks_url = std::env::var("DUNGEON_RAUTHY_JWKS_URL")
                        .or_else(|_| std::env::var("DUNGEON_OIDC_JWKS_URL"))
                        .unwrap_or_else(|_| {
                            if base.contains("localhost") {
                                "http://rauthy:6080/auth/v1/oidc/jwks".to_string()
                            } else {
                                format!("{base}/auth/v1/oidc/jwks")
                            }
                        });
                    let access_name = std::env::var("DUNGEON_RAUTHY_ACCESS_NAME")
                        .or_else(|_| std::env::var("DUNGEON_OIDC_ACCESS_NAME"))
                        .unwrap_or_else(|_| default_access_name());
                    AuthProvider::Rauthy { issuer, audience, jwks_url, access_name }
                } else if std::env::var("DUNGEON_OIDC_ISSUER").is_ok() || std::env::var("DUNGEON_AUTH_OIDC_ISSUER").is_ok() {
                    let issuer = std::env::var("DUNGEON_OIDC_ISSUER").or_else(|_| std::env::var("DUNGEON_AUTH_OIDC_ISSUER")).unwrap();
                    let audience = std::env::var("DUNGEON_OIDC_AUDIENCE").or_else(|_| std::env::var("DUNGEON_AUTH_OIDC_AUDIENCE")).unwrap_or_else(|_| "default-aud".to_string());
                    let jwks_url = std::env::var("DUNGEON_OIDC_JWKS_URL").or_else(|_| std::env::var("DUNGEON_AUTH_JWKS_URL")).unwrap_or_else(|_| format!("{issuer}/.well-known/jwks.json"));
                    let access_name = std::env::var("DUNGEON_OIDC_ACCESS_NAME").unwrap_or_else(|_| default_access_name());
                    AuthProvider::Oidc { issuer, audience, jwks_url, access_name }
                } else if std::env::var("DUNGEON_JWT_SECRET").is_ok() || std::env::var("DUNGEON_DEV_JWT_SECRET").is_ok() || std::env::var("DUNGEON_AUTH_JWT_SECRET").is_ok() {
                    let secret = std::env::var("DUNGEON_JWT_SECRET").or_else(|_| std::env::var("DUNGEON_DEV_JWT_SECRET")).or_else(|_| std::env::var("DUNGEON_AUTH_JWT_SECRET")).unwrap();
                    AuthProvider::Hs256 { secret, issuer: None, audience: None, access_name: default_access_name() }
                } else {
                    return Err(BackendError::Config(
                        "No auth provider configured. Set DUNGEON_AUTH_PROVIDER and its required vars, or legacy DUNGEON_FIREBASE_PROJECT_ID. See src/auth.rs for the env table.".to_string(),
                    ));
                }
            }
        };
        Ok(Self { provider })
    }

    /// For tests: construct directly without env.
    pub fn firebase_for_tests(project_id: impl Into<String>) -> Self {
        Self { provider: AuthProvider::Firebase { project_id: project_id.into() } }
    }
    pub fn hs256_for_tests(secret: impl Into<String>) -> Self {
        Self { provider: AuthProvider::Hs256 { secret: secret.into(), issuer: None, audience: None, access_name: default_access_name() } }
    }

    pub fn provider_name(&self) -> &'static str {
        match &self.provider {
            AuthProvider::Firebase { .. } => "firebase",
            AuthProvider::Rauthy { .. } => "rauthy",
            AuthProvider::Oidc { .. } => "oidc",
            AuthProvider::Hs256 { .. } => "hs256",
            AuthProvider::None => "none",
        }
    }

    pub fn rauthy_for_tests(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_url: impl Into<String>,
    ) -> Self {
        Self {
            provider: AuthProvider::Rauthy {
                issuer: issuer.into(),
                audience: audience.into(),
                jwks_url: jwks_url.into(),
                access_name: default_access_name(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Schema generation — replaces manual string-replace in bootstrap.py
// ---------------------------------------------------------------------------

impl AuthConfig {
    /// Render the full `schema.surql` with this provider's access definition
    /// and row predicates. `template` is the file's contents with
    /// `__AUTH_ACCESS_DEFINITION__` and `__AUTH_ROW_PREDICATE__` placeholders.
    pub fn render_schema(&self, template: &str) -> String {
        let access = self.provider.surreal_access_definition();
        let row = self.provider.row_predicate_sql();
        let ownership = self.provider.ownership_predicate();
        template
            .replace("__AUTH_ACCESS_DEFINITION__", &access)
            .replace("__AUTH_ROW_PREDICATE__", &row)
            .replace("__AUTH_OWNERSHIP_PREDICATE__", &ownership)
            // Legacy placeholder kept for backward compat — single-replace the project id
            .replace("__FIREBASE_PROJECT_ID__", &self.firebase_project_id_compat())
    }

    fn firebase_project_id_compat(&self) -> String {
        match &self.provider {
            AuthProvider::Firebase { project_id } => project_id.clone(),
            _ => "__FIREBASE_PROJECT_ID__".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firebase_row_predicate_pins_iss_and_aud() {
        let cfg = AuthConfig::new(AuthProvider::Firebase { project_id: "my-proj".to_string() });
        assert_eq!(
            cfg.provider.row_predicate_sql(),
            "$token.iss == 'https://securetoken.google.com/my-proj' AND $token.aud == 'my-proj'"
        );
        assert_eq!(cfg.provider.access_name(), "firebase_jwt");
    }

    #[test]
    fn oidc_row_predicate_and_access() {
        let cfg = AuthConfig::new(AuthProvider::Oidc {
            issuer: "https://my-tenant.auth0.com/".to_string(),
            audience: "my-api".to_string(),
            jwks_url: "https://my-tenant.auth0.com/.well-known/jwks.json".to_string(),
            access_name: "platform_jwt".to_string(),
        });
        assert_eq!(
            cfg.provider.row_predicate_sql(),
            "$token.iss == 'https://my-tenant.auth0.com/' AND $token.aud == 'my-api'"
        );
        assert!(cfg.provider.surreal_access_definition().contains("platform_jwt"));
        assert!(cfg.provider.surreal_access_definition().contains("my-api"));
    }

    #[test]
    fn hs256_without_iss_aud_falls_back_to_sub() {
        let cfg = AuthConfig::new(AuthProvider::Hs256 {
            secret: "s".to_string(),
            issuer: None,
            audience: None,
            access_name: "platform_jwt".to_string(),
        });
        assert_eq!(cfg.provider.row_predicate_sql(), "$token.sub != NONE");
    }

    #[test]
    fn decode_handles_string_and_array_aud() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"iss":"https://example.com","aud":["a","b"],"sub":"uid-1"}"#,
        );
        let token = format!("e30.{payload}.sig");
        let claims = decode_claims_unverified(&token).unwrap();
        assert_eq!(claims.sub, "uid-1");
        assert_eq!(claims.iss.as_deref(), Some("https://example.com"));
        match claims.aud.unwrap() {
            AudValue::Multiple(v) => assert_eq!(v, vec!["a", "b"]),
            _ => panic!("expected array aud"),
        }
    }

    #[test]
    fn single_aud_string_decodes() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"iss":"https://example.com","aud":"my-aud","sub":"uid-2"}"#,
        );
        let token = format!("e30.{payload}.sig");
        let claims = decode_claims_unverified(&token).unwrap();
        assert_eq!(claims.aud.unwrap(), AudValue::Single("my-aud".to_string()));
    }
}
