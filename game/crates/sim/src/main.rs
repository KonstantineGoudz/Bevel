//! Standalone authoritative sim server (binary entry point).

use std::net::SocketAddr;
use std::time::Duration;

use party_dungeon_net::TICK_DT;
use party_dungeon_server::SurrealConfig;

use party_dungeon_sim::bridge;
use party_dungeon_sim::net::{host_socket, signaling_server, HostNet};
use party_dungeon_sim::sim::Sim;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let surreal = SurrealConfig::from_env().ok();
    let service_user = std::env::var("DUNGEON_SERVICE_USER").unwrap_or_else(|_| "service".to_string());
    let service_pass = std::env::var("DUNGEON_SERVICE_PASS").unwrap_or_else(|_| "change-me-in-prod".to_string());
    let signal_addr: SocketAddr = std::env::var("DUNGEON_SIGNAL_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:6536".to_string())
        .parse()?;
    let signal_url = std::env::var("DUNGEON_SIGNAL_URL")
        .unwrap_or_else(|_| format!("ws://{}/party-dungeon", signal_addr));

    let mut sim = Sim::new();
    if let Some(cfg) = &surreal {
        match bridge::load_dungeon(&mut sim, cfg, &service_user, &service_pass).await {
            Ok(()) => eprintln!("sim: dungeon loaded, version {}", sim.dungeon_version),
            Err(e) => eprintln!("sim: surreal unavailable (dev mode): {e}"),
        }
    } else {
        eprintln!("sim: no SurrealDB config (set DUNGEON_AUTH_PROVIDER + provider vars — see party-dungeon-server/src/auth.rs), running empty dungeon");
    }

    // Signaling server (ClientServer topology: first peer becomes host).
    let signaling = signaling_server(signal_addr);
    let signal_fut = signaling.serve();
    tokio::spawn(async move {
        if let Err(e) = signal_fut.await {
            eprintln!("signaling error: {e}");
        }
    });
    eprintln!("sim: signaling on {signal_addr}");

    // Self-service signup (in-game form -> Rauthy admin API). Runs alongside
    // the sim; fails open (logs) if the port is taken so the sim still runs.
    let signup_cfg = party_dungeon_sim::signup::SignupConfig::from_env();
    tokio::spawn(async move {
        if let Err(e) = party_dungeon_sim::signup::serve(signup_cfg).await {
            eprintln!("signup server error: {e}");
        }
    });

    // bevy_replicon path (feature `replicon` + DUNGEON_REPLICON=1). Same
    // authority (Sim, DB, 60Hz) and the same signaling room; only the transport
    // and the per-client replication change. The legacy path stays the default
    // and is untouched below.
    #[cfg(feature = "replicon")]
    if party_dungeon_sim::repl::enabled() {
        // The matchbox backend dials the signaling URL as the room host, so an
        // unspecified bind address (0.0.0.0) must be reached over loopback.
        let repl_signal_url = std::env::var("DUNGEON_SIGNAL_URL").unwrap_or_else(|_| {
            let host = if signal_addr.ip().is_unspecified() {
                "127.0.0.1".to_string()
            } else {
                signal_addr.ip().to_string()
            };
            format!("ws://{host}:{}/party-dungeon", signal_addr.port())
        });
        eprintln!("sim: replicon replication path (DUNGEON_REPLICON=1)");
        let authority = party_dungeon_sim::authority::Authority::new(
            surreal.clone(),
            service_user.clone(),
            service_pass.clone(),
        );
        let repl_config =
            party_dungeon_sim::repl::ReplServerConfig::from_env(repl_signal_url.clone());
        eprintln!(
            "sim: replicon server (signaling {}, world_radius {}, player_radius {})",
            repl_config.signaling_url, repl_config.world_radius, repl_config.player_radius
        );
        party_dungeon_sim::repl::run_authority_loop(sim, authority, repl_config).await?;
        return Ok(());
    }
    #[cfg(not(feature = "replicon"))]
    if std::env::var("DUNGEON_REPLICON")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
    {
        eprintln!(
            "sim: DUNGEON_REPLICON=1 but this binary was built without the `replicon` feature; using the legacy path"
        );
    }

    let (socket, socket_fut) = host_socket(signal_url.clone());
    tokio::spawn(socket_fut);
    eprintln!("sim: host socket connecting to {signal_url}");

    let mut net = HostNet::new(
        sim,
        socket,
        surreal.clone(),
        service_user.clone(),
        service_pass.clone(),
    );

    // 60Hz fixed loop.
    let mut tick_interval = tokio::time::interval(Duration::from_secs_f32(TICK_DT));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
    let last_poll = std::time::Instant::now();
    loop {
        tick_interval.tick().await;
        net.poll().await;

        let events = net.step();
        for ev in &events {
            match ev {
                party_dungeon_sim::sim::SimEvent::Kill { target, by } => {
                    eprintln!("sim: kill {target} by {by} at tick {}", net.sim.tick);
                    // Persist kill to SurrealDB feed (best effort).
                    if let Some(cfg) = &surreal {
                        let cfg2 = cfg.clone();
                        let t = target.clone();
                        let b = by.clone();
                        let tick = net.sim.tick;
                        let svc_u = service_user.clone();
                        let svc_p = service_pass.clone();
                        tokio::spawn(async move {
                            if let Ok(backend) =
                                party_dungeon_server::Backend::connect_service(&cfg2, &svc_u, &svc_p).await
                            {
                                let _ = backend.log_death(&b, &t, tick as i64).await;
                            }
                        });
                    }
                    net.broadcast_reliable(&party_dungeon_net::ServerMsg::Kill {
                        target: target.clone(),
                        by: by.clone(),
                    });
                }
                party_dungeon_sim::sim::SimEvent::Respawn { sub } => {
                    eprintln!("sim: respawn {sub}");
                }
                party_dungeon_sim::sim::SimEvent::InteractToggled { piece_rid, paused } => {
                    eprintln!("sim: toggle {piece_rid} paused={paused}");
                    // Version already bumped in Sim::step; the streaming tick
                    // below re-sends only the mover's cell.
                }
            }
        }

        // Coarse "dungeon changed" event once per version bump (HUD/logging);
        // the changed cells follow as WorldChunk messages on this same
        // reliable, ordered channel.
        net.notify_dungeon_changed();
        // Level streaming: rebuild the chunk index on dungeon version bumps,
        // then send each peer its queued cell diffs (bounded per tick) and
        // batched unloads for cells that left its interest radius.
        net.stream_world();

        // One filtered snapshot per peer (players around it, self included).
        let now_ms = party_dungeon_server::now_ms();
        net.broadcast(now_ms);

        // Poll for dungeon edits every ~2 s.
        if net.sim.tick % 120 == 0 {
            if let Some(cfg) = &surreal {
                let mut ver = net.sim.dungeon_version;
                if bridge::poll_edits(&mut net.sim, cfg, &service_user, &service_pass, &mut ver).await {
                    eprintln!("sim: dungeon changed -> version {}", net.sim.dungeon_version);
                }
            }
        }
        // Simple idle handling: if no inputs for a while, we already mark
        // players offline inside Sim::step.
        let _ = last_poll;
    }
}
