//! Saved-map snapshots: the versioned JSON shape kept in browser storage.

use serde::{Deserialize, Serialize};

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

impl Snapshot {
    pub fn from_json(json: &str) -> Result<Snapshot, LoadError> {
        serde_json::from_str(json).map_err(|error| LoadError::Parse(error.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("snapshot serializes to JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece() -> SavedPiece {
        SavedPiece { id: 3, shape: "block".into(), x: 1.5, z: -2.0,
            elevation: 0.5, rot_deg: 30.0 }
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let snapshot = Snapshot {
            version: 1,
            next_id: Some(9),
            pieces: vec![piece()],
            character: Some(SavedCharacter { x: 1.0, z: 2.0, feet: 0.25, yaw_deg: 90.0 }),
            camera: Some(SavedCamera { target_x: 0.0, target_y: 1.0, target_z: 2.0,
                yaw: 0.85, pitch: 0.9, distance: 7.5, mode: "third".into() }),
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
}
