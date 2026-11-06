//! Live bridge for the MCP server: a tiny localhost HTTP endpoint that
//! exposes the running editor's level and applies commands to it.
//!
//! Enabled on native builds when `PARTY_DUNGEON_BRIDGE` is set (`1` means the
//! default `127.0.0.1:3559`, any other value is used as the bind address,
//! `0`/empty disables). The endpoint only ever binds loopback-style
//! addresses; anything else is refused.
//!
//! - `GET /health` — `{"ok":true,...}` liveness probe.
//! - `GET /state` — the live session [`Snapshot`] as JSON.
//! - `POST /command` — `{"op": ...}`; the reply is `{"ok":true,"note":...}`
//!   or `{"ok":false,"error":...}` once the next frame has applied it.
//!
//! The MCP server prefers this when it is reachable and falls back to level
//! files otherwise, so file edits and live edits converge.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::*;
use crate::platforms::PlatformMode;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct Envelope {
    body: serde_json::Value,
    reply: mpsc::Sender<String>,
}

#[derive(Default)]
struct LiveState {
    snapshot: Option<String>,
}

#[derive(Resource)]
pub(super) struct Bridge {
    inbox: Arc<Mutex<Vec<Envelope>>>,
    state: Arc<Mutex<LiveState>>,
    level_path: Option<std::path::PathBuf>,
    last_publish: Instant,
}

fn bridge_addr() -> Option<String> {
    match std::env::var("PARTY_DUNGEON_BRIDGE") {
        Ok(value) => {
            let trimmed = value.trim().to_owned();
            if trimmed.is_empty() || trimmed == "0" {
                None
            } else if trimmed == "1" {
                Some("127.0.0.1:3559".to_owned())
            } else {
                Some(trimmed)
            }
        }
        // On by default for localhost dev so the MCP server just works.
        Err(_) => Some("127.0.0.1:3559".to_owned()),
    }
}

fn loopback_only(addr: &str) -> bool {
    addr.starts_with("127.") || addr.starts_with("localhost") || addr.starts_with("::1")
}

/// Startup: bind the bridge and share the inbox/outbox with the game.
pub(super) fn setup_bridge(mut commands: Commands) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = &mut commands;
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(addr) = bridge_addr() else {
            return;
        };
        if !loopback_only(&addr) {
            warn!("mcp bridge: refusing non-loopback address {addr}");
            return;
        }
        let listener = match std::net::TcpListener::bind(&addr) {
            Ok(listener) => listener,
            Err(error) => {
                warn!("mcp bridge: could not bind {addr}: {error}");
                return;
            }
        };
        let inbox: Arc<Mutex<Vec<Envelope>>> = Arc::default();
        let state: Arc<Mutex<LiveState>> = Arc::default();
        let worker_inbox = Arc::clone(&inbox);
        let worker_state = Arc::clone(&state);
        if std::thread::Builder::new()
            .name("mcp-bridge".into())
            .spawn(move || serve(listener, worker_inbox, worker_state))
            .is_err()
        {
            warn!("mcp bridge: could not spawn worker thread");
            return;
        }
        info!("mcp bridge: listening on {addr}");
        commands.insert_resource(Bridge {
            inbox,
            state,
            level_path: session::level_file_from_env_or_args(),
            last_publish: Instant::now() - Duration::from_secs(10),
        });
    }
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.0 server (standard library only)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
fn read_request(
    stream: &mut std::net::TcpStream,
) -> Option<(String, String, Vec<u8>)> {
    use std::io::Read;

    let mut head = Vec::with_capacity(4096);
    let mut byte = [0u8; 1];
    // Read until the end of the headers; connections are tiny and local.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if head.len() > 1_048_576 {
            return None;
        }
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                head.push(byte[0]);
                if head.len() >= 4 && head[head.len() - 4..] == *b"\r\n\r\n" {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let mut content_length = 0usize;
    for line in lines {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length.min(8_388_608)];
    let mut read = 0;
    while read < body.len() {
        match stream.read(&mut body[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return None,
        }
    }
    body.truncate(read);
    Some((method, path, body))
}

#[cfg(not(target_arch = "wasm32"))]
fn respond(mut stream: std::net::TcpStream, status: &str, body: &str) {
    use std::io::Write;

    let _ = stream.write_all(
        format!(
            "HTTP/1.0 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn serve(
    listener: std::net::TcpListener,
    inbox: Arc<Mutex<Vec<Envelope>>>,
    state: Arc<Mutex<LiveState>>,
) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Some((method, path, body)) = read_request(&mut stream) else {
            continue;
        };
        match (method.as_str(), path.as_str()) {
            ("GET", "/health") => {
                respond(stream, "200 OK", r#"{"ok":true,"game":"party-dungeon"}"#);
            }
            ("GET", "/state") => {
                let snapshot = state
                    .lock()
                    .ok()
                    .and_then(|guard| guard.snapshot.clone());
                match snapshot {
                    Some(json) => respond(stream, "200 OK", &json),
                    None => respond(stream, "503 Busy", r#"{"ok":false,"error":"no snapshot yet"}"#),
                }
            }
            ("POST", "/command") => {
                let body = serde_json::from_slice::<serde_json::Value>(&body)
                    .unwrap_or(serde_json::Value::Null);
                let (tx, rx) = mpsc::channel();
                if inbox
                    .lock()
                    .map(|mut queue| {
                        queue.push(Envelope { body, reply: tx });
                    })
                    .is_err()
                {
                    respond(stream, "503 Busy", r#"{"ok":false,"error":"bridge busy"}"#);
                    continue;
                }
                // The game drains the inbox every frame; wait for its reply.
                match rx.recv_timeout(Duration::from_secs(8)) {
                    Ok(reply) => respond(stream, "200 OK", &reply),
                    Err(_) => respond(
                        stream,
                        "503 Busy",
                        r#"{"ok":false,"error":"game did not answer"}"#,
                    ),
                }
            }
            _ => respond(stream, "404 Not Found", r#"{"ok":false,"error":"unknown route"}"#),
        }
    }
}

// ---------------------------------------------------------------------------
// Frame systems
// ---------------------------------------------------------------------------

/// Publish the live snapshot for `GET /state`, throttled to 2 Hz (or the
/// moment a level edit lands).
pub(super) fn bridge_publish(
    bridge: Option<ResMut<Bridge>>,
    editor: Res<Editor>,
    orbit: Res<Orbit>,
    movers: Res<Movers>,
) {
    let Some(mut bridge) = bridge else {
        return;
    };
    let due = bridge.last_publish.elapsed() > Duration::from_millis(500);
    if !due && !editor.hard_dirty {
        return;
    }
    bridge.last_publish = Instant::now();
    let snapshot = editor.capture(orbit.as_ref(), movers.as_ref());
    if let Ok(mut guard) = bridge.state.lock() {
        guard.snapshot = Some(snapshot.to_json());
    }
}

fn ok_note(note: impl Into<String>) -> String {
    serde_json::json!({"ok": true, "note": note.into()}).to_string()
}

fn err(error: impl Into<String>) -> String {
    serde_json::json!({"ok": false, "error": error.into()}).to_string()
}

fn f32_field(value: &serde_json::Value, key: &str) -> Option<f32> {
    value.get(key).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn waypoints_from(body: &serde_json::Value) -> Result<Vec<Vec2>, String> {
    let items = body
        .get("waypoints")
        .and_then(|v| v.as_array())
        .ok_or("missing waypoints")?;
    items
        .iter()
        .map(|item| {
            let x = item.get("x").and_then(|v| v.as_f64());
            let z = item.get("z").and_then(|v| v.as_f64());
            match (x, z) {
                (Some(x), Some(z)) if x.is_finite() && z.is_finite() => {
                    Ok(Vec2::new(x as f32, z as f32))
                }
                _ => Err("waypoint x/z must be finite".to_owned()),
            }
        })
        .collect()
}

/// Drain queued bridge commands. Runs every frame; each command is applied to
/// the editor exactly like its in-game equivalent (undoable where levels
/// change) and answered over its reply channel.
#[allow(clippy::too_many_arguments)]
pub(super) fn bridge_drain(
    bridge: Option<Res<Bridge>>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
    mut orbit: ResMut<Orbit>,
    nav: Res<Nav>,
    rapier: ReadRapierContext,
    player: Query<Entity, With<CharacterVisual>>,
) {
    let Some(bridge) = bridge.as_ref() else {
        return;
    };
    let pending: Vec<Envelope> = bridge
        .inbox
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default();
    if pending.is_empty() {
        return;
    }
    for envelope in pending {
        let reply = apply_command(
            &mut editor,
            &mut movers,
            &mut orbit,
            &nav,
            &rapier,
            &player,
            bridge.level_path.as_deref(),
            &envelope.body,
        );
        let _ = envelope.reply.send(reply);
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_command(
    editor: &mut Editor,
    movers: &mut Movers,
    orbit: &mut Orbit,
    nav: &Nav,
    rapier: &ReadRapierContext,
    player: &Query<Entity, With<CharacterVisual>>,
    level_path: Option<&std::path::Path>,
    body: &serde_json::Value,
) -> String {
    let op = body.get("op").and_then(|v| v.as_str()).unwrap_or("");
    match op {
        "ping" => ok_note("pong"),
        "status" => ok_note(format!(
            "mode={} pieces={} walk={} stuck={:.2} route={} valid={} nav={}/{} plan={} player={} rapier={} note={}",
            editor.mode.label(),
            editor.pieces.len(),
            editor
                .walk
                .map(|w| format!("{:.2}m", w.distance))
                .unwrap_or_else(|| "no".into()),
            editor.walk.map(|w| w.stuck).unwrap_or(-1.0),
            editor
                .route
                .as_ref()
                .map(|r| format!("{:.1}m", r.track.total()))
                .unwrap_or_else(|| "no".into()),
            editor
                .route
                .as_ref()
                .map(|r| r.valid.to_string())
                .unwrap_or_else(|| "-".into()),
            editor.seen_nav,
            nav.revision,
            editor
                .plan
                .as_ref()
                .map(|p| p.waypoints.len().to_string())
                .unwrap_or_else(|| "no".into()),
            if player.iter().next().is_some() {
                "yes"
            } else {
                "none"
            },
            if rapier.single().is_ok() { "yes" } else { "no" },
            editor.note,
        )),
        "add_piece" => add_piece(editor, body),
        "update_piece" => update_piece(editor, body),
        "delete_pieces" => delete_pieces(editor, movers, body),
        "set_character" => set_character(editor, body),
        "teleport" => teleport(editor, orbit, body),
        "damage" => change_hp(editor, -(body.get("amount").and_then(|v| v.as_f64()).unwrap_or(10.0) as f32)),
        "heal" => change_hp(editor, body.get("amount").and_then(|v| v.as_f64()).unwrap_or(25.0) as f32),
        "set_hp" => set_hp(editor, body),
        "respawn" => respawn(editor),
        "remove_character" => {
            editor.character = None;
            editor.soft_dirty = true;
            ok_note("character removed")
        }
        "set_platform" => set_platform(editor, movers, body),
        "clear_platform" => {
            let piece = body.get("piece").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            movers.map.remove(&piece);
            editor.hard_dirty = true;
            ok_note(format!("cleared platform on piece #{piece}"))
        }
        "walk_to" => walk_to(editor, nav, rapier, player, body),
        "stop" => {
            editor.walk = None;
            editor.note = "stopped";
            ok_note("stopped")
        }
        "reload" => reload(editor, orbit, movers, level_path),
        _ => err(format!("unknown op: {op}")),
    }
}

fn saved_piece_from(body: &serde_json::Value) -> Result<session::SavedPiece, String> {
    serde_json::from_value::<session::SavedPiece>(body.get("piece").cloned().unwrap_or_default())
        .map_err(|error| format!("bad piece: {error}"))
}

fn apply_outline_meta(editor: &mut Editor, id: u32, saved: &session::SavedPiece) {
    if let Some(name) = saved.name.as_deref() {
        editor.outline.set_label(id, name);
    }
    if let Some(group) = saved.group {
        if editor.outline.group(group).is_some() {
            editor.outline.membership.insert(id, group);
        }
    }
    if saved.locked {
        editor.outline.locked.insert(id);
    } else {
        editor.outline.locked.remove(&id);
    }
    editor.outline.bump();
}

fn add_piece(editor: &mut Editor, body: &serde_json::Value) -> String {
    let saved = match saved_piece_from(body) {
        Ok(saved) => saved,
        Err(error) => return err(error),
    };
    let mut piece = match session::piece_from_saved(&saved) {
        Ok(piece) => piece,
        Err(error) => return err(format!("invalid piece: {error:?}")),
    };
    if editor.pieces.iter().any(|p| p.id == piece.id) || piece.id == 0 {
        piece.id = editor.next_id.max(1);
    }
    editor.next_id = editor.next_id.max(piece.id.saturating_add(1)).max(1);
    editor.commit();
    editor.pieces.push(piece);
    apply_outline_meta(editor, piece.id, &saved);
    editor.selection.clear();
    editor.hard_dirty = true;
    ok_note(format!("added piece #{}", piece.id))
}

fn update_piece(editor: &mut Editor, body: &serde_json::Value) -> String {
    let id = body.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let saved = match saved_piece_from(body) {
        Ok(saved) => saved,
        Err(error) => return err(error),
    };
    let piece = match session::piece_from_saved(&saved) {
        Ok(piece) => piece,
        Err(error) => return err(format!("invalid piece: {error:?}")),
    };
    let Some(index) = editor.pieces.iter().position(|p| p.id == id) else {
        return err(format!("no piece with id {id}"));
    };
    editor.commit();
    let mut replaced = piece;
    replaced.id = id;
    editor.pieces[index] = replaced;
    apply_outline_meta(editor, id, &saved);
    editor.selection.clear();
    editor.hard_dirty = true;
    ok_note(format!("updated piece #{id}"))
}

fn delete_pieces(editor: &mut Editor, movers: &mut Movers, body: &serde_json::Value) -> String {
    let ids: Vec<u32> = body
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return err("missing ids");
    }
    editor.commit();
    let before = editor.pieces.len();
    editor.pieces.retain(|p| !ids.contains(&p.id));
    for id in &ids {
        movers.map.remove(id);
        editor.outline.labels.remove(id);
        editor.outline.membership.remove(id);
        editor.outline.locked.remove(id);
    }
    editor.outline.bump();
    if editor.selection.ids().iter().any(|id| ids.contains(id)) {
        editor.selection.clear();
    }
    editor.hard_dirty = true;
    ok_note(format!("deleted {} piece(s)", before - editor.pieces.len()))
}

fn set_character(editor: &mut Editor, body: &serde_json::Value) -> String {
    let saved = match body.get("character").cloned() {
        Some(value) => serde_json::from_value::<session::SavedCharacter>(value),
        None => serde_json::from_value::<session::SavedCharacter>(body.clone()),
    };
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => return err(format!("bad character: {error}")),
    };
    if ![saved.x, saved.z, saved.feet, saved.yaw_deg]
        .iter()
        .all(|v| v.is_finite())
    {
        return err("character coordinates must be finite");
    }
    let health = match (saved.hp, saved.max_hp) {
        (Some(hp), Some(max)) if hp.is_finite() && max.is_finite() => Health::new(hp, max),
        (Some(hp), None) if hp.is_finite() => Health::new(hp, Health::default().max),
        (None, Some(max)) if max.is_finite() => Health::full(max),
        _ => Health::default(),
    };
    editor.character = Some(Character {
        pos: Vec2::new(saved.x, saved.z),
        feet: saved.feet,
        yaw: saved.yaw_deg.to_radians(),
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: false,
        sliding: false,
        health,
    });
    editor.soft_dirty = true;
    ok_note(format!("character at ({:.2}, {:.2})", saved.x, saved.z))
}

fn teleport(editor: &mut Editor, orbit: &mut Orbit, body: &serde_json::Value) -> String {
    let x = f32_field(body, "x");
    let z = f32_field(body, "z");
    let feet = f32_field(body, "feet");
    let yaw_deg = f32_field(body, "yaw_deg");
    if [x, z, feet, yaw_deg]
        .into_iter()
        .flatten()
        .any(|v| !v.is_finite())
    {
        return err("coordinates must be finite");
    }
    let character = editor.character.get_or_insert(Character {
        pos: Vec2::ZERO,
        feet: 0.0,
        yaw: 0.0,
        vy: 0.0,
        hv: Vec2::ZERO,
        ride_vel: Vec2::ZERO,
        grounded: false,
        sliding: false,
        health: Health::default(),
    });
    if let Some(x) = x {
        character.pos.x = x;
    }
    if let Some(z) = z {
        character.pos.y = z;
    }
    if let Some(feet) = feet {
        character.feet = feet;
    }
    if let Some(yaw) = yaw_deg {
        character.yaw = yaw.to_radians();
    }
    character.vy = 0.0;
    character.hv = Vec2::ZERO;
    character.ride_vel = Vec2::ZERO;
    orbit.target = Vec3::new(character.pos.x, 0.0, character.pos.y);
    editor.soft_dirty = true;
    ok_note(format!(
        "teleported to ({:.2}, {:.2})",
        character.pos.x, character.pos.y
    ))
}

fn change_hp(editor: &mut Editor, amount: f32) -> String {
    let Some(character) = editor.character.as_mut() else {
        return err("no character");
    };
    if amount >= 0.0 {
        character.health.heal(amount);
    } else {
        character.health.damage(-amount);
    }
    editor.soft_dirty = true;
    ok_note(format!(
        "hp {:.0}/{:.0}",
        character.health.current, character.health.max
    ))
}

fn set_hp(editor: &mut Editor, body: &serde_json::Value) -> String {
    let Some(character) = editor.character.as_mut() else {
        return err("no character");
    };
    if let Some(max_hp) = f32_field(body, "max_hp") {
        if !max_hp.is_finite() || max_hp <= 0.0 {
            return err("max_hp must be positive");
        }
        character.health.set_max(max_hp);
    }
    if let Some(hp) = f32_field(body, "hp") {
        if !hp.is_finite() {
            return err("hp must be finite");
        }
        character.health.set_current(hp);
    }
    editor.soft_dirty = true;
    ok_note(format!(
        "hp {:.0}/{:.0}",
        character.health.current, character.health.max
    ))
}

fn respawn(editor: &mut Editor) -> String {
    let Some(character) = editor.character.as_mut() else {
        return err("no character");
    };
    character.health.revive();
    character.vy = 0.0;
    character.hv = Vec2::ZERO;
    editor.soft_dirty = true;
    ok_note(format!("respawned at {:.0} hp", character.health.current))
}

fn set_platform(editor: &mut Editor, movers: &mut Movers, body: &serde_json::Value) -> String {
    let platform = match body.get("platform").cloned() {
        Some(value) => serde_json::from_value::<session::SavedPlatform>(value),
        None => serde_json::from_value::<session::SavedPlatform>(body.clone()),
    };
    let platform = match platform {
        Ok(platform) => platform,
        Err(error) => return err(format!("bad platform: {error}")),
    };
    if platform.waypoints.is_empty() {
        return err("platform needs waypoints");
    }
    for point in &platform.waypoints {
        if ![point.x, point.z].iter().all(|v| v.is_finite()) {
            return err("platform waypoints must be finite");
        }
    }
    let Some(piece) = editor.pieces.iter().find(|p| p.id == platform.piece) else {
        return err(format!("no piece with id {}", platform.piece));
    };
    let start = piece.pos;
    let mut mover = Mover::new(start);
    mover.waypoints = platform
        .waypoints
        .iter()
        .map(|w| crate::platforms::PlatformWaypoint {
            pos: Vec2::new(w.x, w.z),
            incoming: w.incoming.map(|[x, y]| Vec2::new(x, y)),
            outgoing: w.outgoing.map(|[x, y]| Vec2::new(x, y)),
        })
        .collect();
    mover.mode = PlatformMode::parse(&platform.mode).unwrap_or_default();
    mover.speed = platform.speed.unwrap_or(1.5).clamp(0.5, 8.0);
    mover.paused = platform.paused;
    mover.rebuild();
    movers.map.insert(platform.piece, mover);
    editor.hard_dirty = true;
    ok_note(format!("platform path on piece #{}", platform.piece))
}

fn walk_to(
    editor: &mut Editor,
    nav: &Nav,
    rapier: &ReadRapierContext,
    player: &Query<Entity, With<CharacterVisual>>,
    body: &serde_json::Value,
) -> String {
    let points = match waypoints_from(body) {
        Ok(points) if !points.is_empty() => points,
        Ok(_) => return err("walk_to needs waypoints"),
        Err(error) => return err(error),
    };
    let Some(character) = editor.character else {
        return err("place a character first");
    };
    let height = character.feet;
    let mut plan = Plan::default();
    for point in points {
        let cell = route::resolve_cell(&editor.pieces, point, height);
        plan.append(point, cell, height);
    }
    editor.plan = Some(plan);
    let filter = player
        .iter()
        .next()
        .map(|entity| QueryFilter::default().exclude_rigid_body(entity))
        .unwrap_or_default();
    let Ok(rapier_ctx) = rapier.single() else {
        return err("physics is not ready");
    };
    if !route::rebuild_route(editor, nav, &rapier_ctx, filter, false) {
        return err("no route to that point");
    }
    editor.walk = Some(Walk {
        distance: 0.0,
        stuck: 0.0,
    });
    editor.note = "walking (bridge)";
    ok_note("walking")
}

fn reload(
    editor: &mut Editor,
    orbit: &mut Orbit,
    movers: &mut Movers,
    level_path: Option<&std::path::Path>,
) -> String {
    let Some(path) = level_path else {
        return err("no level file (set PARTY_DUNGEON_LEVEL or --level)");
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return err(format!("could not read {}: {error}", path.display())),
    };
    let loaded = match session::parse_any_snapshot_text(&text).and_then(|snapshot| snapshot.validate()) {
        Ok(loaded) => loaded,
        Err(error) => return err(format!("invalid level: {error:?}")),
    };
    editor.apply_loaded(orbit, movers, loaded);
    editor.level_path = Some(path.to_path_buf());
    editor.level_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned);
    editor.hard_dirty = false;
    editor.soft_dirty = false;
    ok_note(format!("reloaded {}", path.display()))
}
