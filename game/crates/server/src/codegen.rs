//! Generates the standalone Rust client package from [`crate::reducers`].
//!
//! `cargo run -p party-dungeon-server --bin codegen` renders
//! `clients/rust/` (a `party-dungeon-client` crate). The generated methods
//! issue byte-identical SurrealQL to the [`crate::backend::Backend`]
//! methods, so direct-connect players and the service role speak the same
//! reducers — the row rules are the only difference in enforcement.
//!
//! `tests/client.rs` (conformance) runs both against a live database; the
//! `generated_client_is_fresh` test fails if the checked-in package drifts
//! from what this module renders.

use crate::reducers::{
    ParamType, QueryDef, ReducerDef, ResultKind, ALL as REDUCERS, QUERIES,
};

/// Rendered files: `(relative path, content)`.
pub fn render_rust_client() -> Vec<(String, String)> {
    vec![
        ("Cargo.toml".to_string(), render_cargo_toml()),
        ("src/lib.rs".to_string(), render_lib_rs()),
        ("README.md".to_string(), render_readme()),
    ]
}

fn render_cargo_toml() -> String {
    r#"[package]
name = "party-dungeon-client"
version = "0.1.0"
edition.workspace = true
license.workspace = true
description = "Generated direct-connect client for Party Dungeon. DO NOT EDIT — regenerate with: cargo run -p party-dungeon-server --bin codegen"

[lib]
name = "party_dungeon_client"
crate-type = ["lib"]

[dependencies]
surrealdb = "4.0.1"
futures = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
"#
    .to_string()
}

fn render_readme() -> String {
    r#"# party-dungeon-client

Generated direct-connect client for Party Dungeon. **Do not edit by hand** —
it is rendered from `party-dungeon-server/src/reducers.rs`:

```sh
cargo run -p party-dungeon-server --bin codegen
```

Every method issues the same SurrealQL as the backend's reducer of the same
name. Connect with an auth token (JWT) from the configured provider
(Firebase, Auth0, Clerk, …) via `Client::connect_with_token`; the
row rules in `schema.surql` constrain what that token may touch.
"#
    .to_string()
}

fn render_lib_rs() -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(TYPES);
    out.push_str(ERROR);
    out.push_str(VALIDATORS);
    out.push_str(CLAIMS);
    out.push_str(EVENTS);
    out.push_str(CLIENT_CORE);
    for def in REDUCERS {
        out.push_str(&render_reducer(def));
    }
    for q in QUERIES {
        out.push_str(&render_query(q));
    }
    out.push_str(LIVE_FEED);
    out
}

const HEADER: &str = r#"//! Generated direct-connect client for Party Dungeon.
//!
//! DO NOT EDIT. Rendered from `party-dungeon-server/src/reducers.rs`:
//! `cargo run -p party-dungeon-server --bin codegen`
//!
//! Every method issues byte-identical SurrealQL to the backend reducer of
//! the same name. Connect with an auth token (JWT) from the configured
//! provider (Firebase, Auth0, …); the row rules constrain
//! what that token may touch. Validation lives in the database (row rules);
//! this client surfaces denial as `ClientError::PermissionDenied`.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use surrealdb::engine::remote::ws::{Client as WsClient, Ws};
use surrealdb::opt::auth::{AccessToken, Token};
use surrealdb::types::{SurrealValue, Value};
use surrealdb::{Notification, Surreal};

"#;

const TYPES: &str = r#"// ---------------------------------------------------------------------------
// Row types (mirrors party-dungeon-server; regenerated, not shared, so this
// package stays dependency-free)
// ---------------------------------------------------------------------------

/// One adventurer. `rid` is the auth subject (`sub` claim) — stable across
/// reconnects, independent of provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Player {
    pub rid: String,
    pub name: String,
    pub x: f32,
    pub z: f32,
    pub feet: f32,
    pub yaw_deg: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub online: bool,
    pub last_seen_ms: i64,
}

/// One placed level solid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Piece {
    pub rid: String,
    pub shape: String,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
    pub rot_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    pub dynamic: bool,
    pub damage: f32,
    pub interact: String,
    pub trigger_w: f32,
    pub trigger_h: f32,
    pub trigger_l: f32,
    /// Glade roof pitch (`roof_pitch = trigger_alpha * 2.0`) / trigger opacity.
    pub trigger_alpha: f32,
}

/// Arguments for placing a piece.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct NewPiece {
    pub shape: String,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
    pub rot_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    pub dynamic: bool,
    pub damage: f32,
    pub interact: String,
    pub trigger_w: f32,
    pub trigger_h: f32,
    pub trigger_l: f32,
    /// Glade roof pitch (`roof_pitch = trigger_alpha * 2.0`) / trigger opacity.
    pub trigger_alpha: f32,
}

/// Convenience constructor with editor defaults for everything but shape and
/// position.
pub fn new_piece_args(shape: &str, x: f32, z: f32) -> NewPiece {
    NewPiece {
        shape: shape.to_string(),
        x,
        z,
        elevation: 0.0,
        rot_deg: 0.0,
        pitch_deg: 0.0,
        roll_deg: 0.0,
        dynamic: false,
        damage: 0.0,
        interact: "none".to_string(),
        trigger_w: 2.0,
        trigger_h: 2.0,
        trigger_l: 2.0,
        trigger_alpha: TRIGGER_DEFAULT_ALPHA,
    }
}

/// A control point on a moving-platform track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Waypoint {
    pub x: f32,
    pub z: f32,
}

/// Mover track for one [`Piece`], addressed by that piece's `rid`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct PlatformPath {
    pub piece_rid: String,
    pub mode: String,
    pub speed: f32,
    pub paused: bool,
    pub waypoints: Vec<Waypoint>,
}

/// One entry in the shared event feed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct LogEntry {
    pub rid: String,
    pub at_ms: i64,
    pub kind: String,
    pub actor: String,
    pub text: String,
}

/// Level lobby metadata. Singleton row with `rid == "main"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct LevelMeta {
    pub rid: String,
    pub name: String,
}

/// Portable level snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelSnapshot {
    pub version: u32,
    pub name: String,
    pub pieces: Vec<Piece>,
    pub paths: Vec<PlatformPath>,
}

"#;

const ERROR: &str = r#"// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong in a call. Denied writes surface as empty
/// results, not errors — the client maps "wrote nothing" into these.
#[derive(Debug)]
pub enum ClientError {
    Invalid(String),
    NotFound(String),
    PermissionDenied(String),
    Config(String),
    Store(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Invalid(msg) => write!(f, "invalid: {msg}"),
            ClientError::NotFound(msg) => write!(f, "not found: {msg}"),
            ClientError::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            ClientError::Config(msg) => write!(f, "config: {msg}"),
            ClientError::Store(msg) => write!(f, "store: {msg}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<surrealdb::Error> for ClientError {
    fn from(err: surrealdb::Error) -> Self {
        ClientError::Store(err.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Invalid(format!("bad snapshot JSON: {err}"))
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

"#;

const VALIDATORS: &str = r#"// ---------------------------------------------------------------------------
// Validation (mirrors the backend; the database re-enforces every rule)
// ---------------------------------------------------------------------------

pub const WORLD_BOUND: f32 = 500.0;
pub const ELEV_MIN: f32 = -50.0;
pub const ELEV_MAX: f32 = 200.0;
pub const NAME_MAX_CHARS: usize = 24;
pub const MESSAGE_MAX_CHARS: usize = 280;
pub const PLATFORM_SPEED_MAX: f32 = 30.0;
pub const WAYPOINTS_MIN: usize = 2;
pub const WAYPOINTS_MAX: usize = 16;
pub const MAX_HP_CAP: f32 = 10_000.0;
pub const DEFAULT_MAX_HP: f32 = 100.0;
pub const TRIGGER_DEFAULT_ALPHA: f32 = 0.35;
pub const TRIGGER_MIN_ALPHA: f32 = 0.0;
pub const TRIGGER_MAX_ALPHA: f32 = 1.0;

pub const SHAPE_IDS: &[&str] = &[
    "floor", "pit", "water", "bridge", "wall", "fence", "window", "pillar", "block", "ramp",
    "door", "gate", "secret", "rubble", "crate", "chest", "altar", "brazier", "herb", "ore",
    "spawn", "entry", "spikes", "trigger", "trigger_box", "trigger_sphere", "trigger_cylinder",
    "glade_tower", "glade_room", "glade_wall", "glade_stairs", "glade_door", "glade_window",
    "glade_terrain", "glade_tree", "glade_path",
];

pub const INTERACT_IDS: &[&str] = &["none", "togglePause"];
pub const PLATFORM_MODES: &[&str] = &["pingpong", "loop", "oneshot"];

pub fn is_valid_shape(shape: &str) -> bool {
    SHAPE_IDS.contains(&shape)
}

pub fn canonical_interact(raw: &str) -> Option<String> {
    match raw.to_ascii_lowercase().as_str() {
        "none" => Some("none".to_string()),
        "togglepause" | "toggle_pause" | "toggle-pause" => Some("togglePause".to_string()),
        _ => None,
    }
}

pub fn is_valid_platform_mode(mode: &str) -> bool {
    PLATFORM_MODES.contains(&mode)
}

pub fn is_finite_in(v: f32, lo: f32, hi: f32) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

pub fn normalize_deg(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if d > 360.0 - 1e-4 { 0.0 } else { d }
}

pub fn sanitize_trigger_extent(v: f32) -> f32 {
    if !v.is_finite() {
        2.0
    } else {
        v.max(0.1)
    }
}

pub fn sanitize_trigger_alpha(v: f32) -> f32 {
    if !v.is_finite() {
        TRIGGER_DEFAULT_ALPHA
    } else {
        v.clamp(TRIGGER_MIN_ALPHA, TRIGGER_MAX_ALPHA)
    }
}

"#;

const CLAIMS: &str = r#"// ---------------------------------------------------------------------------
// Identity: provider-agnostic auth claims (display only — the database verifies)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

pub type FirebaseClaims = AuthClaims;

pub fn decode_claims_unverified(token: &str) -> Result<AuthClaims> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1).ok_or_else(|| {
        ClientError::Invalid("Malformed token: expected header.payload.signature.".to_string())
    })?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| ClientError::Invalid(format!("Malformed token payload: {e}")))?;
    serde_json::from_slice(&bytes).map_err(ClientError::from)
}

"#;

const EVENTS: &str = r#"// ---------------------------------------------------------------------------
// Subscription events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableName {
    Player,
    DungeonPiece,
    PlatformPath,
    LevelMeta,
    DungeonLog,
}

impl TableName {
    pub fn as_str(self) -> &'static str {
        match self {
            TableName::Player => "player",
            TableName::DungeonPiece => "dungeon_piece",
            TableName::PlatformPath => "platform_path",
            TableName::LevelMeta => "level_meta",
            TableName::DungeonLog => "dungeon_log",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Insert,
    Update,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub table: TableName,
    pub op: Op,
    pub key: String,
}

pub trait HasRid {
    fn rid(&self) -> &str;
}

impl HasRid for Player {
    fn rid(&self) -> &str {
        &self.rid
    }
}
impl HasRid for Piece {
    fn rid(&self) -> &str {
        &self.rid
    }
}
impl HasRid for PlatformPath {
    fn rid(&self) -> &str {
        &self.piece_rid
    }
}
impl HasRid for LogEntry {
    fn rid(&self) -> &str {
        &self.rid
    }
}
impl HasRid for LevelMeta {
    fn rid(&self) -> &str {
        &self.rid
    }
}

"#;

const CLIENT_CORE: &str = r#"// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

fn v_str(s: impl Into<String>) -> Value {
    Value::from_string(s.into())
}
fn v_f32(v: f32) -> Value {
    Value::from_f32(v)
}
fn v_i64(v: i64) -> Value {
    Value::from_i64(v)
}
fn v_bool(v: bool) -> Value {
    Value::from_bool(v)
}
fn to_value<T: SurrealValue>(v: T) -> Value {
    v.into_value()
}
fn from_values<T: SurrealValue>(rows: Vec<Value>) -> Result<Vec<T>> {
    rows.into_iter()
        .map(|v| T::from_value(v).map_err(|e| ClientError::Store(e.to_string())))
        .collect()
}

fn new_rid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("{:x}-{:x}-{:x}", std::process::id(), ms, n)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A direct, authenticated SurrealDB connection with one typed method per
/// reducer. Methods issue the same SurrealQL as the backend reducer of the
/// same name; the row rules constrain what the caller's token may touch.
pub struct Client {
    db: Surreal<WsClient>,
}

/// Strip an optional URL scheme down to `host:port`.
fn endpoint(url: &str) -> String {
    let u = url.trim();
    for prefix in ["wss://", "ws://", "https://", "http://"] {
        if let Some(rest) = u.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    u.to_string()
}

impl Client {
    async fn open(url: &str, ns: &str, db_name: &str) -> Result<Self> {
        let db: Surreal<WsClient> = Surreal::new::<Ws>(&endpoint(url)).await?;
        db.use_ns(ns).use_db(db_name).await?;
        Ok(Self { db })
    }

    /// Connect as a player with an auth token (JWT) from the configured provider.
    pub async fn connect_with_token(url: &str, ns: &str, db: &str, jwt: &str) -> Result<Self> {
        let client = Self::open(url, ns, db).await?;
        client
            .db
            .authenticate(Token::from(AccessToken::from(jwt.to_string())))
            .await?;
        Ok(client)
    }

    /// Connect as the service role (database user). Bypasses row rules.
    pub async fn connect_service(
        url: &str,
        ns: &str,
        db: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        use surrealdb::opt::auth::Database as DbAuth;
        let client = Self::open(url, ns, db).await?;
        client
            .db
            .signin(DbAuth {
                namespace: ns.to_string(),
                database: db.to_string(),
                username: username.to_string(),
                password: password.to_string(),
            })
            .await?;
        Ok(client)
    }

    async fn run(
        &self,
        statements: &[&str],
        transactional: bool,
        mut vars: BTreeMap<String, Value>,
        now_ms: i64,
    ) -> Result<Vec<Vec<Value>>> {
        vars.entry("rid".to_string()).or_insert_with(|| Value::from_string(new_rid()));
        vars.entry("rid2".to_string()).or_insert_with(|| Value::from_string(new_rid()));
        vars.entry("now".to_string()).or_insert_with(|| Value::from_i64(now_ms));
        let batch = if transactional {
            format!("BEGIN TRANSACTION; {}; COMMIT TRANSACTION;", statements.join("; "))
        } else {
            statements.join("; ")
        };
        let mut q = self.db.query(batch);
        for (k, v) in &vars {
            q = q.bind((k.clone(), v.clone()));
        }
        let mut response = q.await?;
        let errors = response.take_errors();
        if !errors.is_empty() {
            let mut msgs: Vec<String> = errors
                .iter()
                .map(|(i, e)| format!("statement {i}: {e}"))
                .collect();
            msgs.sort();
            return Err(ClientError::Store(format!("reducer failed: {}", msgs.join("; "))));
        }
        let offset = usize::from(transactional);
        let mut out = Vec::with_capacity(statements.len());
        for i in 0..statements.len() {
            out.push(response.take(i + offset)?);
        }
        Ok(out)
    }

    async fn deny_or_missing(
        &self,
        table: &str,
        rid_param: Option<&str>,
        rid_literal: Option<&str>,
        vars: &BTreeMap<String, Value>,
        missing: &str,
    ) -> Result<()> {
        let sql = if let Some(p) = rid_param {
            format!("SELECT VALUE rid FROM {table} WHERE rid = ${p}")
        } else {
            format!(
                "SELECT VALUE rid FROM {table} WHERE rid = {}",
                rid_literal.unwrap_or("'?'")
            )
        };
        let mut q = self.db.query(sql);
        for (k, v) in vars {
            q = q.bind((k.clone(), v.clone()));
        }
        let mut response = q.await?;
        let seen: Vec<Value> = response.take(0)?;
        if seen.is_empty() {
            let rid = match rid_param.and_then(|p| vars.get(p)) {
                Some(Value::String(s)) => s.clone(),
                _ => rid_literal.unwrap_or("?").trim_matches('\'').to_string(),
            };
            return Err(ClientError::NotFound(missing.replace("{rid}", &rid)));
        }
        Err(ClientError::PermissionDenied(
            "reducer matched no rows (denied or missing).".to_string(),
        ))
    }

    async fn check(
        &self,
        table: &str,
        rid_param: Option<&str>,
        rid_literal: Option<&str>,
        vars: &BTreeMap<String, Value>,
        missing: &str,
    ) -> Result<()> {
        let sql = if let Some(p) = rid_param {
            format!("SELECT VALUE rid FROM {table} WHERE rid = ${p}")
        } else {
            format!(
                "SELECT VALUE rid FROM {table} WHERE rid = {}",
                rid_literal.unwrap_or("'?'")
            )
        };
        let mut q = self.db.query(sql);
        for (k, v) in vars {
            q = q.bind((k.clone(), v.clone()));
        }
        let mut response = q.await?;
        let rows: Vec<Value> = response.take(0)?;
        if rows.is_empty() {
            let rid = match rid_param.and_then(|p| vars.get(p)) {
                Some(Value::String(s)) => s.clone(),
                _ => rid_literal.unwrap_or("?").trim_matches('\'').to_string(),
            };
            return Err(ClientError::NotFound(missing.replace("{rid}", &rid)));
        }
        Ok(())
    }

    /// Open a `LIVE` query over one table.
    pub async fn subscribe<T>(&self, table: TableName) -> Result<LiveFeed<T>>
    where
        T: SurrealValue + HasRid + Send + Unpin + 'static,
    {
        let stream = self.db.select::<Vec<T>>(table.as_str()).live().await?;
        let boxed: Pin<
            Box<dyn Stream<Item = std::result::Result<Notification<T>, surrealdb::Error>> + Send>,
        > = Box::pin(stream);
        Ok(LiveFeed { seq: 0, table, stream: boxed })
    }

"#;

const LIVE_FEED: &str = r#"}

/// A live row feed. See [`Client::subscribe`].
pub struct LiveFeed<T> {
    seq: u64,
    table: TableName,
    stream: Pin<
        Box<dyn Stream<Item = std::result::Result<Notification<T>, surrealdb::Error>> + Send>,
    >,
}

impl<T: HasRid> LiveFeed<T> {
    /// Next batch of committed changes. `None` means the feed ended.
    pub async fn next(&mut self) -> Option<Result<Vec<Event>>> {
        let notification = match self.stream.next().await? {
            Ok(n) => n,
            Err(e) => return Some(Err(ClientError::Store(e.to_string()))),
        };
        let op = match notification.action {
            surrealdb::types::Action::Create => Op::Insert,
            surrealdb::types::Action::Update => Op::Update,
            surrealdb::types::Action::Delete => Op::Delete,
            surrealdb::types::Action::Killed => {
                return Some(Err(ClientError::Store(
                    "live query was killed by the server.".to_string(),
                )));
            }
            _ => {
                return Some(Err(ClientError::Store(format!(
                    "unknown live action: {:?}",
                    notification.action
                ))));
            }
        };
        self.seq += 1;
        Some(Ok(vec![Event {
            seq: self.seq,
            table: self.table,
            op,
            key: notification.data.rid().to_string(),
        }]))
    }
}
"#;

fn sig_params(def: &ReducerDef) -> String {
    let roots = transform_roots(def);
    let mut parts: Vec<String> = def
        .params
        .iter()
        .map(|(name, ty)| {
            let mutability = if roots.iter().any(|r| r == name) {
                "mut "
            } else {
                ""
            };
            match ty {
                ParamType::Str => format!("{name}: &str"),
                ParamType::NewPiece => format!("{mutability}{name}: NewPiece"),
                _ => format!("{name}: {}", ty.rust_type()),
            }
        })
        .collect();
    if uses_now(def) {
        parts.push("now_ms: i64".to_string());
    }
    parts.join(", ")
}

fn uses_now(def: &ReducerDef) -> bool {
    def.statements.iter().any(|s| s.contains("$now"))
}

fn emit_binds(out: &mut String, params: &[(&str, ParamType)]) {
    for (name, ty) in params {
        for (var, expr) in ty.binds(name) {
            out.push_str(&format!("        vars.insert({var:?}.to_string(), {expr});\n"));
        }
    }
}

fn render_transform(path: &str, t: crate::reducers::Transform) -> String {
    use crate::reducers::Transform::*;
    if let Some((root, field)) = path.split_once('.') {
        // Struct field: assign back through a mut binding.
        return match t {
            Trim => format!("{root}.{field} = {root}.{field}.trim().to_string();"),
            NormalizeDeg => format!("{root}.{field} = normalize_deg({root}.{field});"),
            SanitizeExtent => {
                format!("{root}.{field} = sanitize_trigger_extent({root}.{field});")
            }
            SanitizeAlpha => {
                format!("{root}.{field} = sanitize_trigger_alpha({root}.{field});")
            }
            CanonicalInteract => format!("{root}.{field} = canonical_interact(&{root}.{field}).ok_or_else(|| ClientError::Invalid(format!(\"Unknown interact action '{{}}'. Expected one of {{INTERACT_IDS:?}}\", {root}.{field})))?;"),
        };
    }
    match t {
        Trim => format!("let {path} = {path}.trim();"),
        NormalizeDeg => format!("let {path} = normalize_deg({path});"),
        SanitizeExtent => format!("let {path} = sanitize_trigger_extent({path});"),
        SanitizeAlpha => format!("let {path} = sanitize_trigger_alpha({path});"),
        CanonicalInteract => format!(
            "let {path} = canonical_interact({path}).ok_or_else(|| ClientError::Invalid(format!(\"Unknown interact action '{{}}'. Expected one of {{INTERACT_IDS:?}}\", {path})))?;"
        ),
    }
}

fn transform_roots(def: &ReducerDef) -> Vec<String> {
    let mut roots = Vec::new();
    for (path, _) in def.transforms {
        if let Some((root, _)) = path.split_once('.') {
            if !roots.iter().any(|r| r == root) {
                roots.push(root.to_string());
            }
        }
    }
    roots
}

fn lit_str(s: &str) -> String {
    format!("{s:?}")
}

fn render_reducer(def: &ReducerDef) -> String {
    if def.custom {
        return render_custom_connect(def);
    }
    let mut out = String::new();
    out.push_str(&format!("    /// {}\n", def.doc));
    let ret = match (def.result, def.result_row) {
        (ResultKind::OneRow, Some(t)) => format!("Result<{}>", t.rust_type()),
        (ResultKind::OptRow, Some(t)) => format!("Result<Option<{}>>", t.rust_type()),
        (ResultKind::Rows, Some(t)) => format!("Result<Vec<{}>>", t.rust_type()),
        (ResultKind::Unit, _) => "Result<()>".to_string(),
        (ResultKind::CountU64, _) => "Result<u64>".to_string(),
        _ => "Result<()>".to_string(),
    };
    let sig = sig_params(def);
    let sig_full = if sig.is_empty() {
        "&self".to_string()
    } else {
        format!("&self, {sig}")
    };
    out.push_str(&format!("    pub async fn {}({}) -> {} {{\n", def.name, sig_full, ret));
    for (name, t) in def.transforms {
        out.push_str(&format!("        {}\n", render_transform(name, *t)));
    }
    out.push_str(if def.params.is_empty() {
        "        let vars = BTreeMap::new();\n"
    } else {
        "        let mut vars = BTreeMap::new();\n"
    });
    emit_binds(&mut out, def.params);
    for check in def.checks {
        let (rp, rl) = match (check.rid_param, check.rid_literal) {
            (Some(p), _) => (format!("Some({p:?})"), "None".to_string()),
            (None, Some(l)) => ("None".to_string(), format!("Some({l:?})")),
            _ => ("None".to_string(), "None".to_string()),
        };
        out.push_str(&format!(
            "        self.check({:?}, {}, {}, &vars, {:?}).await?;\n",
            check.table, rp, rl, check.missing
        ));
    }
    let stmts: Vec<String> = def.statements.iter().map(|s| lit_str(s)).collect();
    let now_arg = if uses_now(def) { "now_ms" } else { "0" };
    out.push_str(&format!(
        "        let out = self.run(&[{}], {}, vars.clone(), {}).await?;\n",
        stmts.join(", "),
        def.transactional,
        now_arg
    ));
    match def.result {
        ResultKind::OneRow => {
            let t = def.result_row.map(|r| r.rust_type()).unwrap_or("()");
            out.push_str(&format!(
                "        let rows: Vec<{t}> = from_values(out.into_iter().nth({}).unwrap_or_default())?;\n",
                def.result_index
            ));
            out.push_str("        if let Some(row) = rows.into_iter().next() { return Ok(row); }\n");
            if def.probe.is_some() {
                out.push_str(&render_deny(def));
                out.push_str("        unreachable!(\"deny_or_missing always errors\")\n");
            } else {
                out.push_str("        return Err(ClientError::PermissionDenied(\"reducer matched no rows.\".to_string()));\n");
            }
        }
        ResultKind::OptRow => {
            let t = def.result_row.map(|r| r.rust_type()).unwrap_or("()");
            out.push_str(&format!(
                "        let rows: Vec<{t}> = from_values(out.into_iter().nth({}).unwrap_or_default())?;\n",
                def.result_index
            ));
            out.push_str("        Ok(rows.into_iter().next())\n");
        }
        ResultKind::Rows => {
            let t = def.result_row.map(|r| r.rust_type()).unwrap_or("()");
            out.push_str(&format!(
                "        let rows: Vec<{t}> = from_values(out.into_iter().nth({}).unwrap_or_default())?;\n",
                def.result_index
            ));
            out.push_str("        Ok(rows)\n");
        }
        ResultKind::Unit => {
            out.push_str(&format!(
                "        let rows: Vec<Value> = out.into_iter().nth({}).unwrap_or_default();\n",
                def.result_index
            ));
            out.push_str("        if rows.is_empty() {\n");
            out.push_str(&indent(&render_deny_call(def), 12));
            out.push_str("        }\n        Ok(())\n");
        }
        ResultKind::CountU64 => {
            out.push_str(&format!(
                "        let rows: Vec<Value> = out.into_iter().nth({}).unwrap_or_default();\n",
                def.result_index
            ));
            out.push_str("        Ok(rows.len() as u64)\n");
        }
    }
    out.push_str("    }\n\n");
    out
}

fn render_deny(def: &ReducerDef) -> String {
    render_deny_call(def)
}

fn render_deny_call(def: &ReducerDef) -> String {
    match def.probe {
        Some(p) => {
            let (rp, rl) = match (p.rid_param, p.rid_literal) {
                (Some(param), _) => (format!("Some({param:?})"), "None".to_string()),
                (None, Some(lit)) => ("None".to_string(), format!("Some({lit:?})")),
                _ => ("None".to_string(), "None".to_string()),
            };
            // The missing message carries the {rid} template from the registry.
            let missing = find_missing(def);
            format!(
                "self.deny_or_missing({:?}, {}, {}, &vars, {:?}).await?;\n",
                p.table, rp, rl, missing
            )
        }
        None => "return Err(ClientError::PermissionDenied(\"reducer matched no rows.\".to_string()));\n"
            .to_string(),
    }
}

fn find_missing(def: &ReducerDef) -> &str {
    // Every probed reducer documents its missing-row message in the probe
    // site; fall back to a generic message.
    match def.name {
        "set_level_name" => "level metadata missing.",
        "disconnect_player" => "No player '{rid}'.",
        "register_player" => "No player '{rid}'.",
        "move_player" => "No player '{rid}'.",
        "damage_player" => "No player '{rid}'.",
        "heal_player" => "No player '{rid}'.",
        "respawn_player" => "No player '{rid}'.",
        "update_piece" => "No piece with rid {rid}.",
        "delete_piece" => "No piece with rid {rid}.",
        "clear_platform_path" => "No platform path on piece {rid}.",
        _ => "row missing.",
    }
}

fn indent(s: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    s.lines()
        .map(|l| if l.is_empty() { l.to_string() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_custom_connect(def: &ReducerDef) -> String {
    // Custom orchestration (SELECT-then-branch), same statements as the
    // backend's connect_player.
    let upd = lit_str(def.statements[1]);
    let cre = lit_str(def.statements[2]);
    let log = lit_str(def.statements[3]);
    format!(
        r#"    /// {doc}
    pub async fn connect_player(&self, sub: &str, now_ms: i64) -> Result<Player> {{
        if self.get_player(sub).await?.is_some() {{
            let mut vars = BTreeMap::new();
            vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
            let out = self.run(&[{upd}], false, vars.clone(), now_ms).await?;
            let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
            if let Some(row) = rows.into_iter().next() {{
                let mut log_vars = BTreeMap::new();
                log_vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
                self.run(&[{log}], false, log_vars, now_ms).await?;
                return Ok(row);
            }}
            return Err(ClientError::PermissionDenied("reconnect matched no rows.".to_string()));
        }}
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
        vars.insert("hp".to_string(), Value::from_f32(DEFAULT_MAX_HP));
        match self.run(&[{cre}], false, vars.clone(), now_ms).await {{
            Ok(out) => {{
                let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
                if let Some(row) = rows.into_iter().next() {{
                    let mut log_vars = BTreeMap::new();
                    log_vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
                    self.run(&[{log}], false, log_vars, now_ms).await?;
                    return Ok(row);
                }}
                match self.get_player(sub).await? {{
                    Some(row) => Ok(row),
                    None => Err(ClientError::PermissionDenied(
                        "connect_player matched no rows.".to_string(),
                    )),
                }}
            }}
            Err(ClientError::Store(msg)) if msg.contains("already exists") => {{
                self.get_player(sub).await?.ok_or_else(|| {{
                    ClientError::Store("player appeared then vanished.".to_string())
                }})
            }}
            Err(e) => Err(e),
        }}
    }}

"#,
        doc = def.doc,
        upd = upd,
        log = log,
        cre = cre,
    )
}

fn render_query(q: &QueryDef) -> String {
    let mut out = String::new();
    out.push_str(&format!("    /// {}\n", q.doc));
    let ret = match (q.result, q.result_row) {
        (ResultKind::Rows, Some(t)) => format!("Result<Vec<{}>>", t.rust_type()),
        (ResultKind::OptRow, Some(t)) => format!("Result<Option<{}>>", t.rust_type()),
        _ => "Result<()>".to_string(),
    };
    let mut sig: Vec<String> = q
        .params
        .iter()
        .map(|(name, ty)| match ty {
            ParamType::Str => format!("{name}: &str"),
            _ => format!("{name}: {}", ty.rust_type()),
        })
        .collect();
    if let Some(lim) = q.truncate_to {
        if !q.params.iter().any(|(n, _)| n == &lim) {
            sig.push(format!("{lim}: i64"));
        }
    }
    let sig_full = if sig.is_empty() {
        "&self".to_string()
    } else {
        format!("&self, {}", sig.join(", "))
    };
    out.push_str(&format!("    pub async fn {}({}) -> {} {{\n", q.name, sig_full, ret));
    out.push_str(if q.params.is_empty() {
        "        let vars = BTreeMap::new();\n"
    } else {
        "        let mut vars = BTreeMap::new();\n"
    });
    emit_binds(&mut out, q.params);
    out.push_str(&format!(
        "        let out = self.run(&[{}], false, vars, 0).await?;\n",
        lit_str(q.sql)
    ));
    match q.result {
        ResultKind::Rows => {
            let t = q.result_row.map(|r| r.rust_type()).unwrap_or("()");
            out.push_str(&format!(
                "        let rows: Vec<{t}> = from_values(out.into_iter().next().unwrap_or_default())?;\n"
            ));
            if let Some(lim) = q.truncate_to {
                out.push_str(&format!(
                    "        let limit = ({lim} as usize).clamp(1, 500);\n        Ok(rows.into_iter().take(limit).collect())\n"
                ));
            } else {
                out.push_str("        Ok(rows)\n");
            }
        }
        ResultKind::OptRow => {
            let t = q.result_row.map(|r| r.rust_type()).unwrap_or("()");
            out.push_str(&format!(
                "        let rows: Vec<{t}> = from_values(out.into_iter().next().unwrap_or_default())?;\n"
            ));
            out.push_str("        Ok(rows.into_iter().next())\n");
        }
        _ => out.push_str("        Ok(())\n"),
    }
    out.push_str("    }\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_every_backend_reducer() {
        // Backend and client must expose the same reducer set.
        assert_eq!(REDUCERS.len(), 18);
        assert_eq!(QUERIES.len(), 8);
        let mut names: Vec<_> = REDUCERS.iter().map(|r| r.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), REDUCERS.len(), "duplicate reducer names");
    }

    #[test]
    fn generated_client_is_fresh() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("clients/rust");
        for (rel, content) in render_rust_client() {
            let path = root.join(&rel);
            let disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("generated file missing: {}", path.display()));
            assert_eq!(
                disk, content,
                "generated client drifted — run: cargo run -p party-dungeon-server --bin codegen"
            );
        }
    }
}
