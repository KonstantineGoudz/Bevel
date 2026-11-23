//! The single source of truth for every player-facing mutation and query.
//!
//! Each reducer is declared once here — name, documentation, caller-supplied
//! parameters, the exact SurrealQL statements, how the result maps, and how
//! to tell "row missing" from "permission denied". Two consumers share these
//! declarations:
//!
//! * [`Backend`](crate::backend::Backend) executes them with full rights
//!   (service role), adding validation up front.
//! * `src/bin/codegen.rs` renders the standalone Rust client package from
//!   the same declarations, so players call byte-identical statements over
//!   their own direct connections (where the row rules in `schema.surql`
//!   constrain them).
//!
//! Conventions honored by both consumers:
//! * `$rid` / `$rid2` / `$now` are injected, never caller-supplied — unless
//!   the parameter list names them (updates address a caller-chosen row).
//!   Caller parameters always win over injection.
//! * A mutation that writes nothing is a denial or a miss: `probe` tells
//!   them apart, and runs only on the empty path.
//! * `checks` are pre-condition reads: empty means `NotFound` with the given
//!   message. They run first, in order.
//! * Multi-statement reducers run `transactional` (one BEGIN/COMMIT batch)
//!   and take their result from `statements[result_index]` (the executor
//!   accounts for the BEGIN offset).
//! * No computed bindings: every `$var` is either a caller parameter or an
//!   injected `$rid`/`$rid2`/`$now`. Feed text is static or a direct param.
//! * `custom` reducers (only `connect_player`: SELECT-then-branch) are
//!   orchestrated by hand in both consumers from the same statement consts.

/// A caller-supplied parameter type. Maps to a Rust type in generated code
/// and to a bound variable at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Str,
    F32,
    I64,
    Bool,
    Waypoints,
    /// A full piece struct, expanded field-by-field into bound variables.
    NewPiece,
}

impl ParamType {
    pub fn rust_type(self) -> &'static str {
        match self {
            ParamType::Str => "String",
            ParamType::F32 => "f32",
            ParamType::I64 => "i64",
            ParamType::Bool => "bool",
            ParamType::Waypoints => "Vec<Waypoint>",
            ParamType::NewPiece => "NewPiece",
        }
    }

    /// Bound-variable assignments for one parameter. Struct params expand.
    pub fn binds(self, name: &str) -> Vec<(String, String)> {
        match self {
            ParamType::NewPiece => vec![
                ("shape".to_string(), format!("v_str({name}.shape.clone())")),
                ("x".to_string(), format!("v_f32({name}.x)")),
                ("z".to_string(), format!("v_f32({name}.z)")),
                ("elevation".to_string(), format!("v_f32({name}.elevation)")),
                ("rot_deg".to_string(), format!("v_f32({name}.rot_deg)")),
                ("pitch_deg".to_string(), format!("v_f32({name}.pitch_deg)")),
                ("roll_deg".to_string(), format!("v_f32({name}.roll_deg)")),
                ("dynamic".to_string(), format!("v_bool({name}.dynamic)")),
                ("damage".to_string(), format!("v_f32({name}.damage)")),
                ("interact".to_string(), format!("v_str({name}.interact.clone())")),
                ("trigger_w".to_string(), format!("v_f32({name}.trigger_w)")),
                ("trigger_h".to_string(), format!("v_f32({name}.trigger_h)")),
                ("trigger_l".to_string(), format!("v_f32({name}.trigger_l)")),
                ("trigger_alpha".to_string(), format!("v_f32({name}.trigger_alpha)")),
            ],
            _ => vec![(name.to_string(), bind_simple(name, self))],
        }
    }
}

fn bind_simple(name: &str, ty: ParamType) -> String {
    match ty {
        ParamType::Str => format!("v_str({name}.to_owned())"),
        ParamType::F32 => format!("v_f32({name})"),
        ParamType::I64 => format!("v_i64({name})"),
        ParamType::Bool => format!("v_bool({name})"),
        ParamType::Waypoints => format!("to_value({name})"),
        ParamType::NewPiece => unreachable!("expanded above"),
    }
}

/// Canonicalization applied to a caller parameter before binding.
/// Mirrors the service layer's normalization; the row rules accept only
/// canonical values, so both paths must send them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    Trim,
    NormalizeDeg,
    SanitizeExtent,
    /// Clamp a trigger alpha / glade roof-pitch alpha into `0.0..=1.0`
    /// (non-finite falls back to the default).
    SanitizeAlpha,
    CanonicalInteract,
}

/// How a result maps. Row kinds take from `statements[result_index]`
/// (`OptRow` yields `None` on empty); `Unit` requires non-empty;
/// `CountU64` counts returned rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    OneRow,
    OptRow,
    Rows,
    Unit,
    CountU64,
}

/// Row type a result maps into. Drives generated signatures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowType {
    Player,
    Piece,
    PlatformPath,
    LogEntry,
    LevelMeta,
}

impl RowType {
    pub fn rust_type(self) -> &'static str {
        match self {
            RowType::Player => "Player",
            RowType::Piece => "Piece",
            RowType::PlatformPath => "PlatformPath",
            RowType::LogEntry => "LogEntry",
            RowType::LevelMeta => "LevelMeta",
        }
    }
}

/// Pre-condition read before mutating: `SELECT VALUE rid FROM {table}
/// WHERE rid = ...`. Empty aborts with `NotFound(missing)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckDef {
    pub table: &'static str,
    pub rid_param: Option<&'static str>,
    pub rid_literal: Option<&'static str>,
    pub missing: &'static str,
}

/// Existence check used when a mutation writes nothing. Empty means the row
/// is missing (`NotFound`); a hit means the write was denied
/// (`PermissionDenied`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeDef {
    pub table: &'static str,
    pub rid_param: Option<&'static str>,
    pub rid_literal: Option<&'static str>,
}

pub struct ReducerDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [(&'static str, ParamType)],
    pub statements: &'static [&'static str],
    pub transactional: bool,
    pub checks: &'static [CheckDef],
    pub result: ResultKind,
    pub result_row: Option<RowType>,
    pub result_index: usize,
    pub probe: Option<ProbeDef>,
    pub custom: bool,
    pub transforms: &'static [(&'static str, Transform)],
    /// Whether the generated client exposes this reducer. Server-only reducers
    /// (physics, kills) are `false`: only the sim (service role) may call them.
    pub client_callable: bool,
}

pub struct QueryDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [(&'static str, ParamType)],
    pub sql: &'static str,
    pub result: ResultKind,
    pub result_row: Option<RowType>,
    /// Truncate `Rows` to this caller-supplied limit after fetching.
    pub truncate_to: Option<&'static str>,
    pub client_callable: bool,
}

// ---------------------------------------------------------------------------
// Reducers
// ---------------------------------------------------------------------------

pub mod level_name {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "level_name",
        doc: "Lobby metadata row, if the dungeon has been seeded.",
        params: &[],
        statements: &["SELECT * FROM level_meta WHERE rid = 'main'"],
        transactional: false,
        checks: &[],
        result: ResultKind::OptRow,
        result_row: Some(RowType::LevelMeta),
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod set_level_name {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "set_level_name",
        doc: "Rename the dungeon and log the change.",
        params: &[("name", ParamType::Str), ("actor", ParamType::Str)],
        statements: &[
            "UPDATE level_meta SET name = $name WHERE rid = 'main' RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'edit', actor: $actor, text: $name } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::Unit,
        result_row: None,
        result_index: 0,
        probe: Some(ProbeDef { table: "level_meta", rid_param: None, rid_literal: Some("'main'") }),
        custom: false,
        transforms: &[("name", Transform::Trim)],
        client_callable: true,
    };
}

pub mod connect_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "connect_player",
        doc: "First-timers get a row (full HP at the origin); returning players resume and flip online.",
        params: &[("sub", ParamType::Str)],
        statements: &[
            "SELECT * FROM player WHERE rid = $sub",
            "UPDATE player SET online = true, last_seen_ms = $now WHERE rid = $sub RETURN *",
            "CREATE player CONTENT { rid: $sub, name: 'Adventurer', x: 0, z: 0, feet: 0, yaw_deg: 0, hp: $hp, max_hp: $hp, online: true, last_seen_ms: $now } RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'join', actor: $sub, text: 'An adventurer connected.' } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: None,
        custom: true,
        transforms: &[],
        client_callable: true,
    };
}

pub mod disconnect_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "disconnect_player",
        doc: "Flag offline, keep the row. Logs a leave entry.",
        params: &[("sub", ParamType::Str)],
        statements: &[
            "UPDATE player SET online = false, last_seen_ms = $now WHERE rid = $sub RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'leave', actor: $sub, text: 'An adventurer disconnected.' } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::Unit,
        result_row: None,
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("sub"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod register_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "register_player",
        doc: "Claim a display name. Authority stays with the Firebase UID key.",
        params: &[("sub", ParamType::Str), ("name", ParamType::Str)],
        transforms: &[("name", Transform::Trim)],
        statements: &[
            "UPDATE player SET name = $name, last_seen_ms = $now WHERE rid = $sub RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'system', actor: $sub, text: $name } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("sub"), rid_literal: None }),
        custom: false,
        client_callable: true,
    };
}

pub mod move_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "move_player",
        doc: "Authoritative movement. Every sample is clamped into the world.",
        params: &[
            ("sub", ParamType::Str),
            ("x", ParamType::F32),
            ("z", ParamType::F32),
            ("feet", ParamType::F32),
            ("yaw_deg", ParamType::F32),
        ],
        statements: &[
            "UPDATE player SET x = $x, z = $z, feet = $feet, yaw_deg = $yaw_deg, last_seen_ms = $now WHERE rid = $sub RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("sub"), rid_literal: None }),
        custom: false,
        transforms: &[("yaw_deg", Transform::NormalizeDeg)],
        client_callable: true,
    };
}

pub mod damage_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "damage_player",
        doc: "Deal damage, floored at zero. When the returned hp is 0, call log_death.",
        params: &[
            ("target", ParamType::Str),
            ("amount", ParamType::F32),
        ],
        statements: &[
            "UPDATE player SET hp = math::max([0, hp - $amount]), last_seen_ms = $now WHERE rid = $target RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("target"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod log_death {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "log_death",
        doc: "Record a kill in the feed. Call when damage_player returns hp 0.",
        params: &[("actor", ParamType::Str), ("target", ParamType::Str)],
        statements: &[
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'kill', actor: $actor, text: $target } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::LogEntry),
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod heal_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "heal_player",
        doc: "Restore health, capped at the target's maximum.",
        params: &[("target", ParamType::Str), ("amount", ParamType::F32)],
        statements: &[
            "UPDATE player SET hp = math::min([max_hp, hp + $amount]), last_seen_ms = $now WHERE rid = $target RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("target"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod respawn_player {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "respawn_player",
        doc: "Full health at the first spawn marker (or the origin).",
        params: &[("sub", ParamType::Str)],
        statements: &[
            "UPDATE player SET x = ((SELECT VALUE x FROM dungeon_piece WHERE shape = 'spawn' ORDER BY rid LIMIT 1)[0] ?? 0), z = ((SELECT VALUE z FROM dungeon_piece WHERE shape = 'spawn' ORDER BY rid LIMIT 1)[0] ?? 0), feet = 0, hp = max_hp, last_seen_ms = $now WHERE rid = $sub RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'system', actor: $sub, text: 'Respawned at the entry.' } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Player),
        result_index: 0,
        probe: Some(ProbeDef { table: "player", rid_param: Some("sub"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod create_piece {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "create_piece",
        doc: "Place a new solid. Unknown shape ids never reach a subscriber.",
        params: &[
            ("piece", ParamType::NewPiece),
            ("actor", ParamType::Str),
        ],
        statements: &[
            "CREATE dungeon_piece CONTENT { rid: $rid, shape: $shape, x: $x, z: $z, elevation: $elevation, rot_deg: $rot_deg, pitch_deg: $pitch_deg, roll_deg: $roll_deg, dynamic: $dynamic, damage: $damage, interact: $interact, trigger_w: $trigger_w, trigger_h: $trigger_h, trigger_l: $trigger_l, trigger_alpha: $trigger_alpha } RETURN *",
            "CREATE dungeon_log CONTENT { rid: $rid2, at_ms: $now, kind: 'edit', actor: $actor, text: 'Placed a piece.' } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Piece),
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[("piece.rot_deg", Transform::NormalizeDeg), ("piece.pitch_deg", Transform::NormalizeDeg), ("piece.roll_deg", Transform::NormalizeDeg), ("piece.trigger_w", Transform::SanitizeExtent), ("piece.trigger_h", Transform::SanitizeExtent), ("piece.trigger_l", Transform::SanitizeExtent), ("piece.trigger_alpha", Transform::SanitizeAlpha), ("piece.interact", Transform::CanonicalInteract)],
        client_callable: true,
    };
}

pub mod update_piece {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "update_piece",
        doc: "Rewrite a placed solid in place (same rid, new shape/transform).",
        params: &[
            ("rid", ParamType::Str),
            ("piece", ParamType::NewPiece),
        ],
        statements: &[
            "UPDATE dungeon_piece CONTENT { rid: $rid, shape: $shape, x: $x, z: $z, elevation: $elevation, rot_deg: $rot_deg, pitch_deg: $pitch_deg, roll_deg: $roll_deg, dynamic: $dynamic, damage: $damage, interact: $interact, trigger_w: $trigger_w, trigger_h: $trigger_h, trigger_l: $trigger_l, trigger_alpha: $trigger_alpha } WHERE rid = $rid RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Piece),
        result_index: 0,
        probe: Some(ProbeDef { table: "dungeon_piece", rid_param: Some("rid"), rid_literal: None }),
        custom: false,
        transforms: &[("piece.rot_deg", Transform::NormalizeDeg), ("piece.pitch_deg", Transform::NormalizeDeg), ("piece.roll_deg", Transform::NormalizeDeg), ("piece.trigger_w", Transform::SanitizeExtent), ("piece.trigger_h", Transform::SanitizeExtent), ("piece.trigger_l", Transform::SanitizeExtent), ("piece.trigger_alpha", Transform::SanitizeAlpha), ("piece.interact", Transform::CanonicalInteract)],
        client_callable: true,
    };
}

pub mod delete_piece {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "delete_piece",
        doc: "Remove a solid and its mover track atomically.",
        params: &[("rid", ParamType::Str), ("actor", ParamType::Str)],
        statements: &[
            "DELETE platform_path WHERE piece_rid = $rid",
            "DELETE dungeon_piece WHERE rid = $rid RETURN BEFORE",
            "CREATE dungeon_log CONTENT { rid: $rid2, at_ms: $now, kind: 'edit', actor: $actor, text: $rid } RETURN *",
        ],
        transactional: true,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::Piece),
        result_index: 1,
        probe: Some(ProbeDef { table: "dungeon_piece", rid_param: Some("rid"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod clear_level {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "clear_level",
        doc: "Wipe the whole dungeon (pieces + paths). The feed entry survives.",
        params: &[("actor", ParamType::Str)],
        statements: &[
            "DELETE platform_path",
            "DELETE dungeon_piece",
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'edit', actor: $actor, text: 'Cleared the level.' } RETURN *",
        ],
        transactional: true,
        checks: &[],
        result: ResultKind::Unit,
        result_row: None,
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod set_platform_path {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "set_platform_path",
        doc: "Attach (or replace) a mover track on an existing piece.",
        params: &[
            ("piece_rid", ParamType::Str),
            ("mode", ParamType::Str),
            ("speed", ParamType::F32),
            ("paused", ParamType::Bool),
            ("waypoints", ParamType::Waypoints),
        ],
        statements: &[
            "DELETE platform_path WHERE piece_rid = $piece_rid",
            "CREATE platform_path CONTENT { piece_rid: $piece_rid, mode: $mode, speed: $speed, paused: $paused, waypoints: $waypoints } RETURN *",
        ],
        transactional: true,
        checks: &[CheckDef {
            table: "dungeon_piece",
            rid_param: Some("piece_rid"),
            rid_literal: None,
            missing: "No piece with rid {rid}.",
        }],
        result: ResultKind::OneRow,
        result_row: Some(RowType::PlatformPath),
        result_index: 1,
        probe: None,
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod clear_platform_path {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "clear_platform_path",
        doc: "Remove a mover track; the piece itself stays put.",
        params: &[("piece_rid", ParamType::Str)],
        statements: &["DELETE platform_path WHERE piece_rid = $piece_rid RETURN BEFORE"],
        transactional: false,
        checks: &[],
        result: ResultKind::Unit,
        result_row: None,
        result_index: 0,
        probe: Some(ProbeDef { table: "platform_path", rid_param: Some("piece_rid"), rid_literal: None }),
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

pub mod send_message {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "send_message",
        doc: "Post a chat line to the shared feed.",
        params: &[("actor", ParamType::Str), ("text", ParamType::Str)],
        statements: &[
            "CREATE dungeon_log CONTENT { rid: $rid, at_ms: $now, kind: 'chat', actor: $actor, text: $text } RETURN *",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::OneRow,
        result_row: Some(RowType::LogEntry),
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[("text", Transform::Trim)],
        client_callable: true,
    };
}

pub mod prune_log {
    use super::*;
    pub const DEF: ReducerDef = ReducerDef {
        name: "prune_log",
        doc: "Bound feed growth: keep the newest 200 rows, drop the rest.",
        params: &[],
        statements: &[
            "DELETE (SELECT * FROM dungeon_log ORDER BY at_ms DESC LIMIT 1000000000 START 200) RETURN BEFORE",
        ],
        transactional: false,
        checks: &[],
        result: ResultKind::CountU64,
        result_row: None,
        result_index: 0,
        probe: None,
        custom: false,
        transforms: &[],
        client_callable: true,
    };
}

/// Every reducer, for iteration (docs, codegen, conformance).
pub const ALL: &[ReducerDef] = &[
    level_name::DEF,
    set_level_name::DEF,
    connect_player::DEF,
    disconnect_player::DEF,
    register_player::DEF,
    move_player::DEF,
    damage_player::DEF,
    log_death::DEF,
    heal_player::DEF,
    respawn_player::DEF,
    create_piece::DEF,
    update_piece::DEF,
    delete_piece::DEF,
    clear_level::DEF,
    set_platform_path::DEF,
    clear_platform_path::DEF,
    send_message::DEF,
    prune_log::DEF,
];

// ---------------------------------------------------------------------------
// Read queries (single SELECT each; same single-source treatment)
// ---------------------------------------------------------------------------

pub mod q_list_players {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "list_players",
        doc: "Live roster, ordered by name.",
        params: &[],
        sql: "SELECT * FROM player ORDER BY name",
        result: ResultKind::Rows,
        result_row: Some(RowType::Player),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_list_online_players {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "list_online_players",
        doc: "Connected adventurers, ordered by name.",
        params: &[],
        sql: "SELECT * FROM player WHERE online = true ORDER BY name",
        result: ResultKind::Rows,
        result_row: Some(RowType::Player),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_get_player {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "get_player",
        doc: "One adventurer by Firebase UID, if visible.",
        params: &[("sub", ParamType::Str)],
        sql: "SELECT * FROM player WHERE rid = $sub",
        result: ResultKind::OptRow,
        result_row: Some(RowType::Player),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_list_pieces {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "list_pieces",
        doc: "The whole dungeon, ordered by rid.",
        params: &[],
        sql: "SELECT * FROM dungeon_piece ORDER BY rid",
        result: ResultKind::Rows,
        result_row: Some(RowType::Piece),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_get_piece {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "get_piece",
        doc: "One solid by rid, if visible.",
        params: &[("rid", ParamType::Str)],
        sql: "SELECT * FROM dungeon_piece WHERE rid = $rid",
        result: ResultKind::OptRow,
        result_row: Some(RowType::Piece),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_list_paths {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "list_paths",
        doc: "All mover tracks, ordered by piece.",
        params: &[],
        sql: "SELECT * FROM platform_path ORDER BY piece_rid",
        result: ResultKind::Rows,
        result_row: Some(RowType::PlatformPath),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_get_path {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "get_path",
        doc: "One mover track by piece rid, if visible.",
        params: &[("piece_rid", ParamType::Str)],
        sql: "SELECT * FROM platform_path WHERE piece_rid = $piece_rid",
        result: ResultKind::OptRow,
        result_row: Some(RowType::PlatformPath),
        truncate_to: None,
        client_callable: true,
    };
}

pub mod q_recent_log {
    use super::*;
    pub const DEF: QueryDef = QueryDef {
        name: "recent_log",
        doc: "Newest-first feed window (server caps at 500 rows).",
        params: &[("limit", ParamType::I64)],
        sql: "SELECT * FROM dungeon_log ORDER BY at_ms DESC LIMIT 500",
        result: ResultKind::Rows,
        result_row: Some(RowType::LogEntry),
        truncate_to: Some("limit"),
        client_callable: true,
    };
}

pub const QUERIES: &[QueryDef] = &[
    q_list_players::DEF,
    q_list_online_players::DEF,
    q_get_player::DEF,
    q_list_pieces::DEF,
    q_get_piece::DEF,
    q_list_paths::DEF,
    q_get_path::DEF,
    q_recent_log::DEF,
];
