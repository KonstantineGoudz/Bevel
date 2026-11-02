//! Saved-map snapshots: the versioned JSON shape kept in browser storage.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use bevy::math::{Vec2, Vec3};
use bevy::prelude::Resource;

use crate::{
    editor::{
        Character, LevelState, Orbit, ViewMode,
        outline::{Group, SceneOutline, clean_label},
    },
    model::{self, Piece},
    statuses::Health,
};

pub const SNAPSHOT_VERSION: u32 = 1;
pub const MAX_HISTORY: usize = 60;
pub(crate) const INDEX_KEY: &str = "party-dungeon.index.v1";
pub(crate) const SESSION_PREFIX: &str = "party-dungeon.session.";
pub(crate) const SOFT_SAVE_INTERVAL: f64 = 1.0;

#[cfg(target_arch = "wasm32")]
pub(crate) fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now_ms() -> f64 {
    0.0
}

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
    /// User label. Missing means "use the shape name".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Owning group id. Missing means the scene root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u32>,
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

/// One undo step. The object shape (rather than a bare array) lets groups and
/// labels travel with the pieces they describe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLevel {
    #[serde(default)]
    pub pieces: Vec<SavedPiece>,
    #[serde(default)]
    pub groups: Vec<SavedGroup>,
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
        Self {
            pieces: state
                .pieces
                .iter()
                .map(|piece| SavedPiece::from_piece(piece, &state.outline))
                .collect(),
            groups: state.outline.groups.iter().map(SavedGroup::from).collect(),
        }
    }
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
            name: None,
            group: None,
        }
    }
}

impl SavedPiece {
    /// Serialize a piece together with the outliner metadata for it.
    pub fn from_piece(piece: &Piece, outline: &SceneOutline) -> Self {
        Self {
            name: outline.labels.get(&piece.id).cloned(),
            group: outline.membership.get(&piece.id).copied(),
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
            let piece = validate_piece(saved)?;
            if let Some(name) = name {
                outline.labels.insert(piece.id, name);
            }
            if let Some(group) = group {
                outline.membership.insert(piece.id, group);
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
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageError {
    QuotaExceeded,
    Unavailable,
    IndexMissing,
}

pub(crate) trait Store {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str) -> Result<(), StorageError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Meta {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: f64,
    pub(crate) updated_at: f64,
    pub(crate) piece_count: usize,
}

pub(crate) fn session_key(id: &str) -> String {
    format!("{SESSION_PREFIX}{id}.v1")
}

pub fn parse_session_id(query: &str) -> Option<String> {
    query
        .strip_prefix('?')
        .unwrap_or(query)
        .split('&')
        .find_map(|pair| pair.strip_prefix("session="))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Default, Resource, Clone)]
pub struct Boot {
    pub id: Option<String>,
    pub loaded: Option<Loaded>,
    /// True when the app should open the Rust/Bevy glTF viewer instead of the
    /// level editor (`viewer-bevy.html?viewer`, or `--viewer` on native).
    pub viewer: bool,
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

#[cfg(target_arch = "wasm32")]
pub fn boot() -> Result<Boot, String> {
    let window =
        web_sys::window().ok_or_else(|| "Could not access the browser window.".to_owned())?;
    let search = window
        .location()
        .search()
        .map_err(|_| "Could not read the browser URL.".to_owned())?;
    if is_viewer_query(&search) {
        return Ok(Boot {
            viewer: true,
            ..Default::default()
        });
    }
    let id = parse_session_id(&search)
        .ok_or_else(|| "Choose a map from the maps page before opening the editor.".to_owned())?;
    let store = LocalStore::new()
        .map_err(|_| "Browser storage is unavailable; the map cannot be opened.".to_owned())?;
    if !read_index(&store).iter().any(|meta| meta.id == id) {
        return Err("This map is not in your saved maps list.".to_owned());
    }
    let loaded = read_session(&store, &id)
        .and_then(Snapshot::validate)
        .map_err(load_error_message)?;

    Ok(Boot {
        id: Some(id),
        loaded: Some(loaded),
        viewer: false,
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn boot() -> Result<Boot, String> {
    let viewer = std::env::args().any(|arg| arg == "--viewer" || arg == "--view");
    Ok(Boot {
        viewer,
        ..Default::default()
    })
}

fn load_error_message(error: LoadError) -> String {
    match error {
        LoadError::MissingVersion => "The saved map is missing its version.".to_owned(),
        LoadError::UnsupportedVersion(version) => {
            format!("The saved map uses unsupported version {version}.")
        }
        LoadError::UnknownShape(shape) => {
            format!("The saved map contains an unknown shape: {shape}.")
        }
        LoadError::NonFinite(_) => "The saved map contains invalid geometry.".to_owned(),
        LoadError::Parse(details) => {
            bevy::log::warn!("Could not read saved map: {details}");
            "The saved map could not be read.".to_owned()
        }
        LoadError::NotFound => "The saved map state is missing from browser storage.".to_owned(),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn show_error(message: &str) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if let Some(error) = document.get_element_by_id("error") {
        error.set_text_content(Some(message));
        let _ = error.set_attribute("style", "display:block");
    }
    if let Some(back) = document.get_element_by_id("back") {
        let _ = back.remove_attribute("hidden");
    }
}

#[cfg(test)]
pub(crate) fn encode_index(metas: &[Meta]) -> String {
    serde_json::to_string(metas).expect("session metadata serializes to JSON")
}

pub(crate) fn parse_index(text: &str) -> Vec<Meta> {
    serde_json::from_str::<Vec<serde_json::Value>>(text)
        .map(|entries| {
            entries
                .into_iter()
                .filter_map(|entry| serde_json::from_value(entry).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn read_index(store: &dyn Store) -> Vec<Meta> {
    store
        .get(INDEX_KEY)
        .map_or_else(Vec::new, |json| parse_index(&json))
}

#[cfg(test)]
pub(crate) fn write_index(store: &mut dyn Store, metas: &[Meta]) -> Result<(), StorageError> {
    store.set(INDEX_KEY, &encode_index(metas))
}

pub(crate) fn read_session(store: &dyn Store, id: &str) -> Result<Snapshot, LoadError> {
    let json = store.get(&session_key(id)).ok_or(LoadError::NotFound)?;
    Snapshot::from_json(&json)
}

pub(crate) fn write_session(
    store: &mut dyn Store,
    id: &str,
    snapshot: &Snapshot,
) -> Result<(), StorageError> {
    store.set(&session_key(id), &snapshot.to_json())
}

pub(crate) fn save_session(
    store: &mut dyn Store,
    id: &str,
    snapshot: &Snapshot,
    now_ms: f64,
) -> Result<(), StorageError> {
    write_session(store, id, snapshot)?;
    let raw = store.get(INDEX_KEY).ok_or(StorageError::IndexMissing)?;
    let mut entries: Vec<serde_json::Value> =
        serde_json::from_str(&raw).map_err(|_| StorageError::IndexMissing)?;
    let meta = entries
        .iter_mut()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(id))
        .ok_or(StorageError::IndexMissing)?;
    meta["updatedAt"] = serde_json::json!(now_ms);
    meta["pieceCount"] = serde_json::json!(snapshot.pieces.len());
    let json = serde_json::to_string(&entries).expect("raw session metadata serializes to JSON");
    store.set(INDEX_KEY, &json)
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct LocalStore {
    storage: web_sys::Storage,
}

#[cfg(target_arch = "wasm32")]
impl LocalStore {
    pub(crate) fn new() -> Result<Self, StorageError> {
        let storage = web_sys::window()
            .ok_or(StorageError::Unavailable)?
            .local_storage()
            .map_err(|_| StorageError::Unavailable)?
            .ok_or(StorageError::Unavailable)?;
        Ok(Self { storage })
    }
}

#[cfg(target_arch = "wasm32")]
impl Store for LocalStore {
    fn get(&self, key: &str) -> Option<String> {
        self.storage.get_item(key).ok().flatten()
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
        use wasm_bindgen::JsCast;

        self.storage.set_item(key, value).map_err(|error| {
            let name = error
                .dyn_ref::<web_sys::DomException>()
                .map(web_sys::DomException::name);
            match name.as_deref() {
                Some("QuotaExceededError" | "NS_ERROR_DOM_QUOTA_REACHED") => {
                    StorageError::QuotaExceeded
                }
                _ => StorageError::Unavailable,
            }
        })
    }
}

#[cfg(test)]
struct MemoryStore {
    entries: std::collections::HashMap<String, String>,
    quota: Option<usize>,
}

#[cfg(test)]
impl MemoryStore {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            quota: None,
        }
    }

    fn with_quota(quota: usize) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            quota: Some(quota),
        }
    }
}

#[cfg(test)]
impl Store for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
        if let Some(quota) = self.quota {
            let size = self
                .entries
                .iter()
                .filter(|(stored_key, _)| stored_key.as_str() != key)
                .map(|(stored_key, stored_value)| stored_key.len() + stored_value.len())
                .sum::<usize>()
                + key.len()
                + value.len();
            if size > quota {
                return Err(StorageError::QuotaExceeded);
            }
        }
        self.entries.insert(key.to_owned(), value.to_owned());
        Ok(())
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
        let piece = validate_piece(saved).ok()?;
        if let Some(name) = name {
            outline.labels.insert(piece.id, name);
        }
        if let Some(group) = group {
            outline.membership.insert(piece.id, group);
        }
        pieces.push(piece);
    }
    outline.groups = validate_groups(level.groups, &pieces);
    let group_ids: HashSet<u32> = outline.groups.iter().map(|group| group.id).collect();
    outline
        .membership
        .retain(|_, group| group_ids.contains(group));
    outline.bump();
    Some(LevelState { pieces, outline })
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

    Ok(Piece {
        id: saved.id,
        shape: shape_index,
        pos: Vec2::new(saved.x, saved.z),
        rot: saved.rot_deg.to_radians(),
        pitch: saved.pitch_deg.to_radians(),
        roll: saved.roll_deg.to_radians(),
        elevation: saved.elevation,
        dynamic: saved.dynamic,
        trigger: model::Trigger::damaging(damage),
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
    fn the_session_id_is_read_from_the_query_string() {
        assert_eq!(parse_session_id("?session=abc-123"), Some("abc-123".into()));
        assert_eq!(
            parse_session_id("?x=1&session=abc-123&y=2"),
            Some("abc-123".into())
        );
        assert_eq!(parse_session_id("?session="), None);
        assert_eq!(parse_session_id(""), None);
        assert_eq!(parse_session_id("?Session=abc"), None);
    }

    #[test]
    fn the_viewer_flag_is_read_from_the_query_string() {
        assert!(is_viewer_query("?viewer"));
        assert!(is_viewer_query("?viewer=1"));
        assert!(is_viewer_query("?session=abc&viewer"));
        assert!(is_viewer_query("?view=models"));
        assert!(!is_viewer_query("?session=abc"));
        assert!(!is_viewer_query(""));
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
            name: None,
            group: None,
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
            }],
            platforms: vec![],
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"rotDeg\":30.0"), "{json}");
        assert!(json.contains("\"pitchDeg\":-15.0"), "{json}");
        assert!(json.contains("\"rollDeg\":10.0"), "{json}");
        assert!(json.contains("\"dynamic\":true"), "{json}");
        assert_eq!(Snapshot::from_json(&json).unwrap(), snapshot);

        let loaded = Snapshot::from_json(&json).unwrap().validate().unwrap();
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
            name: None,
            group: None,
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
                    groups: vec![],
                },
                SavedLevel {
                    pieces: vec![bad],
                    groups: vec![],
                },
            ],
            redo: vec![SavedLevel {
                pieces: vec![saved(3, "block", 0.0, 0.0)],
                groups: vec![],
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
                groups: vec![],
            }],
            redo: vec![SavedLevel {
                pieces: vec![saved(9, "block", 0.0, 0.0)],
                groups: vec![],
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
                groups: vec![],
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
    fn a_session_key_is_prefixed_and_versioned() {
        assert_eq!(session_key("abc"), "party-dungeon.session.abc.v1");
    }

    #[test]
    fn index_json_round_trips_and_skips_broken_entries() {
        let metas = vec![Meta {
            id: "a".into(),
            name: "Alpha".into(),
            created_at: 1.0,
            updated_at: 2.0,
            piece_count: 3,
        }];
        assert_eq!(parse_index(&encode_index(&metas)), metas);
        let mixed = r#"[
            {"id":"a","name":"Alpha","createdAt":1,"updatedAt":2,"pieceCount":3},
            {"name":"no id"}
        ]"#;
        assert_eq!(parse_index(mixed).len(), 1);
    }

    #[test]
    fn saving_a_session_updates_its_index_entry() {
        let mut store = MemoryStore::new();
        let meta = Meta {
            id: "a".into(),
            name: "Alpha".into(),
            created_at: 1.0,
            updated_at: 2.0,
            piece_count: 0,
        };
        write_index(&mut store, &[meta]).unwrap();
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![saved(1, "block", 0.0, 0.0), saved(2, "block", 1.0, 0.0)],
            ..Default::default()
        };
        save_session(&mut store, "a", &snapshot, 42.0).unwrap();
        let metas = read_index(&store);
        assert_eq!(metas[0].piece_count, 2);
        assert_eq!(metas[0].updated_at, 42.0);
        assert_eq!(read_session(&store, "a").unwrap(), snapshot);
    }

    #[test]
    fn saving_a_session_preserves_malformed_sibling_index_entries() {
        let mut store = MemoryStore::new();
        let sibling = serde_json::json!({"id": "broken", "name": "Missing fields"});
        let raw = serde_json::json!([
            {
                "id": "a",
                "name": "Alpha",
                "createdAt": 1,
                "updatedAt": 2,
                "pieceCount": 0
            },
            sibling.clone()
        ])
        .to_string();
        store.set(INDEX_KEY, &raw).unwrap();
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![saved(1, "block", 0.0, 0.0)],
            ..Default::default()
        };

        save_session(&mut store, "a", &snapshot, 42.0).unwrap();

        let entries: Vec<serde_json::Value> =
            serde_json::from_str(&store.get(INDEX_KEY).unwrap()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["updatedAt"], 42.0);
        assert_eq!(entries[0]["pieceCount"], 1);
        assert_eq!(entries[1], sibling);
    }

    #[test]
    fn saving_a_session_with_a_missing_index_entry_returns_an_error() {
        let mut store = MemoryStore::new();
        let raw = serde_json::json!([{
            "id": "a",
            "name": "Alpha",
            "createdAt": 1,
            "updatedAt": 2,
            "pieceCount": 0
        }])
        .to_string();
        store.set(INDEX_KEY, &raw).unwrap();
        let snapshot = Snapshot {
            version: 1,
            ..Default::default()
        };

        assert_eq!(
            save_session(&mut store, "b", &snapshot, 42.0),
            Err(StorageError::IndexMissing)
        );
        assert_eq!(store.get(INDEX_KEY).unwrap(), raw);
        assert_eq!(read_session(&store, "b").unwrap(), snapshot);
    }

    #[test]
    fn saving_a_session_does_not_rewrite_an_invalid_raw_index() {
        let mut store = MemoryStore::new();
        let raw = r#"{"id":"a"}"#;
        store.set(INDEX_KEY, raw).unwrap();
        let snapshot = Snapshot {
            version: 1,
            ..Default::default()
        };

        assert_eq!(
            save_session(&mut store, "a", &snapshot, 42.0),
            Err(StorageError::IndexMissing)
        );
        assert_eq!(store.get(INDEX_KEY).unwrap(), raw);
        assert_eq!(read_session(&store, "a").unwrap(), snapshot);
    }

    #[test]
    fn a_full_store_keeps_the_previous_snapshot() {
        let mut store = MemoryStore::with_quota(400);
        let old = Snapshot {
            version: 1,
            pieces: vec![saved(1, "block", 0.0, 0.0)],
            ..Default::default()
        };
        write_session(&mut store, "a", &old).unwrap();
        let big = Snapshot {
            version: 1,
            pieces: (0..20).map(|i| saved(i, "block", i as f32, 0.0)).collect(),
            ..Default::default()
        };
        assert_eq!(
            write_session(&mut store, "a", &big).unwrap_err(),
            StorageError::QuotaExceeded
        );
        assert_eq!(read_session(&store, "a").unwrap(), old);
    }

    #[test]
    fn a_missing_session_is_not_found() {
        let store = MemoryStore::new();
        assert_eq!(
            read_session(&store, "nope").unwrap_err(),
            LoadError::NotFound
        );
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
    fn labels_groups_and_membership_round_trip() {
        let snapshot = Snapshot {
            version: 1,
            pieces: vec![
                SavedPiece {
                    name: Some("North wall".into()),
                    group: Some(10),
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

        let loaded = snapshot.validate().unwrap();
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
