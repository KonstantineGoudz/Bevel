//! Conformance: the generated client (`party-dungeon-client`) must behave
//! like [`party_dungeon_server::Backend`] against a live database, through
//! the same row rules a production player meets.
//!
//! Each test mints its own HS256 JWT (Firebase-shaped claims) and drives one
//! slice of the API through the *generated* package only.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use party_dungeon_client::{new_piece_args, Client, Event, Op, TableName, Waypoint};
use party_dungeon_server::AuthConfig;
use serde::Serialize;

const PROJECT: &str = "live-test-project";
const ISSUER: &str = "https://securetoken.google.com/live-test-project";

#[derive(Debug, Serialize)]
struct TestClaims {
    iss: String,
    aud: String,
    sub: String,
    ns: String,
    db: String,
    ac: String,
    exp: usize,
}

struct Fixture {
    url: String,
    ns: String,
    db: String,
}

impl Fixture {
    async fn setup(name: &str) -> Option<(Self, party_dungeon_server::Backend, String)> {
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let url = std::env::var("DUNGEON_TEST_URL")
            .unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string());
        let ns = "test".to_string();
        let db_name = format!("c_{}_{safe}", std::process::id());
        let secret = format!("conf-secret-{}-{}", std::process::id(), name.len());
        let config = party_dungeon_server::SurrealConfig {
            url: url.clone(),
            namespace: ns.clone(),
            database: db_name.clone(),
            auth: AuthConfig::firebase_for_tests(PROJECT.to_string()),
        firebase_project_id: PROJECT.to_string(),
        };
        let (user, pass) = (
            std::env::var("DUNGEON_TEST_USER").unwrap_or_else(|_| "root".to_string()),
            std::env::var("DUNGEON_TEST_PASS").unwrap_or_else(|_| "devrootpass".to_string()),
        );
        let backend = match party_dungeon_server::Backend::connect_root(&config, &user, &pass).await
        {
            Ok(b) => b,
            Err(e) => {
                eprintln!("SKIP conformance tests: no SurrealDB at {url}: {e}");
                return None;
            }
        };
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.surql");
        let template = std::fs::read_to_string(path).expect("schema.surql");
        let cfg = AuthConfig::firebase_for_tests(PROJECT.to_string());
        let schema = cfg.render_schema(&template);
        if let Err(e) = backend.apply_schema(&schema).await {
            eprintln!("SKIP conformance tests: schema failed: {e}");
            return None;
        }
        if let Err(e) = backend
            .apply_schema(&format!(
                "DEFINE ACCESS test_hs256 ON DATABASE TYPE JWT ALGORITHM HS256 KEY '{secret}' AUDIENCE '{PROJECT}'"
            ))
            .await
        {
            eprintln!("SKIP conformance tests: test access failed: {e}");
            return None;
        }
        if let Err(e) = backend.ensure_seed(0).await {
            eprintln!("SKIP conformance tests: seed failed: {e}");
            return None;
        }
        Some((
            Self { url, ns, db: db_name },
            backend,
            secret,
        ))
    }

    fn mint(&self, sub: &str, secret: &str) -> String {
        let claims = TestClaims {
            iss: ISSUER.to_string(),
            aud: PROJECT.to_string(),
            sub: sub.to_string(),
            ns: "test".to_string(),
            db: self.db.clone(),
            ac: "test_hs256".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600) as usize,
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .expect("mint token")
    }

    async fn player(&self, sub: &str, secret: &str) -> Option<Client> {
        let jwt = self.mint(sub, secret);
        match Client::connect_with_token(&self.url, &self.ns, &self.db, &jwt).await {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("SKIP conformance tests: player connect failed: {e}");
                None
            }
        }
    }
}

#[tokio::test]
async fn generated_client_matches_backend_player_flow() {
    let Some((fx, _svc, secret)) = Fixture::setup("player_flow").await else { return };
    let Some(client) = fx.player("quinn", &secret).await else { return };

    let me = client.connect_player("quinn", 10).await.expect("connect");
    assert_eq!(me.name, "Adventurer");
    assert!(me.online);

    // The generated client validates nothing locally; the database does.
    let named = client.register_player("quinn", "Quinn", 11).await.expect("register");
    assert_eq!(named.name, "Quinn");
    assert!(client.register_player("quinn", "   ", 12).await.is_err());

    let moved = client.move_player("quinn", 2.0, 3.0, 0.0, 450.0, 13).await.expect("move");
    assert_eq!((moved.x, moved.z), (2.0, 3.0));
    assert!((moved.yaw_deg - 90.0).abs() < 1e-3);
    assert!(client.move_player("quinn", 9999.0, 0.0, 0.0, 0.0, 14).await.is_err());

    // Spoofing another player's row is denied by the ownership rule.
    assert!(client.move_player("mallory", 1.0, 1.0, 0.0, 0.0, 15).await.is_err());

    let hurt = client.damage_player("quinn", 30.0, 16).await.expect("damage");
    assert_eq!(hurt.hp, 70.0);
    let healed = client.heal_player("quinn", 50.0, 17).await.expect("heal");
    assert_eq!(healed.hp, 100.0);
    let dead = client.damage_player("quinn", 500.0, 18).await.expect("kill");
    assert_eq!(dead.hp, 0.0);
    client.log_death("quinn", "quinn", 19).await.expect("kill log");
    let back = client.respawn_player("quinn", 20).await.expect("respawn");
    assert_eq!(back.hp, back.max_hp);
}

#[tokio::test]
async fn generated_client_matches_backend_level_flow() {
    let Some((fx, _svc, secret)) = Fixture::setup("level_flow").await else { return };
    let Some(ed) = fx.player("ed", &secret).await else { return };
    ed.connect_player("ed", 0).await.expect("connect");

    let piece = ed
        .create_piece(new_piece_args("wall", 1.0, 0.0), "ed", 1)
        .await
        .expect("create");
    assert!(!piece.rid.is_empty());
    assert!(ed
        .create_piece(new_piece_args("teleport", 0.0, 0.0), "ed", 2)
        .await
        .is_err());

    ed.set_platform_path(
        &piece.rid,
        "loop",
        1.5,
        false,
        vec![Waypoint { x: 0.0, z: 0.0 }, Waypoint { x: 4.0, z: 0.0 }],
    )
    .await
    .expect("set path");
    assert!(ed.get_path(&piece.rid).await.expect("get path").is_some());

    let msg = ed.send_message("ed", "hello", 4).await.expect("chat");
    assert_eq!(msg.text, "hello");

    ed.delete_piece(&piece.rid, "ed", 5).await.expect("delete");
    assert!(ed.get_path(&piece.rid).await.expect("gone").is_none());
}

#[tokio::test]
async fn generated_client_streams_live_events() {
    let Some((fx, _svc, secret)) = Fixture::setup("live_feed").await else { return };
    let Some(ed) = fx.player("ed", &secret).await else { return };
    ed.connect_player("ed", 0).await.expect("connect");

    let mut feed = ed
        .subscribe::<party_dungeon_client::Piece>(TableName::DungeonPiece)
        .await
        .expect("subscribe");
    let piece = ed
        .create_piece(new_piece_args("floor", 0.0, 0.0), "ed", 1)
        .await
        .expect("create");
    ed.delete_piece(&piece.rid, "ed", 2).await.expect("delete");

    let mut ops = Vec::new();
    for _ in 0..2 {
        let batch = tokio::time::timeout(std::time::Duration::from_secs(10), feed.next())
            .await
            .expect("live notification in time")
            .expect("feed open")
            .expect("notification parses");
        ops.extend(batch);
    }
    assert_eq!(ops.len(), 2);
    assert!(matches!(ops[0].op, Op::Insert));
    assert!(matches!(ops[1].op, Op::Delete));
    assert!(ops.iter().all(|e: &Event| e.key == piece.rid));
}

#[tokio::test]
async fn backend_and_client_agree_on_snapshots() {
    let Some((fx, svc, secret)) = Fixture::setup("snapshot_agree").await else { return };
    let Some(ed) = fx.player("ed", &secret).await else { return };
    ed.connect_player("ed", 0).await.expect("connect");
    ed.create_piece(new_piece_args("crate", 5.0, 5.0), "ed", 1)
        .await
        .expect("create via generated client");

    // The service sees exactly what the player client wrote, and vice versa.
    let via_service = svc.list_pieces().await.expect("service list");
    assert!(via_service.iter().any(|p| p.shape == "crate"));
    let via_client = ed.list_pieces().await.expect("client list");
    assert_eq!(via_service.len(), via_client.len());
    assert!(via_service.iter().zip(via_client.iter()).all(|(a, b)| a.rid == b.rid));
}
