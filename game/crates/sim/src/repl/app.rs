//! The embedded `bevy_replicon` server app (server role) and its thread.
//!
//! The app is deliberately tiny — a fixed-step `ScheduleRunnerPlugin` loop with
//! no renderer — and owns exactly two things: the replicated ECS entities and
//! the matchbox transport (through the backend plugin). The authoritative
//! simulation, the 60 Hz tick and SurrealDB stay on the Tokio thread; see
//! [`crate::repl::run`].
//!
//! Plugin order matters: `RepliconSharedPlugin` must be added with
//! [`AuthMethod::Custom`] before the backend plugin (which asserts it in
//! `finish()`), and `party_dungeon_replication::register` must run on this app
//! exactly once with the same registration order the client uses.

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::prelude::*;
use party_dungeon_repl_matchbox::{
    BackendAuthenticator, BackendIceServer, MatchboxServerPlugin, ServerBackendConfig,
};
use party_dungeon_replication::PieceData;

use crate::repl::backend_glue;
use crate::repl::entities::{
    self, DungeonMeta, ReplBridge, ReplConfig, SessionEntities, WorldEntities,
};
use crate::repl::protocol::{self, CommandReceiver, IntentSender};

/// How often the embedded server app updates (a touch above the 60 Hz
/// authority loop, so every authority frame is consumed promptly and replicon
/// gets a chance to send between frames).
const SERVER_APP_HZ: f32 = 62.5;

/// Settings for the embedded server app.
#[derive(Clone, Debug)]
pub struct ReplServerConfig {
    /// Signaling URL the backend dials as the room host. The room path must stay
    /// `/party-dungeon` (it is the matchbox room id and the NPM location).
    pub signaling_url: String,
    /// Largest single matchbox message (`ConnectedClient::max_size`).
    pub max_message_size: usize,
    /// World streaming radius in metres (`VisibleCells`).
    pub world_radius: f32,
    /// Player visibility radius in metres (informational here: replicon filters
    /// world cells; the legacy per-receiver player filtering used this knob).
    pub player_radius: f32,
}

impl ReplServerConfig {
    /// From the environment, with the same defaults/semantics as the legacy
    /// WebRTC path (`DUNGEON_SIGNAL_URL`, `DUNGEON_WORLD_RADIUS`, …).
    pub fn from_env(signaling_url: impl Into<String>) -> Self {
        Self {
            signaling_url: signaling_url.into(),
            max_message_size: ServerBackendConfig::default().max_message_size,
            world_radius: protocol::world_radius_from_env(),
            player_radius: protocol::player_radius_from_env(),
        }
    }
}

/// Channel endpoints handed to the app (and kept by the Tokio thread).
pub struct ReplServerChannels {
    /// Tokio → app commands.
    pub commands: CommandReceiver,
    /// App → Tokio intents.
    pub intents: IntentSender,
}

/// Build the embedded server app. Separated from [`spawn_server_thread`] so the
/// two-app in-process tests can drive the same app with the backend testkit.
pub fn build_server_app(
    config: ReplServerConfig,
    channels: ReplServerChannels,
    authenticator: Option<BackendAuthenticator>,
) -> App {
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f32(
            1.0 / SERVER_APP_HZ,
        ))),
        StatesPlugin,
        RepliconPlugins.set(RepliconSharedPlugin {
            auth_method: AuthMethod::Custom,
        }),
    ));

    // Authority bridge + interest configuration.
    app.insert_resource(ReplConfig {
        world_radius: config.world_radius,
        player_radius: config.player_radius,
    });
    app.insert_resource(ReplBridge {
        commands: std::sync::Mutex::new(channels.commands),
        intents: channels.intents,
    });
    app.init_resource::<WorldEntities>();
    app.init_resource::<SessionEntities>();
    app.init_resource::<DungeonMeta>();

    // Shared protocol registration (must match the client byte for byte).
    party_dungeon_replication::register(&mut app);

    // Matchbox backend: host socket + channel pump + JWT handshake.
    app.add_plugins(MatchboxServerPlugin);
    app.insert_resource(ServerBackendConfig {
        signaling_url: config.signaling_url,
        max_message_size: config.max_message_size,
    });
    // TURN/STUN parity with the legacy path: the same `DUNGEON_ICE_SERVERS`
    // JSON list (parsed and logged by `net::configured_ice_server`).
    if let Some(ice) = crate::net::configured_ice_server() {
        app.insert_resource(BackendIceServer(ice));
    }
    if let Some(authenticator) = authenticator {
        app.insert_resource(authenticator);
    }

    // Backend lifecycle → authority intents.
    app.add_observer(backend_glue::on_client_session_added);

    // Replicated client messages arrive in `ServerSystems::Receive` (PreUpdate);
    // forward them to Tokio once the backend has decoded them.
    app.add_systems(
        PreUpdate,
        entities::forward_inputs.after(ServerSystems::Receive),
    );
    app.add_systems(
        PreUpdate,
        entities::forward_world_edits.after(ServerSystems::Receive),
    );
    // Apply authority frames / replies, refresh interest sets, detect disconnects.
    app.add_systems(
        Update,
        (
            entities::apply_server_commands,
            entities::sync_visibility,
            entities::detect_disconnects,
        )
            .chain(),
    );

    app
}

/// Spawn the embedded server app on its own thread (native only: the app owns
/// the matchbox socket future through the backend plugin).
pub fn spawn_server_thread(
    config: ReplServerConfig,
    channels: ReplServerChannels,
    authenticator: Option<BackendAuthenticator>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let signaling_url = config.signaling_url.clone();
    std::thread::Builder::new()
        .name("repl-server".to_string())
        .spawn(move || {
            eprintln!("sim: replicon server app starting (signaling {signaling_url})");
            let mut app = build_server_app(config, channels, authenticator);
            app.run();
        })
}

/// Whether the replicon path should be used: the `replicon` feature must be
/// compiled in *and* `DUNGEON_REPLICON=1` must be set. The legacy path stays
/// the default and can be forced with `DUNGEON_REPLICON=0`.
///
/// (The function lives in `app` because that is the feature-gated module; it
/// reads the environment on every call, cheaply.)
pub fn enabled() -> bool {
    std::env::var("DUNGEON_REPLICON")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

/// Convenience: is this a piece delta we need to send? (Tests/clarity.)
pub fn piece_changed(previous: Option<&PieceData>, data: &PieceData) -> bool {
    previous != Some(data)
}
