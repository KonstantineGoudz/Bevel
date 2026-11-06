//! File store + level-data helpers for the Party Dungeon MCP server.
//!
//! Canonical on-disk format is the game session [`Snapshot`] (version 1):
//! `{ "version": 1, "pieces": [...], "character": {...}, ... }`. The native
//! game also boots from these files (`--level <path>` /
//! `PARTY_DUNGEON_LEVEL`), so file edits apply on (re)start. When the game is
//! running with the live bridge (`PARTY_DUNGEON_BRIDGE`), mutations are also
//! forwarded so they apply instantly.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use party_dungeon::model::{self, Piece};
use party_dungeon::path;
use party_dungeon::session::{
    self, SavedCharacter, SavedPiece, SavedPlatform, SavedPlatformPoint, Snapshot,
    SNAPSHOT_VERSION,
};

/// Which backend a tool should use.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Try the live bridge first, fall back to the file.
    #[default]
    Auto,
    File,
    Live,
}

impl Source {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("auto").to_ascii_lowercase().as_str() {
            "file" => Source::File,
            "live" => Source::Live,
            _ => Source::Auto,
        }
    }
}

/// Resolve a level name to a file path. `None` means the default level.
pub fn sanitize_level_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return None;
    }
    if trimmed == "." || trimmed == ".." || trimmed.contains("..") {
        return None;
    }
    Some(trimmed.to_owned())
}

#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    default_level: String,
    bridge: Option<String>,
}

impl Store {
    pub fn from_env() -> Self {
        let dir = std::env::var("PARTY_DUNGEON_LEVELS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                for candidate in ["game/levels", "levels"] {
                    let path = cwd.join(candidate);
                    if path.is_dir() {
                        return path;
                    }
                }
                cwd.join("game/levels")
            });
        let default_level = std::env::var("PARTY_DUNGEON_LEVEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "level".to_owned());
        // `PARTY_DUNGEON_LEVEL` may be a full snapshot path instead of a name.
        let bridge = match std::env::var("PARTY_DUNGEON_BRIDGE") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(value),
            Err(_) => Some("127.0.0.1:3559".to_owned()),
        };
        Self {
            dir,
            default_level,
            bridge,
        }
    }

    pub fn levels_dir(&self) -> &Path {
        &self.dir
    }

    pub fn default_level_name(&self) -> &str {
        if self.default_level.ends_with(".json")
            || self.default_level.contains('/')
            || self.default_level.contains('\\')
        {
            "level"
        } else {
            &self.default_level
        }
    }

    /// An explicit snapshot file via `PARTY_DUNGEON_LEVEL=/path/to/file.json`.
    pub fn explicit_file(&self) -> Option<PathBuf> {
        let value = std::env::var("PARTY_DUNGEON_LEVEL").ok()?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let path = PathBuf::from(trimmed);
        let looks_like_path = trimmed.ends_with(".json")
            || trimmed.contains('/')
            || trimmed.contains('\\');
        looks_like_path.then_some(path)
    }

    pub fn bridge_addr(&self) -> Option<&str> {
        self.bridge.as_deref()
    }

    pub fn path_for(&self, level: Option<&str>) -> Result<PathBuf, String> {
        if level.is_none() {
            if let Some(path) = self.explicit_file() {
                return Ok(path);
            }
        }
        let name = level
            .map(sanitize_level_name)
            .unwrap_or_else(|| sanitize_level_name(&self.default_level).or(Some("level".into())))
            .ok_or_else(|| "invalid level name (use a-z, 0-9, '-', '_')".to_owned())?;
        let mut file = name;
        if !file.ends_with(".json") {
            file.push_str(".json");
        }
        Ok(self.dir.join(file))
    }

    pub fn load_snapshot(&self, level: Option<&str>) -> Result<(PathBuf, Snapshot), String> {
        let path = self.path_for(level)?;
        if !path.exists() {
            return Ok((
                path,
                Snapshot {
                    version: SNAPSHOT_VERSION,
                    next_id: Some(1),
                    ..Default::default()
                },
            ));
        }
        let text =
            std::fs::read_to_string(&path).map_err(|error| format!("could not read level: {error}"))?;
        let display = path.display().to_string();
        Self::parse_snapshot_text(&text)
            .map(|snapshot| (path, snapshot))
            .map_err(|error| format!("{display}: {error}"))
    }

    /// Parse either a session Snapshot (v1) or the debug `level.json` (v2)
    /// the game writes with `S` / CopyJson.
    pub fn parse_snapshot_text(text: &str) -> Result<Snapshot, String> {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            if value.get("format").and_then(|v| v.as_str()) == Some("party-dungeon-level") {
                return snapshot_from_level_json(&value);
            }
        }
        Snapshot::from_json(text).map_err(|error| format!("{error:?}"))
    }

    pub fn save_snapshot(&self, path: &Path, snapshot: &Snapshot) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create levels dir: {error}"))?;
        }
        // Validate before writing so a bad edit never corrupts the file.
        snapshot
            .clone()
            .validate()
            .map_err(|error| format!("level failed validation: {error:?}"))?;
        if path.exists() {
            let backup = path.with_extension("json.bak");
            if let Ok(text) = std::fs::read_to_string(path) {
                let _ = std::fs::write(&backup, text);
            }
        }
        let pretty = serde_json::to_string_pretty(snapshot)
            .map_err(|error| format!("could not serialize level: {error}"))?;
        std::fs::write(path, pretty).map_err(|error| format!("could not write level: {error}"))?;
        Ok(())
    }

    pub fn list_levels(&self) -> Result<Vec<LevelSummary>, String> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(out);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) == Some("json")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem.ends_with(".bak"))
            {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("")
                .to_owned();
            if name.ends_with(".bak") {
                continue;
            }
            let summary = match std::fs::read_to_string(&path) {
                Ok(text) => match Self::parse_snapshot_text(&text) {
                    Ok(snapshot) => LevelSummary {
                        name,
                        pieces: snapshot.pieces.len(),
                        platforms: snapshot.platforms.len(),
                        has_character: snapshot.character.is_some(),
                        ..LevelSummary::default_name()
                    },
                    Err(_) => LevelSummary {
                        name,
                        pieces: 0,
                        platforms: 0,
                        has_character: false,
                        unreadable: true,
                    },
                },
                Err(_) => LevelSummary {
                    name,
                    pieces: 0,
                    platforms: 0,
                    has_character: false,
                    unreadable: true,
                },
            };
            out.push(summary);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LevelSummary {
    pub name: String,
    pub pieces: usize,
    pub platforms: usize,
    pub has_character: bool,
    #[serde(default)]
    pub unreadable: bool,
}

impl LevelSummary {
    fn default_name() -> Self {
        Self {
            name: String::new(),
            pieces: 0,
            platforms: 0,
            has_character: false,
            unreadable: false,
        }
    }
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
    let default_damage = model::shapes()
        .get(shape)
        .map(|s| s.damage)
        .unwrap_or(0.0);
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
    let next = snapshot.next_id.unwrap_or(1).max(highest.saturating_add(1)).max(1);
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
        dynamic: item.get("dynamic").and_then(|v| v.as_bool()).unwrap_or(false),
        damage: item.get("damage").and_then(|v| v.as_f64()).map(|v| v as f32),
        interact: item
            .get("interact")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_width: item.get("triggerWidth").and_then(|v| v.as_f64()).map(|v| v as f32),
        trigger_length: item.get("triggerLength").and_then(|v| v.as_f64()).map(|v| v as f32),
        trigger_height: item.get("triggerHeight").and_then(|v| v.as_f64()).map(|v| v as f32),
        trigger_shape: item
            .get("triggerShape")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_alpha: item.get("triggerAlpha").and_then(|v| v.as_f64()).map(|v| v as f32),
        trigger_color: item
            .get("triggerColor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned()),
        trigger_visible: item.get("triggerVisible").and_then(|v| v.as_bool()),
        name: item.get("name").and_then(|v| v.as_str()).map(|s| s.to_owned()),
        group: item.get("group").and_then(|v| v.as_u64()).map(|v| v as u32),
        locked: item.get("locked").and_then(|v| v.as_bool()).unwrap_or(false),
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

async fn http_roundtrip(addr: &str, request: String) -> Option<String> {
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
        std::time::Duration::from_millis(2000),
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf),
    )
    .await.ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let body = text.split("\r\n\r\n").nth(1).or_else(|| text.split("\n\n").nth(1))?;
    Some(body.to_owned())
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

pub async fn bridge_command_json(addr: &str, command: &serde_json::Value) -> Result<String, String> {
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

/// Load preferring the live bridge; falls back to the file.
pub async fn load_effective(
    store: &Store,
    level: Option<&str>,
    source: Source,
) -> Result<(PathBuf, Snapshot, bool), String> {
    if source != Source::File {
        if let Some(addr) = store.bridge_addr() {
            if level.is_none() {
                if let Some(snapshot) = bridge_state(addr).await {
                    return Ok((store.path_for(None)?, snapshot, true));
                }
            }
            if source == Source::Live {
                return Err(format!("live bridge at {addr} is unreachable"));
            }
        } else if source == Source::Live {
            return Err("live bridge is disabled (PARTY_DUNGEON_BRIDGE is empty)".to_owned());
        }
    }
    store.load_snapshot(level).map(|(path, snapshot)| (path, snapshot, false))
}

/// Persist to the file and, when live, forward the whole snapshot so the
/// running game adopts it instantly.
pub async fn save_effective(
    store: &Store,
    path: &Path,
    snapshot: &Snapshot,
    live: bool,
    live_note: Option<serde_json::Value>,
) -> Result<String, String> {
    store.save_snapshot(path, snapshot)?;
    if live {
        if let Some(addr) = store.bridge_addr() {
            let command = live_note.unwrap_or_else(|| serde_json::json!({"op":"reload"}));
            match bridge_command_json(addr, &command).await {
                Ok(note) => {
                    return Ok(format!("saved {} and applied live ({note})", path.display()));
                }
                Err(error) => {
                    return Ok(format!(
                        "saved {} (live bridge not applied: {error})",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(format!("saved {}", path.display()))
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
