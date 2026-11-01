//! Saved-map snapshots: the versioned JSON shape kept in browser storage.

use serde::{Deserialize, Serialize};

use bevy::math::{Vec2, Vec3};
use bevy::prelude::Resource;

use crate::{
    editor::{Character, Orbit, ViewMode},
    model::{self, Piece},
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
    #[serde(default)]
    pub character: Option<SavedCharacter>,
    #[serde(default)]
    pub camera: Option<SavedCamera>,
    #[serde(default)]
    pub undo: Vec<Vec<SavedPiece>>,
    #[serde(default)]
    pub redo: Vec<Vec<SavedPiece>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPiece {
    pub id: u32,
    pub shape: String,
    pub x: f32,
    pub z: f32,
    pub elevation: f32,
    pub rot_deg: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCharacter {
    pub x: f32,
    pub z: f32,
    #[serde(default)]
    pub feet: f32,
    pub yaw_deg: f32,
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

impl From<&Piece> for SavedPiece {
    fn from(piece: &Piece) -> Self {
        Self {
            id: piece.id,
            shape: model::shapes()[piece.shape].id.to_owned(),
            x: piece.pos.x,
            z: piece.pos.y,
            elevation: piece.elevation,
            rot_deg: piece.rot.to_degrees(),
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

        let pieces = self
            .pieces
            .into_iter()
            .map(validate_piece)
            .collect::<Result<Vec<_>, _>>()?;
        let mut highest_id = pieces.iter().map(|piece| piece.id).max().unwrap_or(0);

        let undo = validate_history(self.undo, &mut highest_id);
        let redo = validate_history(self.redo, &mut highest_id);

        let character = self.character.and_then(|saved| {
            [saved.x, saved.z, saved.feet, saved.yaw_deg]
                .iter()
                .all(|value| value.is_finite())
                .then_some(Character {
                    pos: Vec2::new(saved.x, saved.z),
                    feet: saved.feet,
                    yaw: saved.yaw_deg.to_radians(),
                    vy: 0.0,
                    grounded: false,
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

        Ok(Loaded {
            pieces,
            character,
            camera,
            undo,
            redo,
            next_id,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageError {
    QuotaExceeded,
    Unavailable,
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
}

#[cfg(target_arch = "wasm32")]
pub fn boot() -> Result<Boot, String> {
    let window =
        web_sys::window().ok_or_else(|| "Could not access the browser window.".to_owned())?;
    let search = window
        .location()
        .search()
        .map_err(|_| "Could not read the browser URL.".to_owned())?;
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
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn boot() -> Result<Boot, String> {
    Ok(Boot::default())
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
    let mut metas = read_index(store);
    if let Some(meta) = metas.iter_mut().find(|meta| meta.id == id) {
        meta.updated_at = now_ms;
        meta.piece_count = snapshot.pieces.len();
    }
    write_index(store, &metas)
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
    pub character: Option<Character>,
    pub camera: Option<Orbit>,
    pub undo: Vec<Vec<Piece>>,
    pub redo: Vec<Vec<Piece>>,
    pub next_id: u32,
}

fn validate_piece(saved: SavedPiece) -> Result<Piece, LoadError> {
    let shape = model::shape_index(&saved.shape)
        .ok_or_else(|| LoadError::UnknownShape(saved.shape.clone()))?;
    for (value, name) in [
        (saved.x, "piece x"),
        (saved.z, "piece z"),
        (saved.elevation, "piece elevation"),
        (saved.rot_deg, "piece rotation"),
    ] {
        if !value.is_finite() {
            return Err(LoadError::NonFinite(name));
        }
    }

    Ok(Piece {
        id: saved.id,
        shape,
        pos: Vec2::new(saved.x, saved.z),
        rot: saved.rot_deg.to_radians(),
        elevation: saved.elevation,
    })
}

fn validate_history(history: Vec<Vec<SavedPiece>>, highest_id: &mut u32) -> Vec<Vec<Piece>> {
    let mut valid = Vec::new();
    for entry in history {
        let pieces = entry
            .into_iter()
            .map(validate_piece)
            .collect::<Result<Vec<_>, _>>();
        if let Ok(pieces) = pieces {
            *highest_id = (*highest_id).max(pieces.iter().map(|piece| piece.id).max().unwrap_or(0));
            valid.push(pieces);
        }
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

    fn piece() -> SavedPiece {
        SavedPiece {
            id: 3,
            shape: "block".into(),
            x: 1.5,
            z: -2.0,
            elevation: 0.5,
            rot_deg: 30.0,
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
            undo: vec![vec![]],
            redo: vec![vec![piece()]],
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"rotDeg\":30.0"), "{json}");
        assert_eq!(Snapshot::from_json(&json).unwrap(), snapshot);
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

    fn saved(id: u32, shape: &str, x: f32, z: f32) -> SavedPiece {
        SavedPiece {
            id,
            shape: shape.into(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
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
            undo: vec![vec![saved(2, "block", 0.0, 0.0)], vec![bad]],
            redo: vec![vec![saved(3, "block", 0.0, 0.0)]],
            ..Default::default()
        };
        let loaded = snapshot.validate().unwrap();
        assert_eq!(loaded.undo.len(), 1);
        assert_eq!(loaded.undo[0][0].id, 2);
        assert_eq!(loaded.redo.len(), 1);
    }

    #[test]
    fn next_id_covers_ids_that_only_exist_in_history() {
        let snapshot = Snapshot {
            version: 1,
            next_id: Some(5),
            pieces: vec![saved(2, "block", 0.0, 0.0)],
            undo: vec![vec![saved(7, "block", 0.0, 0.0)]],
            redo: vec![vec![saved(9, "block", 0.0, 0.0)]],
            ..Default::default()
        };
        assert_eq!(snapshot.validate().unwrap().next_id, 10);
    }

    #[test]
    fn history_is_trimmed_to_the_newest_sixty() {
        let undo = (0..70)
            .map(|i| vec![saved(i + 1, "block", 0.0, 0.0)])
            .collect();
        let loaded = Snapshot {
            version: 1,
            undo,
            ..Default::default()
        }
        .validate()
        .unwrap();
        assert_eq!(loaded.undo.len(), 60);
        assert_eq!(loaded.undo[0][0].id, 11);
        assert_eq!(loaded.undo[59][0].id, 70);
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
}
