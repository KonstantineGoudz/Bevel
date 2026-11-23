//! Typed commands over a direct SurrealDB connection. See [`Backend`].
//!
//! Every player-facing mutation runs through a [`ReducerDef`] from
//! [`crate::reducers`]: the same declaration the code generator renders into
//! the client package. [`Backend`] adds validation and identity; the row
//! rules in `schema.surql` constrain direct clients.

use std::collections::BTreeMap;
use std::pin::Pin;

use futures::{Stream, StreamExt};

use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::{Database, Record, Root, Token};
use surrealdb::types::{SurrealValue, Value};
use surrealdb::{Notification, Surreal};

use crate::reducers::{self, ReducerDef};
use crate::{
    AuthClaims, BackendError, Event, LevelMeta, LogEntry, NewPiece, Op, Piece, PlatformPath,
    Player, Result, SurrealConfig, TableName, Waypoint, DEFAULT_MAX_HP, ELEV_MAX, ELEV_MIN,
    INTERACT_IDS, MAX_HP_CAP, MESSAGE_MAX_CHARS, PLATFORM_MODES, PLATFORM_SPEED_MAX,
    SNAPSHOT_VERSION, WORLD_BOUND, canonical_interact, is_finite_in, is_valid_platform_mode,
    is_valid_shape, log_kind, new_rid, normalize_deg, sanitize_trigger_alpha,
    sanitize_trigger_extent, validate_name, validate_waypoints,
};

/// The authoritative backend: one direct, authenticated SurrealDB connection.
///
/// Two roles share this type:
/// - **Service role** ([`Backend::connect_service`] / `connect_root`): a
///   database user that bypasses row rules and enforces game logic in code.
///   Used by tests, seeding, imports, and any trusted sidecar.
/// - **Player role** ([`Backend::connect_with_token`]): a JWT from the
///   configured provider (Firebase, Auth0, etc.). Every statement additionally
///   passes the table rules in `schema.surql`, so a compromised client still
///   cannot read or write outside your tenant. The provider is configured
///   via `AuthConfig` — no code change to swap it.
pub struct Backend {
    db: Surreal<Client>,
    config: SurrealConfig,
}

impl Backend {
    async fn open(config: &SurrealConfig) -> Result<Surreal<Client>> {
        let db: Surreal<Client> = Surreal::new::<Ws>(config.endpoint()).await?;
        db.use_ns(config.namespace.as_str())
            .use_db(config.database.as_str())
            .await?;
        Ok(db)
    }

    /// Connect as a player with a provider JWT (Firebase, Auth0, …). `iss`/`aud`
    /// are enforced by the row rules on every statement; use
    /// [`decode_claims_unverified`](crate::decode_claims_unverified) for
    /// display purposes only.
    pub async fn connect_with_token(config: &SurrealConfig, jwt: &str) -> Result<Self> {
        use surrealdb::opt::auth::AccessToken;
        let db = Self::open(config).await?;
        db.authenticate(Token::from(AccessToken::from(jwt.to_string())))
            .await?;
        Ok(Self { db, config: config.clone() })
    }

    /// Connect as the service role (a `DEFINE USER ... ON DATABASE` user).
    /// Bypasses row rules; game logic in this file is the enforcement.
    pub async fn connect_service(
        config: &SurrealConfig,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let db = Self::open(config).await?;
        db.signin(Database {
            namespace: config.namespace.clone(),
            database: config.database.clone(),
            username: username.to_string(),
            password: password.to_string(),
        })
        .await?;
        Ok(Self { db, config: config.clone() })
    }

    /// Connect as root (local development and bootstrap only — never bake
    /// root credentials into a shipped client).
    pub async fn connect_root(
        config: &SurrealConfig,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let db = Self::open(config).await?;
        db.signin(Root {
            username: username.to_string(),
            password: password.to_string(),
        })
        .await?;
        Ok(Self { db, config: config.clone() })
    }

    /// Who am I? Decodes (not verifies) the connection's JWT for display.
    /// The database re-verifies on every statement.
    pub fn my_claims(&self, jwt: &str) -> Result<AuthClaims> {
        crate::decode_claims_unverified(jwt)
    }

    pub fn config(&self) -> &SurrealConfig {
        &self.config
    }

    /// Apply schema text (root/service only). "Already exists" is not a
    /// failure — the schema declares the desired end state.
    pub async fn apply_schema(&self, surql: &str) -> Result<()> {
        let mut response = self.db.query(surql).await?;
        let errors = response.take_errors();
        let mut real: Vec<String> = errors
            .iter()
            .filter_map(|(i, e)| {
                let msg = e.to_string();
                if msg.contains("already exists") {
                    None
                } else {
                    Some(format!("statement {i}: {msg}"))
                }
            })
            .collect();
        real.sort();
        if real.is_empty() {
            return Ok(());
        }
        Err(BackendError::Store(format!(
            "schema failed: {}",
            real.join("; ")
        )))
    }

    /// Create the local dev login (`dev_login` RECORD access). Authenticates
    /// this connection as that user.
    pub async fn signup_dev(&self, username: &str, password: &str) -> Result<()> {
        let mut params = BTreeMap::new();
        params.insert("user".to_string(), username.to_string());
        params.insert("pass".to_string(), password.to_string());
        self.db
            .signup(Record {
                namespace: self.config.namespace.clone(),
                database: self.config.database.clone(),
                access: "dev_login".to_string(),
                params,
            })
            .await?;
        Ok(())
    }

    // -- reducer execution ----------------------------------------------------

    /// Inject `$rid` / `$rid2` / `$now` unless the caller supplied them.
    fn inject(vars: &mut BTreeMap<String, Value>, now_ms: i64) {
        vars.entry("rid".to_string()).or_insert_with(|| Value::from_string(new_rid()));
        vars.entry("rid2".to_string()).or_insert_with(|| Value::from_string(new_rid()));
        vars.entry("now".to_string()).or_insert_with(|| Value::from_i64(now_ms));
    }

    fn rid_value(vars: &BTreeMap<String, Value>, param: Option<&str>, literal: Option<&str>) -> String {
        if let Some(p) = param {
            if let Some(Value::String(s)) = vars.get(p) {
                return s.clone();
            }
            return format!("{:?}", vars.get(p));
        }
        literal.unwrap_or("?").trim_matches('\'').to_string()
    }

    /// Run a reducer's checks then statements, returning each statement's
    /// rows. transactional batches shift take indices by one (BEGIN).
    async fn run_reducer(
        &self,
        def: &ReducerDef,
        mut vars: BTreeMap<String, Value>,
        now_ms: i64,
    ) -> Result<Vec<Vec<Value>>> {
        Self::inject(&mut vars, now_ms);
        for check in def.checks {
            let sql = if let Some(p) = check.rid_param {
                format!("SELECT VALUE rid FROM {} WHERE rid = ${p}", check.table)
            } else {
                format!(
                    "SELECT VALUE rid FROM {} WHERE rid = {}",
                    check.table,
                    check.rid_literal.unwrap_or("'?'")
                )
            };
            let rows: Vec<Value> = self.query_vec_at(&sql, vars.clone(), 0).await?;
            if rows.is_empty() {
                let rid = Self::rid_value(&vars, check.rid_param, check.rid_literal);
                return Err(BackendError::NotFound(check.missing.replace("{rid}", &rid)));
            }
        }
        let batch = if def.transactional {
            format!("BEGIN TRANSACTION; {}; COMMIT TRANSACTION;", def.statements.join("; "))
        } else {
            def.statements.join("; ")
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
            return Err(BackendError::Store(format!(
                "reducer {} failed: {}",
                def.name,
                msgs.join("; ")
            )));
        }
        let offset = usize::from(def.transactional);
        let mut out = Vec::with_capacity(def.statements.len());
        for i in 0..def.statements.len() {
            out.push(response.take(i + offset)?);
        }
        Ok(out)
    }

    /// Map a mutation result: non-empty wins; empty runs the probe
    /// (`NotFound` vs `PermissionDenied`).
    async fn one<T: SurrealValue>(
        &self,
        def: &ReducerDef,
        out: Vec<Vec<Value>>,
        vars: &BTreeMap<String, Value>,
        missing: String,
    ) -> Result<T> {
        let rows: Vec<T> = from_values(out.into_iter().nth(def.result_index).unwrap_or_default())?;
        if let Some(row) = rows.into_iter().next() {
            return Ok(row);
        }
        self.deny_or_missing(def, vars, missing).await?;
        unreachable!("deny_or_missing always errors")
    }

    async fn deny_or_missing(
        &self,
        def: &ReducerDef,
        vars: &BTreeMap<String, Value>,
        missing: String,
    ) -> Result<()> {
        if let Some(probe) = &def.probe {
            let sql = if let Some(p) = probe.rid_param {
                format!("SELECT VALUE rid FROM {} WHERE rid = ${p}", probe.table)
            } else {
                format!(
                    "SELECT VALUE rid FROM {} WHERE rid = {}",
                    probe.table,
                    probe.rid_literal.unwrap_or("'?'")
                )
            };
            let seen: Vec<Value> = self.query_vec_at(&sql, vars.clone(), 0).await.unwrap_or_default();
            if seen.is_empty() {
                let rid = Self::rid_value(vars, probe.rid_param, probe.rid_literal);
                return Err(BackendError::NotFound(missing.replace("{rid}", &rid)));
            }
        }
        Err(BackendError::PermissionDenied(format!(
            "reducer {} matched no rows (denied or missing).",
            def.name
        )))
    }

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

    async fn query_vec<T: SurrealValue>(&self, sql: &str, vars: BTreeMap<String, Value>) -> Result<Vec<T>> {
        self.query_vec_at(sql, vars, 0).await
    }

    async fn query_vec_at<T: SurrealValue>(
        &self,
        sql: &str,
        vars: BTreeMap<String, Value>,
        index: usize,
    ) -> Result<Vec<T>> {
        let mut q = self.db.query(sql);
        for (k, v) in vars {
            q = q.bind((k, v));
        }
        let mut response = q.await?;
        Ok(response.take(index)?)
    }

    // -- reads ----------------------------------------------------------------

    pub async fn level_name(&self) -> Result<Option<LevelMeta>> {
        let def = &reducers::level_name::DEF;
        let out = self.run_reducer(def, BTreeMap::new(), 0).await?;
        let rows: Vec<LevelMeta> = from_values(out.into_iter().next().unwrap_or_default())?;
        Ok(rows.into_iter().next())
    }

    pub async fn set_level_name(&self, name: &str, actor: &str, now_ms: i64) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(BackendError::Invalid(
                "Level name must be 1–80 characters.".to_string(),
            ));
        }
        let def = &reducers::set_level_name::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("name".to_string(), Self::v_str(name));
        vars.insert("actor".to_string(), Self::v_str(actor));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        let rows: Vec<Value> = out.into_iter().next().unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing(def, &vars, "level metadata missing.".to_string()).await?;
        }
        Ok(())
    }

    pub async fn get_player(&self, sub: &str) -> Result<Option<Player>> {
        self.opt_row(reducers::q_get_player::DEF.sql, "sub", sub).await
    }

    async fn opt_row<T: SurrealValue>(&self, sql: &str, param: &str, value: &str) -> Result<Option<T>> {
        let mut vars = BTreeMap::new();
        vars.insert(param.to_string(), Self::v_str(value));
        let rows: Vec<T> = self.query_vec(sql, vars).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_players(&self) -> Result<Vec<Player>> {
        self.query_vec(reducers::q_list_players::DEF.sql, BTreeMap::new()).await
    }

    pub async fn list_online_players(&self) -> Result<Vec<Player>> {
        self.query_vec(reducers::q_list_online_players::DEF.sql, BTreeMap::new()).await
    }

    pub async fn get_piece(&self, rid: &str) -> Result<Option<Piece>> {
        self.opt_row(reducers::q_get_piece::DEF.sql, "rid", rid).await
    }

    pub async fn list_pieces(&self) -> Result<Vec<Piece>> {
        self.query_vec(reducers::q_list_pieces::DEF.sql, BTreeMap::new()).await
    }

    pub async fn get_path(&self, piece_rid: &str) -> Result<Option<PlatformPath>> {
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), Self::v_str(piece_rid));
        let rows: Vec<PlatformPath> = self.query_vec(reducers::q_get_path::DEF.sql, vars).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_paths(&self) -> Result<Vec<PlatformPath>> {
        self.query_vec(reducers::q_list_paths::DEF.sql, BTreeMap::new()).await
    }

    /// Newest-first feed window (capped server-side at 500 rows).
    pub async fn recent_log(&self, limit: usize) -> Result<Vec<LogEntry>> {
        let limit = limit.clamp(1, 500);
        let rows: Vec<LogEntry> = self
            .query_vec(reducers::q_recent_log::DEF.sql, BTreeMap::new())
            .await?;
        Ok(rows.into_iter().take(limit).collect())
    }

    // -- players --------------------------------------------------------------

    pub async fn connect_player(&self, sub: &str, now_ms: i64) -> Result<Player> {
        let sub = sub.trim();
        if sub.is_empty() || sub.len() > 128 {
            return Err(BackendError::Invalid(
                "Identity must be 1–128 characters.".to_string(),
            ));
        }
        let def = &reducers::connect_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Self::v_str(sub));
        vars.insert("hp".to_string(), Self::v_f32(DEFAULT_MAX_HP));
        Self::inject(&mut vars, now_ms);
        if self.get_player(sub).await?.is_some() {
            let out = self
                .run_statements(&def.statements[1..2], vars.clone(), now_ms, false)
                .await?;
            let row: Player = self.one(def, out, &vars, format!("No player '{sub}'.")).await?;
            self.run_statements(&def.statements[3..4], vars, now_ms, false).await?;
            return Ok(row);
        }
        let created: Result<Player> = (|| async {
            let out = self
                .run_statements(&def.statements[2..3], vars.clone(), now_ms, false)
                .await?;
            self.one(def, out, &vars, format!("No player '{sub}'.")).await
        })()
        .await;
        match created {
            Ok(row) => {
                self.run_statements(&def.statements[3..4], vars, now_ms, false).await?;
                Ok(row)
            }
            Err(BackendError::Store(msg)) if msg.contains("already exists") => {
                // Lost a create race: the other writer won.
                self.get_player(sub).await?.ok_or_else(|| {
                    BackendError::Store("player appeared then vanished.".to_string())
                })
            }
            Err(BackendError::PermissionDenied(_)) => {
                // Denied CREATE with no visible row: either a race the
                // loser can't see, or a rule denial. Re-check.
                if let Some(row) = self.get_player(sub).await? {
                    Ok(row)
                } else {
                    Err(BackendError::PermissionDenied(
                        "connect_player matched no rows.".to_string(),
                    ))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Run raw statement slices (custom reducers), returning per-statement rows.
    async fn run_statements(
        &self,
        statements: &[&str],
        mut vars: BTreeMap<String, Value>,
        now_ms: i64,
        transactional: bool,
    ) -> Result<Vec<Vec<Value>>> {
        Self::inject(&mut vars, now_ms);
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
            return Err(BackendError::Store(format!("reducer failed: {}", msgs.join("; "))));
        }
        let offset = usize::from(transactional);
        let mut out = Vec::with_capacity(statements.len());
        for i in 0..statements.len() {
            out.push(response.take(i + offset)?);
        }
        Ok(out)
    }

    pub async fn disconnect_player(&self, sub: &str, now_ms: i64) -> Result<()> {
        let def = &reducers::disconnect_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Self::v_str(sub));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        let rows: Vec<Value> = out.into_iter().next().unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing(def, &vars, format!("No player '{sub}'.")).await?;
        }
        Ok(())
    }

    pub async fn register_player(&self, sub: &str, name: &str, now_ms: i64) -> Result<Player> {
        let name = validate_name(name)?;
        let def = &reducers::register_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Self::v_str(sub));
        vars.insert("name".to_string(), Self::v_str(name.clone()));
        vars.insert("text".to_string(), Self::v_str(name.clone()));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, format!("No player '{sub}'.")).await
    }

    pub async fn move_player(
        &self,
        sub: &str,
        x: f32,
        z: f32,
        feet: f32,
        yaw_deg: f32,
        now_ms: i64,
    ) -> Result<Player> {
        if !is_finite_in(x, -WORLD_BOUND, WORLD_BOUND) || !is_finite_in(z, -WORLD_BOUND, WORLD_BOUND) {
            return Err(BackendError::Invalid(format!(
                "Position must be within ±{WORLD_BOUND} m."
            )));
        }
        if !is_finite_in(feet, 0.0, 100.0) {
            return Err(BackendError::Invalid(
                "Feet height must be within [0, 100] m.".to_string(),
            ));
        }
        if !yaw_deg.is_finite() {
            return Err(BackendError::Invalid("Yaw must be finite.".to_string()));
        }
        let def = &reducers::move_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Self::v_str(sub));
        vars.insert("x".to_string(), Self::v_f32(x));
        vars.insert("z".to_string(), Self::v_f32(z));
        vars.insert("feet".to_string(), Self::v_f32(feet));
        vars.insert("yaw_deg".to_string(), Self::v_f32(normalize_deg(yaw_deg)));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, format!("No player '{sub}'.")).await
    }

    pub async fn damage_player(
        &self,
        target: &str,
        amount: f32,
        now_ms: i64,
    ) -> Result<Player> {
        if !(amount.is_finite() && amount > 0.0 && amount <= MAX_HP_CAP) {
            return Err(BackendError::Invalid(
                "Damage must be within (0, 10000].".to_string(),
            ));
        }
        let def = &reducers::damage_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("target".to_string(), Self::v_str(target));
        vars.insert("amount".to_string(), Self::v_f32(amount));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, format!("No player '{target}'.")).await
    }

    /// Record a kill in the feed. Call when [`Backend::damage_player`]
    /// returns hp 0.
    pub async fn log_death(&self, actor: &str, target: &str, now_ms: i64) -> Result<LogEntry> {
        let def = &reducers::log_death::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), Self::v_str(actor));
        vars.insert("target".to_string(), Self::v_str(target));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, "feed unavailable.".to_string()).await
    }

    pub async fn heal_player(&self, target: &str, amount: f32, now_ms: i64) -> Result<Player> {
        if !(amount.is_finite() && amount > 0.0 && amount <= MAX_HP_CAP) {
            return Err(BackendError::Invalid(
                "Heal amount must be within (0, 10000].".to_string(),
            ));
        }
        let def = &reducers::heal_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("target".to_string(), Self::v_str(target));
        vars.insert("amount".to_string(), Self::v_f32(amount));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, format!("No player '{target}'.")).await
    }

    pub async fn respawn_player(&self, sub: &str, now_ms: i64) -> Result<Player> {
        let def = &reducers::respawn_player::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("sub".to_string(), Self::v_str(sub));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, format!("No player '{sub}'.")).await
    }

    // -- level editing --------------------------------------------------------

    fn validated_piece(&self, args: &NewPiece) -> Result<NewPiece> {
        if !is_valid_shape(&args.shape) {
            return Err(BackendError::Invalid(format!("Unknown shape '{}'.", args.shape)));
        }
        if !is_finite_in(args.x, -WORLD_BOUND, WORLD_BOUND)
            || !is_finite_in(args.z, -WORLD_BOUND, WORLD_BOUND)
        {
            return Err(BackendError::Invalid(format!(
                "Position must be within ±{WORLD_BOUND} m."
            )));
        }
        if !is_finite_in(args.elevation, ELEV_MIN, ELEV_MAX) {
            return Err(BackendError::Invalid(format!(
                "Elevation must be within [{ELEV_MIN}, {ELEV_MAX}] m."
            )));
        }
        if !(args.damage.is_finite() && (0.0..=MAX_HP_CAP).contains(&args.damage)) {
            return Err(BackendError::Invalid("Damage must be within [0, 10000].".to_string()));
        }
        if !args.rot_deg.is_finite() || !args.pitch_deg.is_finite() || !args.roll_deg.is_finite() {
            return Err(BackendError::Invalid("Rotation must be finite.".to_string()));
        }
        let interact = canonical_interact(&args.interact).ok_or_else(|| {
            BackendError::Invalid(format!(
                "Unknown interact action '{}'. Expected one of {INTERACT_IDS:?}.",
                args.interact
            ))
        })?;
        for (label, v) in [
            ("trigger_w", args.trigger_w),
            ("trigger_h", args.trigger_h),
            ("trigger_l", args.trigger_l),
        ] {
            if !(v.is_finite() && v > 0.0 && v <= 100.0) {
                return Err(BackendError::Invalid(format!("{label} must be within (0, 100].")));
            }
        }
        let mut out = args.clone();
        out.interact = interact;
        out.rot_deg = normalize_deg(out.rot_deg);
        out.pitch_deg = normalize_deg(out.pitch_deg);
        out.roll_deg = normalize_deg(out.roll_deg);
        out.trigger_w = sanitize_trigger_extent(out.trigger_w);
        out.trigger_h = sanitize_trigger_extent(out.trigger_h);
        out.trigger_l = sanitize_trigger_extent(out.trigger_l);
        // Alpha is a gameplay value for glade pieces (roof pitch = alpha * 2)
        // and a cosmetic elsewhere; clamp instead of rejecting so old clients
        // and stored snapshots always write a valid row.
        out.trigger_alpha = sanitize_trigger_alpha(out.trigger_alpha);
        Ok(out)
    }

    fn piece_vars(args: &NewPiece) -> BTreeMap<String, Value> {
        let mut vars = BTreeMap::new();
        vars.insert("shape".to_string(), Self::v_str(args.shape.clone()));
        vars.insert("x".to_string(), Self::v_f32(args.x));
        vars.insert("z".to_string(), Self::v_f32(args.z));
        vars.insert("elevation".to_string(), Self::v_f32(args.elevation));
        vars.insert("rot_deg".to_string(), Self::v_f32(args.rot_deg));
        vars.insert("pitch_deg".to_string(), Self::v_f32(args.pitch_deg));
        vars.insert("roll_deg".to_string(), Self::v_f32(args.roll_deg));
        vars.insert("dynamic".to_string(), Self::v_bool(args.dynamic));
        vars.insert("damage".to_string(), Self::v_f32(args.damage));
        vars.insert("interact".to_string(), Self::v_str(args.interact.clone()));
        vars.insert("trigger_w".to_string(), Self::v_f32(args.trigger_w));
        vars.insert("trigger_h".to_string(), Self::v_f32(args.trigger_h));
        vars.insert("trigger_l".to_string(), Self::v_f32(args.trigger_l));
        vars.insert("trigger_alpha".to_string(), Self::v_f32(args.trigger_alpha));
        vars
    }

    pub async fn create_piece(&self, args: NewPiece, actor: &str, now_ms: i64) -> Result<Piece> {
        let args = self.validated_piece(&args)?;
        let def = &reducers::create_piece::DEF;
        let mut vars = Self::piece_vars(&args);
        vars.insert("actor".to_string(), Self::v_str(actor));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, "piece store unavailable.".to_string()).await
    }

    pub async fn update_piece(&self, rid: &str, args: NewPiece) -> Result<Piece> {
        let args = self.validated_piece(&args)?;
        let def = &reducers::update_piece::DEF;
        let mut vars = Self::piece_vars(&args);
        vars.insert("rid".to_string(), Self::v_str(rid));
        let out = self.run_reducer(def, vars.clone(), 0).await?;
        self.one(def, out, &vars, format!("No piece with rid {rid}.")).await
    }

    pub async fn delete_piece(&self, rid: &str, actor: &str, now_ms: i64) -> Result<()> {
        let def = &reducers::delete_piece::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("rid".to_string(), Self::v_str(rid));
        vars.insert("actor".to_string(), Self::v_str(actor));
        vars.insert("text".to_string(), Self::v_str(rid));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        let rows: Vec<Piece> = from_values(out.into_iter().nth(def.result_index).unwrap_or_default())?;
        if rows.is_empty() {
            self.deny_or_missing(def, &vars, format!("No piece with rid {rid}.")).await?;
        }
        Ok(())
    }

    pub async fn clear_level(&self, actor: &str, now_ms: i64) -> Result<()> {
        let def = &reducers::clear_level::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), Self::v_str(actor));
        self.run_reducer(def, vars, now_ms).await?;
        Ok(())
    }

    pub async fn set_platform_path(
        &self,
        piece_rid: &str,
        mode: &str,
        speed: f32,
        paused: bool,
        waypoints: Vec<Waypoint>,
    ) -> Result<PlatformPath> {
        if !is_valid_platform_mode(mode) {
            return Err(BackendError::Invalid(format!(
                "Unknown platform mode '{mode}'. Expected one of {PLATFORM_MODES:?}."
            )));
        }
        if !(speed.is_finite() && (0.0..=PLATFORM_SPEED_MAX).contains(&speed)) {
            return Err(BackendError::Invalid(format!(
                "Speed must be within [0, {PLATFORM_SPEED_MAX}]."
            )));
        }
        validate_waypoints(&waypoints)?;
        let def = &reducers::set_platform_path::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), Self::v_str(piece_rid));
        vars.insert("mode".to_string(), Self::v_str(mode));
        vars.insert("speed".to_string(), Self::v_f32(speed));
        vars.insert("paused".to_string(), Self::v_bool(paused));
        vars.insert("waypoints".to_string(), to_value(waypoints)?);
        let out = self.run_reducer(def, vars.clone(), 0).await?;
        self.one(
            def,
            out,
            &vars,
            format!("No piece with rid {piece_rid}."),
        )
        .await
    }

    pub async fn clear_platform_path(&self, piece_rid: &str) -> Result<()> {
        let def = &reducers::clear_platform_path::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("piece_rid".to_string(), Self::v_str(piece_rid));
        let out = self.run_reducer(def, vars.clone(), 0).await?;
        let rows: Vec<Value> = out.into_iter().next().unwrap_or_default();
        if rows.is_empty() {
            self.deny_or_missing(
                def,
                &vars,
                format!("No platform path on piece {piece_rid}."),
            )
            .await?;
        }
        Ok(())
    }

    /// Seed the singleton metadata row and a minimal playable room when the
    /// database is empty. Idempotent.
    pub async fn ensure_seed(&self, now_ms: i64) -> Result<()> {
        if !self.list_pieces().await?.is_empty() {
            return Ok(());
        }
        for shape in ["spawn", "floor"] {
            let args = crate::new_piece_args(shape, 0.0, 0.0);
            let clean = self.validated_piece(&args)?;
            let mut vars = Self::piece_vars(&clean);
            vars.insert("rid".to_string(), Self::v_str(crate::new_rid()));
            let mut q = self.db.query(reducers::create_piece::DEF.statements[0]);
            for (k, v) in &vars {
                q = q.bind((k.clone(), v.clone()));
            }
            let mut r = q.await?;
            let _: Vec<Value> = r.take(0)?;
        }
        if self.level_name().await?.is_none() {
            let mut vars = BTreeMap::new();
            vars.insert("name".to_string(), Self::v_str("Party Dungeon"));
            let mut q = self.db.query("CREATE level_meta CONTENT { rid: 'main', name: $name } RETURN *");
            for (k, v) in &vars {
                q = q.bind((k.clone(), v.clone()));
            }
            let mut r = q.await?;
            let _: Vec<Value> = r.take(0)?;
            self.append_log(now_ms, log_kind::SYSTEM, "", "Dungeon created.".to_string())
                .await?;
        }
        Ok(())
    }

    async fn append_log(&self, at_ms: i64, kind: &str, actor: &str, text: String) -> Result<LogEntry> {
        let mut vars = BTreeMap::new();
        vars.insert("at_ms".to_string(), Self::v_i64(at_ms));
        vars.insert("kind".to_string(), Self::v_str(kind));
        vars.insert("actor".to_string(), Self::v_str(actor));
        vars.insert("text".to_string(), Self::v_str(text));
        let mut q = self.db.query(
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $at_ms, kind: $kind, actor: $actor, text: $text } RETURN *",
        );
        Self::inject(&mut vars, at_ms);
        vars.remove("rid2");
        for (k, v) in &vars {
            q = q.bind((k.clone(), v.clone()));
        }
        let mut response = q.await?;
        let rows: Vec<LogEntry> = response.take(0)?;
        rows.into_iter().next().ok_or_else(|| {
            BackendError::PermissionDenied("feed unavailable.".to_string())
        })
    }

    // -- feed -----------------------------------------------------------------

    pub async fn send_message(&self, actor: &str, text: &str, now_ms: i64) -> Result<LogEntry> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(BackendError::Invalid("Message must not be empty.".to_string()));
        }
        if trimmed.chars().count() > MESSAGE_MAX_CHARS {
            return Err(BackendError::Invalid(format!(
                "Message must be at most {MESSAGE_MAX_CHARS} characters."
            )));
        }
        let def = &reducers::send_message::DEF;
        let mut vars = BTreeMap::new();
        vars.insert("actor".to_string(), Self::v_str(actor));
        vars.insert("text".to_string(), Self::v_str(trimmed));
        let out = self.run_reducer(def, vars.clone(), now_ms).await?;
        self.one(def, out, &vars, "feed unavailable.".to_string()).await
    }

    pub async fn prune_log(&self) -> Result<u64> {
        let def = &reducers::prune_log::DEF;
        let out = self.run_reducer(def, BTreeMap::new(), 0).await?;
        let rows: Vec<Value> = out.into_iter().next().unwrap_or_default();
        Ok(rows.len() as u64)
    }

    // -- snapshots (service role) ---------------------------------------------

    pub async fn export_snapshot(&self) -> Result<String> {
        use crate::LevelSnapshot;
        let snapshot = LevelSnapshot {
            version: SNAPSHOT_VERSION,
            name: self
                .level_name()
                .await?
                .map(|m| m.name)
                .unwrap_or_else(|| "Party Dungeon".to_string()),
            pieces: self.list_pieces().await?,
            paths: self.list_paths().await?,
        };
        serde_json::to_string_pretty(&snapshot)
            .map_err(|e| BackendError::Store(format!("snapshot encode: {e}")))
    }

    pub async fn import_snapshot(&self, json: &str, actor: &str, now_ms: i64) -> Result<()> {
        use crate::LevelSnapshot;
        let snapshot: LevelSnapshot = serde_json::from_str(json)?;
        if snapshot.version != SNAPSHOT_VERSION {
            return Err(BackendError::Invalid(format!(
                "Unsupported snapshot version {}.",
                snapshot.version
            )));
        }
        if snapshot.name.trim().is_empty() || snapshot.name.chars().count() > 80 {
            return Err(BackendError::Invalid(
                "Snapshot name must be 1–80 characters.".to_string(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for piece in &snapshot.pieces {
            if piece.rid.is_empty() || !seen.insert(piece.rid.clone()) {
                return Err(BackendError::Invalid(format!(
                    "Snapshot piece rids must be unique and non-empty (got '{}').",
                    piece.rid
                )));
            }
            self.validated_piece(&NewPiece {
                shape: piece.shape.clone(),
                x: piece.x,
                z: piece.z,
                elevation: piece.elevation,
                rot_deg: piece.rot_deg,
                pitch_deg: piece.pitch_deg,
                roll_deg: piece.roll_deg,
                dynamic: piece.dynamic,
                damage: piece.damage,
                interact: piece.interact.clone(),
                trigger_w: piece.trigger_w,
                trigger_h: piece.trigger_h,
                trigger_l: piece.trigger_l,
                trigger_alpha: piece.trigger_alpha,
            })?;
        }
        for path in &snapshot.paths {
            if !snapshot.pieces.iter().any(|p| p.rid == path.piece_rid) {
                return Err(BackendError::Invalid(format!(
                    "Snapshot path references missing piece {}.",
                    path.piece_rid
                )));
            }
            if !is_valid_platform_mode(&path.mode) {
                return Err(BackendError::Invalid(format!(
                    "Unknown platform mode '{}'.",
                    path.mode
                )));
            }
            if !(path.speed.is_finite() && (0.0..=PLATFORM_SPEED_MAX).contains(&path.speed)) {
                return Err(BackendError::Invalid("Snapshot path speed out of range.".to_string()));
            }
            validate_waypoints(&path.waypoints)?;
        }

        let mut statements = vec![
            "DELETE platform_path".to_string(),
            "DELETE dungeon_piece".to_string(),
        ];
        for piece in &snapshot.pieces {
            let args = NewPiece {
                shape: piece.shape.clone(),
                x: piece.x,
                z: piece.z,
                elevation: piece.elevation,
                rot_deg: piece.rot_deg,
                pitch_deg: piece.pitch_deg,
                roll_deg: piece.roll_deg,
                dynamic: piece.dynamic,
                damage: piece.damage,
                interact: piece.interact.clone(),
                trigger_w: piece.trigger_w,
                trigger_h: piece.trigger_h,
                trigger_l: piece.trigger_l,
                trigger_alpha: piece.trigger_alpha,
            };
            let clean = self.validated_piece(&args)?;
            statements.push(piece_content_literal(&piece.rid, &clean));
        }
        for path in &snapshot.paths {
            statements.push(path_content_literal(path));
        }
        statements.push(format!(
            "UPDATE level_meta SET name = '{}' WHERE rid = 'main'",
            escape_str(&snapshot.name)
        ));
        let batch = format!("BEGIN TRANSACTION; {}; COMMIT TRANSACTION;", statements.join("; "));
        let mut response = self.db.query(batch).await?;
        let errors = response.take_errors();
        if !errors.is_empty() {
            let mut msgs: Vec<String> = errors
                .iter()
                .map(|(i, e)| format!("statement {i}: {e}"))
                .collect();
            msgs.sort();
            return Err(BackendError::Store(format!("import failed: {}", msgs.join("; "))));
        }
        self.append_log(
            now_ms,
            log_kind::EDIT,
            actor,
            "Imported a level snapshot.".to_string(),
        )
        .await?;
        Ok(())
    }

    // -- subscriptions --------------------------------------------------------

    /// Open a `LIVE` query over one table. Each notification maps to the
    /// changed rows' [`Event`]s, in commit order. The feed ends (`None`)
    /// only if the server kills the query or the connection drops.
    pub async fn subscribe<T>(&self, table: TableName) -> Result<LiveFeed<T>>
    where
        T: SurrealValue + crate::HasRid + Send + Unpin + 'static,
    {
        let stream = self
            .db
            .select::<Vec<T>>(table.as_str())
            .live()
            .await?;
        let boxed: Pin<
            Box<dyn Stream<Item = std::result::Result<Notification<T>, surrealdb::Error>> + Send>,
        > = Box::pin(stream);
        Ok(LiveFeed { seq: 0, table, stream: boxed })
    }
}

/// A live row feed. See [`Backend::subscribe`].
pub struct LiveFeed<T> {
    seq: u64,
    table: TableName,
    stream: Pin<
        Box<dyn Stream<Item = std::result::Result<Notification<T>, surrealdb::Error>> + Send>,
    >,
}

impl<T: crate::HasRid> LiveFeed<T> {
    /// Next batch of committed changes. `None` means the feed ended.
    pub async fn next(&mut self) -> Option<Result<Vec<Event>>> {
        let notification = match self.stream.next().await? {
            Ok(n) => n,
            Err(e) => return Some(Err(BackendError::Store(e.to_string()))),
        };
        let op = match notification.action {
            surrealdb::types::Action::Create => Op::Insert,
            surrealdb::types::Action::Update => Op::Update,
            surrealdb::types::Action::Delete => Op::Delete,
            surrealdb::types::Action::Killed => {
                return Some(Err(BackendError::Store(
                    "live query was killed by the server.".to_string(),
                )));
            }
            _ => {
                return Some(Err(BackendError::Store(format!(
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

// ---------------------------------------------------------------------------
// Row mapping + value helpers
// ---------------------------------------------------------------------------

fn from_values<T: SurrealValue>(rows: Vec<Value>) -> Result<Vec<T>> {
    rows.into_iter()
        .map(|v| T::from_value(v).map_err(|e| BackendError::Store(e.to_string())))
        .collect()
}

fn to_value<T: SurrealValue>(v: T) -> Result<Value> {
    Ok(v.into_value())
}

fn piece_content_literal(rid: &str, args: &NewPiece) -> String {
    format!(
        "CREATE dungeon_piece CONTENT {{ rid: '{}', shape: '{}', x: {}, z: {}, elevation: {}, rot_deg: {}, pitch_deg: {}, roll_deg: {}, dynamic: {}, damage: {}, interact: '{}', trigger_w: {}, trigger_h: {}, trigger_l: {}, trigger_alpha: {} }}",
        escape_str(rid),
        escape_str(&args.shape),
        num(args.x),
        num(args.z),
        num(args.elevation),
        num(args.rot_deg),
        num(args.pitch_deg),
        num(args.roll_deg),
        args.dynamic,
        num(args.damage),
        escape_str(&args.interact),
        num(args.trigger_w),
        num(args.trigger_h),
        num(args.trigger_l),
        num(args.trigger_alpha),
    )
}

fn path_content_literal(path: &PlatformPath) -> String {
    let points: Vec<String> = path
        .waypoints
        .iter()
        .map(|w| format!("{{ x: {}, z: {} }}", num(w.x), num(w.z)))
        .collect();
    format!(
        "CREATE platform_path CONTENT {{ piece_rid: '{}', mode: '{}', speed: {}, paused: {}, waypoints: [{}] }}",
        escape_str(&path.piece_rid),
        escape_str(&path.mode),
        num(path.speed),
        path.paused,
        points.join(", ")
    )
}

fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn num(v: f32) -> String {
    let s = format!("{v:?}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

// ---------------------------------------------------------------------------
// Query SQL for read queries (shared with codegen via reducers::QUERIES)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Read-query SQL lives once in reducers::QUERIES; Backend references it.
// ---------------------------------------------------------------------------
