//! Live integration tests. They need a running SurrealDB:
//!
//! ```sh
//! docker compose up -d            # from game/crates/server
//! ./scripts/bootstrap.sh          # once per database
//! cargo test -p party-dungeon-server
//! ```
//!
//! Each run uses a fresh unique database, so runs never interfere. Without a
//! reachable server the suite prints SKIP and passes — unit tests in
//! `src/` always run.

use party_dungeon_server::{
    new_piece_args, AuthConfig, Backend, BackendError, Event, LevelSnapshot, Op, Piece, SurrealConfig, TableName,
    Waypoint,
};

fn test_config(name: &str) -> SurrealConfig {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    SurrealConfig {
        url: std::env::var("DUNGEON_TEST_URL").unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string()),
        namespace: "test".to_string(),
        database: format!("t_{}_{safe}", std::process::id()),
        firebase_project_id: "live-test-project".to_string(),
        auth: AuthConfig::firebase_for_tests("live-test-project".to_string()),
    }
}

fn test_user() -> (String, String) {
    (
        std::env::var("DUNGEON_TEST_USER").unwrap_or_else(|_| "root".to_string()),
        std::env::var("DUNGEON_TEST_PASS").unwrap_or_else(|_| "devrootpass".to_string()),
    )
}

async fn live_backend(name: &str) -> Option<Backend> {
    let config = test_config(name);
    let (user, pass) = test_user();
    match Backend::connect_root(&config, &user, &pass).await {
        Ok(backend) => {
            if let Err(e) = backend.apply_schema(&schema_text()).await {
                eprintln!("SKIP live tests: schema apply failed: {e}");
                return None;
            }
            Some(backend)
        }
        Err(e) => {
            eprintln!("SKIP live tests: no SurrealDB at {}: {e}", config.url);
            None
        }
    }
}

fn schema_text() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.surql");
    let template = std::fs::read_to_string(path).expect("schema.surql");
    let cfg = AuthConfig::firebase_for_tests("live-test-project".to_string());
    cfg.render_schema(&template)
}

async fn seed(backend: &Backend) {
    backend.ensure_seed(0).await.expect("seed");
}

#[tokio::test]
async fn player_lifecycle() {
    let Some(db) = live_backend("player_lifecycle").await else { return };
    seed(&db).await;

    let me = db.connect_player("uid-quinn", 10).await.unwrap();
    assert_eq!(me.name, "Adventurer");
    assert!(me.online);

    let named = db.register_player("uid-quinn", "Quinn", 11).await.unwrap();
    assert_eq!(named.name, "Quinn");
    assert!(db.register_player("uid-quinn", "   ", 12).await.is_err());

    let moved = db.move_player("uid-quinn", 2.0, 3.0, 0.0, 450.0, 13).await.unwrap();
    assert_eq!((moved.x, moved.z), (2.0, 3.0));
    assert!((moved.yaw_deg - 90.0).abs() < 1e-3);
    assert!(db.move_player("uid-quinn", 9999.0, 0.0, 0.0, 0.0, 14).await.is_err());

    db.disconnect_player("uid-quinn", 15).await.unwrap();
    assert!(!db.get_player("uid-quinn").await.unwrap().unwrap().online);
    let back = db.connect_player("uid-quinn", 16).await.unwrap();
    assert!(back.online);
    assert_eq!(back.name, "Quinn", "progress persists across reconnects");
}

#[tokio::test]
async fn damage_kill_heal_respawn() {
    let Some(db) = live_backend("damage_kill_heal_respawn").await else { return };
    seed(&db).await;
    db.connect_player("uid-a", 0).await.unwrap();
    db.connect_player("uid-b", 0).await.unwrap();

    let hurt = db.damage_player("uid-a", 30.0, 1).await.unwrap();
    assert_eq!(hurt.hp, 70.0);
    assert!(db.damage_player("uid-a", f32::NAN, 2).await.is_err());
    assert!(db.damage_player("nobody", 10.0, 3).await.is_err());

    let healed = db.heal_player("uid-a", 50.0, 4).await.unwrap();
    assert_eq!(healed.hp, 100.0);

    let dead = db.damage_player("uid-a", 500.0, 5).await.unwrap();
    assert_eq!(dead.hp, 0.0);
    db.log_death("uid-b", "uid-a", 6).await.unwrap();
    // The dead can still be moved (position is harmless); respawn resets them.
    db.move_player("uid-a", 1.0, 1.0, 0.0, 0.0, 7).await.unwrap();

    let feed = db.recent_log(10).await.unwrap();
    assert!(feed.iter().any(|e| e.kind == "kill"));

    let back = db.respawn_player("uid-a", 8).await.unwrap();
    assert_eq!(back.hp, back.max_hp);
}

#[tokio::test]
async fn pieces_validate_and_cascade_paths() {
    let Some(db) = live_backend("pieces_validate_and_cascade_paths").await else { return };
    seed(&db).await;

    let mut bad = new_piece_args("teleport", 0.0, 0.0);
    assert!(db.create_piece(bad.clone(), "ed", 0).await.is_err());
    bad.shape = "floor".to_string();
    bad.x = 99999.0;
    assert!(db.create_piece(bad.clone(), "ed", 0).await.is_err());
    bad.x = 1.0;
    let piece = db.create_piece(bad, "ed", 1).await.unwrap();
    assert!(!piece.rid.is_empty());

    let updated = db
        .update_piece(&piece.rid, new_piece_args("wall", 2.0, 0.0))
        .await
        .unwrap();
    assert_eq!(updated.shape, "wall");
    assert!(db.update_piece("nope", new_piece_args("wall", 0.0, 0.0)).await.is_err());

    db.set_platform_path(
        &piece.rid,
        "loop",
        1.5,
        false,
        vec![Waypoint { x: 0.0, z: 0.0 }, Waypoint { x: 4.0, z: 0.0 }],
    )
    .await
    .unwrap();
    assert!(db.get_path(&piece.rid).await.unwrap().is_some());
    assert!(db
        .set_platform_path(&piece.rid, "warp", 1.0, false, vec![
            Waypoint { x: 0.0, z: 0.0 },
            Waypoint { x: 1.0, z: 0.0 }
        ])
        .await
        .is_err());

    db.delete_piece(&piece.rid, "ed", 2).await.unwrap();
    assert!(db.get_path(&piece.rid).await.unwrap().is_none());
    assert!(db.delete_piece(&piece.rid, "ed", 3).await.is_err());
}

#[tokio::test]
async fn feed_and_prune() {
    let Some(db) = live_backend("feed_and_prune").await else { return };
    seed(&db).await;

    let entry = db.send_message("uid-a", "hello dungeon", 1).await.unwrap();
    assert_eq!(entry.text, "hello dungeon");
    assert!(db.send_message("uid-a", "   ", 2).await.is_err());

    for i in 0..5 {
        db.send_message("uid-a", &format!("spam {i}"), 10 + i).await.unwrap();
    }
    let dropped = db.prune_log().await.unwrap();
    assert_eq!(dropped, 0, "feed is small; nothing to prune");

    let feed = db.recent_log(3).await.unwrap();
    assert_eq!(feed.len(), 3);
    assert!(feed.windows(2).all(|w| w[0].at_ms >= w[1].at_ms));
}

#[tokio::test]
async fn snapshot_round_trip() {
    let Some(db) = live_backend("snapshot_round_trip").await else { return };
    seed(&db).await;

    let a = db.create_piece(new_piece_args("floor", 0.0, 0.0), "ed", 0).await.unwrap();
    db.set_platform_path(
        &a.rid,
        "pingpong",
        1.0,
        false,
        vec![Waypoint { x: 0.0, z: 0.0 }, Waypoint { x: 1.0, z: 1.0 }],
    )
    .await
    .unwrap();
    let json = db.export_snapshot().await.unwrap();
    let parsed: LevelSnapshot = serde_json::from_str(&json).unwrap();
    assert!(parsed.pieces.len() >= 3); // seed spawn + floor + new piece
    assert_eq!(parsed.paths.len(), 1);

    assert!(db.import_snapshot("{nope", "ed", 1).await.is_err());
    assert!(db
        .import_snapshot(r#"{"version":99,"name":"x","pieces":[],"paths":[]}"#, "ed", 2)
        .await
        .is_err());

    db.clear_level("ed", 3).await.unwrap();
    assert!(db.list_pieces().await.unwrap().is_empty());
    db.import_snapshot(&json, "ed", 4).await.unwrap();
    let restored = db.list_pieces().await.unwrap();
    assert_eq!(restored.len(), parsed.pieces.len());
    assert_eq!(db.get_path(&a.rid).await.unwrap().unwrap().mode, "pingpong");
}

#[tokio::test]
async fn live_feed_streams_commits() {
    let Some(db) = live_backend("live_feed_streams_commits").await else { return };
    seed(&db).await;

    let mut feed = db.subscribe::<Piece>(TableName::DungeonPiece).await.unwrap();
    let piece = db.create_piece(new_piece_args("wall", 1.0, 0.0), "ed", 1).await.unwrap();
    db.update_piece(&piece.rid, new_piece_args("wall", 2.0, 0.0)).await.unwrap();
    db.delete_piece(&piece.rid, "ed", 2).await.unwrap();

    let mut ops = Vec::new();
    for _ in 0..3 {
        let batch = tokio::time::timeout(std::time::Duration::from_secs(10), feed.next())
            .await
            .expect("live notification in time")
            .expect("feed open")
            .expect("notification parses");
        ops.extend(batch);
    }
    assert_eq!(ops.len(), 3);
    assert!(matches!(ops[0].op, Op::Insert));
    assert!(matches!(ops[1].op, Op::Update));
    assert!(matches!(ops[2].op, Op::Delete));
    assert!(ops.iter().all(|e: &Event| e.key == piece.rid));
    assert!(ops.windows(2).all(|w| w[1].seq > w[0].seq));
}

#[tokio::test]
async fn dev_login_is_gated_like_a_foreign_jwt() {
    let Some(db) = live_backend("dev_login_is_gated_like_a_foreign_jwt").await else { return };
    seed(&db).await;
    // From here this connection IS the dev record user: no iss/aud claims.
    db.signup_dev("tester", "secret123").await.unwrap();
    // Seeded rows are invisible and unwritable through the gate — exactly
    // what a token minted for a different Firebase project would see.
    assert!(db.list_pieces().await.unwrap().is_empty());
    let denied = db.create_piece(new_piece_args("wall", 0.0, 0.0), "tester", 1).await;
    assert!(matches!(denied, Err(BackendError::PermissionDenied(_))));
}
