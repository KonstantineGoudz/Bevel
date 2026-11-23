//! Single-world store for the Party Dungeon MCP server.
//!
//! The MMO has exactly one world: SurrealDB is the system of record
//! (`dungeon_piece` / `platform_path` rows), the sim is authoritative at
//! runtime, and the live game bridge is the hot path for instant edits.
//! There are no level files and no level names anymore.
//!
//! MCP tools work on an editor-style [`Snapshot`] (local u32 ids); this
//! module translates to/from SurrealDB rows (stable string `rid`s). The
//! `rids` map remembers the row key behind each local id for the session so
//! saves update rows instead of forking them.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use party_dungeon::model::{self, Piece};
use party_dungeon::path;
use party_dungeon::session::{
    self, SNAPSHOT_VERSION, SavedCharacter, SavedPiece, SavedPlatform, SavedPlatformPoint, Snapshot,
};
use party_dungeon_server::{Backend, SurrealConfig};

/// Which backend a tool should use.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Source {
    /// Try the live bridge first, fall back to the database.
    #[default]
    Auto,
    /// SurrealDB only (the single shared world).
    Db,
    /// Live game only; error when the bridge is unreachable.
    Live,
}

impl Source {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("auto").to_ascii_lowercase().as_str() {
            // "file" is the pre-world name for the stored backend.
            "db" | "file" => Source::Db,
            "live" => Source::Live,
            _ => Source::Auto,
        }
    }
}

/// World handle: live bridge address plus the local-id -> row-rid map that
/// makes saves round-trip without forking rows.
#[derive(Clone, Default)]
pub struct Store {
    bridge: Option<String>,
    rids: Arc<Mutex<HashMap<u32, String>>>,
}

impl Store {
    pub fn from_env() -> Self {
        // `PARTY_DUNGEON_BRIDGE` may be an address, "1" for the default, or
        // empty/"0" to disable the live path.
        let bridge = match std::env::var("PARTY_DUNGEON_BRIDGE") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(value),
            Err(_) => Some("127.0.0.1:6559".to_owned()),
        };
        Self {
            bridge,
            rids: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn bridge_addr(&self) -> Option<&str> {
        self.bridge.as_deref()
    }

    async fn service_backend() -> Result<Backend, String> {
        let cfg = SurrealConfig::from_env()
            .map_err(|error| format!("surreal config (DUNGEON_* env): {error}"))?;
        let user = std::env::var("DUNGEON_SERVICE_USER")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "set DUNGEON_SERVICE_USER (trusted sidecar)".to_string())?;
        let pass = std::env::var("DUNGEON_SERVICE_PASS")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "set DUNGEON_SERVICE_PASS (trusted sidecar)".to_string())?;
        Backend::connect_service(&cfg, &user, &pass)
            .await
            .map_err(|error| format!("surreal connect: {error}"))
    }

    /// Load the single shared world: live bridge first (unless `Db`), else
    /// SurrealDB. Returns the snapshot and whether it came from the live game.
    pub async fn load_world(&self, source: Source) -> Result<(Snapshot, bool), String> {
        if source != Source::Db {
            if let Some(addr) = self.bridge_addr() {
                if let Some(snapshot) = bridge_state(addr).await {
                    self.remember_rids_fresh(&snapshot);
                    return Ok((snapshot, true));
                }
                if source == Source::Live {
                    return Err(format!("live bridge at {addr} is unreachable"));
                }
            } else if source == Source::Live {
                return Err("live bridge is disabled (PARTY_DUNGEON_BRIDGE is empty)".to_owned());
            }
        }
        self.db_snapshot().await.map(|snapshot| (snapshot, false))
    }

    /// Read the world from SurrealDB and remember row rids behind fresh
    /// sequential local ids.
    async fn db_snapshot(&self) -> Result<Snapshot, String> {
        let backend = Self::service_backend().await?;
        let pieces = backend
            .list_pieces()
            .await
            .map_err(|error| format!("list pieces: {error}"))?;
        let paths = backend
            .list_paths()
            .await
            .map_err(|error| format!("list paths: {error}"))?;
        let mut rids = HashMap::new();
        let mut saved_pieces = Vec::with_capacity(pieces.len());
        for (index, row) in pieces.iter().enumerate() {
            let id = index as u32 + 1;
            rids.insert(id, row.rid.clone());
            saved_pieces.push(SavedPiece {
                id,
                shape: row.shape.clone(),
                x: row.x,
                z: row.z,
                elevation: row.elevation,
                rot_deg: row.rot_deg,
                pitch_deg: row.pitch_deg,
                roll_deg: row.roll_deg,
                dynamic: row.dynamic,
                damage: Some(row.damage),
                interact: if row.interact == "none" {
                    None
                } else {
                    Some(row.interact.clone())
                },
                trigger_width: Some(row.trigger_w),
                trigger_height: Some(row.trigger_h),
                trigger_length: Some(row.trigger_l),
                trigger_shape: None,
                trigger_alpha: None,
                trigger_color: None,
                trigger_visible: None,
                name: None,
                group: None,
                locked: false,
            });
        }
        let mut saved_paths = Vec::new();
        let mut skipped = 0usize;
        for path in &paths {
            let Some(piece) = rids.iter().find_map(|(id, rid)| (*rid == path.piece_rid).then_some(*id)) else {
                skipped += 1;
                continue;
            };
            saved_paths.push(SavedPlatform {
                piece,
                mode: path.mode.clone(),
                speed: Some(path.speed),
                paused: path.paused,
                waypoints: path
                    .waypoints
                    .iter()
                    .map(|w| SavedPlatformPoint {
                        x: w.x,
                        z: w.z,
                        incoming: None,
                        outgoing: None,
                    })
                    .collect(),
            });
        }
        if skipped > 0 {
            eprintln!("mcp: skipped {skipped} orphan platform path(s)");
        }
        *self.rids.lock().unwrap() = rids;
        Ok(Snapshot {
            version: SNAPSHOT_VERSION,
            next_id: Some(saved_pieces.len() as u32 + 1),
            pieces: saved_pieces,
            character: None,
            camera: None,
            groups: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            platforms: saved_paths,
            grass: Vec::new(),
            grass_slows: false,
            vines: Vec::new(),
        })
    }

    /// Validate + persist the world to SurrealDB (full replace, like the
    /// sim's hot-reload path). Unknown local ids get fresh row rids and are
    /// remembered for the rest of the session.
    pub async fn save_world(&self, snapshot: &Snapshot, actor: &str) -> Result<String, String> {
        let loaded = snapshot
            .clone()
            .validate()
            .map_err(|error| format!("invalid world: {error:?}"))?;
        let backend = Self::service_backend().await?;
        let world_name = backend
            .level_name()
            .await
            .map_err(|error| format!("read world name: {error}"))?
            .map(|meta| meta.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "world".to_owned());
        let mut rids = self.rids.lock().unwrap().clone();
        let mut pieces = Vec::with_capacity(loaded.pieces.len());
        for piece in &loaded.pieces {
            let rid = rids.entry(piece.id).or_insert_with(party_dungeon_server::new_rid).clone();
            let shape_id = model::shapes()
                .get(piece.shape)
                .map(|shape| shape.id.to_string())
                .unwrap_or_else(|| "block".to_string());
            pieces.push(party_dungeon_server::Piece {
                rid,
                shape: shape_id,
                x: piece.pos.x,
                z: piece.pos.y,
                elevation: piece.elevation,
                rot_deg: piece.rot.to_degrees(),
                pitch_deg: piece.pitch.to_degrees(),
                roll_deg: piece.roll.to_degrees(),
                dynamic: piece.dynamic,
                damage: piece.trigger.damage,
                interact: piece.interact.id().to_string(),
                trigger_w: piece.trigger_size.x,
                trigger_h: piece.trigger_size.y,
                trigger_l: piece.trigger_size.z,
                trigger_alpha: piece.trigger_alpha,
            });
        }
        let mut paths = Vec::new();
        for platform in &loaded.platforms {
            let Some(rid) = rids.get(&platform.piece) else {
                return Err(format!(
                    "platform path references unknown piece id {}",
                    platform.piece
                ));
            };
            paths.push(party_dungeon_server::PlatformPath {
                piece_rid: rid.clone(),
                mode: platform.mode.clone(),
                speed: platform.speed.ok_or_else(|| {
                    format!("platform path on piece {} needs a speed", platform.piece)
                })?,
                paused: platform.paused,
                waypoints: platform
                    .waypoints
                    .iter()
                    .map(|w| party_dungeon_server::Waypoint { x: w.x, z: w.z })
                    .collect(),
            });
        }
        let doc = serde_json::to_string(&party_dungeon_server::LevelSnapshot {
            version: party_dungeon_server::SNAPSHOT_VERSION,
            name: world_name,
            pieces,
            paths,
        })
        .map_err(|error| format!("serialize world: {error}"))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        backend
            .import_snapshot(&doc, actor, now_ms)
            .await
            .map_err(|error| format!("save world: {error}"))?;
        *self.rids.lock().unwrap() = rids;
        Ok(format!(
            "saved shared world ({} pieces, {} paths)",
            loaded.pieces.len(),
            loaded.platforms.len()
        ))
    }

    /// Remember rids for snapshots that came from the live bridge: the
    /// bridge serves game pieces with local ids, so fresh rids are minted on
    /// the next save (full replace keeps the world consistent either way).
    fn remember_rids_fresh(&self, _snapshot: &Snapshot) {
        *self.rids.lock().unwrap() = HashMap::new();
    }

    /// One-line summary of the single shared world.
    pub async fn world_summary(&self) -> Result<WorldSummary, String> {
        let (snapshot, live) = self.load_world(Source::Db).await?;
        Ok(WorldSummary {
            pieces: snapshot.pieces.len(),
            platforms: snapshot.platforms.len(),
            live,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorldSummary {
    pub pieces: usize,
    pub platforms: usize,
    pub live: bool,
}

// ---------------------------------------------------------------------------
// Piece conversion (SavedPiece <-> game Piece for nav/path queries)
// ---------------------------------------------------------------------------

/// Convert a saved piece into the game type used by nav/path queries.
pub fn saved_to_piece(saved: &SavedPiece) -> Result<Piece, String> {
    let shape = model::shape_index(&saved.shape)
        .ok_or_else(|| format!("unknown shape: {}", saved.shape))?;
    for (value, name) in [
        (saved.x, "x"),
        (saved.z, "z"),
        (saved.elevation, "elevation"),
        (saved.rot_deg, "rot_deg"),
        (saved.pitch_deg, "pitch_deg"),
        (saved.roll_deg, "roll_deg"),
    ] {
        if !value.is_finite() {
            return Err(format!("piece {name} must be finite"));
        }
    }
    let default_damage = model::shapes().get(shape).map(|s| s.damage).unwrap_or(0.0);
    let damage = saved.damage.unwrap_or(default_damage);
    if !damage.is_finite() || damage < 0.0 || damage > 10000.0 {
        return Err("damage must be within 0..=10000".to_owned());
    }
    let mut trigger_size = model::TRIGGER_DEFAULT_SIZE;
    if let Some(w) = saved.trigger_width {
        if !w.is_finite() {
            return Err("trigger_width must be finite".to_owned());
        }
        trigger_size.x = w.max(model::TRIGGER_MIN_SIZE);
    }
    if let Some(l) = saved.trigger_length {
        if !l.is_finite() {
            return Err("trigger_length must be finite".to_owned());
        }
        trigger_size.z = l.max(model::TRIGGER_MIN_SIZE);
    }
    if let Some(h) = saved.trigger_height {
        if !h.is_finite() {
            return Err("trigger_height must be finite".to_owned());
        }
        trigger_size.y = h.max(model::TRIGGER_MIN_SIZE);
    }
    let trigger_shape_override = saved
        .trigger_shape
        .as_deref()
        .and_then(model::TriggerShape::parse)
        .unwrap_or(model::TriggerShape::Box);
    let trigger_alpha = saved
        .trigger_alpha
        .map(|v| {
            if !v.is_finite() {
                Err("trigger_alpha must be finite".to_owned())
            } else {
                Ok(model::sanitize_trigger_alpha(v))
            }
        })
        .transpose()?
        .unwrap_or(model::TRIGGER_DEFAULT_ALPHA);
    let trigger_color = saved
        .trigger_color
        .as_deref()
        .and_then(model::trigger_color_parse_hex)
        .unwrap_or(model::TRIGGER_DEFAULT_COLOR);
    let trigger_visible = saved
        .trigger_visible
        .unwrap_or(model::TRIGGER_DEFAULT_VISIBLE);
    let interact = saved
        .interact
        .as_deref()
        .and_then(model::InteractAction::parse)
        .unwrap_or_default();
    Ok(Piece {
        id: saved.id,
        shape,
        pos: bevy_math_vec2(saved.x, saved.z),
        rot: saved.rot_deg.to_radians(),
        pitch: saved.pitch_deg.to_radians(),
        roll: saved.roll_deg.to_radians(),
        elevation: saved.elevation,
        dynamic: saved.dynamic,
        trigger: model::Trigger::damaging(damage),
        trigger_size,
        trigger_shape_override,
        trigger_alpha,
        trigger_color,
        trigger_visible,
        interact,
    })
}

fn bevy_math_vec2(x: f32, y: f32) -> bevy_math::Vec2 {
    bevy_math::Vec2::new(x, y)
}

/// Game pieces for nav/path queries. Unknown shapes are skipped (the file
/// itself still fails validation, surfaced by `validate_level`).
pub fn pieces_for_nav(snapshot: &Snapshot) -> Vec<Piece> {
    snapshot
        .pieces
        .iter()
        .filter_map(|saved| saved_to_piece(saved).ok())
        .collect()
}

pub fn grass_map(snapshot: &Snapshot) -> HashMap<path::Hex, u8> {
    snapshot
        .grass
        .iter()
        .map(|cell| ((cell.q, cell.r), cell.density))
        .collect()
}

pub fn grass_step_cost(slows: bool, grass: &HashMap<path::Hex, u8>, hex: path::Hex) -> u32 {
    if slows {
        grass.get(&hex).copied().unwrap_or(0).clamp(0, 4) as u32 + 1
    } else {
        1
    }
}

pub fn blocked_for(snapshot: &Snapshot) -> (HashSet<path::Edge>, HashSet<path::Hex>) {
    path::blocked(&pieces_for_nav(snapshot))
}

/// Next free piece/group id and updated `next_id` after a mutation.
pub fn reassign_next_id(snapshot: &mut Snapshot) {
    let highest = snapshot
        .pieces
        .iter()
        .map(|piece| piece.id)
        .chain(snapshot.groups.iter().map(|group| group.id))
        .max()
        .unwrap_or(0);
    let next = snapshot
        .next_id
        .unwrap_or(1)
        .max(highest.saturating_add(1))
        .max(1);
    snapshot.next_id = Some(next);
}

pub fn claim_id(snapshot: &Snapshot) -> u32 {
    let highest = snapshot
        .pieces
        .iter()
        .map(|piece| piece.id)
        .chain(snapshot.groups.iter().map(|group| group.id))
        .max()
        .unwrap_or(0);
    snapshot
        .next_id
        .unwrap_or(1)
        .max(highest.saturating_add(1))
        .max(1)
}

/// Human-readable one-line summary of a piece.
pub fn describe_piece(saved: &SavedPiece) -> String {
    format!(
        "#{} {} at ({:.2}, {:.2}) elev {:.2} rot {:.0}°",
        saved.id, saved.shape, saved.x, saved.z, saved.elevation, saved.rot_deg
    )
}

// ---------------------------------------------------------------------------
// Debug `level.json` (format party-dungeon-level v2) import/export
// ---------------------------------------------------------------------------

/// Convert the debug `level.json` document (what the game writes with `S`)
/// into a session Snapshot.
pub fn snapshot_from_level_json(doc: &serde_json::Value) -> Result<Snapshot, String> {
    let get = |key: &str| doc.get(key);
    let pieces = get("pieces")
        .and_then(|v| v.as_array())
        .ok_or("level.json: missing pieces array")?
        .iter()
        .map(saved_piece_from_level_json)
        .collect::<Result<Vec<_>, _>>()?;
    let groups = get("groups")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|g| serde_json::from_value::<session::SavedGroup>(g.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("level.json: bad groups: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    let platforms = get("platforms")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|p| serde_json::from_value::<SavedPlatform>(p.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("level.json: bad platforms: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    let character = get("character")
        .map(|c| {
            Ok::<_, String>(SavedCharacter {
                x: c.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                z: c.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                feet: c.get("feet").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                yaw_deg: c.get("yawDeg").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                hp: c.get("hp").and_then(|v| v.as_f64()).map(|v| v as f32),
                max_hp: c.get("maxHp").and_then(|v| v.as_f64()).map(|v| v as f32),
            })
        })
        .transpose()?;
    let camera = get("camera")
        .map(|c| {
            Ok::<_, String>(session::SavedCamera {
                target_x: c.get("targetX").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                target_y: c.get("targetY").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                target_z: c.get("targetZ").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                yaw: c.get("yawDeg").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                pitch: c.get("pitchDeg").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                distance: c.get("distance").and_then(|v| v.as_f64()).unwrap_or(7.5) as f32,
                mode: c
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("third")
                    .to_owned(),
            })
        })
        .transpose()?;
    let next_id = get("nextId").and_then(|v| v.as_u64()).map(|v| v as u32);
    Ok(Snapshot {
        version: SNAPSHOT_VERSION,
        next_id,
        pieces,
        character,
        camera,
        groups,
        undo: Vec::new(),
        redo: Vec::new(),
        platforms,
        grass: Vec::new(),
        grass_slows: false,
        vines: Vec::new(),
    })
}

fn f64_field(obj: &serde_json::Value, key: &str, default: f64) -> f32 {
    obj.get(key).and_then(|v| v.as_f64()).unwrap_or(default) as f32
}

fn saved_piece_from_level_json(item: &serde_json::Value) -> Result<SavedPiece, String> {
    let shape = item
        .get("shape")
        .and_then(|v| v.as_str())
        .ok_or("level.json piece: missing shape")?
        .to_owned();
    let id = item
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or("level.json piece: missing id")? as u32;
    Ok(SavedPiece {
        id,
        shape,
        x: f64_field(item, "x", 0.0),
        z: f64_field(item, "z", 0.0),
        elevation: f64_field(item, "y", 0.0),
        rot_deg: f64_field(item, "rotDeg", 0.0),
        pitch_deg: f64_field(item, "pitchDeg", 0.0),
        roll_deg: f64_field(item, "rollDeg", 0.0),
        dynamic: item
            .get("dynamic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        damage: item
            .get("damage")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        interact: item
            .get("interact")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_width: item
            .get("triggerWidth")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        trigger_length: item
            .get("triggerLength")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        trigger_height: item
            .get("triggerHeight")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        trigger_shape: item
            .get("triggerShape")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_alpha: item
            .get("triggerAlpha")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        trigger_color: item
            .get("triggerColor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_visible: item.get("triggerVisible").and_then(|v| v.as_bool()),
        name: item
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        group: item.get("group").and_then(|v| v.as_u64()).map(|v| v as u32),
        locked: item
            .get("locked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// Export a Snapshot in the debug `level.json` shape (same fields the game
/// writes with `S`: `y` for elevation, `yawDeg`/`pitchDeg` camera, ...).
pub fn export_level_json(snapshot: &Snapshot) -> serde_json::Value {
    let pieces: Vec<serde_json::Value> = snapshot
        .pieces
        .iter()
        .map(|p| {
            let mut obj = serde_json::json!({
                "id": p.id,
                "shape": p.shape,
                "x": (p.x * 1000.0).round() / 1000.0,
                "z": (p.z * 1000.0).round() / 1000.0,
                "y": (p.elevation * 1000.0).round() / 1000.0,
                "rotDeg": (p.rot_deg * 10.0).round() / 10.0,
                "pitchDeg": (p.pitch_deg * 10.0).round() / 10.0,
                "rollDeg": (p.roll_deg * 10.0).round() / 10.0,
                "dynamic": p.dynamic,
                "damage": p.damage.unwrap_or(0.0),
            });
            if let Some(interact) = &p.interact {
                obj["interact"] = serde_json::Value::String(interact.clone());
            }
            if let Some(w) = p.trigger_width {
                obj["triggerWidth"] = serde_json::json!(w);
            }
            if let Some(l) = p.trigger_length {
                obj["triggerLength"] = serde_json::json!(l);
            }
            if let Some(h) = p.trigger_height {
                obj["triggerHeight"] = serde_json::json!(h);
            }
            if let Some(shape) = &p.trigger_shape {
                obj["triggerShape"] = serde_json::Value::String(shape.clone());
            }
            if let Some(alpha) = p.trigger_alpha {
                obj["triggerAlpha"] = serde_json::json!(alpha);
            }
            if let Some(color) = &p.trigger_color {
                obj["triggerColor"] = serde_json::Value::String(color.clone());
            }
            if let Some(visible) = p.trigger_visible {
                obj["triggerVisible"] = serde_json::Value::Bool(visible);
            }
            if let Some(name) = &p.name {
                obj["name"] = serde_json::Value::String(name.clone());
            }
            if let Some(group) = p.group {
                obj["group"] = serde_json::json!(group);
            }
            if p.locked {
                obj["locked"] = serde_json::Value::Bool(true);
            }
            obj
        })
        .collect();
    let groups: Vec<serde_json::Value> = snapshot
        .groups
        .iter()
        .map(|g| {
            serde_json::json!({
                "id": g.id,
                "name": g.name,
                "parent": g.parent,
                "collapsed": g.collapsed,
            })
        })
        .collect();
    let platforms: Vec<serde_json::Value> = snapshot
        .platforms
        .iter()
        .map(|p| serde_json::to_value(p).unwrap_or(serde_json::Value::Null))
        .collect();
    let mut doc = serde_json::json!({
        "format": "party-dungeon-level",
        "version": 2,
        "units": "metres",
        "hexSize": model::HEX_SIZE,
        "nextId": snapshot.next_id.unwrap_or(1),
        "groups": groups,
        "pieces": pieces,
        "platforms": platforms,
    });
    if let Some(character) = &snapshot.character {
        doc["character"] = serde_json::json!({
            "x": character.x,
            "z": character.z,
            "feet": character.feet,
            "yawDeg": character.yaw_deg,
            "hp": character.hp.unwrap_or(100.0),
            "maxHp": character.max_hp.unwrap_or(100.0),
        });
    }
    if let Some(camera) = &snapshot.camera {
        doc["camera"] = serde_json::json!({
            "targetX": camera.target_x,
            "targetY": camera.target_y,
            "targetZ": camera.target_z,
            "yawDeg": camera.yaw,
            "pitchDeg": camera.pitch,
            "distance": camera.distance,
            "mode": camera.mode,
        });
    }
    doc
}

// ---------------------------------------------------------------------------
// Live bridge client (game running with PARTY_DUNGEON_BRIDGE)
// ---------------------------------------------------------------------------

/// PNG signature that precedes every frame the bridge can serve.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Whether `bytes` looks like a PNG (magic-number check; the bridge serves
/// raw `image/png` frames on `/screenshot`).
fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_MAGIC)
}

/// Split an HTTP/1.x response into its head (status line + headers) and raw
/// body. Binary-safe: the body is never decoded as UTF-8, so PNG bytes
/// survive the round trip.
fn split_http_response(buf: &[u8]) -> Option<(String, &[u8])> {
    let (head, body) = if let Some(index) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        (&buf[..index], &buf[index + 4..])
    } else {
        // Be liberal about bare-LF headers; the bridge itself always writes
        // CRLF, this only keeps the helper forgiving.
        let index = buf.windows(2).position(|w| w == b"\n\n")?;
        (&buf[..index], &buf[index + 2..])
    };
    Some((String::from_utf8_lossy(head).into_owned(), body))
}

/// Status code from a response head (`HTTP/1.0 200 OK` -> `200`).
fn http_status_code(head: &str) -> Option<u16> {
    head.lines().next()?.split_whitespace().nth(1)?.parse().ok()
}

/// Binary-safe HTTP/1.0 round trip to the live bridge. Returns the response
/// head plus the raw body bytes; `None` = unreachable or timed out before the
/// response arrived.
async fn http_roundtrip_bytes(
    addr: &str,
    request: String,
    read_timeout: std::time::Duration,
) -> Option<(String, Vec<u8>)> {
    let connect = tokio::time::timeout(
        std::time::Duration::from_millis(600),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .ok()?
    .ok()?;
    let (mut reader, mut writer) = connect.into_split();
    let write = tokio::time::timeout(
        std::time::Duration::from_millis(1000),
        tokio::io::AsyncWriteExt::write_all(&mut writer, request.as_bytes()),
    )
    .await
    .ok()
    .is_some();
    if !write {
        return None;
    }
    let mut buf = Vec::with_capacity(65536);
    let _read = tokio::time::timeout(
        read_timeout,
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf),
    )
    .await
    .ok()?;
    let (head, body) = split_http_response(&buf)?;
    Some((head, body.to_vec()))
}

/// String-only convenience wrapper used by the JSON endpoints.
async fn http_roundtrip(addr: &str, request: String) -> Option<String> {
    let (_head, body) =
        http_roundtrip_bytes(addr, request, std::time::Duration::from_millis(2000)).await?;
    Some(String::from_utf8_lossy(&body).into_owned())
}

/// Fetch one PNG frame from a headless game (`GET /screenshot`).
///
/// Screenshots only work when the game runs with `--headless`; anything else
/// answers `503` with a JSON `{"ok":false,"error":...}` body, which is
/// surfaced verbatim as the error. The first frame right after startup can
/// take several seconds (renderer warm-up), so the read timeout is generous.
pub async fn bridge_screenshot(addr: &str) -> Result<Vec<u8>, String> {
    let request =
        format!("GET /screenshot HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    // The bridge keeps the request open for up to 12 s because the first
    // capture can only be armed once the renderer has drawn a frame (a
    // software renderer spends several seconds compiling pipelines first).
    // Wait 20 s here so a cold game still yields a frame instead of a bogus
    // "unreachable".
    let Some((head, body)) =
        http_roundtrip_bytes(addr, request, std::time::Duration::from_secs(20)).await
    else {
        return Err(format!(
            "live bridge at {addr} is unreachable; screenshots need the game running \
             with --headless (live bridge on by default at 127.0.0.1:6559)"
        ));
    };
    let status = http_status_code(&head).unwrap_or(0);
    if status != 200 {
        let text = String::from_utf8_lossy(&body);
        let text = text.trim();
        let detail = serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| text.to_owned());
        return Err(if detail.is_empty() {
            format!("bridge /screenshot returned HTTP {status}")
        } else {
            format!("bridge /screenshot returned HTTP {status}: {detail}")
        });
    }
    if body.is_empty() {
        return Err("bridge /screenshot returned an empty body".to_owned());
    }
    if !is_png(&body) {
        return Err(format!(
            "bridge /screenshot did not return a PNG ({} bytes, HTTP {status})",
            body.len()
        ));
    }
    Ok(body)
}

/// `GET /health` from the live bridge as JSON (`headless`, `render`,
/// `screenshot` flags). `None` = unreachable or not a JSON reply.
pub async fn bridge_health(addr: &str) -> Option<serde_json::Value> {
    let request = format!("GET /health HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let (head, body) =
        http_roundtrip_bytes(addr, request, std::time::Duration::from_millis(2000)).await?;
    if http_status_code(&head)? != 200 {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

/// Fetch the live editor Snapshot from a running game. `None` = unreachable.
pub async fn bridge_state(addr: &str) -> Option<Snapshot> {
    let body = http_roundtrip(
        addr,
        format!("GET /state HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )
    .await?;
    Snapshot::from_json(&body).ok()
}

/// POST a command to the live bridge. Returns the raw response body.
pub async fn bridge_command(addr: &str, command: &serde_json::Value) -> Option<String> {
    let body = command.to_string();
    let request = format!(
        "POST /command HTTP/1.0\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    http_roundtrip(addr, request).await
}

pub async fn bridge_command_json(
    addr: &str,
    command: &serde_json::Value,
) -> Result<String, String> {
    let body = bridge_command(addr, command)
        .await
        .ok_or_else(|| format!("live bridge at {addr} is unreachable"))?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| format!("bad bridge reply: {body}"))?;
    if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        Ok(value
            .get("note")
            .and_then(|v| v.as_str())
            .unwrap_or("ok")
            .to_owned())
    } else {
        Err(value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("bridge command failed")
            .to_owned())
    }
}

/// Load preferring the live bridge; falls back to SurrealDB (the single
/// shared world). There are no levels and no files anymore.
pub async fn load_effective(
    store: &Store,
    source: Source,
) -> Result<(Snapshot, bool), String> {
    if source != Source::Db {
        if let Some(addr) = store.bridge_addr() {
            if let Some(snapshot) = bridge_state(addr).await {
                store.remember_rids_fresh(&snapshot);
                return Ok((snapshot, true));
            }
            if source == Source::Live {
                return Err(format!("live bridge at {addr} is unreachable"));
            }
        } else if source == Source::Live {
            return Err("live bridge is disabled (PARTY_DUNGEON_BRIDGE is empty)".to_owned());
        }
    }
    store.db_snapshot().await.map(|snapshot| (snapshot, false))
}

/// Persist to SurrealDB (full replace via import) and, when live, forward the
/// edit command so the running game applies it instantly too.
pub async fn save_effective(
    store: &Store,
    snapshot: &Snapshot,
    live: bool,
    live_note: Option<serde_json::Value>,
) -> Result<String, String> {
    let mut note = store.save_world(snapshot, "mcp").await?;
    // Forward a targeted edit command so a live-attached editor applies it
    // instantly; everyone else follows via the sim's interest stream (it polls
    // the DB for edits, then streams only the changed cells to each peer). No
    // command means DB-only.
    if live {
        if let Some(command) = live_note {
            if let Some(addr) = store.bridge_addr() {
                match bridge_command_json(addr, &command).await {
                    Ok(applied) => {
                        note = format!("{note} and applied live ({applied})");
                    }
                    Err(error) => {
                        note = format!("{note} (live bridge not applied: {error})");
                    }
                }
            }
        }
    }
    Ok(note)
}

#[allow(dead_code)]
pub fn platform_summary(platform: &SavedPlatform) -> serde_json::Value {
    serde_json::json!({
        "piece": platform.piece,
        "mode": platform.mode,
        "speed": platform.speed,
        "paused": platform.paused,
        "waypoints": platform.waypoints.iter().map(|w| serde_json::json!({
            "x": w.x, "z": w.z, "incoming": w.incoming, "outgoing": w.outgoing,
        })).collect::<Vec<_>>(),
    })
}

#[allow(dead_code)]
pub fn waypoint_json(x: f32, z: f32) -> SavedPlatformPoint {
    SavedPlatformPoint {
        x,
        z,
        incoming: None,
        outgoing: None,
    }
}

/// Validate a platform path the same way the game does on load.
pub fn validate_platform_path(piece_id: u32, platform: &SavedPlatform) -> Result<(), String> {
    if platform.piece != piece_id {
        return Err("platform.piece must match the piece id".to_owned());
    }
    if platform.waypoints.is_empty() {
        return Err("platform path needs at least one waypoint".to_owned());
    }
    for point in &platform.waypoints {
        if ![point.x, point.z].iter().all(|v| v.is_finite()) {
            return Err("platform waypoints must be finite".to_owned());
        }
        for handle in [point.incoming, point.outgoing].into_iter().flatten() {
            if !handle[0].is_finite() || !handle[1].is_finite() {
                return Err("platform handles must be finite".to_owned());
            }
        }
    }
    if let Some(speed) = platform.speed {
        if !speed.is_finite() || speed <= 0.0 {
            return Err("platform speed must be positive".to_owned());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal one-shot HTTP bridge: accepts one connection per response
    /// and writes the given raw bytes back.
    async fn fake_bridge(responses: Vec<Vec<u8>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake bridge");
        let addr = listener.local_addr().expect("fake bridge addr").to_string();
        tokio::spawn(async move {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut request).await;
                let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &response).await;
            }
        });
        addr
    }

    fn response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = format!(
            "HTTP/1.0 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn split_http_response_keeps_binary_body_intact() {
        // The body deliberately contains CRLF pairs and the PNG signature, so
        // a UTF-8 round trip would corrupt it.
        let png = b"\x89PNG\r\n\x1a\n\x00\x01\x02\r\n\r\nrest-of-frame";
        let mut raw = b"HTTP/1.0 200 OK\r\nContent-Type: image/png\r\n\r\n".to_vec();
        raw.extend_from_slice(png);
        let (head, body) = split_http_response(&raw).expect("splits");
        assert!(head.contains("200 OK"), "unexpected head: {head}");
        assert_eq!(body, png);
    }

    #[test]
    fn status_line_and_png_detection() {
        assert_eq!(
            http_status_code("HTTP/1.0 200 OK\r\nContent-Length: 0"),
            Some(200)
        );
        assert_eq!(
            http_status_code("HTTP/1.0 503 Service Unavailable"),
            Some(503)
        );
        assert_eq!(http_status_code("garbage"), None);
        assert!(is_png(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_png(br#"{"ok":false}"#));
    }

    #[tokio::test]
    async fn screenshot_fetches_png_bytes() {
        let png = b"\x89PNG\r\n\x1a\n-idat".to_vec();
        let addr = fake_bridge(vec![response("200 OK", "image/png", &png)]).await;
        let got = bridge_screenshot(&addr).await.expect("png");
        assert_eq!(got, png);
    }

    #[tokio::test]
    async fn screenshot_surfaces_bridge_json_error() {
        let body = br#"{"ok":false,"error":"screenshots need the game in headless mode (--headless)"}"#;
        let addr = fake_bridge(vec![response(
            "503 Service Unavailable",
            "application/json",
            body,
        )])
        .await;
        let error = bridge_screenshot(&addr).await.expect_err("503 is an error");
        assert!(error.contains("503"), "unexpected error: {error}");
        assert!(error.contains("--headless"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn health_reads_headless_flags() {
        let body =
            br#"{"ok":true,"game":"party-dungeon","headless":true,"render":null,"screenshot":true}"#;
        let addr = fake_bridge(vec![response("200 OK", "application/json", body)]).await;
        let health = bridge_health(&addr).await.expect("health json");
        assert_eq!(
            health.get("headless").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            health.get("screenshot").and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
