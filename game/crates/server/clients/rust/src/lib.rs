//! Generated direct-connect client for Party Dungeon.
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

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
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

    /// Lobby metadata row, if the dungeon has been seeded.
    pub async fn level_name(&self) -> Result<Option<LevelMeta>> {
        let vars = BTreeMap::new();
        let out = self.run(&["SELECT * FROM level_meta WHERE rid = 'main'"], false, vars.clone(), 0).await?;
        let rows: Vec<LevelMeta> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        Ok(rows.into_iter().next())
    }

    /// Rename the dungeon and log the change.
    pub async fn set_level_name(&self, name: &str, actor: &str, now_ms: i64) -> Result<()> {
        let name = name.trim();
        let mut vars = BTreeMap::new();
        vars.insert("name".to_string(), v_str(name.to_owned()));
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        let out = self.run(&["UPDATE level_meta SET name = $name WHERE rid = 'main' RETURN *", "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'edit', actor: $actor, text: $name } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Value> = out.into_iter().nth(0).unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing("level_meta", None, Some("'main'"), &vars, "level metadata missing.").await?;
        }
        Ok(())
    }

    /// First-timers get a row (full HP at the origin); returning players resume and flip online.
    pub async fn connect_player(&self, sub: &str, now_ms: i64) -> Result<Player> {
        if self.get_player(sub).await?.is_some() {
            let mut vars = BTreeMap::new();
            vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
            let out = self.run(&["UPDATE player SET online = true, last_seen_ms = $now WHERE rid = $sub RETURN *"], false, vars.clone(), now_ms).await?;
            let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
            if let Some(row) = rows.into_iter().next() {
                let mut log_vars = BTreeMap::new();
                log_vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
                self.run(&["CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'join', actor: $sub, text: 'An adventurer connected.' } RETURN *"], false, log_vars, now_ms).await?;
                return Ok(row);
            }
            return Err(ClientError::PermissionDenied("reconnect matched no rows.".to_string()));
        }
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
        vars.insert("hp".to_string(), Value::from_f32(DEFAULT_MAX_HP));
        match self.run(&["CREATE player CONTENT { rid: $sub, name: 'Adventurer', x: 0, z: 0, feet: 0, yaw_deg: 0, hp: $hp, max_hp: $hp, online: true, last_seen_ms: $now } RETURN *"], false, vars.clone(), now_ms).await {
            Ok(out) => {
                let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
                if let Some(row) = rows.into_iter().next() {
                    let mut log_vars = BTreeMap::new();
                    log_vars.insert("sub".to_string(), Value::from_string(sub.to_owned()));
                    self.run(&["CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'join', actor: $sub, text: 'An adventurer connected.' } RETURN *"], false, log_vars, now_ms).await?;
                    return Ok(row);
                }
                match self.get_player(sub).await? {
                    Some(row) => Ok(row),
                    None => Err(ClientError::PermissionDenied(
                        "connect_player matched no rows.".to_string(),
                    )),
                }
            }
            Err(ClientError::Store(msg)) if msg.contains("already exists") => {
                self.get_player(sub).await?.ok_or_else(|| {
                    ClientError::Store("player appeared then vanished.".to_string())
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Flag offline, keep the row. Logs a leave entry.
    pub async fn disconnect_player(&self, sub: &str, now_ms: i64) -> Result<()> {
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), v_str(sub.to_owned()));
        let out = self.run(&["UPDATE player SET online = false, last_seen_ms = $now WHERE rid = $sub RETURN *", "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'leave', actor: $sub, text: 'An adventurer disconnected.' } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Value> = out.into_iter().nth(0).unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing("player", Some("sub"), None, &vars, "No player '{rid}'.").await?;
        }
        Ok(())
    }

    /// Claim a display name. Authority stays with the Firebase UID key.
    pub async fn register_player(&self, sub: &str, name: &str, now_ms: i64) -> Result<Player> {
        let name = name.trim();
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), v_str(sub.to_owned()));
        vars.insert("name".to_string(), v_str(name.to_owned()));
        let out = self.run(&["UPDATE player SET name = $name, last_seen_ms = $now WHERE rid = $sub RETURN *", "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'system', actor: $sub, text: $name } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Player> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("player", Some("sub"), None, &vars, "No player '{rid}'.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Authoritative movement. Every sample is clamped into the world.
    pub async fn move_player(&self, sub: &str, x: f32, z: f32, feet: f32, yaw_deg: f32, now_ms: i64) -> Result<Player> {
        let yaw_deg = normalize_deg(yaw_deg);
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), v_str(sub.to_owned()));
        vars.insert("x".to_string(), v_f32(x));
        vars.insert("z".to_string(), v_f32(z));
        vars.insert("feet".to_string(), v_f32(feet));
        vars.insert("yaw_deg".to_string(), v_f32(yaw_deg));
        let out = self.run(&["UPDATE player SET x = $x, z = $z, feet = $feet, yaw_deg = $yaw_deg, last_seen_ms = $now WHERE rid = $sub RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Player> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("player", Some("sub"), None, &vars, "No player '{rid}'.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Deal damage, floored at zero. When the returned hp is 0, call log_death.
    pub async fn damage_player(&self, target: &str, amount: f32, now_ms: i64) -> Result<Player> {
        let mut vars = BTreeMap::new();
        vars.insert("target".to_string(), v_str(target.to_owned()));
        vars.insert("amount".to_string(), v_f32(amount));
        let out = self.run(&["UPDATE player SET hp = math::max([0, hp - $amount]), last_seen_ms = $now WHERE rid = $target RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Player> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("player", Some("target"), None, &vars, "No player '{rid}'.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Record a kill in the feed. Call when damage_player returns hp 0.
    pub async fn log_death(&self, actor: &str, target: &str, now_ms: i64) -> Result<LogEntry> {
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        vars.insert("target".to_string(), v_str(target.to_owned()));
        let out = self.run(&["CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'kill', actor: $actor, text: $target } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<LogEntry> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
        return Err(ClientError::PermissionDenied("reducer matched no rows.".to_string()));
    }

    /// Restore health, capped at the target's maximum.
    pub async fn heal_player(&self, target: &str, amount: f32, now_ms: i64) -> Result<Player> {
        let mut vars = BTreeMap::new();
        vars.insert("target".to_string(), v_str(target.to_owned()));
        vars.insert("amount".to_string(), v_f32(amount));
        let out = self.run(&["UPDATE player SET hp = math::min([max_hp, hp + $amount]), last_seen_ms = $now WHERE rid = $target RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Player> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("player", Some("target"), None, &vars, "No player '{rid}'.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Full health at the first spawn marker (or the origin).
    pub async fn respawn_player(&self, sub: &str, now_ms: i64) -> Result<Player> {
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), v_str(sub.to_owned()));
        let out = self.run(&["UPDATE player SET x = ((SELECT VALUE x FROM dungeon_piece WHERE shape = 'spawn' ORDER BY rid LIMIT 1)[0] ?? 0), z = ((SELECT VALUE z FROM dungeon_piece WHERE shape = 'spawn' ORDER BY rid LIMIT 1)[0] ?? 0), feet = 0, hp = max_hp, last_seen_ms = $now WHERE rid = $sub RETURN *", "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'system', actor: $sub, text: 'Respawned at the entry.' } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Player> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("player", Some("sub"), None, &vars, "No player '{rid}'.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Place a new solid. Unknown shape ids never reach a subscriber.
    pub async fn create_piece(&self, mut piece: NewPiece, actor: &str, now_ms: i64) -> Result<Piece> {
        piece.rot_deg = normalize_deg(piece.rot_deg);
        piece.pitch_deg = normalize_deg(piece.pitch_deg);
        piece.roll_deg = normalize_deg(piece.roll_deg);
        piece.trigger_w = sanitize_trigger_extent(piece.trigger_w);
        piece.trigger_h = sanitize_trigger_extent(piece.trigger_h);
        piece.trigger_l = sanitize_trigger_extent(piece.trigger_l);
        piece.trigger_alpha = sanitize_trigger_alpha(piece.trigger_alpha);
        piece.interact = canonical_interact(&piece.interact).ok_or_else(|| ClientError::Invalid(format!("Unknown interact action '{}'. Expected one of {INTERACT_IDS:?}", piece.interact)))?;
        let mut vars = BTreeMap::new();
        vars.insert("shape".to_string(), v_str(piece.shape.clone()));
        vars.insert("x".to_string(), v_f32(piece.x));
        vars.insert("z".to_string(), v_f32(piece.z));
        vars.insert("elevation".to_string(), v_f32(piece.elevation));
        vars.insert("rot_deg".to_string(), v_f32(piece.rot_deg));
        vars.insert("pitch_deg".to_string(), v_f32(piece.pitch_deg));
        vars.insert("roll_deg".to_string(), v_f32(piece.roll_deg));
        vars.insert("dynamic".to_string(), v_bool(piece.dynamic));
        vars.insert("damage".to_string(), v_f32(piece.damage));
        vars.insert("interact".to_string(), v_str(piece.interact.clone()));
        vars.insert("trigger_w".to_string(), v_f32(piece.trigger_w));
        vars.insert("trigger_h".to_string(), v_f32(piece.trigger_h));
        vars.insert("trigger_l".to_string(), v_f32(piece.trigger_l));
        vars.insert("trigger_alpha".to_string(), v_f32(piece.trigger_alpha));
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        let out = self.run(&["CREATE dungeon_piece CONTENT { rid: $rid, shape: $shape, x: $x, z: $z, elevation: $elevation, rot_deg: $rot_deg, pitch_deg: $pitch_deg, roll_deg: $roll_deg, dynamic: $dynamic, damage: $damage, interact: $interact, trigger_w: $trigger_w, trigger_h: $trigger_h, trigger_l: $trigger_l, trigger_alpha: $trigger_alpha } RETURN *", "CREATE dungeon_log CONTENT { rid: $rid2, at_ms: $now, kind: 'edit', actor: $actor, text: 'Placed a piece.' } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
        return Err(ClientError::PermissionDenied("reducer matched no rows.".to_string()));
    }

    /// Rewrite a placed solid in place (same rid, new shape/transform).
    pub async fn update_piece(&self, rid: &str, mut piece: NewPiece) -> Result<Piece> {
        piece.rot_deg = normalize_deg(piece.rot_deg);
        piece.pitch_deg = normalize_deg(piece.pitch_deg);
        piece.roll_deg = normalize_deg(piece.roll_deg);
        piece.trigger_w = sanitize_trigger_extent(piece.trigger_w);
        piece.trigger_h = sanitize_trigger_extent(piece.trigger_h);
        piece.trigger_l = sanitize_trigger_extent(piece.trigger_l);
        piece.trigger_alpha = sanitize_trigger_alpha(piece.trigger_alpha);
        piece.interact = canonical_interact(&piece.interact).ok_or_else(|| ClientError::Invalid(format!("Unknown interact action '{}'. Expected one of {INTERACT_IDS:?}", piece.interact)))?;
        let mut vars = BTreeMap::new();
        vars.insert("rid".to_string(), v_str(rid.to_owned()));
        vars.insert("shape".to_string(), v_str(piece.shape.clone()));
        vars.insert("x".to_string(), v_f32(piece.x));
        vars.insert("z".to_string(), v_f32(piece.z));
        vars.insert("elevation".to_string(), v_f32(piece.elevation));
        vars.insert("rot_deg".to_string(), v_f32(piece.rot_deg));
        vars.insert("pitch_deg".to_string(), v_f32(piece.pitch_deg));
        vars.insert("roll_deg".to_string(), v_f32(piece.roll_deg));
        vars.insert("dynamic".to_string(), v_bool(piece.dynamic));
        vars.insert("damage".to_string(), v_f32(piece.damage));
        vars.insert("interact".to_string(), v_str(piece.interact.clone()));
        vars.insert("trigger_w".to_string(), v_f32(piece.trigger_w));
        vars.insert("trigger_h".to_string(), v_f32(piece.trigger_h));
        vars.insert("trigger_l".to_string(), v_f32(piece.trigger_l));
        vars.insert("trigger_alpha".to_string(), v_f32(piece.trigger_alpha));
        let out = self.run(&["UPDATE dungeon_piece CONTENT { rid: $rid, shape: $shape, x: $x, z: $z, elevation: $elevation, rot_deg: $rot_deg, pitch_deg: $pitch_deg, roll_deg: $roll_deg, dynamic: $dynamic, damage: $damage, interact: $interact, trigger_w: $trigger_w, trigger_h: $trigger_h, trigger_l: $trigger_l, trigger_alpha: $trigger_alpha } WHERE rid = $rid RETURN *"], false, vars.clone(), 0).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("dungeon_piece", Some("rid"), None, &vars, "No piece with rid {rid}.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Remove a solid and its mover track atomically.
    pub async fn delete_piece(&self, rid: &str, actor: &str, now_ms: i64) -> Result<Piece> {
        let mut vars = BTreeMap::new();
        vars.insert("rid".to_string(), v_str(rid.to_owned()));
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        let out = self.run(&["DELETE platform_path WHERE piece_rid = $rid", "DELETE dungeon_piece WHERE rid = $rid RETURN BEFORE", "CREATE dungeon_log CONTENT { rid: $rid2, at_ms: $now, kind: 'edit', actor: $actor, text: $rid } RETURN *"], true, vars.clone(), now_ms).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().nth(1).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
self.deny_or_missing("dungeon_piece", Some("rid"), None, &vars, "No piece with rid {rid}.").await?;
        unreachable!("deny_or_missing always errors")
    }

    /// Wipe the whole dungeon (pieces + paths). The feed entry survives.
    pub async fn clear_level(&self, actor: &str, now_ms: i64) -> Result<()> {
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        let out = self.run(&["DELETE platform_path", "DELETE dungeon_piece", "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'edit', actor: $actor, text: 'Cleared the level.' } RETURN *"], true, vars.clone(), now_ms).await?;
        let rows: Vec<Value> = out.into_iter().nth(0).unwrap_or_default();
        if rows.is_empty() {
            return Err(ClientError::PermissionDenied("reducer matched no rows.".to_string()));
        }
        Ok(())
    }

    /// Attach (or replace) a mover track on an existing piece.
    pub async fn set_platform_path(&self, piece_rid: &str, mode: &str, speed: f32, paused: bool, waypoints: Vec<Waypoint>) -> Result<PlatformPath> {
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), v_str(piece_rid.to_owned()));
        vars.insert("mode".to_string(), v_str(mode.to_owned()));
        vars.insert("speed".to_string(), v_f32(speed));
        vars.insert("paused".to_string(), v_bool(paused));
        vars.insert("waypoints".to_string(), to_value(waypoints));
        self.check("dungeon_piece", Some("piece_rid"), None, &vars, "No piece with rid {rid}.").await?;
        let out = self.run(&["DELETE platform_path WHERE piece_rid = $piece_rid", "CREATE platform_path CONTENT { piece_rid: $piece_rid, mode: $mode, speed: $speed, paused: $paused, waypoints: $waypoints } RETURN *"], true, vars.clone(), 0).await?;
        let rows: Vec<PlatformPath> = from_values(out.into_iter().nth(1).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
        return Err(ClientError::PermissionDenied("reducer matched no rows.".to_string()));
    }

    /// Remove a mover track; the piece itself stays put.
    pub async fn clear_platform_path(&self, piece_rid: &str) -> Result<()> {
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), v_str(piece_rid.to_owned()));
        let out = self.run(&["DELETE platform_path WHERE piece_rid = $piece_rid RETURN BEFORE"], false, vars.clone(), 0).await?;
        let rows: Vec<Value> = out.into_iter().nth(0).unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing("platform_path", Some("piece_rid"), None, &vars, "No platform path on piece {rid}.").await?;
        }
        Ok(())
    }

    /// Post a chat line to the shared feed.
    pub async fn send_message(&self, actor: &str, text: &str, now_ms: i64) -> Result<LogEntry> {
        let text = text.trim();
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), v_str(actor.to_owned()));
        vars.insert("text".to_string(), v_str(text.to_owned()));
        let out = self.run(&["CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'chat', actor: $actor, text: $text } RETURN *"], false, vars.clone(), now_ms).await?;
        let rows: Vec<LogEntry> = from_values(out.into_iter().nth(0).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() { return Ok(row); }
        return Err(ClientError::PermissionDenied("reducer matched no rows.".to_string()));
    }

    /// Bound feed growth: keep the newest 200 rows, drop the rest.
    pub async fn prune_log(&self) -> Result<u64> {
        let vars = BTreeMap::new();
        let out = self.run(&["DELETE (SELECT * FROM dungeon_log ORDER BY at_ms DESC LIMIT 1000000000 START 200) RETURN BEFORE"], false, vars.clone(), 0).await?;
        let rows: Vec<Value> = out.into_iter().nth(0).unwrap_or_default();
        Ok(rows.len() as u64)
    }

    /// Live roster, ordered by name.
    pub async fn list_players(&self) -> Result<Vec<Player>> {
        let vars = BTreeMap::new();
        let out = self.run(&["SELECT * FROM player ORDER BY name"], false, vars, 0).await?;
        let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows)
    }

    /// Connected adventurers, ordered by name.
    pub async fn list_online_players(&self) -> Result<Vec<Player>> {
        let vars = BTreeMap::new();
        let out = self.run(&["SELECT * FROM player WHERE online = true ORDER BY name"], false, vars, 0).await?;
        let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows)
    }

    /// One adventurer by Firebase UID, if visible.
    pub async fn get_player(&self, sub: &str) -> Result<Option<Player>> {
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), v_str(sub.to_owned()));
        let out = self.run(&["SELECT * FROM player WHERE rid = $sub"], false, vars, 0).await?;
        let rows: Vec<Player> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows.into_iter().next())
    }

    /// The whole dungeon, ordered by rid.
    pub async fn list_pieces(&self) -> Result<Vec<Piece>> {
        let vars = BTreeMap::new();
        let out = self.run(&["SELECT * FROM dungeon_piece ORDER BY rid"], false, vars, 0).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows)
    }

    /// One solid by rid, if visible.
    pub async fn get_piece(&self, rid: &str) -> Result<Option<Piece>> {
        let mut vars = BTreeMap::new();
        vars.insert("rid".to_string(), v_str(rid.to_owned()));
        let out = self.run(&["SELECT * FROM dungeon_piece WHERE rid = $rid"], false, vars, 0).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows.into_iter().next())
    }

    /// All mover tracks, ordered by piece.
    pub async fn list_paths(&self) -> Result<Vec<PlatformPath>> {
        let vars = BTreeMap::new();
        let out = self.run(&["SELECT * FROM platform_path ORDER BY piece_rid"], false, vars, 0).await?;
        let rows: Vec<PlatformPath> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows)
    }

    /// One mover track by piece rid, if visible.
    pub async fn get_path(&self, piece_rid: &str) -> Result<Option<PlatformPath>> {
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), v_str(piece_rid.to_owned()));
        let out = self.run(&["SELECT * FROM platform_path WHERE piece_rid = $piece_rid"], false, vars, 0).await?;
        let rows: Vec<PlatformPath> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows.into_iter().next())
    }

    /// Newest-first feed window (server caps at 500 rows).
    pub async fn recent_log(&self, limit: i64) -> Result<Vec<LogEntry>> {
        let mut vars = BTreeMap::new();
        vars.insert("limit".to_string(), v_i64(limit));
        let out = self.run(&["SELECT * FROM dungeon_log ORDER BY at_ms DESC LIMIT 500"], false, vars, 0).await?;
        let rows: Vec<LogEntry> = from_values(out.into_iter().next().unwrap_or_default())?;
        let limit = (limit as usize).clamp(1, 500);
        Ok(rows.into_iter().take(limit).collect())
    }

}

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
