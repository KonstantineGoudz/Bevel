//! Two-app in-process tests for the embedded replicon server.
//!
//! These drive a *real* server app and a *real* client app over the backend's
//! in-memory transport: no sockets, no DB, no Tokio — just the replicated world,
//! the visibility filter and the client/server message channels. The Tokio
//! authority is simulated by writing the same `ServerCommand`s the real loop
//! sends and reading the `ClientIntent`s the real loop would consume.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::prelude::*;
use party_dungeon::model;
use party_dungeon_net::{Input, WorldOp};
use party_dungeon_repl_matchbox::{ClientBackendConfig, MatchboxClientPlugin, testkit};
use party_dungeon_replication::{
    InputMsg, PieceData, PieceId, PiecePath, PlayerCell, PlayerPose, PlayerSub, VisibleCells,
    VisiblePlayerCells, WelcomeMsg, WorldCell, WorldEditMsg,
};

use crate::repl::app::{ReplServerChannels, ReplServerConfig, build_server_app};
use crate::repl::convert;
use crate::repl::protocol::{
    ClientIntent, PieceDelta, PlayerView, ServerCommand, ServerFrame,
};

const TEST_TOKEN: &str = "test-sub";

fn server_app() -> (App, std::sync::mpsc::Sender<ServerCommand>, std::sync::mpsc::Receiver<ClientIntent>) {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (intent_tx, intent_rx) = std::sync::mpsc::channel();
    let app = build_server_app(
        ReplServerConfig {
            signaling_url: "memory://".to_string(),
            max_message_size: 16 * 1024,
            world_radius: 64.0,
            player_radius: 96.0,
        },
        ReplServerChannels {
            commands: command_rx,
            intents: intent_tx,
        },
        // No authenticator: the testkit installs its `TestAuthenticator`
        // (token == sub) when the resource is absent.
        None,
    );
    (app, command_tx, intent_rx)
}

fn client_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(1))),
        StatesPlugin,
        RepliconPlugins.set(RepliconSharedPlugin {
            auth_method: AuthMethod::Custom,
        }),
    ));
    party_dungeon_replication::register(&mut app);
    app.add_plugins(MatchboxClientPlugin);
    app.insert_resource(ClientBackendConfig {
        signaling_url: "memory://".to_string(),
        token: TEST_TOKEN.to_string(),
    });
    app.init_resource::<TestOutbox>();
    app.add_systems(Update, flush_test_outbox);
    app
}

#[derive(Resource, Default)]
struct TestOutbox {
    inputs: Vec<InputMsg>,
    edits: Vec<WorldEditMsg>,
}

fn flush_test_outbox(
    mut outbox: ResMut<TestOutbox>,
    mut inputs: MessageWriter<InputMsg>,
    mut edits: MessageWriter<WorldEditMsg>,
) {
    for message in outbox.inputs.drain(..) {
        inputs.write(message);
    }
    for message in outbox.edits.drain(..) {
        edits.write(message);
    }
}

#[derive(Resource, Default)]
struct Collected {
    inputs: Vec<Input>,
    edits: Vec<Vec<WorldOp>>,
}

fn collect_client_messages(
    mut inputs: MessageReader<FromClient<InputMsg>>,
    mut edits: MessageReader<FromClient<WorldEditMsg>>,
    mut collected: ResMut<Collected>,
) {
    for message in inputs.read() {
        collected.inputs.push(message.message.0);
    }
    for message in edits.read() {
        collected.edits.push(message.message.ops.clone());
    }
}

#[derive(Resource, Default)]
struct Welcomes(Vec<WelcomeMsg>);

fn collect_welcomes(mut messages: MessageReader<WelcomeMsg>, mut welcomes: ResMut<Welcomes>) {
    for message in messages.read() {
        welcomes.0.push(message.clone());
    }
}

// Read-only helpers: `run_until` predicates get `&App`, so these walk entities
// through `World::iter_entities` instead of a mutable query state.
fn piece_rids(app: &App) -> Vec<String> {
    let mut rids: Vec<String> = app
        .world()
        .iter_entities()
        .filter_map(|entity| entity.get::<PieceId>().map(|id| id.0.clone()))
        .collect();
    rids.sort();
    rids
}

fn player_subs(app: &App) -> Vec<String> {
    let mut subs: Vec<String> = app
        .world()
        .iter_entities()
        .filter_map(|entity| entity.get::<PlayerSub>().map(|sub| sub.0.clone()))
        .collect();
    subs.sort();
    subs
}

fn player_pose(app: &App, sub: &str) -> Option<PlayerPose> {
    app.world()
        .iter_entities()
        .find(|entity| entity.get::<PlayerSub>().is_some_and(|player| player.0 == sub))
        .and_then(|entity| entity.get::<PlayerPose>().copied())
}

fn piece_path(app: &App, rid: &str) -> Option<PiecePath> {
    app.world()
        .iter_entities()
        .find(|entity| entity.get::<PieceId>().is_some_and(|id| id.0 == rid))
        .and_then(|entity| entity.get::<PiecePath>().cloned())
}

fn piece_data(app: &App, rid: &str) -> Option<PieceData> {
    app.world()
        .iter_entities()
        .find(|entity| entity.get::<PieceId>().is_some_and(|id| id.0 == rid))
        .and_then(|entity| entity.get::<PieceData>().copied())
}

/// A piece at world `(x, z)` with the given shape id, in the replicated shape.
fn piece(shape: &str, x: f32, z: f32) -> PieceData {
    PieceData {
        shape: model::shape_index(shape).expect("known shape") as u16,
        x_cm: (x * 100.0).round() as i32,
        z_cm: (z * 100.0).round() as i32,
        elevation_cm: 0,
        rot: 0,
        pitch: 0,
        roll: 0,
        damage: 0,
        flags: 0,
        interact: convert::INTERACT_NONE,
        trigger_w_cm: 200,
        trigger_h_cm: 200,
        trigger_l_cm: 200,
        trigger_alpha: 0,
    }
}

fn player_view(sub: &str, name: &str, x: f32, z: f32) -> PlayerView {
    let pose = PlayerPose {
        x_cm: (x * 100.0).round() as i32,
        z_cm: (z * 100.0).round() as i32,
        feet_cm: 0,
        vx_cms: 0,
        vy_cms: 0,
        vz_cms: 0,
        yaw: 0,
        hp: 10000,
        flags: convert::FLAG_ALIVE | convert::FLAG_ONLINE,
    };
    PlayerView {
        sub: sub.to_string(),
        name: name.to_string(),
        cell: PlayerCell::containing(pose.x(), convert::pose_z(&pose)),
        pose,
    }
}

fn next_intent(intents: &std::sync::mpsc::Receiver<ClientIntent>) -> Option<ClientIntent> {
    intents.try_recv().ok()
}

/// `Plugin::finish` is what computes the protocol hash and constructs the
/// backend systems; `App::run` does it for the real server thread, tests must
/// do it explicitly (same as the backend's own tests).
fn finish(app: &mut App) {
    app.finish();
    app.cleanup();
}

#[test]
fn visibility_filters_pieces_and_players_by_cell() {
    let (mut server, command_tx, intents) = server_app();
    server.init_resource::<Collected>();
    server.add_systems(Update, collect_client_messages);

    let mut client = client_app();
    client.init_resource::<Welcomes>();
    client.add_systems(Update, collect_welcomes);

    finish(&mut server);
    finish(&mut client);
    let mut link = testkit::connect(&mut server, &mut client, TEST_TOKEN);
    assert!(
        link.settle(&mut server, &mut client),
        "replicon handshake should settle"
    );

    // The backend authorized the client; our session observer forwards a Join
    // intent for the authority.
    let join = next_intent(&intents).expect("join intent after authorization");
    assert_eq!(
        join,
        ClientIntent::Join {
            sub: TEST_TOKEN.to_string(),
            name: TEST_TOKEN.to_string(), // TestAuthenticator: name == token
        }
    );

    // The authority answers with the joined session and then a frame with one
    // in-radius piece, one far-away piece, our own player, and a far player.
    command_tx
        .send(ServerCommand::Joined {
            sub: TEST_TOKEN.to_string(),
            name: "Test Player".to_string(),
            tick: 1,
            dungeon_version: 1,
            world_radius: 64.0,
            x: 0.0,
            z: 0.0,
        })
        .expect("command channel");
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 1,
            dungeon_version: 1,
            pieces: PieceDelta {
                upsert: vec![
                    ("dungeon_piece:in".to_string(), piece("floor", 10.0, 10.0)),
                    ("dungeon_piece:out".to_string(), piece("floor", 200.0, 200.0)),
                ],
                remove: Vec::new(),
                ..Default::default()
            },
            players: vec![
                player_view(TEST_TOKEN, "Test Player", 0.0, 0.0),
                player_view("far-sub", "Far Player", 300.0, 0.0),
            ],
        }))
        .expect("command channel");

    let ready = link.run_until(&mut server, &mut client, 300, |_server, client| {
        !piece_rids(client).is_empty()
            && player_pose(client, TEST_TOKEN).is_some()
            && collect_welcome_count(client) > 0
    });
    assert!(ready, "in-cell piece and own player pose should replicate");

    let rids = piece_rids(&client);
    assert_eq!(rids, vec!["dungeon_piece:in".to_string()], "only the in-cell piece replicates");
    assert!(
        !rids.contains(&"dungeon_piece:out".to_string()),
        "a piece outside VisibleCells must not replicate"
    );
    let subs = player_subs(&client);
    assert_eq!(subs, vec![TEST_TOKEN.to_string()], "only the in-cell player replicates");
    assert!(player_pose(&client, TEST_TOKEN).is_some());
    assert!(player_pose(&client, "far-sub").is_none());

    // The welcome carries the row name/version the authority supplied.
    let welcomes = client.world().resource::<Welcomes>();
    assert_eq!(welcomes.0.len(), 1, "exactly one WelcomeMsg");
    let welcome = &welcomes.0[0];
    assert_eq!(welcome.sub, TEST_TOKEN);
    assert_eq!(welcome.name, "Test Player");
    assert_eq!(welcome.dungeon_version, 1);
    assert_eq!(welcome.world_radius, 64.0);

    // The replicated piece data matches the authority's quantized form.
    let data = piece_data(&client, "dungeon_piece:in").expect("in-cell piece data");
    assert_eq!(data.x(), 10.0);
    assert_eq!(data.z(), 10.0);
}

/// Small helper: count welcomes without a mutable borrow of the app in the
/// `run_until` closure (the closure gets `&App`).
fn collect_welcome_count(client: &App) -> usize {
    client
        .world()
        .get_resource::<Welcomes>()
        .map(|welcomes| welcomes.0.len())
        .unwrap_or(0)
}

#[test]
fn replication_updates_and_removals_reach_the_client() {
    let (mut server, command_tx, intents) = server_app();
    let mut client = client_app();
    finish(&mut server);
    finish(&mut client);
    let mut link = testkit::connect(&mut server, &mut client, TEST_TOKEN);
    assert!(link.settle(&mut server, &mut client));
    // Consume the join intent and answer it like the authority loop does; the
    // client needs `VisibleCells` for anything to replicate.
    let _ = next_intent(&intents);
    command_tx
        .send(ServerCommand::Joined {
            sub: TEST_TOKEN.to_string(),
            name: "Test Player".to_string(),
            tick: 1,
            dungeon_version: 1,
            world_radius: 64.0,
            x: 0.0,
            z: 0.0,
        })
        .expect("command channel");

    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 1,
            dungeon_version: 1,
            pieces: PieceDelta {
                upsert: vec![("dungeon_piece:a".to_string(), piece("floor", 0.0, 0.0))],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 100, |_s, c| !piece_rids(c).is_empty()),
        "initial piece replicates"
    );

    // An update (same rid, moved + rotated) mutates the client's component.
    let mut moved = piece("floor", 32.5, 0.0);
    moved.rot = 16_384; // 90°
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 2,
            dungeon_version: 2,
            pieces: PieceDelta {
                upsert: vec![("dungeon_piece:a".to_string(), moved)],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 100, |_s, c| {
            piece_data(c, "dungeon_piece:a").is_some_and(|data| data.x() == 32.5)
        }),
        "moved piece replicates as a mutation"
    );

    // A removal despawns the replicated entity.
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 3,
            dungeon_version: 3,
            pieces: PieceDelta {
                remove: vec!["dungeon_piece:a".to_string()],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 100, |_s, c| piece_rids(c).is_empty()),
        "deleted piece despawns on the client"
    );
}

#[test]
fn client_input_and_world_edit_reach_the_authority_channel() {
    let (mut server, _command_tx, intents) = server_app();
    server.init_resource::<Collected>();
    server.add_systems(Update, collect_client_messages);

    let mut client = client_app();
    finish(&mut server);
    finish(&mut client);
    let mut link = testkit::connect(&mut server, &mut client, TEST_TOKEN);
    assert!(link.settle(&mut server, &mut client));

    // Our observer maps the client entity; consume its join intent.
    let _ = next_intent(&intents);

    // A replicated input tick and a world edit, as the real client sends them.
    let input = Input {
        tick: 42,
        mx: 0.5,
        mz: 1.0,
        jump: true,
        interact: false,
        slide: false,
        sprint: false,
        yaw: 1.5,
    };
    {
        let mut outbox = client.world_mut().resource_mut::<TestOutbox>();
        outbox.inputs.push(InputMsg(input));
        outbox.edits.push(WorldEditMsg {
            ops: vec![WorldOp::Delete {
                rid: "dungeon_piece:gone".to_string(),
            }],
        });
    }

    // Both directions take a couple of transport frames to land.
    let mut got_input = None;
    let mut got_edit = false;
    for _ in 0..50 {
        link.step(&mut server, &mut client);
        while let Some(intent) = next_intent(&intents) {
            if let ClientIntent::Input { sub, input } = &intent {
                assert_eq!(sub, TEST_TOKEN);
                got_input = Some(*input);
            }
            if let ClientIntent::WorldEdit { sub, ops } = &intent {
                assert_eq!(sub, TEST_TOKEN);
                assert_eq!(ops.len(), 1);
                got_edit = true;
            }
        }
        if got_input.is_some() && got_edit {
            break;
        }
    }
    let collected_debug = {
        let collected = server.world().resource::<Collected>();
        format!(
            "server decoded inputs={:?} edits={}",
            collected.inputs.iter().map(|input| input.tick).collect::<Vec<_>>(),
            collected.edits.len()
        )
    };
    assert_eq!(
        got_input,
        Some(input),
        "client InputMsg reaches the authority channel ({collected_debug})"
    );
    assert!(
        got_edit,
        "client WorldEditMsg reaches the authority channel ({collected_debug})"
    );

    // The server app also decoded them into the regular replicon messages.
    let collected = server.world().resource::<Collected>();
    assert!(
        collected.inputs.iter().any(|received| received.tick == 42),
        "FromClient<InputMsg> decoded server-side"
    );
    assert_eq!(collected.edits.len(), 1, "FromClient<WorldEditMsg> decoded server-side");
}

#[test]
fn visible_cells_cover_the_interest_radius() {
    // radius 0 = exactly the containing cell (the old `chunks_within` rule).
    let centre = WorldCell::containing(10.0, -10.0);
    assert_eq!(VisibleCells::around(10.0, -10.0, 0.0).0, vec![centre.0]);

    // radius 64 m = the 32 m cells intersecting the disc: a 5x5 window at the
    // origin (cells -2..=2 on both axes).
    let cells = VisibleCells::around(0.0, 0.0, 64.0);
    assert!(cells.0.contains(&WorldCell::containing(0.0, 0.0).0));
    assert!(cells.0.contains(&WorldCell::containing(60.0, 0.0).0));
    assert!(cells.0.contains(&WorldCell::containing(-60.0, -60.0).0));
    assert!(!cells.0.contains(&WorldCell::containing(200.0, 200.0).0));
    assert_eq!(cells.0.len(), 25, "5x5 window");

    // The player filter is the same grid at the legacy 96 m radius: it covers
    // the 3-cell border and excludes a player 300 m away.
    let player_cells = VisiblePlayerCells(VisibleCells::around(0.0, 0.0, 96.0).0);
    assert!(player_cells
        .0
        .contains(&PlayerCell::containing(0.0, 0.0).0));
    assert!(player_cells
        .0
        .contains(&PlayerCell::containing(90.0, 0.0).0));
    assert!(!player_cells
        .0
        .contains(&PlayerCell::containing(300.0, 0.0).0));

    // A centre outside the encoded ±4096 m grid yields no cells (instead of
    // panicking or wrapping); an in-range far corner stays within the bounds.
    assert!(VisibleCells::around(-5000.0, 5000.0, 16.0).0.is_empty());
    let far_corner = VisibleCells::around(3000.0, -3000.0, 512.0);
    assert!(!far_corner.0.is_empty());
    assert!(far_corner.0.iter().all(|encoded| *encoded <= u16::MAX));
}

#[test]
fn mover_paths_replicate_and_can_be_removed() {
    let (mut server, command_tx, intents) = server_app();
    let mut client = client_app();
    finish(&mut server);
    finish(&mut client);
    let mut link = testkit::connect(&mut server, &mut client, TEST_TOKEN);
    assert!(link.settle(&mut server, &mut client));
    let _ = next_intent(&intents);
    command_tx
        .send(ServerCommand::Joined {
            sub: TEST_TOKEN.to_string(),
            name: "Test Player".to_string(),
            tick: 1,
            dungeon_version: 1,
            world_radius: 64.0,
            x: 0.0,
            z: 0.0,
        })
        .expect("command channel");

    let rid = "dungeon_piece:pad";
    let path = PiecePath {
        waypoints: vec![[0, 0], [400, 0]],
        speed_cm_s: 150,
        mode: 0,
        paused: false,
    };
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 1,
            dungeon_version: 1,
            pieces: PieceDelta {
                upsert: vec![(rid.to_string(), piece("block", 0.0, 0.0))],
                paths: vec![(rid.to_string(), path.clone())],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 200, |_s, c| {
            piece_path(c, rid).is_some()
        }),
        "mover path replicates onto the piece entity"
    );
    assert_eq!(piece_path(&client, rid), Some(path));

    // An interact pause toggle changes only the path component.
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 2,
            dungeon_version: 2,
            pieces: PieceDelta {
                paths: vec![(
                    rid.to_string(),
                    PiecePath {
                        paused: true,
                        ..piece_path(&client, rid).expect("path")
                    },
                )],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 200, |_s, c| {
            piece_path(c, rid).is_some_and(|path| path.paused)
        }),
        "pause toggle replicates"
    );

    // Deleting the track removes the component from the piece entity.
    command_tx
        .send(ServerCommand::Frame(ServerFrame {
            tick: 3,
            dungeon_version: 3,
            pieces: PieceDelta {
                path_remove: vec![rid.to_string()],
                ..Default::default()
            },
            players: Vec::new(),
        }))
        .expect("command channel");
    assert!(
        link.run_until(&mut server, &mut client, 200, |_s, c| {
            piece_path(c, rid).is_none()
        }),
        "deleted mover track is removed on the client"
    );
}


