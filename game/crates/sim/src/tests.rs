//! Headless sim tests: world building, movement, hazards, movers, respawn.
//! No network, no database — pure simulation against hand-built levels.

use super::sim::Sim;
use bevy_math::Vec2;
use party_dungeon_net::{ChunkId, Input, WorldPiece};

use crate::stream::{ChunkIndex, PeerView};
use crate::world::SimMoverSpec;

fn server_piece(rid: &str, shape: &str, x: f32, z: f32) -> (String, party_dungeon_server::Piece) {
    (
        rid.to_string(),
        party_dungeon_server::Piece {
            rid: rid.to_string(),
            shape: shape.to_string(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: if shape == "spikes" { 10.0 } else { 0.0 },
            interact: "none".to_string(),
            trigger_w: 2.0,
            trigger_h: 2.0,
            trigger_l: 2.0,
            trigger_alpha: party_dungeon_server::TRIGGER_DEFAULT_ALPHA,
        },
    )
}

fn flat_world() -> Sim {
    let mut sim = Sim::new();
    // Dense cross of floors covering X and Z axes so walks in either cardinal
    // direction stay grounded for the test window (≈5 m at 2.8 m/s). Hex
    // floors have radius ~1.2; spacing 1.0 guarantees overlap.
    let mut rows = Vec::new();
    for i in -6..=6 {
        let x = i as f32 * 1.0;
        rows.push(server_piece(&format!("floor-x-{i}"), "floor", x, 0.0));
        rows.push(server_piece(&format!("floor-z-{i}"), "floor", 0.0, x));
    }
    rows.push(server_piece("spawn-1", "spawn", 0.0, 0.0));
    let pieces = sim.convert_pieces(&rows);
    let paths = std::collections::HashMap::new();
    sim.rebuild(&pieces, &paths);
    sim
}

fn walk_input(tick: u32, yaw: f32) -> Input {
    Input {
        tick,
        mx: 0.0,
        mz: 1.0,
        jump: false,
        interact: false,
        slide: false,
        sprint: false,
        yaw,
    }
}

#[test]
fn world_builds_solids_from_catalogue() {
    let sim = flat_world();
    assert!(sim.world.solids.len() >= 3, "floors are solid");
    assert_eq!(sim.spawn, Vec2::new(0.0, 0.0));
}

#[test]
fn player_falls_and_lands_on_floor() {
    let mut sim = flat_world();
    sim.join("a", "A", 100.0, 100.0);
    // Drop from 5 m up.
    sim.players.get_mut("a").unwrap().feet = 5.0;
    sim.players.get_mut("a").unwrap().grounded = false;
    for _ in 0..120 {
        sim.step(&std::collections::HashMap::new());
    }
    let p = &sim.players["a"];
    assert!(p.grounded, "landed");
    assert!((p.feet - 0.0).abs() < 0.05, "feet on floor top, got {}", p.feet);
}

#[test]
fn player_walks_forward_on_flat_ground() {
    let mut sim = flat_world();
    sim.join("a", "A", 100.0, 100.0);
    // Face -z (yaw 0 => forward (0,-1)); walk forward (+mz).
    let mut inputs = std::collections::HashMap::new();
    for t in 1..=120u32 {
        inputs.insert("a".to_string(), walk_input(t, 0.0));
        sim.step(&inputs);
    }
    let p = &sim.players["a"];
    assert!(p.grounded, "stays grounded: feet={}", p.feet);
    assert!(
        p.pos.y < -1.0,
        "moved forward (-z) at walk pace, got {:?}",
        p.pos
    );
    assert!((p.feet - 0.0).abs() < 0.08, "stays on floor");
}

#[test]
fn wall_blocks_movement() {
    let mut sim = flat_world();
    // Wall bar along z at x = 2 (rot 90 deg turns the x-bar onto z).
    let mut wall = server_piece("wall-1", "wall", 2.0, 0.0);
    wall.1.rot_deg = 90.0;
    // Strip of floors so the walk stays grounded all the way to the wall.
    let mut rows = vec![wall];
    rows.extend(vec![
        server_piece("floor-1", "floor", 0.0, 0.0),
        server_piece("floor-2", "floor", 1.5, 0.0),
        server_piece("floor-3", "floor", 3.0, 0.0),
    ]);
    let pieces = sim.convert_pieces(&rows);
    sim.rebuild(&pieces, &std::collections::HashMap::new());
    sim.join("a", "A", 100.0, 100.0);
    // Walk +x (yaw -90 deg => forward = +x).
    let mut inputs = std::collections::HashMap::new();
    for t in 1..=240u32 {
        let mut input = walk_input(t, -std::f32::consts::FRAC_PI_2);
        input.mx = 0.0;
        input.mz = 1.0;
        inputs.insert("a".to_string(), input);
        sim.step(&inputs);
    }
    let p = &sim.players["a"];
    assert!(
        p.pos.x < 1.9,
        "wall stops the walk, got x={}",
        p.pos.x
    );
    assert!(p.pos.x > 0.5, "made progress before the wall");
}

#[test]
fn jump_arcs_and_lands() {
    let mut sim = flat_world();
    sim.join("a", "A", 100.0, 100.0);
    let mut inputs = std::collections::HashMap::new();
    let mut apex = 0.0f32;
    for t in 1..=180u32 {
        let mut input = Input::idle(t);
        if t == 5 {
            input.jump = true;
        }
        inputs.insert("a".to_string(), input);
        sim.step(&inputs);
        apex = apex.max(sim.players["a"].feet);
    }
    let p = &sim.players["a"];
    assert!(apex > 0.5, "left the ground, apex={apex}");
    assert!(p.grounded, "landed again");
    assert!(p.feet.abs() < 0.08, "back on the floor");
}

#[test]
fn spikes_damage_kill_and_respawn() {
    let mut sim = flat_world();
    let rows = vec![
        server_piece("spikes-1", "spikes", 0.0, 0.0),
        server_piece("floor-1", "floor", 0.0, 0.0),
        server_piece("spawn-1", "spawn", 0.0, 0.0),
    ];
    let pieces = sim.convert_pieces(&rows);
    sim.rebuild(&pieces, &std::collections::HashMap::new());
    sim.join("a", "A", 25.0, 100.0);
    let mut inputs = std::collections::HashMap::new();
    let mut died = false;
    let mut back = false;
    for t in 1..=600u32 {
        inputs.insert("a".to_string(), Input::idle(t));
        for e in sim.step(&inputs) {
            if let super::sim::SimEvent::Kill { .. } = e {
                died = true;
            }
            if let super::sim::SimEvent::Respawn { .. } = e {
                back = true;
            }
        }
        if back {
            break;
        }
    }
    assert!(died, "standing on spikes kills");
    assert!(back, "auto-respawn fires");
    let p = &sim.players["a"];
    assert!(p.alive && p.hp == p.max_hp, "respawned whole");
}

#[test]
fn mover_carries_standing_player() {
    let mut sim = flat_world();
    // A floor slab on a 4 m track, speed 1 m/s.
    let rows = vec![
        server_piece("pad", "block", 0.0, 0.0),
        server_piece("floor-1", "floor", 0.0, 0.0),
        server_piece("floor-2", "floor", -3.0, 0.0),
        server_piece("floor-3", "floor", 3.0, 0.0),
        server_piece("spawn-1", "spawn", -6.0, 0.0),
    ];
    let pieces = sim.convert_pieces(&rows);
    let mut paths = std::collections::HashMap::new();
    paths.insert(
        "pad".to_string(),
        crate::world::SimMoverSpec {
            waypoints: vec![
                party_dungeon::platforms::PlatformWaypoint::new(Vec2::new(0.0, 0.0)),
                party_dungeon::platforms::PlatformWaypoint::new(Vec2::new(4.0, 0.0)),
            ],
            closed: false,
            mode: party_dungeon::platforms::PlatformMode::PingPong,
            speed: 1.0,
            paused: false,
            elevation: 0.9,
        },
    );
    // Raise the pad to ride height.
    let mut pieces = pieces;
    for (rid, p) in pieces.iter_mut() {
        if rid == "pad" {
            p.elevation = 0.9;
        }
    }
    sim.rebuild(&pieces, &paths);
    eprintln!("solids={} movers={} track={:?} platform_bodies={:?}", sim.world.solids.len(), sim.world.movers.len(), sim.world.movers.get("pad").and_then(|m| m.track.as_ref().map(|t| t.total())), sim.world.platform_bodies.keys().collect::<Vec<_>>());
    sim.join("a", "A", 100.0, 100.0);
    // Stand the player on the pad (block height 0.9 at elevation 0.9 => top 1.8).
    {
        let p = sim.players.get_mut("a").unwrap();
        p.pos = Vec2::new(0.0, 0.0);
        p.feet = 1.8;
        p.grounded = true;
    }
    let mut inputs = std::collections::HashMap::new();
    for t in 1..=120u32 {
        inputs.insert("a".to_string(), Input::idle(t));
        sim.step(&inputs);
    }
    let p = &sim.players["a"];
    assert!(
        p.pos.x > 1.0,
        "rider travelled with the platform, got {:?}",
        p.pos
    );
}

/// A server that has been up for a while has a large `tick`; a joining player
/// must not be considered stale before their first input arrives, and a player
/// who timed out (background tab, network stall) must come back on the next
/// input instead of staying invisible to their own reconciliation forever.
#[test]
fn joins_are_not_stale_at_a_large_tick_and_input_revives() {
    use std::collections::HashMap;

    let mut sim = flat_world();
    sim.tick = 10_000;
    sim.join("late", "Late", 100.0, 100.0);
    sim.step(&HashMap::new());
    assert!(sim.players["late"].online, "join must start with a fresh input clock");

    // No input for longer than the timeout: marked offline (as before).
    for _ in 0..(party_dungeon_net::INPUT_TIMEOUT_TICKS + 2) {
        sim.step(&HashMap::new());
    }
    assert!(!sim.players["late"].online, "a silent player times out");

    // The next input revives the row; crucially it also rejoins the snapshot
    // the player reconciles against.
    let mut inputs = HashMap::new();
    inputs.insert(
        "late".to_string(),
        Input {
            tick: 20_000,
            ..Input::idle(20_000)
        },
    );
    sim.step(&inputs);
    assert!(sim.players["late"].online, "input must revive a timed-out player");
    let snapshot = sim.snapshot(0);
    assert!(
        snapshot.players.iter().any(|player| player.sub == "late"),
        "a revived player is back in the (unfiltered) snapshot"
    );
}

/// A reconnecting client starts a new input clock at (or near) tick 0, while
/// the sim still remembers the previous session's much higher input tick.
/// Joining must reset that clock, or every input is rejected as "old", the
/// player times out to invisible and does not move for minutes.
#[test]
fn a_rejoin_restarts_the_input_clock_and_keeps_the_player_online() {
    use std::collections::HashMap;

    let mut sim = flat_world();
    sim.tick = 5_000;
    sim.join("returning", "Returning", 100.0, 100.0);
    {
        let player = sim.players.get_mut("returning").expect("joined");
        // What a long previous session left behind.
        player.last_input = Input::idle(4_000);
        player.last_input_tick = 4_980;
    }
    // Reconnect: Hello again with the same subject.
    sim.join("returning", "Returning", 100.0, 100.0);

    // New session inputs count up from 1; feed them for longer than the input
    // timeout and the player must stay online throughout.
    let mut tick = 0u32;
    for _ in 0..(party_dungeon_net::INPUT_TIMEOUT_TICKS + 120) {
        tick = tick.wrapping_add(1);
        let mut inputs = HashMap::new();
        inputs.insert("returning".to_string(), Input::idle(tick));
        sim.step(&inputs);
        assert!(sim.players["returning"].online, "player went stale on tick {tick}");
    }
    let snapshot = sim.snapshot(0);
    assert!(snapshot.players.iter().any(|player| player.sub == "returning"));
}

#[test]
fn snapshots_are_sorted_and_ticked() {
    let mut sim = flat_world();
    sim.join("b", "B", 100.0, 100.0);
    sim.join("a", "A", 100.0, 100.0);
    sim.step(&std::collections::HashMap::new());
    let snap = sim.snapshot(12345);
    assert_eq!(snap.tick, sim.tick as u32);
    assert_eq!(snap.server_ms, 12345);
    let subs: Vec<_> = snap.players.iter().map(|p| p.sub.as_str()).collect();
    assert_eq!(subs, vec!["a", "b"]);
}

// ---------------------------------------------------------------------------
// Authoritative world edits (place/update/delete -> sim -> streamed cells)
// ---------------------------------------------------------------------------

/// A wire piece as a client would author it (`rid` empty for a fresh Place).
fn wire_piece(rid: &str, shape: &str, x: f32, z: f32) -> WorldPiece {
    WorldPiece {
        rid: rid.to_string(),
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
        trigger_alpha: party_dungeon_server::TRIGGER_DEFAULT_ALPHA,
    }
}

/// A mover spec for `pad` (a `block` piece) travelling 4 m along +x.
fn pad_path() -> std::collections::HashMap<String, SimMoverSpec> {
    let mut paths = std::collections::HashMap::new();
    paths.insert(
        "pad".to_string(),
        SimMoverSpec {
            waypoints: vec![
                party_dungeon::platforms::PlatformWaypoint::new(Vec2::new(0.0, 0.0)),
                party_dungeon::platforms::PlatformWaypoint::new(Vec2::new(4.0, 0.0)),
            ],
            closed: false,
            mode: party_dungeon::platforms::PlatformMode::PingPong,
            speed: 1.0,
            paused: false,
            elevation: 0.0,
        },
    );
    paths
}

#[test]
fn wire_piece_converts_to_the_game_model() {
    let mut sim = Sim::new();
    let mut wire = wire_piece("dungeon_piece:abc", "glade_wall", 12.0, -4.0);
    wire.rot_deg = 90.0;
    wire.elevation = 1.25;
    wire.damage = 12.0;
    wire.interact = "togglePause".to_string();
    wire.dynamic = true;
    wire.trigger_w = 3.0;
    wire.trigger_h = 2.5;
    wire.trigger_l = 1.5;
    wire.trigger_alpha = 0.2;

    let piece = sim.piece_from_wire(&wire).expect("known shape");
    assert_eq!(piece.shape, party_dungeon::model::shape_index("glade_wall").unwrap());
    assert_eq!(piece.pos, Vec2::new(12.0, -4.0));
    assert!((piece.rot - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    assert!((piece.elevation - 1.25).abs() < 1e-6);
    assert!(piece.dynamic);
    assert!((piece.trigger.damage - 12.0).abs() < 1e-6);
    assert!((piece.trigger_size.x - 3.0).abs() < 1e-6);
    assert!((piece.trigger_size.y - 2.5).abs() < 1e-6);
    assert!((piece.trigger_size.z - 1.5).abs() < 1e-6);
    assert!(
        (piece.trigger_alpha - 0.2).abs() < 1e-6,
        "the wire roof-pitch alpha reaches the model"
    );
    assert_eq!(
        piece.interact,
        party_dungeon::model::InteractAction::TogglePause
    );
    // The rid-keyed model id is stable across conversions (store-loaded
    // pieces use the same map), so the renderer/streaming identity is stable.
    let again = sim.piece_from_wire(&wire).expect("known shape");
    assert_eq!(piece.id, again.id);

    // A shape this build does not know is refused (rejected before persist).
    assert!(
        sim.piece_from_wire(&wire_piece("dungeon_piece:bad", "not_a_shape", 0.0, 0.0))
            .is_none()
    );
}

#[test]
fn apply_and_delete_bump_version_and_chunk_index_follows() {
    let mut sim = Sim::new();
    let start = sim.dungeon_version;
    let mut index = ChunkIndex::new();
    index.sync(&sim);

    // Apply + a single batch rebuild is what an accepted Place does.
    let cell = ChunkId::containing(10.0, 10.0);
    assert!(sim.apply_wire_piece(&wire_piece("dungeon_piece:a", "floor", 10.0, 10.0)));
    // Staged only until the batch is finalized.
    assert_eq!(sim.dungeon_version, start);
    assert!(!index.has_content(cell), "index not synced before the rebuild");
    sim.rebuild_current();
    assert_eq!(sim.dungeon_version, start + 1, "one bump per edited batch");
    index.sync(&sim);
    assert!(index.has_content(cell));
    let content = index.content(cell).unwrap();
    assert_eq!(content.pieces.len(), 1);
    assert_eq!(content.pieces[0].rid, "dungeon_piece:a");
    assert_eq!(content.pieces[0].shape, "floor");

    // Update = upsert by rid: moving it to another cell moves the streamed
    // content rather than duplicating it.
    assert!(sim.apply_wire_piece(&wire_piece("dungeon_piece:a", "floor", 70.0, 10.0)));
    sim.rebuild_current();
    index.sync(&sim);
    assert_eq!(sim.pieces.len(), 1, "upsert, not append");
    let far = ChunkId::containing(70.0, 10.0);
    assert!(index.has_content(far));
    assert!(!index.has_content(cell), "emptied cell is tombstoned");
    assert_eq!(index.content(far).unwrap().pieces[0].x, 70.0);

    // Delete removes the piece and tombstones the cell so peers holding it
    // get an empty WorldChunk.
    assert!(sim.delete_wire_piece("dungeon_piece:a"));
    assert!(!sim.delete_wire_piece("dungeon_piece:a"), "second delete is a no-op");
    sim.rebuild_current();
    index.sync(&sim);
    assert!(sim.pieces.is_empty());
    assert!(!index.has_content(far));
    let clearing = index.chunk(far).expect("tombstone chunk for a cleared cell");
    assert!(clearing.is_empty());
}

#[test]
fn delete_removes_the_mover_track() {
    let mut sim = Sim::new();
    let rows = vec![
        server_piece("pad", "block", 0.0, 0.0),
        server_piece("floor-1", "floor", 0.0, 0.0),
        server_piece("spawn-1", "spawn", -6.0, 0.0),
    ];
    let pieces = sim.convert_pieces(&rows);
    sim.rebuild(&pieces, &pad_path());
    assert!(sim.world.movers.contains_key("pad"), "mover exists");
    assert!(sim.mover_specs.contains_key("pad"), "spec retained");

    assert!(sim.delete_wire_piece("pad"));
    assert!(!sim.mover_specs.contains_key("pad"), "mover spec dropped");
    assert!(!sim.world.movers.contains_key("pad"), "runtime mover dropped");

    sim.rebuild_current();
    assert!(!sim.world.movers.contains_key("pad"), "stays gone after rebuild");
    assert!(sim.pieces.iter().all(|(rid, _)| rid != "pad"));
    assert!(
        sim.world.solids.iter().all(|solid| solid.rid != "pad"),
        "collider dropped too"
    );
}

#[test]
fn batch_rebuild_keeps_existing_pieces_and_mover_paths() {
    let mut sim = Sim::new();
    let rows = vec![
        server_piece("pad", "block", 0.0, 0.0),
        server_piece("floor-1", "floor", 0.0, 0.0),
        server_piece("wall-1", "wall", 10.0, 10.0),
        server_piece("spawn-1", "spawn", -6.0, 0.0),
    ];
    let pieces = sim.convert_pieces(&rows);
    sim.rebuild(&pieces, &pad_path());
    let version = sim.dungeon_version;
    assert!(sim.world.movers["pad"].track.is_some());

    // An edit to one piece, finalized with one rebuild.
    assert!(sim.apply_wire_piece(&wire_piece("floor-1", "floor", 2.0, 0.0)));
    sim.rebuild_current();
    assert_eq!(sim.dungeon_version, version + 1);

    assert!(sim.pieces.iter().any(|(rid, _)| rid == "wall-1"), "untouched piece kept");
    assert!(sim.pieces.iter().any(|(rid, piece)| {
        rid == "floor-1" && (piece.pos.x - 2.0).abs() < 1e-6
    }));
    assert!(
        sim.world.movers.contains_key("pad"),
        "mover track survives an in-place rebuild"
    );
    assert!(sim.world.movers["pad"].track.is_some());
    assert!(sim.mover_specs.contains_key("pad"));
}

/// A store poll reload (`poll_edits`) can rebuild the sim from identical rows
/// ~2 s after an edit. Content-equal rebuilds must not re-stream every cell:
/// `ChunkIndex` keeps each cell's version and a peer that already holds it is
/// not queued again.
#[test]
fn content_equal_reload_does_not_restream_cells() {
    let mut sim = Sim::new();
    let mut index = ChunkIndex::new();
    index.sync(&sim);
    assert!(sim.apply_wire_piece(&wire_piece("dungeon_piece:a", "floor", 10.0, 10.0)));
    sim.rebuild_current();
    index.sync(&sim);

    let mut view = PeerView::new();
    let first = view.tick(&index, [0.0, 0.0], 16);
    assert_eq!(first.chunks.len(), 1, "the edited cell streams once");
    let cell = first.chunks[0].id;
    let version = first.chunks[0].version;

    // Simulate `bridge::poll_edits`: same rows reloaded, full rebuild.
    let pieces = sim.pieces.clone();
    let specs = sim.mover_specs.clone();
    let before = sim.dungeon_version;
    sim.rebuild(&pieces, &specs);
    assert_ne!(sim.dungeon_version, before, "reload bumps the global version");
    index.sync(&sim);

    assert_eq!(
        index.version(cell),
        Some(version),
        "identical content keeps the cell version"
    );
    let second = view.tick(&index, [0.0, 0.0], 16);
    assert!(
        second.chunks.is_empty(),
        "a content-equal reload must not re-stream the cell"
    );
    assert!(second.unload.is_empty());
}
