//! End-to-end backend tests: the same handshake/channel-pump code over the
//! in-memory transport, plus a real matchbox/WebRTC round trip through a local
//! signaling server (native only).

#![cfg(all(feature = "server", feature = "client", not(target_arch = "wasm32")))]

use std::time::Duration;

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::MinimalPlugins;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use super::*;

// ---------------------------------------------------------------------------
// Test protocol (a stand-in for the shared `party_dungeon_replication` schema)
// ---------------------------------------------------------------------------

#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct TestPiece {
    value: u32,
}

/// Visibility-filter component (the testkit stand-in for `WorldCell`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
struct TestCell(u16);

/// Client interest set (the testkit stand-in for `VisibleCells`).
#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[component(immutable)]
struct TestVisible(Vec<u16>);

impl VisibilityFilter for TestCell {
    type ClientComponent = TestVisible;
    type Scope = Entity;

    fn is_visible(&self, _client: Entity, visible: Option<&TestVisible>) -> bool {
        visible.is_some_and(|cells| cells.0.contains(&self.0))
    }
}

#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
struct TestInput(u32);

#[derive(Message, Clone, Copy, Debug, Serialize, Deserialize)]
struct TestNotice(u32);

/// Identical registration on both ends (part of the protocol hash).
fn register_protocol(app: &mut App) {
    app.replicate::<TestPiece>();
    app.add_visibility_filter::<TestCell>();
    app.add_client_message::<TestInput>(Channel::Unreliable);
    app.add_server_message::<TestNotice>(Channel::Ordered)
        .make_message_independent::<TestNotice>();
}

#[derive(Resource, Default)]
struct Collected {
    inputs: Vec<u32>,
    notices: Vec<u32>,
}

fn collect_inputs(mut reader: MessageReader<FromClient<TestInput>>, mut out: ResMut<Collected>) {
    out.inputs.extend(reader.read().map(|input| input.0));
}

fn collect_notices(mut reader: MessageReader<TestNotice>, mut out: ResMut<Collected>) {
    out.notices.extend(reader.read().map(|notice| notice.0));
}








// ---------------------------------------------------------------------------
// Apps
// ---------------------------------------------------------------------------

fn base_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        RepliconPlugins.set(RepliconSharedPlugin {
            auth_method: AuthMethod::Custom,
        }),
    ));
    register_protocol(&mut app);
    app
}

fn server_app() -> App {
    let mut app = base_app();
    app.add_plugins(MatchboxServerPlugin)
        .init_resource::<Collected>()
        .add_systems(Update, collect_inputs);
    // `App::update()` does not run plugin `finish`; the backend asserts
    // AuthMethod::Custom there, so finish explicitly (as `App::run` would).
    app.finish();
    app.cleanup();
    app
}

fn client_app() -> App {
    let mut app = base_app();
    app.add_plugins(MatchboxClientPlugin)
        .init_resource::<Collected>()
        .add_systems(Update, collect_notices);
    app.finish();
    app.cleanup();
    app
}

fn piece_values(client_app: &mut App) -> Vec<u32> {
    let world = client_app.world_mut();
    let mut query = world.query::<&TestPiece>();
    let mut values: Vec<u32> = query.iter(world).map(|piece| piece.value).collect();
    values.sort_unstable();
    values
}

fn notice_values(client_app: &App) -> Vec<u32> {
    client_app.world().resource::<Collected>().notices.clone()
}

fn client_client_entity(server_app: &mut App) -> Entity {
    let world = server_app.world_mut();
    let mut query = world.query_filtered::<Entity, With<ClientSession>>();
    query
        .iter(world)
        .next()
        .expect("an authorized client entity")
}

fn pump(server: &mut App, client: &mut App, steps: usize) {
    for _ in 0..steps {
        server.update();
        client.update();
    }
}

// ---------------------------------------------------------------------------
// In-memory transport
// ---------------------------------------------------------------------------

#[cfg(feature = "testkit")]
#[test]
fn in_memory_transport_handshakes_replicates_and_round_trips() {
    let mut server = server_app();
    let mut client = client_app();
    let mut link = testkit::connect(&mut server, &mut client, "test-sub");
    assert!(
        link.settle(&mut server, &mut client),
        "in-memory handshake settles"
    );

    // Server-side identity: one authorized client carrying the session.
    let client_entity = link.client_entity(&server).expect("client entity");
    let session = server
        .world()
        .get::<ClientSession>(client_entity)
        .expect("ClientSession");
    assert_eq!(session.sub, "test-sub");
    assert!(server
        .world()
        .get::<AuthorizedClient>(client_entity)
        .is_some());
    let network_id = server
        .world()
        .get::<bevy_replicon::shared::backend::connected_client::NetworkId>(client_entity)
        .expect("NetworkId")
        .get();
    assert!(network_id >= 1, "network id comes from the matchbox peer key");

    // Interest set: only cell 0 is visible at first.
    server
        .world_mut()
        .entity_mut(client_entity)
        .insert(TestVisible(vec![0]));
    server
        .world_mut()
        .spawn((Replicated, TestPiece { value: 7 }, TestCell(0)));
    server
        .world_mut()
        .spawn((Replicated, TestPiece { value: 9 }, TestCell(1)));

    for _ in 0..10 {
        link.step(&mut server, &mut client);
    }
    assert_eq!(
        piece_values(&mut client),
        vec![7],
        "only the visible cell replicates"
    );

    // Widen interest: the second piece arrives.
    server
        .world_mut()
        .entity_mut(client_entity)
        .insert(TestVisible(vec![0, 1]));
    for _ in 0..10 {
        link.step(&mut server, &mut client);
    }
    assert_eq!(piece_values(&mut client), vec![7, 9]);

    // Server -> client message.
    server.world_mut().write_message(ToClients {
        targets: SendTargets::CLIENTS_ONLY,
        message: TestNotice(5),
    });
    for _ in 0..5 {
        link.step(&mut server, &mut client);
    }
    assert_eq!(notice_values(&client), vec![5]);

    // Client -> server message (the round trip the sim depends on).
    client.world_mut().write_message(TestInput(42));
    for _ in 0..5 {
        link.step(&mut server, &mut client);
    }
    assert_eq!(
        server.world().resource::<Collected>().inputs,
        vec![42],
        "client message reached the server side"
    );
}

#[cfg(feature = "testkit")]
#[test]
fn in_memory_transport_denies_a_bad_token() {
    struct Strict;

    impl Authenticator for Strict {
        fn authenticate(&self, token: &str) -> Result<Session, String> {
            if token == "good" {
                Ok(Session {
                    sub: "sub".to_string(),
                    name: "Name".to_string(),
                })
            } else {
                Err("bad token".to_string())
            }
        }
    }

    let mut server = server_app();
    server
        .world_mut()
        .insert_resource(BackendAuthenticator(Box::new(Strict)));
    let mut client = client_app();
    let mut link = testkit::connect(&mut server, &mut client, "rejected");

    for _ in 0..32 {
        link.step(&mut server, &mut client);
    }
    assert!(
        !client_reached_connected(&client),
        "the client never reaches Connected on a deny"
    );
    let status = client.world().resource::<BackendStatus>();
    assert_eq!(status.state, BackendState::Failed);
    assert!(
        status.detail.contains("denied"),
        "detail should carry the deny reason, got {:?}",
        status.detail
    );
    assert!(
        !server_has_client(&mut server),
        "no client entity is authorized on a deny"
    );
}

// ---------------------------------------------------------------------------
// Real matchbox/WebRTC transport through a local signaling server
// ---------------------------------------------------------------------------

/// Accepts any non-empty token (the matchbox test runs without the testkit
/// feature, so it brings its own authenticator).
struct AnyToken;

impl Authenticator for AnyToken {
    fn authenticate(&self, token: &str) -> Result<Session, String> {
        if token.is_empty() {
            return Err("empty token".to_string());
        }
        Ok(Session {
            sub: token.to_string(),
            name: token.to_string(),
        })
    }
}

#[test]
fn matchbox_webrtc_handshakes_replicates_and_round_trips() {
    // Local signaling server on an ephemeral port.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime for the test signaling server");
    let mut signaling = matchbox_signaling::SignalingServer::client_server_builder(
        std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
    )
    .build();
    let addr = signaling.bind().expect("bind the test signaling server");
    runtime.handle().spawn(async move {
        if let Err(err) = signaling.serve().await {
            log::warn!("test signaling server ended: {err}");
        }
    });
    let url = format!("ws://{addr}{ROOM_PATH}");

    // Loopback ICE only: no STUN/TURN round trips in a test.
    let ice = || {
        BackendIceServer(matchbox_socket::RtcIceServerConfig {
            urls: Vec::new(),
            username: None,
            credential: None,
        })
    };

    // The sim host joins the room first (matchbox `ClientServer` topology: the
    // first peer is the host).
    let mut server = server_app();
    server.insert_resource(ServerBackendConfig {
        signaling_url: url.clone(),
        ..Default::default()
    });
    server.insert_resource(ice());
    server
        .world_mut()
        .insert_resource(BackendAuthenticator(Box::new(AnyToken)));
    for _ in 0..10 {
        server.update();
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut client = client_app();
    client.insert_resource(ClientBackendConfig {
        signaling_url: url,
        token: "webrtc-sub".to_string(),
    });
    client.insert_resource(ice());

    let mut settled = false;
    for _ in 0..600 {
        server.update();
        client.update();
        if server_has_client(&mut server) && client_reached_connected(&client)
        {
            settled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        settled,
        "matchbox handshake did not settle; client status: {:?}",
        client.world().resource::<BackendStatus>()
    );

    // Replication + visibility over the real data channels.
    let client_entity = client_client_entity(&mut server);
    server
        .world_mut()
        .entity_mut(client_entity)
        .insert(TestVisible(vec![0]));
    server
        .world_mut()
        .spawn((Replicated, TestPiece { value: 11 }, TestCell(0)));

    let mut replicated = false;
    for _ in 0..300 {
        pump(&mut server, &mut client, 1);
        if piece_values(&mut client) == vec![11] {
            replicated = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(replicated, "the replicated piece arrived over WebRTC");

    // Client -> server round trip over the unreliable InputMsg-style channel.
    client.world_mut().write_message(TestInput(77));
    let mut received = false;
    for _ in 0..200 {
        pump(&mut server, &mut client, 1);
        if server.world().resource::<Collected>().inputs == vec![77] {
            received = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        received,
        "the client message arrived at the server over WebRTC"
    );

    // Bandwidth/RTT stats: the backend fills replicon's `ClientStats` (client)
    // and `ConnectedClientStats` (server) so the game's
    // `replicon stats: rtt/loss/sent/recv` log and the before/after
    // measurement are real. bps comes from a ~250 ms byte window, rtt from the
    // ~1 s control-channel ping.
    // Keep player-pose-like mutations flowing (the production case: a mutated
    // component every tick), so both directions carry payload each window.
    server
        .world_mut()
        .spawn((Replicated, TestPiece { value: 0 }, TestCell(0)));

    let mut client_sent = false;
    let mut client_recv = false;
    let mut server_sent = false;
    let mut server_recv = false;
    let mut rtt = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        // mutate the replicated component every tick
        let mut query = server.world_mut().query_filtered::<&mut TestPiece, With<Replicated>>();
        for mut piece in query.iter_mut(server.world_mut()) {
            piece.value = piece.value.wrapping_add(1);
        }
        pump(&mut server, &mut client, 1);
        std::thread::sleep(Duration::from_millis(10));
        let client_stats = *client.world().resource::<ClientStats>();
        if client_stats.sent_bps > 0.0 {
            client_sent = true;
        }
        if client_stats.received_bps > 0.0 {
            client_recv = true;
        }
        if let Some(stats) = server.world().get::<ConnectedClientStats>(client_entity) {
            if stats.0.sent_bps > 0.0 {
                server_sent = true;
            }
            if stats.0.received_bps > 0.0 {
                server_recv = true;
            }
        }
        if client_stats.rtt > 0.0 {
            rtt = true;
        }
        if client_sent && client_recv && server_sent && server_recv && rtt {
            break;
        }
    }
    assert!(
        client_sent,
        "client ClientStats.sent_bps > 0, got {:?}",
        client.world().resource::<ClientStats>()
    );
    assert!(
        client_recv,
        "client ClientStats.received_bps > 0, got {:?}",
        client.world().resource::<ClientStats>()
    );
    assert!(
        server_sent,
        "server ConnectedClientStats.sent_bps > 0, got {:?}",
        server
            .world()
            .get::<ConnectedClientStats>(client_entity)
            .map(|stats| stats.0)
    );
    assert!(
        server_recv,
        "server ConnectedClientStats.received_bps > 0, got {:?}",
        server
            .world()
            .get::<ConnectedClientStats>(client_entity)
            .map(|stats| stats.0)
    );
    assert!(
        rtt,
        "client RTT measured via the control ping, got {}",
        client.world().resource::<ClientStats>().rtt
    );
    // Cumulative totals prove payload actually flowed in BOTH directions; a
    // windowed rate can legitimately be 0 during an idle second (replicon only
    // sends mutations for changed values), which is why the exact measurement
    // reads these counters.
    let client_traffic = *client.world().resource::<BackendTraffic>();
    assert!(
        client_traffic.received_bytes > 0,
        "client received bytes, got {client_traffic:?}"
    );
    assert!(
        client_traffic.sent_bytes > 0,
        "client sent bytes, got {client_traffic:?}"
    );
    assert!(
        client_traffic.received_bps > 0.0 && client_traffic.sent_bps > 0.0,
        "client windowed rates, got {client_traffic:?}"
    );
    let server_traffic = *server
        .world()
        .get::<ClientTraffic>(client_entity)
        .expect("ClientTraffic component");
    assert!(
        server_traffic.sent_bytes > 0,
        "server sent bytes to the client, got {server_traffic:?}"
    );
    assert!(
        server_traffic.received_bytes > 0,
        "server received bytes from the client, got {server_traffic:?}"
    );

    // A `DisconnectRequest` ends the session with a control deny.
    server
        .world_mut()
        .write_message(DisconnectRequest {
            client: client_entity,
        });
    let mut disconnected = false;
    for _ in 0..100 {
        pump(&mut server, &mut client, 1);
        if server.world().get_entity(client_entity).is_err() {
            disconnected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(disconnected, "DisconnectRequest despawns the client entity");
}


// ---------------------------------------------------------------------------
// Lazy client startup / reconnect (no server needed)
// ---------------------------------------------------------------------------

use crate::transport::ClientTransport;

fn unreachable_client_config(token: &str) -> ClientBackendConfig {
    ClientBackendConfig {
        // Nothing listens here; we only assert on local resource/time state.
        signaling_url: "ws://127.0.0.1:9/party-dungeon".to_string(),
        token: token.to_string(),
    }
}

fn no_ice() -> BackendIceServer {
    BackendIceServer(matchbox_socket::RtcIceServerConfig {
        urls: Vec::new(),
        username: None,
        credential: None,
    })
}

#[test]
fn client_waits_for_config_before_creating_the_socket() {
    // The in-game login inserts the config only after the user signs in, so
    // the backend must not dial the default URL (or send an empty token) at
    // boot.
    let mut client = client_app();
    for _ in 0..3 {
        client.update();
    }
    assert!(
        client.world().get_resource::<ClientTransport>().is_none(),
        "no socket before a ClientBackendConfig exists"
    );
    assert_eq!(
        client.world().resource::<BackendStatus>().state,
        BackendState::Idle
    );
    assert!(!client_reached_connected(&client));

    // Login happened: config + ICE are inserted (same frame), the next update
    // dials the real URL with the real token.
    client.insert_resource(unreachable_client_config("tok"));
    client.insert_resource(no_ice());
    client.update();
    assert!(
        client.world().get_resource::<ClientTransport>().is_some(),
        "socket is created once the config exists"
    );
    assert_ne!(
        client.world().resource::<BackendStatus>().state,
        BackendState::Idle,
        "the backend leaves Idle once it has a socket"
    );
}

#[test]
fn reconnect_request_and_config_change_recreate_the_socket() {
    let mut client = client_app();
    client.insert_resource(unreachable_client_config("first"));
    client.insert_resource(no_ice());
    client.update();
    assert!(client.world().get_resource::<ClientTransport>().is_some());

    // Explicit request: the socket is replaced (a fresh generation) so the
    // handshake runs again from the current config.
    let generation = |client: &App| {
        client
            .world()
            .get_resource::<ClientTransport>()
            .expect("socket")
            .generation()
    };
    let before = generation(&client);
    client.world_mut().write_message(ReconnectRequest);
    client.update();
    assert_ne!(
        generation(&client),
        before,
        "a reconnect request replaces the socket"
    );

    // Replacing the config (e.g. a refreshed token) reconnects as well.
    let before = generation(&client);
    client.insert_resource(unreachable_client_config("second"));
    client.update();
    assert_ne!(
        generation(&client),
        before,
        "a changed config replaces the socket"
    );
}
