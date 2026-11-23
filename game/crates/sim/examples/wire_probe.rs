//! End-to-end wire smoke test for a deployed sim.
//!
//! `signal_probe` proves the WebRTC data channel opens; this probe proves the
//! *protocol* on top of it works: it joins as a player, sends `Hello` and
//! `SetInterest`, and reports the `Welcome`, the streamed `WorldChunk`s and the
//! per-receiver `Snapshot`. Use it after deploying the sim (or when a browser
//! is not available):
//!
//! ```sh
//! cargo run -p party-dungeon-sim --example wire_probe -- ws://127.0.0.1:6536/party-dungeon wireprobe
//! cargo run -p party-dungeon-sim --example wire_probe -- wss://play.conman.biz/party-dungeon wireprobe
//! ```
//!
//! The token is an *unsigned* JWT: the sim only decodes claims at the join gate
//! (SurrealDB row rules verify signatures on every write), so a dev probe can
//! authenticate with any `sub`. It creates a `player:<sub>` row; delete it after
//! a production test (`DELETE player:wireprobe;` in SurrealDB).

use std::time::{Duration, Instant};

use matchbox_socket::{ChannelConfig, WebRtcSocketBuilder};
use party_dungeon_net::{
    decode, encode, ClientMsg, ServerMsg, Snapshot, CHANNEL_RELIABLE, CHANNEL_UNRELIABLE,
};

const BASE64URL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let take = [4, 2, 3, 4][chunk.len()];
        for i in 0..take {
            let index = (n >> (18 - 6 * i)) & 0x3F;
            out.push(BASE64URL[index as usize] as char);
        }
    }
    out
}

fn fake_jwt(sub: &str) -> String {
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = format!(
        r#"{{"sub":"{sub}","iss":"wire-probe","aud":"party-dungeon","exp":4102444800}}"#
    );
    format!("{header}.{}.sig", b64url(payload.as_bytes()))
}

#[tokio::main]
async fn main() {
    // A `wss://` URL needs a process-level rustls provider (several crates
    // link rustls in this workspace).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args: Vec<String> = std::env::args().collect();
    let url = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "ws://127.0.0.1:6536/party-dungeon".to_string());
    let sub = args.get(2).cloned().unwrap_or_else(|| "wireprobe".to_string());
    let watch_secs: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let move_forward = args.get(5).map(|s| s == "move").unwrap_or(false);
    println!("signaling: {url}");
    println!("player:    {sub}");
    if watch_secs > 0 {
        println!("watching for {watch_secs}s after first success");
    }

    let mut builder = WebRtcSocketBuilder::new(url)
        .add_channel(ChannelConfig::reliable())
        .add_channel(ChannelConfig::unreliable());
    if let Some(ice_server) = party_dungeon_sim::net::configured_ice_server() {
        builder = builder.ice_server(ice_server);
    }
    let (mut socket, fut) = builder.build();
    tokio::spawn(async move {
        if let Err(error) = fut.await {
            eprintln!("signaling socket ended: {error:?}");
        }
    });

    let timeout = Duration::from_secs(25);
    let start = Instant::now();
    let mut hello_sent = false;
    let mut interest_sent = false;
    let mut welcome = false;
    let mut chunks = 0usize;
    let mut pieces = 0usize;
    let mut snapshots = 0usize;
    let mut self_seen = false;
    let mut denied = false;
    let mut latest_subs: Vec<String> = Vec::new();
    let mut success_at: Option<Instant> = None;
    // The sim marks a player offline when no input has arrived for
    // `INPUT_TIMEOUT_TICKS`; a real client sends one every tick, so the probe
    // must too (its start tick is whatever `Welcome` reported).
    let mut input_tick: u32 = 0;
    let mut inputs_sent = 0usize;

    while start.elapsed() < timeout {
        let _ = socket.try_update_peers();
        let host = socket.connected_peers().next();

        if let Some(peer) = host
            && !hello_sent
        {
            let hello = ClientMsg::Hello {
                token: fake_jwt(&sub),
                name: Some(sub.clone()),
            };
            let _ = socket
                .get_channel_mut(CHANNEL_RELIABLE)
                .unwrap()
                .try_send(encode(&hello).into(), peer);
            hello_sent = true;
            println!("sent Hello");
        }

        for (_peer, bytes) in socket.get_channel_mut(CHANNEL_RELIABLE).unwrap().receive() {
            match decode::<ServerMsg>(&bytes) {
                Ok(ServerMsg::Welcome { sub: who, tick, dungeon_version }) => {
                    welcome = true;
                    input_tick = tick;
                    println!("Welcome: sub={who} tick={tick} dungeon_version={dungeon_version}");
                    if let (Some(peer), false) = (host, interest_sent) {
                        // Optional 5th arg: player radius ("all" = None). The
                        // browser sends `Some(DEFAULT_PLAYER_RADIUS)`, so be
                        // able to exercise the same filter branch.
                        let player_radius = match args.get(4).map(String::as_str) {
                            Some("all") | None => None,
                            Some(raw) => raw.parse::<f32>().ok(),
                        };
                        let msg = ClientMsg::SetInterest {
                            world_radius: None,
                            player_radius,
                        };
                        let _ = socket
                            .get_channel_mut(CHANNEL_RELIABLE)
                            .unwrap()
                            .try_send(encode(&msg).into(), peer);
                        interest_sent = true;
                        println!("sent SetInterest(full world)");
                    }
                }
                Ok(ServerMsg::WorldChunk(chunk)) => {
                    chunks += 1;
                    pieces += chunk.pieces.len();
                    if chunks <= 3 {
                        println!(
                            "WorldChunk {:?} v{} pieces={} paths={}",
                            chunk.id,
                            chunk.version,
                            chunk.pieces.len(),
                            chunk.paths.len()
                        );
                    }
                }
                Ok(ServerMsg::WorldUnload { cells }) => println!("WorldUnload {} cell(s)", cells.len()),
                Ok(ServerMsg::Kill { target, by }) => println!("Kill {target} by {by}"),
                Ok(ServerMsg::DungeonChanged { dungeon_version }) => {
                    println!("DungeonChanged -> {dungeon_version}");
                }
                Ok(ServerMsg::WorldEditApplied { tag, rid }) => {
                    println!("WorldEditApplied tag={tag} rid={rid}");
                }
                Ok(ServerMsg::WorldEditRejected { key, reason }) => {
                    println!("WorldEditRejected key={key} reason={reason}");
                }
                Ok(ServerMsg::Deny { reason }) => {
                    denied = true;
                    println!("DENIED: {reason}");
                    break;
                }
                Err(error) => println!("undecodable ServerMsg: {error}"),
            }
        }

        if welcome
            && let Some(peer) = host
        {
            input_tick = input_tick.wrapping_add(1);
            let mut input = party_dungeon_net::Input::idle(input_tick);
            if move_forward {
                // Wish direction is camera-relative in the real client; yaw 0
                // makes +mz forward here, which is all a movement test needs.
                input.mz = 1.0;
            }
            let msg = ClientMsg::Input(input);
            let _ = socket
                .get_channel_mut(CHANNEL_RELIABLE)
                .unwrap()
                .try_send(encode(&msg).into(), peer);
            inputs_sent += 1;
        }

        for (_peer, bytes) in socket.get_channel_mut(CHANNEL_UNRELIABLE).unwrap().receive() {
            if let Ok(snapshot) = decode::<Snapshot>(&bytes) {
                snapshots += 1;
                if snapshots <= 2 {
                    let subs: Vec<&str> =
                        snapshot.players.iter().map(|player| player.sub.as_str()).collect();
                    println!(
                        "Snapshot tick={} capacity={} players={} subs={subs:?}",
                        snapshot.tick,
                        snapshot.players.capacity(),
                        snapshot.players.len()
                    );
                }
                latest_subs = snapshot
                    .players
                    .iter()
                    .map(|player| player.sub.clone())
                    .collect();
                if snapshot.tick % 60 == 0 {
                    let positions: Vec<String> = snapshot
                        .players
                        .iter()
                        .map(|p| format!("{}@({:.2},{:.2},{:.2})", p.sub, p.x, p.y, p.z))
                        .collect();
                    println!("positions t={} {}", snapshot.tick, positions.join(" "));
                }
                if snapshot.players.iter().any(|player| player.sub == sub) {
                    self_seen = true;
                }
            }
        }

        if welcome && chunks > 0 && snapshots > 0 && self_seen {
            if success_at.is_none() {
                success_at = Some(Instant::now());
                if watch_secs == 0 {
                    break;
                }
            }
            if let Some(since) = success_at
                && since.elapsed() >= Duration::from_secs(watch_secs)
            {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    println!("---");
    println!(
        "welcome={welcome} chunks={chunks} pieces={pieces} snapshots={snapshots} \
         inputs={inputs_sent} self_in_snapshot={self_seen} denied={denied}"
    );
    println!("players in latest snapshot: {latest_subs:?}");
    let ok = welcome && chunks > 0 && snapshots > 0 && self_seen && !denied;
    println!(
        "{}",
        if ok {
            "OK: deployed sim streams Welcome + WorldChunk + a filtered Snapshot (self included)"
        } else {
            "FAIL: see the counters above"
        }
    );
    if !ok {
        std::process::exit(1);
    }
}
