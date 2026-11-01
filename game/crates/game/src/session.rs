//! Saved-map snapshots: the versioned JSON shape kept in browser storage.

use serde::{Deserialize, Serialize};

use bevy::math::{Vec2, Vec3};

use crate::{
    editor::{Character, Orbit, ViewMode},
    model::{self, Piece},
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
}
