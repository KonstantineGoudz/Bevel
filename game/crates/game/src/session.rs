//! Saved-map snapshots: the versioned JSON shape kept in browser storage.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use bevy::math::{Vec2, Vec3};
use bevy::prelude::Resource;

use crate::{
    editor::{
        Character, LevelState, Orbit, ViewMode, VineNode,
        outline::{Group, SceneOutline, clean_label},
    },
    model::{self, Piece},
    statuses::Health,
};

pub const SNAPSHOT_VERSION: u32 = 1;
pub const MAX_HISTORY: usize = 60;


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    MissingVersion,
    UnsupportedVersion(u32),
    UnknownShape(String),
    NonFinite(&'static str),
    Parse(String),
    NotFound,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub version: u32,
    #[serde(default)]
    pub next_id: Option<u32>,
    #[serde(default)]
    pub pieces: Vec<SavedPiece>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub character: Option<SavedCharacter>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub camera: Option<SavedCamera>,
    /// Named groups. Missing on older saves.
    #[serde(default)]
    pub groups: Vec<SavedGroup>,
    #[serde(default, deserialize_with = "deserialize_history")]
    pub undo: Vec<SavedLevel>,
    #[serde(default, deserialize_with = "deserialize_history")]
    pub redo: Vec<SavedLevel>,
    /// Moving-platform paths, keyed by piece id. Missing on older saves.
    #[serde(default)]
    pub platforms: Vec<SavedPlatform>,
    /// Painted grass cover. Missing on older saves, which grow none.
    #[serde(default)]
    pub grass: Vec<SavedGrass>,
    /// Whether grass slows movement and routes. Missing means decoration.
    #[serde(default)]
    pub grass_slows: bool,
    /// Painted procedural 3D vines. Missing on older saves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vines: Vec<SavedVine>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn deserialize_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value::<Option<T>>(value).ok().flatten())
}

fn deserialize_history<'de, D>(deserializer: D) -> Result<Vec<SavedLevel>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Ok(entries) = serde_json::from_value::<Vec<serde_json::Value>>(value) else {
        return Ok(Vec::new());
    };
    Ok(entries.into_iter().filter_map(parse_saved_level).collect())
}

/// History entries used to be a bare piece array. Accept both that and the
/// newer `{ pieces, groups }` object so old autosaves keep loading.
fn parse_saved_level(value: serde_json::Value) -> Option<SavedLevel> {
    if value.is_array() {
        serde_json::from_value::<Vec<SavedPiece>>(value)
            .ok()
            .map(|pieces| SavedLevel {
                pieces,
                groups: Vec::new(),
                grass: Vec::new(),
                vines: Vec::new(),
            })
    } else {
        serde_json::from_value::<SavedLevel>(value).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPiece {
    pub id: u32,
    pub shape: String,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
    /// Yaw in degrees. Kept under its original name so older levels load.
    pub rot_deg: f32,
    /// Pitch in degrees. Missing on older levels, which are upright.
    #[serde(default)]
    pub pitch_deg: f32,
    /// Roll in degrees. Missing on older levels, which are upright.
    #[serde(default)]
    pub roll_deg: f32,
    /// Physics-driven pieces fall and collide. Missing on older levels.
    #[serde(default)]
    pub dynamic: bool,
    /// Per-piece collision-trigger damage. Missing on older saves, which use
    /// the catalogue default (`Shape::damage`); `None` on new saves also
    /// means "use the catalogue default" so untouched pieces stay small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<f32>,
    /// Interaction effect id (e.g. "togglePause"). Missing or unknown means
    /// the piece is not interactable, so old saves and newer unknown actions
    /// keep loading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact: Option<String>,
    /// Parametric trigger volume size (width, length, height) for trigger shapes.
    /// Missing on older saves or non-trigger pieces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_length: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_height: Option<f32>,
    /// Trigger volume form: "box", "sphere", "cylinder". Missing means Box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_shape: Option<String>,
    /// Transparency for trigger zones (0 transparent .. 1 opaque). Missing means default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_alpha: Option<f32>,
    /// RGB color for trigger zones as #RRGGBB. Missing means default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_color: Option<String>,
    /// Whether trigger is rendered in edit mode. Missing means true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_visible: Option<bool>,
    /// User label. Missing means "use the shape name".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Owning group id. Missing means the scene root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
    /// Locked against viewport selection. Missing means unlocked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
}

/// A named group in the object graph. Groups nest through `parent`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedGroup {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub parent: Option<u32>,
    #[serde(default)]
    pub collapsed: bool,
}

/// One painted grass hex: axial coords plus density.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedGrass {
    pub q: i32,
    pub r: i32,
    /// Paint density 1..=4; clamped on load.
    pub density: u8,
}

/// One painted procedural vine anchor on a 3D surface.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedVine {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub nx: f32,
    pub ny: f32,
    pub nz: f32,
    pub seed: u32,
}

impl From<&VineNode> for SavedVine {
    fn from(node: &VineNode) -> Self {
        Self {
            x: node.pos.x,
            y: node.pos.y,
            z: node.pos.z,
            nx: node.normal.x,
            ny: node.normal.y,
            nz: node.normal.z,
            seed: node.seed,
        }
    }
}

/// One undo step. The object shape (rather than a bare array) lets groups,
/// labels, grass and vines travel with the pieces they describe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLevel {
    #[serde(default)]
    pub pieces: Vec<SavedPiece>,
    #[serde(default)]
    pub groups: Vec<SavedGroup>,
    /// Grass at this history step. Missing on older saves, which grew none.
    #[serde(default)]
    pub grass: Vec<SavedGrass>,
    /// Vines at this history step. Missing on older saves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vines: Vec<SavedVine>,
}

impl From<&Group> for SavedGroup {
    fn from(group: &Group) -> Self {
        Self {
            id: group.id,
            name: group.name.clone(),
            parent: group.parent,
            collapsed: group.collapsed,
        }
    }
}

impl From<&LevelState> for SavedLevel {
    fn from(state: &LevelState) -> Self {
        let mut grass: Vec<SavedGrass> = state
            .grass
            .iter()
            .map(|(&(q, r), &density)| SavedGrass { q, r, density })
            .collect();
        grass.sort_by_key(|saved| (saved.q, saved.r));
        Self {
            pieces: state
                .pieces
                .iter()
                .map(|piece| SavedPiece::from_piece(piece, &state.outline))
                .collect(),
            groups: state.outline.groups.iter().map(SavedGroup::from).collect(),
            grass,
            vines: state.vines.iter().map(SavedVine::from).collect(),
        }
    }
}

/// Keep one painted hex per load: density clamped into range, zeroes dropped.
fn validate_grass(saved: &[SavedGrass]) -> std::collections::HashMap<(i32, i32), u8> {
    let mut grass = std::collections::HashMap::new();
    for cell in saved {
        if cell.density == 0 {
            continue;
        }
        grass.insert((cell.q, cell.r), cell.density.clamp(1, 4));
    }
    grass
}

/// Validate saved vine anchors: drop non-finite coordinates and normalize normals.
fn validate_vines(saved: &[SavedVine]) -> Vec<VineNode> {
    saved
        .iter()
        .filter_map(|v| {
            let finite = [v.x, v.y, v.z, v.nx, v.ny, v.nz]
                .iter()
                .all(|f| f.is_finite());
            if !finite {
                return None;
            }
            let n = Vec3::new(v.nx, v.ny, v.nz).normalize_or_zero();
            let normal = if n.length_squared() < 0.5 { Vec3::Y } else { n };
            Some(VineNode {
                pos: Vec3::new(v.x, v.y, v.z),
                normal,
                seed: v.seed,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCharacter {
    pub x: f32,
    pub z: f32,
    #[serde(default)]
    pub feet: f32,
    pub yaw_deg: f32,
    /// Current health points. Missing on older saves, which load at full.
    #[serde(default)]
    pub hp: Option<f32>,
    /// Maximum health points. Missing on older saves, which use the default.
    #[serde(default)]
    pub max_hp: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCamera {
    pub target_x: f32,
    pub target_y: f32,
    pub target_z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlatformPoint {
    pub x: f32,
    pub z: f32,
    #[serde(default)]
    pub incoming: Option<[f32; 2]>,
    #[serde(default)]
    pub outgoing: Option<[f32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPlatform {
    pub piece: u32,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub waypoints: Vec<SavedPlatformPoint>,
}

impl From<&Piece> for SavedPiece {
    fn from(piece: &Piece) -> Self {
        // Omit damage when it matches the catalogue default so untouched
        // maps keep their old (smaller) JSON; any real edit persists.
        let default = model::shapes()
            .get(piece.shape)
            .map(|shape| shape.damage)
            .unwrap_or(0.0);
        let damage = if (piece.trigger.damage - default).abs() < 1e-6 {
            None
        } else {
            Some(piece.trigger.damage)
        };
        let shape_ref = model::shapes().get(piece.shape);
        let is_parametric = shape_ref.is_some_and(|s| s.is_parametric());
        let is_glade = shape_ref.is_some_and(|s| s.is_glade());
        let (
            trigger_width,
            trigger_length,
            trigger_height,
            trigger_shape,
            trigger_alpha,
            trigger_color,
            trigger_visible,
        ) = if is_parametric {
            let size = model::sanitize_trigger_size(piece.trigger_size);
            let alpha = if is_glade {
                crate::glade::roof_pitch(piece, shape_ref.unwrap())
            } else {
                model::sanitize_trigger_alpha(piece.trigger_alpha)
            };
            let color = piece.trigger_color;
            let visible = piece.trigger_visible;
            let shape_str = if is_glade {
                piece.trigger_shape_override.id().to_owned()
            } else {
                model::piece_trigger_shape(piece, shape_ref.unwrap())
                    .id()
                    .to_owned()
            };
            // Only persist non-default to keep JSON small; loader defaults handle missing.
            let is_default = (size - model::TRIGGER_DEFAULT_SIZE).length_squared() < 1e-6
                && shape_str == "box"
                && (alpha - model::TRIGGER_DEFAULT_ALPHA).abs() < 1e-6
                && color == model::TRIGGER_DEFAULT_COLOR
                && visible == model::TRIGGER_DEFAULT_VISIBLE;
            if is_default {
                (None, None, None, None, None, None, None)
            } else {
                (
                    Some(size.x),
                    Some(size.z),
                    Some(size.y),
                    if shape_str == "box" {
                        None
                    } else {
                        Some(shape_str)
                    },
                    if (alpha - model::TRIGGER_DEFAULT_ALPHA).abs() < 1e-6 {
                        None
                    } else {
                        Some(alpha)
                    },
                    if color == model::TRIGGER_DEFAULT_COLOR {
                        None
                    } else {
                        Some(model::trigger_color_to_hex_string(color))
                    },
                    if visible == model::TRIGGER_DEFAULT_VISIBLE {
                        None
                    } else {
                        Some(visible)
                    },
                )
            }
        } else {
            (None, None, None, None, None, None, None)
        };
        Self {
            id: piece.id,
            shape: model::shapes()[piece.shape].id.to_owned(),
            x: piece.pos.x,
            z: piece.pos.y,
            elevation: piece.elevation,
            rot_deg: piece.rot.to_degrees(),
            pitch_deg: piece.pitch.to_degrees(),
            roll_deg: piece.roll.to_degrees(),
            dynamic: piece.dynamic,
            damage,
            interact: if piece.interact.is_none() {
                None
            } else {
                Some(piece.interact.id().to_owned())
            },
            trigger_width,
            trigger_length,
            trigger_height,
            trigger_shape,
            trigger_alpha,
            trigger_color,
            trigger_visible,
            name: None,
            group: None,
            locked: false,
        }
    }
}

impl SavedPiece {
    /// Serialize a piece together with the outliner metadata for it.
    pub fn from_piece(piece: &Piece, outline: &SceneOutline) -> Self {
        Self {
            name: outline.labels.get(&piece.id).cloned(),
            group: outline.membership.get(&piece.id).copied(),
            locked: outline.is_locked(piece.id),
            ..Self::from(piece)
        }
    }
}

impl From<&Character> for SavedCharacter {
    fn from(character: &Character) -> Self {
        Self {
            x: character.pos.x,
            z: character.pos.y,
            feet: character.feet,
            yaw_deg: character.yaw.to_degrees(),
            hp: Some(character.health.current),
            max_hp: Some(character.health.max),
        }
    }
}

impl From<&Orbit> for SavedCamera {
    fn from(orbit: &Orbit) -> Self {
        Self {
            target_x: orbit.target.x,
            target_y: orbit.target.y,
            target_z: orbit.target.z,
            yaw: orbit.yaw,
            pitch: orbit.pitch,
            distance: orbit.distance,
            mode: orbit.mode.id().to_owned(),
        }
    }
}

impl Snapshot {
    pub fn from_json(json: &str) -> Result<Snapshot, LoadError> {
        serde_json::from_str(json).map_err(|error| LoadError::Parse(error.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("snapshot serializes to JSON")
    }

    pub fn validate(self) -> Result<Loaded, LoadError> {
        if self.version == 0 {
            return Err(LoadError::MissingVersion);
        }
        if self.version != SNAPSHOT_VERSION {
            return Err(LoadError::UnsupportedVersion(self.version));
        }

        let mut outline = SceneOutline::default();
        let mut pieces = Vec::new();
        for saved in self.pieces {
            let name = saved
                .name
                .clone()
                .map(|name| clean_label(&name))
                .filter(|name| !name.is_empty());
            let group = saved.group;
            let locked = saved.locked;
            let piece = validate_piece(saved)?;
            if let Some(name) = name {
                outline.labels.insert(piece.id, name);
            }
            if let Some(group) = group {
                outline.membership.insert(piece.id, group);
            }
            if locked {
                outline.locked.insert(piece.id);
            }
            pieces.push(piece);
        }
        outline.groups = validate_groups(self.groups, &pieces);
        let group_ids: HashSet<u32> = outline.groups.iter().map(|group| group.id).collect();
        outline
            .membership
            .retain(|_, group| group_ids.contains(group));
        outline.bump();

        let mut highest_id = pieces
            .iter()
            .map(|piece| piece.id)
            .chain(outline.groups.iter().map(|group| group.id))
            .max()
            .unwrap_or(0);

        let undo = validate_history(self.undo, &mut highest_id);
        let redo = validate_history(self.redo, &mut highest_id);

        let character = self.character.and_then(|saved| {
            let health = match (saved.hp, saved.max_hp) {
                (Some(hp), Some(max)) => {
                    if !hp.is_finite() || !max.is_finite() {
                        return None;
                    }
                    Health::new(hp, max)
                }
                (Some(hp), None) => {
                    if !hp.is_finite() {
                        return None;
                    }
                    Health::new(hp, Health::default().max)
                }
                (None, Some(max)) => {
                    if !max.is_finite() {
                        return None;
                    }
                    Health::full(max)
                }
                (None, None) => Health::default(),
            };
            [saved.x, saved.z, saved.feet, saved.yaw_deg]
                .iter()
                .all(|value| value.is_finite())
                .then_some(Character {
                    pos: Vec2::new(saved.x, saved.z),
                    feet: saved.feet,
                    yaw: saved.yaw_deg.to_radians(),
                    vy: 0.0,
                    hv: Vec2::ZERO,
                    ride_vel: Vec2::ZERO,
                    grounded: false,
                    sliding: false,
                    health,
                })
        });

        let camera = self.camera.and_then(|saved| {
            let mode = ViewMode::parse(&saved.mode)?;
            [
                saved.target_x,
                saved.target_y,
                saved.target_z,
                saved.yaw,
                saved.pitch,
                saved.distance,
            ]
            .iter()
            .all(|value| value.is_finite())
            .then_some(Orbit {
                target: Vec3::new(saved.target_x, saved.target_y, saved.target_z),
                yaw: saved.yaw,
                pitch: saved.pitch,
                distance: saved.distance,
                mode,
            })
            .filter(|camera| camera.distance > 0.0)
        });

        let next_id = self
            .next_id
            .unwrap_or(1)
            .max(highest_id.saturating_add(1))
            .max(1);

        let platforms = self
            .platforms
            .iter()
            .filter_map(validate_platform)
            .collect();

        Ok(Loaded {
            pieces,
            outline,
            character,
            camera,
            undo,
            redo,
            next_id,
            platforms,
            grass: validate_grass(&self.grass),
            grass_slows: self.grass_slows,
            vines: validate_vines(&self.vines),
        })
    }
}

/// Where native control bindings live. Explicit `PARTY_DUNGEON_CONTROLS` wins;
/// otherwise `game/controls.json` when the checkout layout is visible,
/// falling back to `./controls.json`.
#[cfg(not(target_arch = "wasm32"))]
pub fn native_controls_path() -> std::path::PathBuf {
    if let Ok(value) = std::env::var("PARTY_DUNGEON_CONTROLS") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return std::path::PathBuf::from(trimmed);
        }
    }
    // Single world: no levels dir anymore. Prefer `game/controls.json` when the
    // checkout layout is visible so existing bindings keep loading.
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if cwd.join("game").is_dir() {
        return cwd.join("game").join("controls.json");
    }
    cwd.join("controls.json")
}

/// Parse either a session Snapshot (v1) or the debug `level.json`
/// (format `party-dungeon-level` v2) the game writes with `S` / CopyJson.
pub fn parse_any_snapshot_text(text: &str) -> Result<Snapshot, LoadError> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if value.get("format").and_then(|v| v.as_str()) == Some("party-dungeon-level") {
            return snapshot_from_level_json(&value).map_err(LoadError::Parse);
        }
    }
    Snapshot::from_json(text)
}

/// Convert the debug `level.json` document into a session Snapshot.
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
                .map(|g| serde_json::from_value::<SavedGroup>(g.clone()))
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
            Ok::<_, String>(SavedCamera {
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

fn saved_piece_from_level_json(item: &serde_json::Value) -> Result<SavedPiece, String> {
    let f64_field =
        |key: &str, default: f64| item.get(key).and_then(|v| v.as_f64()).unwrap_or(default) as f32;
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
        x: f64_field("x", 0.0),
        z: f64_field("z", 0.0),
        elevation: f64_field("y", 0.0),
        rot_deg: f64_field("rotDeg", 0.0),
        pitch_deg: f64_field("pitchDeg", 0.0),
        roll_deg: f64_field("rollDeg", 0.0),
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

/// Convert one saved piece into the game type (same rules as map loading).
/// The bridge uses this for live `add_piece` / `update_piece`.
pub(crate) fn piece_from_saved(saved: &SavedPiece) -> Result<Piece, LoadError> {
    validate_piece(saved.clone())
}
/// Boot mode. There is a single unified world (owned by the sim, backed by
/// SurrealDB), so booting never loads a file: the editor starts empty and the
/// world arrives over the net after login.
#[derive(Debug, Default, Resource, Clone)]
pub struct Boot {
    /// True when the app should open the Rust/Bevy glTF viewer instead of the
    /// level editor (`viewer-bevy.html?viewer`, or `--viewer` on native).
    pub viewer: bool,
    /// Headless GPU mode (native only): no window, offscreen render target.
    /// `None` boots the normal windowed client. See `crate::headless`.
    pub headless: Option<crate::headless::HeadlessConfig>,
}

/// The viewer is a separate Bevy path: `viewer-bevy.html?viewer` (or any query
/// string containing a `viewer` flag). Pure so it can be unit-tested.
pub fn is_viewer_query(query: &str) -> bool {
    query
        .strip_prefix('?')
        .unwrap_or(query)
        .split('&')
        .any(|pair| {
            let key = pair.split('=').next().unwrap_or("");
            key == "viewer" || key == "view"
        })
}

/// Boot the app. There is a single unified world (owned by the sim), so this
/// never loads a file: `viewer` selects the glTF model viewer, otherwise the
/// editor starts empty and the world arrives over the net after login.
#[cfg(target_arch = "wasm32")]
pub fn boot() -> Boot {
    let search = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default();
    Boot {
        viewer: is_viewer_query(&search),
        // A browser tab always has a window; headless GPU mode is native-only.
        headless: None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn boot() -> Boot {
    let viewer = std::env::args().any(|arg| arg == "--viewer" || arg == "--view");
    // `--headless` / `PARTY_DUNGEON_HEADLESS`: no window, offscreen GPU target.
    let headless = crate::headless::HeadlessConfig::from_env();
    Boot { viewer, headless }
}


#[cfg(target_arch = "wasm32")]
pub fn show_error(message: &str) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if let Some(loading) = document.get_element_by_id("loading") {
        let _ = loading.set_attribute("hidden", "");
    }
    if let Some(error) = document.get_element_by_id("error") {
        error.set_text_content(Some(message));
        let _ = error.set_attribute("style", "display:block");
    }
    if let Some(back) = document.get_element_by_id("back") {
        let _ = back.remove_attribute("hidden");
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub pieces: Vec<Piece>,
    pub outline: SceneOutline,
    pub character: Option<Character>,
    pub camera: Option<Orbit>,
    pub undo: Vec<LevelState>,
    pub redo: Vec<LevelState>,
    pub next_id: u32,
    pub platforms: Vec<SavedPlatform>,
    pub grass: std::collections::HashMap<(i32, i32), u8>,
    pub grass_slows: bool,
    pub vines: Vec<VineNode>,
}

/// Accept as many saved groups as possible: duplicate/zero/conflicting ids and
/// missing parents are dropped, and a parent cycle is broken rather than
/// refusing the whole map.
fn validate_groups(saved: Vec<SavedGroup>, pieces: &[Piece]) -> Vec<Group> {
    let piece_ids: HashSet<u32> = pieces.iter().map(|piece| piece.id).collect();
    let mut seen = HashSet::new();
    let mut groups: Vec<Group> = Vec::new();
    for group in saved {
        if group.id == 0 || piece_ids.contains(&group.id) || !seen.insert(group.id) {
            continue;
        }
        groups.push(Group {
            id: group.id,
            name: clean_label(&group.name),
            parent: group.parent,
            collapsed: group.collapsed,
        });
    }
    let ids: HashSet<u32> = groups.iter().map(|group| group.id).collect();
    for group in &mut groups {
        if group.parent.is_some_and(|parent| !ids.contains(&parent)) {
            group.parent = None;
        }
    }
    let parents: HashMap<u32, Option<u32>> = groups
        .iter()
        .map(|group| (group.id, group.parent))
        .collect();
    for group in &mut groups {
        let mut chain = HashSet::new();
        let mut current = group.parent;
        while let Some(id) = current {
            if !chain.insert(id) {
                group.parent = None;
                break;
            }
            current = parents.get(&id).copied().flatten();
        }
    }
    groups
}

fn validate_level(level: SavedLevel) -> Option<LevelState> {
    let mut outline = SceneOutline::default();
    let mut pieces = Vec::new();
    for saved in level.pieces {
        let name = saved
            .name
            .clone()
            .map(|name| clean_label(&name))
            .filter(|name| !name.is_empty());
        let group = saved.group;
        let locked = saved.locked;
        let piece = validate_piece(saved).ok()?;
        if let Some(name) = name {
            outline.labels.insert(piece.id, name);
        }
        if let Some(group) = group {
            outline.membership.insert(piece.id, group);
        }
        if locked {
            outline.locked.insert(piece.id);
        }
        pieces.push(piece);
    }
    outline.groups = validate_groups(level.groups, &pieces);
    let group_ids: HashSet<u32> = outline.groups.iter().map(|group| group.id).collect();
    outline
        .membership
        .retain(|_, group| group_ids.contains(group));
    outline.bump();
    Some(LevelState {
        pieces,
        outline,
        grass: validate_grass(&level.grass),
        vines: validate_vines(&level.vines),
    })
}

fn validate_platform(saved: &SavedPlatform) -> Option<SavedPlatform> {
    if saved.waypoints.is_empty() {
        return None;
    }
    for point in &saved.waypoints {
        if !point.x.is_finite() || !point.z.is_finite() {
            return None;
        }
        for handle in [point.incoming, point.outgoing].into_iter().flatten() {
            if !handle[0].is_finite() || !handle[1].is_finite() {
                return None;
            }
        }
    }
    if let Some(speed) = saved.speed
        && (!speed.is_finite() || speed <= 0.0)
    {
        return None;
    }
    Some(saved.clone())
}

fn validate_piece(saved: SavedPiece) -> Result<Piece, LoadError> {
    let shape_index = model::shape_index(&saved.shape)
        .ok_or_else(|| LoadError::UnknownShape(saved.shape.clone()))?;
    let shape_default = model::shapes()
        .get(shape_index)
        .map(|shape| shape.damage)
        .unwrap_or(0.0);
    for (value, name) in [
        (saved.x, "piece x"),
        (saved.z, "piece z"),
        (saved.elevation, "piece elevation"),
        (saved.rot_deg, "piece rotation"),
        (saved.pitch_deg, "piece pitch"),
        (saved.roll_deg, "piece roll"),
    ] {
        if !value.is_finite() {
            return Err(LoadError::NonFinite(name));
        }
    }
    if let Some(damage) = saved.damage
        && (!damage.is_finite() || damage < 0.0 || damage > 10000.0)
    {
        return Err(LoadError::NonFinite("piece damage"));
    }
    let damage = saved.damage.unwrap_or(shape_default);
    // Trigger parametric size: validate if present, else defaults.
    let mut trigger_size = model::TRIGGER_DEFAULT_SIZE;
    if let Some(w) = saved.trigger_width {
        if !w.is_finite() {
            return Err(LoadError::NonFinite("trigger width"));
        }
        trigger_size.x = w.max(model::TRIGGER_MIN_SIZE);
    }
    if let Some(l) = saved.trigger_length {
        if !l.is_finite() {
            return Err(LoadError::NonFinite("trigger length"));
        }
        trigger_size.z = l.max(model::TRIGGER_MIN_SIZE);
    }
    if let Some(h) = saved.trigger_height {
        if !h.is_finite() {
            return Err(LoadError::NonFinite("trigger height"));
        }
        trigger_size.y = h.max(model::TRIGGER_MIN_SIZE);
    }
    // Clamp non-finite handled above; also ensure overall finite.
    if !trigger_size.x.is_finite() || !trigger_size.y.is_finite() || !trigger_size.z.is_finite() {
        return Err(LoadError::NonFinite("trigger size"));
    }
    let trigger_shape_override = saved
        .trigger_shape
        .as_deref()
        .and_then(model::TriggerShape::parse)
        .unwrap_or(model::TriggerShape::Box);
    let is_glade = model::shapes()
        .get(shape_index)
        .is_some_and(|s| s.is_glade());
    let trigger_alpha = saved
        .trigger_alpha
        .map(|v| {
            if !v.is_finite() {
                return Err(LoadError::NonFinite("trigger alpha"));
            }
            Ok(if is_glade {
                v.clamp(0.08, 2.0)
            } else {
                model::sanitize_trigger_alpha(v)
            })
        })
        .transpose()?
        .unwrap_or(if is_glade {
            crate::glade::GLADE_DEFAULT_ROOF_PITCH
        } else {
            model::TRIGGER_DEFAULT_ALPHA
        });
    let trigger_color = saved
        .trigger_color
        .as_deref()
        .and_then(model::trigger_color_parse_hex)
        .unwrap_or(model::TRIGGER_DEFAULT_COLOR);
    let trigger_visible = saved
        .trigger_visible
        .unwrap_or(model::TRIGGER_DEFAULT_VISIBLE);
    // Unknown interaction ids are ignored (not an error) so maps authored
    // by newer versions keep loading; the piece is just not interactable.
    let interact = saved
        .interact
        .as_deref()
        .and_then(model::InteractAction::parse)
        .unwrap_or_default();

    Ok(Piece {
        id: saved.id,
        shape: shape_index,
        pos: Vec2::new(saved.x, saved.z),
        rot: saved.rot_deg.to_radians(),
        pitch: if is_glade {
            0.0
        } else {
            saved.pitch_deg.to_radians()
        },
        roll: if is_glade {
            0.0
        } else {
            saved.roll_deg.to_radians()
        },
        elevation: saved.elevation,
        dynamic: saved.dynamic,
        trigger: model::Trigger::damaging(damage),
        trigger_size,
        trigger_shape_override,
        // `SavedPiece.trigger_alpha` carries the glade **roof pitch** for glade
        // shapes (0.08..2.0, see `capture`/`from_piece`) and the raw opacity
        // for everything else; `Piece.trigger_alpha` is always the raw 0..1
        // value that `glade::roof_pitch` doubles again.
        trigger_alpha: if is_glade { trigger_alpha / 2.0 } else { trigger_alpha },
        trigger_color,
        trigger_visible,
        interact,
    })
}

fn validate_history(history: Vec<SavedLevel>, highest_id: &mut u32) -> Vec<LevelState> {
    let mut valid = Vec::new();
    for entry in history {
        let Some(state) = validate_level(entry) else {
            continue;
        };
        let highest = state
            .pieces
            .iter()
            .map(|piece| piece.id)
            .chain(state.outline.groups.iter().map(|group| group.id))
            .max()
            .unwrap_or(0);
        *highest_id = (*highest_id).max(highest);
        valid.push(state);
    }
    if valid.len() > MAX_HISTORY {
        valid.drain(..valid.len() - MAX_HISTORY);
    }
    valid
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_viewer_flag_is_read_from_the_query_string() {
        assert!(is_viewer_query("?viewer"));
        assert!(is_viewer_query("?viewer=1"));
        assert!(is_viewer_query("?session=abc&viewer"));
        assert!(is_viewer_query("?view=models"));
        assert!(!is_viewer_query("?session=abc"));
        assert!(!is_viewer_query(""));
    }

    #[test]
    fn demo_level_loads_successfully() {
        let text = include_str!("../../../levels/demo.json");
        let snapshot = parse_any_snapshot_text(text).expect("parses");
        let loaded = snapshot.validate().expect("validates");
        assert!(!loaded.pieces.is_empty());
    }

    fn piece() -> SavedPiece {
        SavedPiece {
            id: 3,
            shape: "block".into(),
            x: 1.5,
            z: -2.0,
            elevation: 0.5,
            dynamic: true,
            damage: None,
            trigger_width: None,
            trigger_length: None,
            trigger_height: None,
            trigger_shape: None,
            trigger_alpha: None,
            trigger_color: None,
            trigger_visible: None,
            name: None,
            group: None,
            locked: false,
            interact: None,
            rot_deg: 30.0,
            pitch_deg: -15.0,
            roll_deg: 10.0,
        }
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let snapshot = Snapshot {
            version: 1,
            next_id: Some(9),
            pieces: vec![piece()],
            character: Some(SavedCharacter {
                x: 1.0,
                z: 2.0,
                feet: 0.25,
                yaw_deg: 90.0,
                hp: Some(80.0),
                max_hp: Some(100.0),
            }),
            camera: Some(SavedCamera {
                target_x: 0.0,
                target_y: 1.0,
                target_z: 2.0,
                yaw: 0.85,
                pitch: 0.9,
                distance: 7.5,
                mode: "third".into(),
            }),
            groups: vec![],
            undo: vec![SavedLevel::default()],
            redo: vec![SavedLevel {
                pieces: vec![piece()],
                groups: vec![],
                grass: vec![SavedGrass {
                    q: 1,
                    r: 2,
                    density: 3,
                }],
                vines: vec![],
            }],
            platforms: vec![],
            grass: vec![SavedGrass {
                q: 1,
                r: 2,
                density: 3,
            }],
            grass_slows: true,
            vines: vec![],
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"rotDeg\":30.0"), "{json}");
        assert!(json.contains("\"pitchDeg\":-15.0"), "{json}");
        assert!(json.contains("\"rollDeg\":10.0"), "{json}");
        assert!(json.contains("\"dynamic\":true"), "{json}");
        assert!(json.contains("\"grassSlows\":true"), "{json}");
        assert_eq!(Snapshot::from_json(&json).unwrap(), snapshot);

        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
        assert_eq!(loaded.grass.get(&(1, 2)), Some(&3));
        assert!(loaded.grass_slows);
        let piece = loaded.pieces[0];
        assert!((piece.rot - 30.0_f32.to_radians()).abs() < 1e-5);
        assert!((piece.pitch - (-15.0_f32).to_radians()).abs() < 1e-5);
        assert!((piece.roll - 10.0_f32.to_radians()).abs() < 1e-5);
        assert!(piece.dynamic);
        let character = loaded.character.unwrap();
        assert!((character.health.current - 80.0).abs() < 1e-5);
        assert!((character.health.max - 100.0).abs() < 1e-5);
    }

    #[test]
    fn an_old_piece_without_pitch_or_roll_loads_upright() {
        let snapshot = Snapshot::from_json(
            r#"{"version":1,"pieces":[{"id":1,"shape":"block","x":0,"z":0,"elevation":0,"rotDeg":30}]}"#,
        )
        .unwrap();
        let loaded = snapshot.validate().unwrap();
        let piece = loaded.pieces[0];
        assert!((piece.rot - 30.0_f32.to_radians()).abs() < 1e-5);
        assert_eq!(piece.pitch, 0.0);
        assert_eq!(piece.roll, 0.0);
        assert!(!piece.dynamic);
    }

    #[test]
    fn a_minimal_snapshot_reads_with_defaults() {
        let snapshot = Snapshot::from_json(r#"{"version":1}"#).unwrap();
        assert_eq!(snapshot.version, 1);
        assert!(snapshot.pieces.is_empty());
        assert!(snapshot.character.is_none());
        assert!(snapshot.camera.is_none());
        assert!(snapshot.undo.is_empty());
        assert!(snapshot.redo.is_empty());
        assert!(snapshot.next_id.is_none());
    }

    #[test]
    fn malformed_camera_json_falls_back_to_no_camera() {
        let snapshot = Snapshot::from_json(
            r#"{
                "version": 1,
                "camera": {
                    "targetX": "bad",
                    "targetY": 1,
                    "targetZ": 2,
                    "yaw": 0.85,
                    "pitch": 0.9,
                    "distance": 7.5,
                    "mode": "third"
                }
            }"#,
        )
        .unwrap();

        assert!(snapshot.camera.is_none());
        assert!(snapshot.validate().unwrap().camera.is_none());
    }

    #[test]
    fn malformed_character_json_falls_back_to_no_character() {
        let snapshot =
            Snapshot::from_json(r#"{"version":1,"character":{"x":1,"z":2,"feet":0}}"#).unwrap();

        assert!(snapshot.character.is_none());
        assert!(snapshot.validate().unwrap().character.is_none());
    }

    #[test]
    fn an_old_character_without_health_loads_at_full() {
        let snapshot =
            Snapshot::from_json(r#"{"version":1,"character":{"x":1,"z":2,"feet":0,"yawDeg":45}}"#)
                .unwrap();
        let loaded = snapshot.validate().unwrap();
        let character = loaded.character.expect("old character loads");
        assert_eq!(character.health, Health::default());
    }

    #[test]
    fn a_character_with_bad_health_is_dropped() {
        let snapshot = Snapshot {
            version: 1,
            character: Some(SavedCharacter {
                x: 0.0,
                z: 0.0,
                feet: 0.0,
                yaw_deg: 0.0,
                hp: Some(f32::NAN),
                max_hp: Some(100.0),
            }),
            ..Default::default()
        };
        assert!(snapshot.validate().unwrap().character.is_none());
    }

    #[test]
    fn malformed_history_json_drops_only_the_bad_entry() {
        let snapshot = Snapshot::from_json(
            r#"{
                "version": 1,
                "undo": [
                    [{"id":2,"shape":"block","x":0,"z":0,"elevation":0,"rotDeg":0}],
                    [{"id":9,"shape":"block"}],
                    [{"id":3,"shape":"block","x":1,"z":0,"elevation":0,"rotDeg":0}]
                ]
            }"#,
        )
        .unwrap();

        let loaded = snapshot.validate().unwrap();
        assert_eq!(loaded.undo.len(), 2);
        assert_eq!(loaded.undo[0].pieces[0].id, 2);
        assert_eq!(loaded.undo[1].pieces[0].id, 3);
    }

    fn saved(id: u32, shape: &str, x: f32, z: f32) -> SavedPiece {
        SavedPiece {
            id,
            shape: shape.into(),
            x,
            z,
            elevation: 0.0,
            dynamic: false,
            damage: None,
            trigger_width: None,
            trigger_length: None,
            trigger_height: None,
            trigger_shape: None,
            trigger_alpha: None,
            trigger_color: None,
            trigger_visible: None,
            name: None,
            group: None,
            locked: false,
            interact: None,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        }
    }

    fn camera() -> SavedCamera {
        SavedCamera {
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            yaw: 0.85,
            pitch: 0.9,
            distance: 7.5,
            mode: "third".into(),
        }
    }

    #[test]
    fn a_minimal_snapshot_validates_to_an_empty_map() {
        let loaded = Snapshot {
            version: 1,
            ..Default::default()
        }
        .validate()
        .unwrap();
        assert!(loaded.pieces.is_empty());
        assert!(loaded.character.is_none());
        assert!(loaded.camera.is_none());
        assert_eq!(loaded.next_id, 1);
    }

    #[test]
    fn an_unknown_shape_refuses_the_map() {
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![saved(1, "cloud", 0.0, 0.0)],
            ..Default::default()
        };
        assert_eq!(
            snapshot.validate().unwrap_err(),
            LoadError::UnknownShape("cloud".into())
        );
    }

    #[test]
    fn a_non_finite_piece_refuses_the_map() {
        let mut piece = saved(1, "block", 0.0, 0.0);
        piece.x = f32::NAN;
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![piece],
            ..Default::default()
        };
        assert_eq!(
            snapshot.validate().unwrap_err(),
            LoadError::NonFinite("piece x")
        );

        let mut piece = saved(1, "block", 0.0, 0.0);
        piece.pitch_deg = f32::INFINITY;
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![piece],
            ..Default::default()
        };
        assert_eq!(
            snapshot.validate().unwrap_err(),
            LoadError::NonFinite("piece pitch")
        );
    }

    #[test]
    fn a_wrong_version_refuses_the_map() {
        let zero = Snapshot {
            version: 0,
            ..Default::default()
        };
        assert_eq!(zero.validate().unwrap_err(), LoadError::MissingVersion);
        let two = Snapshot {
            version: 2,
            ..Default::default()
        };
        assert_eq!(
            two.validate().unwrap_err(),
            LoadError::UnsupportedVersion(2)
        );
    }

    #[test]
    fn bad_camera_and_character_fall_back_to_defaults() {
        let snapshot = Snapshot {
            version: 1,
            camera: Some(SavedCamera {
                target_x: f32::NAN,
                ..camera()
            }),
            character: Some(SavedCharacter {
                x: f32::INFINITY,
                z: 0.0,
                feet: 0.0,
                yaw_deg: 0.0,
                hp: None,
                max_hp: None,
            }),
            ..Default::default()
        };
        let loaded = snapshot.validate().unwrap();
        assert!(loaded.camera.is_none());
        assert!(loaded.character.is_none());
    }

    #[test]
    fn invalid_history_entries_are_dropped_individually() {
        let mut bad = saved(9, "block", 0.0, 0.0);
        bad.x = f32::NAN;
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![saved(1, "block", 0.0, 0.0)],
            undo: vec![
                SavedLevel {
                    pieces: vec![saved(2, "block", 0.0, 0.0)],
                    ..Default::default()
                },
                SavedLevel {
                    pieces: vec![bad],
                    ..Default::default()
                },
            ],
            redo: vec![SavedLevel {
                pieces: vec![saved(3, "block", 0.0, 0.0)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let loaded = snapshot.validate().unwrap();
        assert_eq!(loaded.undo.len(), 1);
        assert_eq!(loaded.undo[0].pieces[0].id, 2);
        assert_eq!(loaded.redo.len(), 1);
    }

    #[test]
    fn next_id_covers_ids_that_only_exist_in_history() {
        let snapshot = Snapshot {
            version: 1,
            next_id: Some(5),
            pieces: vec![saved(2, "block", 0.0, 0.0)],
            undo: vec![SavedLevel {
                pieces: vec![saved(7, "block", 0.0, 0.0)],
                ..Default::default()
            }],
            redo: vec![SavedLevel {
                pieces: vec![saved(9, "block", 0.0, 0.0)],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(snapshot.validate().unwrap().next_id, 10);
    }

    #[test]
    fn history_is_trimmed_to_the_newest_sixty() {
        let undo = (0..70)
            .map(|i| SavedLevel {
                pieces: vec![saved(i + 1, "block", 0.0, 0.0)],
                ..Default::default()
            })
            .collect();
        let loaded = Snapshot {
            version: 1,
            undo,
            ..Default::default()
        }
        .validate()
        .unwrap();
        assert_eq!(loaded.undo.len(), 60);
        assert_eq!(loaded.undo[0].pieces[0].id, 11);
        assert_eq!(loaded.undo[59].pieces[0].id, 70);
    }
    #[test]
    fn per_piece_damage_round_trips_through_json() {
        let mut damaged = saved(1, "block", 0.0, 0.0);
        damaged.damage = Some(7.5);
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![damaged],
            ..Default::default()
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"damage\":7.5"), "{json}");
        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
        assert!((loaded.pieces[0].trigger.damage - 7.5).abs() < 1e-5);
        // Saving again omits nothing: a real edit persists.
        let resaved = SavedPiece::from(&loaded.pieces[0]);
        assert_eq!(resaved.damage, Some(7.5));
    }

    #[test]
    fn unedited_pieces_omit_damage_and_reload_with_the_catalogue_default() {
        let block = SavedPiece {
            damage: None,
            ..saved(1, "block", 0.0, 0.0)
        };
        let json = Snapshot {
            version: 1,
            pieces: vec![block],
            ..Default::default()
        }
        .to_json();
        assert!(!json.contains("damage"), "{json}");
        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
        assert_eq!(loaded.pieces[0].trigger.damage, 0.0);

        // Old saves without a damage key: spikes hurt, blocks do not.
        let old_spikes = Snapshot::from_json(
            r#"{"version":1,"pieces":[{"id":1,"shape":"spikes","x":0,"z":0,"elevation":0,"rotDeg":0}]}"#,
        )
        .unwrap()
        .validate()
        .unwrap();
        assert!((old_spikes.pieces[0].trigger.damage - 10.0).abs() < 1e-5);
    }

    #[test]
    fn interact_round_trips_through_json() {
        let mut piece = saved(1, "block", 0.0, 0.0);
        piece.interact = Some("togglePause".into());
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![piece],
            ..Default::default()
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"interact\":\"togglePause\""), "{json}");
        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
        assert_eq!(
            loaded.pieces[0].interact,
            model::InteractAction::TogglePause
        );
        // Saving again omits nothing: a real edit persists.
        let resaved = SavedPiece::from(&loaded.pieces[0]);
        assert_eq!(resaved.interact.as_deref(), Some("togglePause"));
    }

    #[test]
    fn unedited_pieces_omit_interact_and_old_saves_load_without_it() {
        let block = SavedPiece {
            interact: None,
            ..saved(1, "block", 0.0, 0.0)
        };
        let json = Snapshot {
            version: 1,
            pieces: vec![block],
            ..Default::default()
        }
        .to_json();
        assert!(!json.contains("interact"), "{json}");
        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
        assert!(loaded.pieces[0].interact.is_none());

        // Unknown future actions are ignored, not fatal.
        let future = Snapshot::from_json(
            r#"{"version":1,"pieces":[{"id":1,"shape":"block","x":0,"z":0,"elevation":0,"rotDeg":0,"interact":"teleport"}]}"#,
        )
        .unwrap()
        .validate()
        .unwrap();
        assert!(future.pieces[0].interact.is_none());
    }

    #[test]
    fn labels_groups_and_membership_round_trip() {
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![
                SavedPiece {
                    name: Some("North wall".into()),
                    group: Some(10),
                    locked: true,
                    interact: None,
                    ..saved(1, "wall", 0.0, 0.0)
                },
                SavedPiece {
                    group: Some(11),
                    ..saved(2, "crate", 1.0, 0.0)
                },
            ],
            groups: vec![
                SavedGroup {
                    id: 10,
                    name: "Rooms".into(),
                    parent: None,
                    collapsed: false,
                },
                SavedGroup {
                    id: 11,
                    name: "Props".into(),
                    parent: Some(10),
                    collapsed: true,
                },
            ],
            ..Default::default()
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"name\":\"North wall\""), "{json}");
        assert!(json.contains("\"groups\""), "{json}");
        assert!(json.contains("\"locked\":true"), "{json}");

        let loaded = snapshot.validate().unwrap();
        assert!(loaded.outline.is_locked(1));
        assert!(!loaded.outline.is_locked(2));
        assert_eq!(
            loaded.outline.labels.get(&1).map(String::as_str),
            Some("North wall")
        );
        assert_eq!(loaded.outline.piece_parent(1), Some(10));
        assert_eq!(loaded.outline.piece_parent(2), Some(11));
        assert_eq!(loaded.outline.group(11).unwrap().parent, Some(10));
        assert!(loaded.outline.group(11).unwrap().collapsed);
        assert_eq!(loaded.next_id, 12);
    }

    #[test]
    fn broken_group_links_are_repaired_instead_of_refusing_the_map() {
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![SavedPiece {
                group: Some(99),
                ..saved(1, "block", 0.0, 0.0)
            }],
            groups: vec![
                SavedGroup {
                    id: 20,
                    name: "A".into(),
                    parent: Some(21),
                    collapsed: false,
                },
                SavedGroup {
                    id: 21,
                    name: "B".into(),
                    parent: Some(20),
                    collapsed: false,
                },
                SavedGroup {
                    id: 22,
                    name: "C".into(),
                    parent: Some(999),
                    collapsed: false,
                },
                SavedGroup {
                    id: 1,
                    name: "collides with a piece".into(),
                    parent: None,
                    collapsed: false,
                },
            ],
            ..Default::default()
        };
        let loaded = snapshot.validate().unwrap();
        assert_eq!(loaded.outline.piece_parent(1), None);
        assert!(loaded.outline.group(20).is_some());
        assert!(loaded.outline.group(21).is_some());
        assert_eq!(loaded.outline.group(20).unwrap().parent, None);
        assert_eq!(loaded.outline.group(21).unwrap().parent, None);
        assert_eq!(loaded.outline.group(22).unwrap().parent, None);
        // A group id that collides with a piece id is dropped, not merged.
        assert!(loaded.outline.group(1).is_none());
    }

    #[test]
    fn old_array_history_still_loads_with_labels_and_groups() {
        let snapshot = Snapshot::from_json(
            r#"{
                "version": 1,
                "groups": [{"id": 4, "name": "Old room"}],
                "undo": [
                    [{"id": 1, "shape": "block", "x": 0, "z": 0, "elevation": 0, "rotDeg": 0}]
                ]
            }"#,
        )
        .unwrap();
        let loaded = snapshot.validate().unwrap();
        assert_eq!(loaded.undo.len(), 1);
        assert_eq!(loaded.undo[0].pieces.len(), 1);
        assert!(loaded.undo[0].outline.groups.is_empty());
        assert_eq!(loaded.outline.group(4).unwrap().name, "Old room");
    }

    #[test]
    fn bad_damage_refuses_the_map() {
        let mut bad = saved(1, "block", 0.0, 0.0);
        bad.damage = Some(-5.0);
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![bad],
            ..Default::default()
        };
        assert_eq!(
            snapshot.validate().unwrap_err(),
            LoadError::NonFinite("piece damage")
        );
        let mut nan = saved(1, "block", 0.0, 0.0);
        nan.damage = Some(f32::NAN);
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![nan],
            ..Default::default()
        };
        assert_eq!(
            snapshot.validate().unwrap_err(),
            LoadError::NonFinite("piece damage")
        );
    }
}
