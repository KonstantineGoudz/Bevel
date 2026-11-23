//! Does a client actually reach the sim host over WebRTC?
//!
//! Signaling can look perfectly healthy (`WebSocket` open, id assigned) while the
//! peer connection never completes — that is exactly the state the game shows as
//! "signaling is reachable but the world server is not in the room yet". This
//! probe reports which layer fails, without needing a browser:
//!
//! ```sh
//! cargo run -p party-dungeon-sim --example signal_probe
//! cargo run -p party-dungeon-sim --example signal_probe -- wss://play.conman.biz/party-dungeon
//! ```

use std::time::{Duration, Instant};

use matchbox_socket::{ChannelConfig, WebRtcSocketBuilder};

#[tokio::main]
async fn main() {
    // Several crates in the workspace link rustls, so rustls 0.23 cannot pick a
    // process-level `CryptoProvider` on its own. Without this a `wss://` probe
    // panics before it connects (the game itself is fine: the browser provides
    // its own TLS stack).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:6536/party-dungeon".to_string());
    println!("signaling: {url}");

    let mut builder = WebRtcSocketBuilder::new(url)
        .add_channel(ChannelConfig::reliable())
        .add_channel(ChannelConfig::unreliable());
    // Exercise the same relay configuration the sim/client use, so this probe
    // can also tell whether a TURN relay is reachable from here.
    if let Some(ice_server) = party_dungeon_sim::net::configured_ice_server() {
        builder = builder.ice_server(ice_server);
    }
    let (mut socket, fut) = builder.build();
    tokio::spawn(async move {
        if let Err(err) = fut.await {
            eprintln!("signaling socket ended: {err:?}");
        }
    });

    let start = Instant::now();
    let mut last_id = false;
    while start.elapsed() < Duration::from_secs(20) {
        for (peer, state) in socket.update_peers() {
            println!("  peer state: {peer:?} -> {state:?}");
        }
        let has_id = socket.id().is_some();
        if has_id != last_id {
            println!("  signaling id assigned: {has_id}");
            last_id = has_id;
        }
        if socket.connected_peers().next().is_some() {
            println!("CONNECTED: the host peer is reachable");
            println!("  elapsed: {:.1}s", start.elapsed().as_secs_f32());
            for (peer, bytes) in socket.get_channel_mut(0).unwrap().receive() {
                println!("  first reliable packet from {peer:?}: {} bytes", bytes.len());
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!(
        "NO HOST PEER after 20s (id assigned: {}) — signaling works, WebRTC does not",
        socket.id().is_some()
    );
}
