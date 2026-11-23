//! Row-rule enforcement tests. They mint their own HS256 JWTs (the test-only
//! `test_hs256` access below) and prove, end to end, that the rules in
//! `schema.surql` hold for direct connections:
//!
//! * project gate: wrong `aud` fails authentication outright (AUDIENCE clause)
//! * ownership: a token can only write the player row its `sub` names
//! * value rules: bad shapes and out-of-bounds writes match no rows
//!
//! Needs a running SurrealDB like tests/live.rs (skips loudly without one).

use std::collections::BTreeMap;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use party_dungeon_server::{AuthConfig, SurrealConfig};
use serde::Serialize;
use surrealdb::engine::remote::ws::Ws;
use surrealdb::opt::auth::{AccessToken, Token};
use surrealdb::types::Value;
use surrealdb::Surreal;

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

fn test_config(name: &str) -> SurrealConfig {
    let safe: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    SurrealConfig {
        url: std::env::var("DUNGEON_TEST_URL").unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string()),
        namespace: "test".to_string(),
        database: format!("r_{}_{safe}", std::process::id()),
        auth: AuthConfig::firebase_for_tests(PROJECT.to_string()),
        firebase_project_id: PROJECT.to_string(),
    }
}

fn schema_text() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.surql");
    let template = std::fs::read_to_string(path).expect("schema.surql");
    let cfg = AuthConfig::firebase_for_tests(PROJECT.to_string());
    cfg.render_schema(&template)
}

struct Fixture {
    db: Surreal<surrealdb::engine::remote::ws::Client>,
    secret: String,
    db_name: String,
}

impl Fixture {
    async fn setup(name: &str) -> Option<Self> {
        let config = test_config(name);
        let (user, pass) = (
            std::env::var("DUNGEON_TEST_USER").unwrap_or_else(|_| "root".to_string()),
            std::env::var("DUNGEON_TEST_PASS").unwrap_or_else(|_| "devrootpass".to_string()),
        );
        let endpoint = config.url.trim_start_matches("ws://").trim_start_matches("wss://");
        let db = match Surreal::new::<Ws>(endpoint).await {
            Ok(db) => db,
            Err(e) => {
                eprintln!("SKIP rules tests: no SurrealDB at {}: {e}", config.url);
                return None;
            }
        };
        if let Err(e) = (async {
            use surrealdb::opt::auth::Root;
            db.signin(Root { username: user, password: pass })
                .await
                .map_err(|e| e.to_string())?;
            db.use_ns(&config.namespace)
                .use_db(&config.database)
                .await
                .map_err(|e| e.to_string())?;
            let mut r = db.query(&schema_text()).await.map_err(|e| e.to_string())?;
            let errors = r.take_errors();
            let real: Vec<_> = errors.iter().filter(|(_, e)| !e.to_string().contains("already exists")).collect();
            if !real.is_empty() {
                return Err(format!("schema failed: {real:?}"));
            }
            Ok::<(), String>(())
        }
        .await)
        {
            eprintln!("SKIP rules tests: setup failed: {e}");
            return None;
        }
        // Test-only HS256 access. Random key per run, database-scoped, never
        // shipped: this is what lets the test mint tokens without Firebase.
        let secret = format!("test-secret-{}-{}", std::process::id(), party_dungeon_server::now_ms());
        let mut r = db
            .query(&format!(
                "DEFINE ACCESS test_hs256 ON DATABASE TYPE JWT ALGORITHM HS256 KEY '{secret}' AUDIENCE '{PROJECT}'"
            ))
            .await
            .expect("define test access");
        assert!(r.take_errors().is_empty());
        Some(Self { db, secret, db_name: config.database })
    }

    fn mint(&self, sub: &str, aud: &str) -> String {
        let claims = TestClaims {
            iss: ISSUER.to_string(),
            aud: aud.to_string(),
            sub: sub.to_string(),
            ns: "test".to_string(),
            db: self.db_name.clone(),
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
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
        .expect("mint token")
    }

    async fn authed(&self, jwt: &str) -> Surreal<surrealdb::engine::remote::ws::Client> {
        let endpoint = std::env::var("DUNGEON_TEST_URL")
            .unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string());
        let endpoint = endpoint.trim_start_matches("ws://").trim_start_matches("wss://");
        let db: Surreal<surrealdb::engine::remote::ws::Client> =
            Surreal::new::<Ws>(endpoint).await.expect("connect");
        let cfg_ns = "test";
        db.use_ns(cfg_ns).use_db(&self.db_name).await.expect("use");
        db.authenticate(Token::from(AccessToken::from(jwt.to_string())))
            .await
            .expect("authenticate");
        db
    }

    async fn create_player(
        &self,
        db: &Surreal<surrealdb::engine::remote::ws::Client>,
        rid: &str,
    ) -> Vec<Value> {
        let mut vars = BTreeMap::new();
        vars.insert("rid".to_string(), Value::from_string(rid.to_string()));
        let mut r = db
            .query("CREATE player CONTENT { rid: $rid, name: 'T', x: 0, z: 0, feet: 0, yaw_deg: 0, hp: 100, max_hp: 100, online: true, last_seen_ms: 0 } RETURN *")
            .bind(vars)
            .await
            .expect("query");
        r.take(0).expect("take")
    }
}

#[tokio::test]
async fn ownership_and_value_rules_hold() {
    let Some(fx) = Fixture::setup("ownership").await else { return };

    // Own row, valid values: allowed.
    let alice = fx.authed(&fx.mint("alice", PROJECT)).await;
    let rows = fx.create_player(&alice, "alice").await;
    assert_eq!(rows.len(), 1, "own-row create must succeed");

    // Spoofed sub: the statement names another row, the token names alice.
    let spoofed = fx.create_player(&alice, "bob").await;
    assert!(spoofed.is_empty(), "writing another player's row must match nothing");

    // Bad shape on a shared table: value rule.
    let mut r = alice
        .query("CREATE dungeon_piece CONTENT { rid: 'x1', shape: 'teleport', x: 0, z: 0, elevation: 0, rot_deg: 0, pitch_deg: 0, roll_deg: 0, dynamic: false, damage: 0, interact: 'none', trigger_w: 2, trigger_h: 2, trigger_l: 2 } RETURN *")
        .await
        .expect("query");
    let rows: Vec<Value> = r.take(0).expect("take");
    assert!(rows.is_empty(), "unknown shape must match nothing");

    // Out-of-bounds move on own row: value rule.
    let mut r = alice
        .query("UPDATE player SET x = 99999 WHERE rid = 'alice' RETURN *")
        .await
        .expect("query");
    let rows: Vec<Value> = r.take(0).expect("take");
    assert!(rows.is_empty(), "out-of-bounds write must match nothing");

    // In-bounds move on own row: allowed.
    let mut r = alice
        .query("UPDATE player SET x = 3 WHERE rid = 'alice' RETURN *")
        .await
        .expect("query");
    let rows: Vec<Value> = r.take(0).expect("take");
    assert_eq!(rows.len(), 1, "in-bounds own-row write must succeed");
}

#[tokio::test]
async fn wrong_audience_fails_authentication() {
    let Some(fx) = Fixture::setup("audience").await else { return };
    let evil = fx.mint("mallory", "EVIL-PROJECT");
    let endpoint = std::env::var("DUNGEON_TEST_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string());
    let endpoint = endpoint.trim_start_matches("ws://").trim_start_matches("wss://");
    let db: Surreal<surrealdb::engine::remote::ws::Client> =
        Surreal::new::<Ws>(endpoint).await.expect("connect");
    db.use_ns("test").use_db(&fx.db_name).await.expect("use");
    let res = db
        .authenticate(Token::from(AccessToken::from(evil)))
        .await;
    assert!(res.is_err(), "wrong-audience token must not authenticate");
}
