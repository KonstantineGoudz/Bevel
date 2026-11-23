//! End-to-end probe for the authoritative world-edit path.
//!
//! Joins as a player, sends a `ClientMsg::WorldEdit`, and verifies the sim's
//! answer + the streamed echo. Useful after a deploy and for persistence tests:
//!
//! ```sh
//! # place a block at (100, 0) and wait for the streamed cell to contain it
//! cargo run -p party-dungeon-sim --example world_edit_probe -- ws://127.0.0.1:6536/party-dungeon author place
//! # another client can observe a persisted piece by rid
//! cargo run -p party-dungeon-sim --example world_edit_probe -- ws://... observer observe dungeon_piece:xyz
//! # delete it again (cleanup; also exercises the delete path)
//! cargo run -p party-dungeon-sim --example world_edit_probe -- ws://... author delete dungeon_piece:xyz
//! ```
//!
//! Prints `RID <rid>` on success so a shell test can capture it. The token is
//! an unsigned JWT (the sim only decodes claims at the join gate), and it
//! leaves a `player:<sub>` row plus (until deleted) one piece in the store.

use std::time::{Duration, Instant};

use matchbox_socket::{ChannelConfig, WebRtcSocketBuilder};
use party_dungeon_net::{
    decode, encode, ClientMsg, ServerMsg, WorldOp, WorldPiece, CHANNEL_RELIABLE, CHANNEL_UNRELIABLE,
};

const BASE64URL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64url(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..[4, 2, 3, 4][chunk.len()] {
            out.push(BASE64URL[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
        }
    }
    out
}

fn fake_jwt(sub: &str) -> String {
    let header = b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload =
        format!(r#"{{"sub":"{sub}","iss":"world-edit-probe","aud":"party-dungeon","exp":4102444800}}"#);
    format!("{header}.{}.sig", b64url(payload.as_bytes()))
}

fn block(rid: &str, x: f32, z: f32) -> WorldPiece {
    WorldPiece {
        rid: rid.to_string(),
        shape: "block".to_string(),
        x,
        z,
        elevation: 0.0,
        rot_deg: 0.0,
        pitch_deg: 0.0,
        roll_deg: 0.0,
        dynamic: false,
        damage: 0.0,
        interact: "none".to_string(),
        trigger_w: 1.0,
        trigger_h: 1.0,
        trigger_l: 1.0,
        trigger_alpha: 0.35,
    }
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let args: Vec<String> = std::env::args().collect();
    let url = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "ws://127.0.0.1:6536/party-dungeon".to_string());
    let sub = args.get(2).cloned().unwrap_or_else(|| "editprobe".to_string());
    let mode = args.get(3).cloned().unwrap_or_else(|| "place".to_string());
    let target_rid = args.get(4).cloned();
    println!("signaling: {url}");
    println!("player:    {sub}  mode: {mode}");

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

    let timeout = Duration::from_secs(30);
    let start = Instant::now();
    let mut hello_sent = false;
    let mut sent = false;
    let mut rid: Option<String> = None;
    let (mut placed_echo, mut deleted_echo) = (false, false);
    let mut input_tick = 0u32;

    while start.elapsed() < timeout {
        let _ = socket.try_update_peers();
        let host = socket.connected_peers().next();
        if let (Some(peer), false) = (host, hello_sent) {
            let hello = ClientMsg::Hello {
                token: fake_jwt(&sub),
                name: Some(sub.clone()),
            };
            let _ = socket
                .get_channel_mut(CHANNEL_RELIABLE)
                .unwrap()
                .try_send(encode(&hello).into(), peer);
            hello_sent = true;
        }
        if let (Some(peer), true, false) = (host, hello_sent, sent) {
            let ops = match mode.as_str() {
                "place" => vec![WorldOp::Place {
                    tag: format!("probe-{}", sub),
                    piece: block("", 100.0, 0.0),
                }],
                "update" => vec![WorldOp::Update {
                    piece: block(target_rid.as_deref().unwrap_or(""), 104.0, 0.0),
                }],
                "delete" => vec![WorldOp::Delete {
                    rid: target_rid.clone().unwrap_or_default(),
                }],
                "observe" => Vec::new(),
                other => {
                    eprintln!("unknown mode {other} (place|update|delete|observe)");
                    std::process::exit(2);
                }
            };
            if !ops.is_empty() {
                let msg = ClientMsg::WorldEdit { ops };
                if socket
                    .get_channel_mut(CHANNEL_RELIABLE)
                    .unwrap()
                    .try_send(encode(&msg).into(), peer)
                    .is_ok()
                {
                    sent = true;
                    println!("sent WorldEdit ({mode})");
                }
            }
            let interest = ClientMsg::SetInterest {
                world_radius: None,
                player_radius: None,
            };
            let _ = socket
                .get_channel_mut(CHANNEL_RELIABLE)
                .unwrap()
                .try_send(encode(&interest).into(), peer);
        }
        if let Some(peer) = host {
            input_tick = input_tick.wrapping_add(1);
            let msg = ClientMsg::Input(party_dungeon_net::Input::idle(input_tick));
            let _ = socket
                .get_channel_mut(CHANNEL_RELIABLE)
                .unwrap()
                .try_send(encode(&msg).into(), peer);
        }

        for (_peer, bytes) in socket.get_channel_mut(CHANNEL_RELIABLE).unwrap().receive() {
            match decode::<ServerMsg>(&bytes) {
                Ok(ServerMsg::Welcome { tick, .. }) => {
                    input_tick = tick;
                    println!("Welcome (tick {tick})");
                }
                Ok(ServerMsg::WorldEditApplied { tag, rid: new_rid }) => {
                    println!("WorldEditApplied tag={tag} rid={new_rid}");
                    rid = Some(new_rid);
                }
                Ok(ServerMsg::WorldEditRejected { key, reason }) => {
                    println!("WorldEditRejected key={key} reason={reason}");
                    std::process::exit(1);
                }
                Ok(ServerMsg::WorldChunk(chunk)) => match mode.as_str() {
                    "place" | "update" => {
                        if let Some(wanted) = rid.as_deref().or(target_rid.as_deref())
                            && let Some(piece) =
                                chunk.pieces.iter().find(|p| p.rid == wanted)
                        {
                            println!(
                                "echo: cell {:?} contains {wanted} shape={} trigger_alpha={}",
                                chunk.id, piece.shape, piece.trigger_alpha
                            );
                            placed_echo = true;
                        }
                    }
                    "delete" => {
                        if let Some(wanted) = target_rid.as_deref()
                            && !chunk.pieces.iter().any(|p| p.rid == wanted)
                        {
                            println!("echo: cell {:?} no longer contains {wanted}", chunk.id);
                            deleted_echo = true;
                        }
                    }
                    "observe" => {
                        if let Some(wanted) = target_rid.as_deref()
                            && let Some(piece) =
                                chunk.pieces.iter().find(|p| p.rid == wanted)
                        {
                            println!(
                                "echo: cell {:?} contains {wanted} shape={} trigger_alpha={}",
                                chunk.id, piece.shape, piece.trigger_alpha
                            );
                            placed_echo = true;
                        }
                    }
                    _ => {}
                },
                Ok(ServerMsg::DungeonChanged { dungeon_version }) => {
                    println!("DungeonChanged -> {dungeon_version}");
                }
                Ok(ServerMsg::WorldUnload { cells }) => {
                    println!("WorldUnload {} cell(s)", cells.len());
                }
                Ok(ServerMsg::Kill { target, by }) => println!("Kill {target} by {by}"),
                Ok(ServerMsg::Deny { reason }) => {
                    println!("DENIED: {reason}");
                    std::process::exit(1);
                }
                Err(error) => println!("undecodable ServerMsg: {error}"),
            }
        }
        let done = match mode.as_str() {
            "place" | "update" | "observe" => placed_echo,
            "delete" => deleted_echo,
            _ => false,
        };
        if done {
            if let Some(rid) = rid.as_deref().or(target_rid.as_deref()) {
                println!("RID {rid}");
            }
            println!("OK: {mode} visible in the streamed world");
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    println!("FAIL: timed out waiting for the {mode} echo");
    std::process::exit(1);
}
