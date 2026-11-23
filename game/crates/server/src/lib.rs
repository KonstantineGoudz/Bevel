//! Authoritative backend for Party Dungeon, hosted on SurrealDB.
//!
//! Clients hold direct, authenticated connections to the database. Game logic
//! lives in two places, split by who enforces it:
//!
//! - **Row permissions** (`schema.surql`) decide which authenticated callers
//!   may touch which tables. Every game table requires a JWT from the
//!   configured auth provider (Firebase, Auth0, Clerk, …) whose `iss`/`aud`
//!   match the provider's expected values; anything else — other tenants'
//!   tokens, the local dev login, anonymous connections — matches no rows.
//!   Swap the provider via `AuthConfig` (`DUNGEON_AUTH_PROVIDER`) — no code
//!   or schema change.
//! - **Commands** ([`Backend`]) validate game rules the schema cannot express
//!   (shape catalogue, world bounds, HP clamping, waypoint counts) and issue
//!   parameterized SurrealQL. The service layer connects with a database
//!   user that bypasses row rules, so the same validation applies to every
//!   write path.
//!
//! Reads are subscriptions, not polls: [`Backend::subscribe`] opens a `LIVE`
//! query and streams [`Event`]s as rows change.
//!
//! ```no_run
//! use party_dungeon_server::{Backend, SurrealConfig};
//!
//! # async fn example() -> Result<(), party_dungeon_server::BackendError> {
//! let backend = Backend::connect_service(
//!     &SurrealConfig::from_env()?,
//!     "service",
//!     "change-me",
//! ).await?;
//! let me = backend.connect_player("firebase-uid-123", 0).await?;
//! backend.register_player(&me.rid, "Quinn", 1).await?;
//! # Ok(())
//! # }
//! ```

pub mod auth;
pub mod backend;
pub mod codegen;
pub mod reducers;

pub use auth::{
    decode_claims_unverified, AuthClaims, AuthConfig, AuthProvider, AudValue, FirebaseClaims,
};
pub use backend::Backend;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

// ---------------------------------------------------------------------------
// Configuration (environment-driven: compose today, Kubernetes tomorrow)
// ---------------------------------------------------------------------------

/// Connection settings. Every field is overridable by environment variable
/// so the same binary runs against compose, staging, and prod without
/// recompiling.
#[derive(Clone, Debug)]
pub struct SurrealConfig {
    /// Server address as `host:port` (e.g. `127.0.0.1:6000`). A leading
    /// `ws://`, `wss://`, `http://`, or `https://` is accepted and stripped.
    pub url: String,
    pub namespace: String,
    pub database: String,
    /// Auth provider. Decoupled from any single IdP — swap via env without
    /// touching code or schema. The row rules and `DEFINE ACCESS` are derived
    /// from this, not from a hard-coded Firebase project.
    pub auth: AuthConfig,
    /// Deprecated: use `auth` instead. Kept for backward compat with
    /// `DUNGEON_FIREBASE_PROJECT_ID` and existing struct literals.
    pub firebase_project_id: String,
}

impl SurrealConfig {
    /// Address in the `host:port` form the client expects.
    pub fn endpoint(&self) -> String {
        let u = self.url.trim();
        for prefix in ["wss://", "ws://", "https://", "http://"] {
            if let Some(rest) = u.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
        u.to_string()
    }

    /// The auth provider's row predicate (e.g. `$token.iss == ...`).
    pub fn auth_row_predicate(&self) -> String {
        self.auth.provider.row_predicate_sql()
    }
}

impl SurrealConfig {
    pub fn from_env() -> Result<Self> {
        let auth = AuthConfig::from_env()?;
        // Keep the legacy field in sync for callers that still read it.
        let firebase_project_id = match &auth.provider {
            AuthProvider::Firebase { project_id } => project_id.clone(),
            _ => std::env::var("DUNGEON_FIREBASE_PROJECT_ID").unwrap_or_default(),
        };
        Ok(Self {
            url: std::env::var("DUNGEON_SURREAL_URL")
                .unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string()),
            namespace: std::env::var("DUNGEON_SURREAL_NS")
                .unwrap_or_else(|_| "party_dungeon".to_string()),
            database: std::env::var("DUNGEON_SURREAL_DB")
                .unwrap_or_else(|_| "main".to_string()),
            auth,
            firebase_project_id,
        })
    }

    /// Construct for tests / explicit wiring without env.
    pub fn new(url: String, namespace: String, database: String, auth: AuthConfig) -> Self {
        let firebase_project_id = match &auth.provider {
            AuthProvider::Firebase { project_id } => project_id.clone(),
            _ => String::new(),
        };
        Self { url, namespace, database, auth, firebase_project_id }
    }

    /// Helper for the common case in tests — Firebase with a fake project.
    pub fn for_tests_firebase(project_id: impl Into<String>) -> Self {
        let project_id = project_id.into();
        let auth = AuthConfig::firebase_for_tests(project_id.clone());
        Self {
            url: "ws://127.0.0.1:6000".to_string(),
            namespace: "test".to_string(),
            database: "test".to_string(),
            auth,
            firebase_project_id: project_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Tuning / validation constants (mirror `game/crates/game` catalogue rules)
// ---------------------------------------------------------------------------

/// Half-extent of the playable world in metres. Commands reject moves and
/// placements outside `[-WORLD_BOUND, WORLD_BOUND]`.
pub const WORLD_BOUND: f32 = 500.0;
/// Piece lift range in metres.
pub const ELEV_MIN: f32 = -50.0;
/// Piece lift range in metres.
pub const ELEV_MAX: f32 = 200.0;
/// Longest player name callers may pass.
pub const NAME_MAX_CHARS: usize = 24;
/// Longest chat message.
pub const MESSAGE_MAX_CHARS: usize = 280;
/// Fastest platform speed in m/s.
pub const PLATFORM_SPEED_MAX: f32 = 30.0;
/// Platform path waypoint count bounds.
pub const WAYPOINTS_MIN: usize = 2;
/// Platform path waypoint count bounds.
pub const WAYPOINTS_MAX: usize = 16;
/// Hard cap for any health maximum, so a bad value can't mint immortals.
pub const MAX_HP_CAP: f32 = 10_000.0;
/// Fresh adventurer health. Mirrors `statuses::DEFAULT_MAX_HEALTH`.
pub const DEFAULT_MAX_HP: f32 = 100.0;
/// How many feed rows [`Backend::prune_log`] keeps.
pub const LOG_KEEP_ROWS: usize = 200;
/// Current snapshot format version for [`LevelSnapshot`].
pub const SNAPSHOT_VERSION: u32 = 1;

/// Every shape id the level editor can place. Mirrors the `Shape::id`
/// catalogue in `game/crates/game/src/model.rs`; commands reject anything
/// else so clients can never smuggle an unknown solid into the dungeon.
pub const SHAPE_IDS: &[&str] = &[
    "floor", "pit", "water", "bridge", //
    "wall", "fence", "window", "pillar", //
    "block", "ramp", //
    "door", "gate", "secret", "rubble", //
    "crate", "chest", "altar", "brazier", //
    "herb", "ore", //
    "spawn", "entry", //
    "spikes", //
    "trigger", "trigger_box", "trigger_sphere", "trigger_cylinder", //
    "glade_tower", "glade_room", "glade_wall", "glade_stairs", "glade_door", "glade_window",
    "glade_terrain", "glade_tree", "glade_path",
];

/// Interaction effect ids. Mirrors `InteractAction::id` in `model.rs`.
pub const INTERACT_IDS: &[&str] = &["none", "togglePause"];
/// Moving-platform travel modes. Mirrors `PlatformMode::id` in `platforms.rs`.
pub const PLATFORM_MODES: &[&str] = &["pingpong", "loop", "oneshot"];
/// Default trigger alpha / glade roof-pitch alpha. Mirrors
/// `model::TRIGGER_DEFAULT_ALPHA` in the game crate.
pub const TRIGGER_DEFAULT_ALPHA: f32 = 0.35;
/// Valid trigger alpha range (also glade roof pitch = alpha * 2).
pub const TRIGGER_MIN_ALPHA: f32 = 0.0;
pub const TRIGGER_MAX_ALPHA: f32 = 1.0;

/// Feed entry kinds stored in `LogEntry::kind`.
pub mod log_kind {
    pub const JOIN: &str = "join";
    pub const LEAVE: &str = "leave";
    pub const KILL: &str = "kill";
    pub const EDIT: &str = "edit";
    pub const CHAT: &str = "chat";
    pub const SYSTEM: &str = "system";
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong in a command.
#[derive(Debug)]
pub enum BackendError {
    /// Caller input failed validation. Nothing was written.
    Invalid(String),
    /// The referenced row does not exist.
    NotFound(String),
    /// The caller is authenticated but the row rules matched no rows.
    /// SurrealDB answers denied writes with empty results rather than
    /// errors, so the command layer converts "wrote nothing" into this.
    PermissionDenied(String),
    /// Missing/invalid environment configuration.
    Config(String),
    /// The store itself failed.
    Store(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::Invalid(msg) => write!(f, "invalid: {msg}"),
            BackendError::NotFound(msg) => write!(f, "not found: {msg}"),
            BackendError::PermissionDenied(msg) => write!(f, "permission denied: {msg}"),
            BackendError::Config(msg) => write!(f, "config: {msg}"),
            BackendError::Store(msg) => write!(f, "store: {msg}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<surrealdb::Error> for BackendError {
    fn from(err: surrealdb::Error) -> Self {
        BackendError::Store(err.to_string())
    }
}

impl From<serde_json::Error> for BackendError {
    fn from(err: serde_json::Error) -> Self {
        BackendError::Invalid(format!("bad snapshot JSON: {err}"))
    }
}

pub type Result<T> = std::result::Result<T, BackendError>;

// ---------------------------------------------------------------------------
// Row types — one struct per table. `rid` is the application key: a stored
// string field (Firebase UID for players, generated otherwise), so commands
// and live payloads address rows without touching record-id internals.
// ---------------------------------------------------------------------------

/// One adventurer. `rid` is the Firebase UID (`sub` claim).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Player {
    pub rid: String,
    pub name: String,
    pub x: f32,
    pub z: f32,
    pub feet: f32,
    /// Facing yaw in degrees, normalized to `[0, 360)`.
    pub yaw_deg: f32,
    /// Current health, always in `[0, max_hp]`.
    pub hp: f32,
    pub max_hp: f32,
    /// `false` after [`Backend::disconnect_player`]; the row is kept.
    pub online: bool,
    /// Last command time in millis since the Unix epoch.
    pub last_seen_ms: i64,
}

/// One placed level solid. `shape` is a [`SHAPE_IDS`] entry; geometry and
/// colour stay client-side in `model.rs`, so art tweaks never need a schema
/// migration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Piece {
    /// Application key, generated by [`new_rid`].
    pub rid: String,
    pub shape: String,
    pub x: f32,
    pub z: f32,
    /// Lift above the natural placement in metres.
    pub elevation: f32,
    pub rot_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    /// Physics-driven piece (falls and collides).
    pub dynamic: bool,
    /// Touch damage per hit; `0` means harmless.
    pub damage: f32,
    /// Interaction effect id (`"none"` or `"togglePause"`).
    pub interact: String,
    /// Parametric trigger volume extents; meaningful for triggers.
    pub trigger_w: f32,
    pub trigger_h: f32,
    pub trigger_l: f32,
    /// Glade roof pitch (`roof_pitch = trigger_alpha * 2.0`) / trigger-zone
    /// opacity in `0.0..=1.0`. Gameplay for glade pieces; an editor-local
    /// cosmetic for other shapes.
    pub trigger_alpha: f32,
}

/// Arguments for placing a piece. `rid` is assigned by the store layer.
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
/// position. Mirrors the palette defaults in the Bevy editor.
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

/// A control point on a moving-platform track. Handles are intentionally
/// omitted in v1 (position-only paths); the curve is rebuilt client-side by
/// the same Bezier code as `platforms.rs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct Waypoint {
    pub x: f32,
    pub z: f32,
}

/// Mover track for one [`Piece`], addressed by that piece's `rid`. Kept in
/// its own table so path edits never rewrite geometry rows and path
/// subscriptions stay small. Deleted alongside its piece by the command
/// layer (the store has no foreign keys; the pair ships in one transaction).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct PlatformPath {
    pub piece_rid: String,
    /// One of [`PLATFORM_MODES`].
    pub mode: String,
    /// Travel speed in m/s.
    pub speed: f32,
    /// Paused paths hold their piece in place.
    pub paused: bool,
    /// Track control points, [`WAYPOINTS_MIN`]–[`WAYPOINTS_MAX`] entries.
    pub waypoints: Vec<Waypoint>,
}

/// One entry in the shared event feed (joins, kills, edits, chat).
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

/// Portable level snapshot: everything needed to move a dungeon between
/// databases or seed one from the editor's JSON. See
/// [`Backend::export_snapshot`] and [`Backend::import_snapshot`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelSnapshot {
    pub version: u32,
    pub name: String,
    pub pieces: Vec<Piece>,
    pub paths: Vec<PlatformPath>,
}

// ---------------------------------------------------------------------------
// Subscription events — the portable face of LIVE query notifications
// ---------------------------------------------------------------------------

/// Which table changed.
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

/// How a row changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Insert,
    Update,
    Delete,
}

/// One committed change. `seq` is a client-side monotonic counter per
/// subscription (the store does not sequence across tables); `key` is the
/// affected row's `rid`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub seq: u64,
    pub table: TableName,
    pub op: Op,
    pub key: String,
}

/// Rows that carry the application key, so live notifications map to [`Event`]s.
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
// Identity — provider-agnostic claims.
// ---------------------------------------------------------------------------
// `AuthClaims` / `decode_claims_unverified` live in `auth.rs` so the rest
// of the crate never imports a Firebase/Auth0 type directly. `FirebaseClaims`
// is a type alias kept for backward compat with callers that still mention
// it.

// ---------------------------------------------------------------------------
// Pure helpers (no store access; unit-tested below)
// ---------------------------------------------------------------------------

static RID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Application row key: process-unique, time-ordered, URL-safe. Stored as a
/// plain `rid` field so commands and live payloads address rows identically.
pub fn new_rid() -> String {
    let n = RID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}-{:x}", std::process::id(), now_ms() as u64, n)
}

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

/// Clamp a trigger alpha / glade roof-pitch alpha into `0.0..=1.0`; a
/// non-finite value falls back to [`TRIGGER_DEFAULT_ALPHA`].
pub fn sanitize_trigger_alpha(v: f32) -> f32 {
    if !v.is_finite() {
        TRIGGER_DEFAULT_ALPHA
    } else {
        v.clamp(TRIGGER_MIN_ALPHA, TRIGGER_MAX_ALPHA)
    }
}

/// Millis since the Unix epoch, for stamping commands. Passed into commands
/// explicitly so game logic stays deterministic and tests stay hermetic.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(BackendError::Invalid("Name must not be empty.".to_string()));
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(BackendError::Invalid(format!(
            "Name must be at most {NAME_MAX_CHARS} characters."
        )));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_waypoints(waypoints: &[Waypoint]) -> Result<()> {
    if !(WAYPOINTS_MIN..=WAYPOINTS_MAX).contains(&waypoints.len()) {
        return Err(BackendError::Invalid(format!(
            "Path needs {WAYPOINTS_MIN}–{WAYPOINTS_MAX} waypoints, got {}.",
            waypoints.len()
        )));
    }
    for w in waypoints {
        if !is_finite_in(w.x, -WORLD_BOUND, WORLD_BOUND)
            || !is_finite_in(w.z, -WORLD_BOUND, WORLD_BOUND)
        {
            return Err(BackendError::Invalid(format!(
                "Waypoints must be within ±{WORLD_BOUND} m."
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_covers_every_editor_shape() {
        // 36 shapes across Surface / Barrier / Traversal / Threshold /
        // Fixture / Resource / Marker / Hazard / Trigger / Building.
        assert_eq!(SHAPE_IDS.len(), 36);
        for id in [
            "floor", "wall", "block", "door", "crate", "herb", "spawn", "spikes", "trigger",
        ] {
            assert!(is_valid_shape(id), "missing {id}");
        }
        assert!(!is_valid_shape("teleport"));
        assert!(!is_valid_shape(""));
    }

    #[test]
    fn interact_ids_round_trip() {
        assert_eq!(canonical_interact("none").as_deref(), Some("none"));
        assert_eq!(canonical_interact("togglePause").as_deref(), Some("togglePause"));
        assert_eq!(canonical_interact("TOGGLE_PAUSE").as_deref(), Some("togglePause"));
        assert_eq!(canonical_interact("teleport"), None);
    }

    #[test]
    fn degrees_fold_into_range() {
        assert!((normalize_deg(720.0) - 0.0).abs() < 1e-3);
        assert!((normalize_deg(-90.0) - 270.0).abs() < 1e-3);
        assert!((normalize_deg(45.0) - 45.0).abs() < 1e-3);
    }

    #[test]
    fn finite_bounds_checks_reject_nan_and_infinities() {
        assert!(!is_finite_in(f32::NAN, -1.0, 1.0));
        assert!(!is_finite_in(f32::INFINITY, -1.0, 1.0));
        assert!(is_finite_in(0.5, -1.0, 1.0));
        assert!(!is_finite_in(2.0, -1.0, 1.0));
    }

    #[test]
    fn names_must_be_short_and_non_blank() {
        assert!(validate_name("  ").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(25)).is_err());
        assert_eq!(validate_name("  Quinn  ").unwrap(), "Quinn");
    }

    #[test]
    fn rids_are_unique_and_url_safe() {
        let a = new_rid();
        let b = new_rid();
        assert_ne!(a, b);
        for rid in [a, b] {
            assert!(rid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        }
    }

    #[test]
    fn unverified_decode_reads_standard_claims() {
        // header.payload.signature where payload is base64url JSON.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            r#"{"iss":"https://securetoken.google.com/demo","aud":"demo","sub":"uid-1","email":"q@example.com"}"#,
        );
        let token = format!("e30.{payload}.sig");
        let claims = decode_claims_unverified(&token).unwrap();
        assert_eq!(claims.sub, "uid-1");
        assert_eq!(claims.email.as_deref(), Some("q@example.com"));
        assert!(decode_claims_unverified("not-a-jwt").is_err());
    }
}
