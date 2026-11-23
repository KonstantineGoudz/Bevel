-- Party Dungeon schema (SurrealDB 4.x).
--
-- Apply with scripts/bootstrap.sh, which renders this template with the
-- configured auth provider (see `src/auth.rs` — `AuthConfig::render_schema`).
-- Placeholders (rendered at bootstrap):
--   AUTH_ACCESS_DEFINITION  → full `DEFINE ACCESS ...` for the provider
--   AUTH_ROW_PREDICATE      → `$token.iss == ... AND $token.aud == ...`
--   AUTH_OWNERSHIP_PREDICATE → `rid = $token.sub` (player table only)
-- The file backend and the compose stack both start empty, so this file is the
-- entire migration story for v1: namespace/database are chosen at connection
-- time, and every statement here is re-runnable (DEFINE is idempotent; REMOVE
-- + re-DEFINE when a rule changes).
--
-- Rows carry an application key, `rid` (a stored string field): the player
-- sub claim for `player` (stable across reconnects), generated otherwise.
-- Commands and live payloads address rows through `rid`.
--
-- Auth model. External JWTs must carry SurrealDB routing claims {ns,db,ac}
-- via the IdP's custom-claims hook (Firebase Admin SDK `setCustomUserClaims`,
-- Auth0 Action, etc.). Sessions then expose two objects:
--   * $token ... the verified JWT claims (iss/aud/sub + routing). Used by
--                the row rules below. NULL for non-JWT sessions (fail closed).
--   * $auth  ... the record/user identity. NULL for pure JWT sessions.
--
-- Three doors, one gate each:
--   * platform_jwt (aliased as firebase_jwt for compat) ... players. Signature
--                        verified against the provider's JWKS, aud pinned, iss
--                        pinned. A token from another project/tenant verifies
--                        its signature but matches no rows.
--   * dev_login ...... local development + tests. A RECORD access whose
--                        sessions carry no iss/aud, so they are gated exactly
--                        like a foreign JWT: proves the rules.
--   * database users . the service layer (DEFINE USER ... ROLES OWNER, or
--                        root in dev). Bypass row rules; enforce game logic
--                        in code instead.
--
-- Ownership. Player-table writes are bound to the caller's own row
-- (`__AUTH_OWNERSHIP_PREDICATE__`), so a stolen-or-spoofed `sub` parameter is
-- useless. Shared tables are project-gated: any member may edit.

-- ---------------------------------------------------------------------------
-- Access: provider JWT (rendered from AuthConfig)
-- ---------------------------------------------------------------------------
__AUTH_ACCESS_DEFINITION__

-- ---------------------------------------------------------------------------
-- Access: local record login (development + tests only)
-- ---------------------------------------------------------------------------
DEFINE TABLE dev_user SCHEMAFULL PERMISSIONS FULL;
DEFINE FIELD user ON dev_user TYPE string;
DEFINE FIELD pass ON dev_user TYPE string;
DEFINE INDEX dev_user_name ON dev_user FIELDS user UNIQUE;

DEFINE ACCESS dev_login ON DATABASE TYPE RECORD
	SIGNUP (CREATE dev_user SET user = $user, pass = crypto::argon2::generate($pass))
	SIGNIN (SELECT * FROM dev_user WHERE user = $user AND crypto::argon2::compare(pass, $pass))
	DURATION FOR SESSION 12h;

-- ---------------------------------------------------------------------------
-- player: one row per adventurer, rid = sub claim
-- ---------------------------------------------------------------------------
DEFINE TABLE player SCHEMAFULL
	PERMISSIONS FOR select
		WHERE __AUTH_ROW_PREDICATE__
		FOR create, update, delete
		WHERE __AUTH_ROW_PREDICATE__
		  AND __AUTH_OWNERSHIP_PREDICATE__
		  AND x >= -500.0 AND x <= 500.0
		  AND z >= -500.0 AND z <= 500.0
		  AND feet >= 0.0 AND feet <= 100.0
		  AND yaw_deg >= 0.0 AND yaw_deg <= 360.0
		  AND hp >= 0.0 AND hp <= max_hp
		  AND max_hp > 0.0 AND max_hp <= 10000.0
		  AND string::len(name) <= 24
		  AND string::len(string::trim(name)) > 0;
DEFINE FIELD rid ON player TYPE string;
DEFINE FIELD name ON player TYPE string;
DEFINE FIELD x ON player TYPE float;
DEFINE FIELD z ON player TYPE float;
DEFINE FIELD feet ON player TYPE float;
DEFINE FIELD yaw_deg ON player TYPE float;
DEFINE FIELD hp ON player TYPE float;
DEFINE FIELD max_hp ON player TYPE float;
DEFINE FIELD online ON player TYPE bool;
DEFINE FIELD last_seen_ms ON player TYPE int;
DEFINE INDEX player_rid ON player FIELDS rid UNIQUE;

-- ---------------------------------------------------------------------------
-- dungeon_piece: the authoritative dungeon
-- ---------------------------------------------------------------------------
DEFINE TABLE dungeon_piece SCHEMAFULL
	PERMISSIONS FOR select
		WHERE __AUTH_ROW_PREDICATE__
		FOR create, update, delete
		WHERE __AUTH_ROW_PREDICATE__
		  AND shape IN ['floor', 'pit', 'water', 'bridge', 'wall', 'fence', 'window', 'pillar', 'block', 'ramp', 'door', 'gate', 'secret', 'rubble', 'crate', 'chest', 'altar', 'brazier', 'herb', 'ore', 'spawn', 'entry', 'spikes', 'trigger', 'trigger_box', 'trigger_sphere', 'trigger_cylinder', 'glade_tower', 'glade_room', 'glade_wall', 'glade_stairs', 'glade_door', 'glade_window', 'glade_terrain', 'glade_tree', 'glade_path']
		  AND x >= -500.0 AND x <= 500.0
		  AND z >= -500.0 AND z <= 500.0
		  AND elevation >= -50.0 AND elevation <= 200.0
		  AND rot_deg >= 0.0 AND rot_deg <= 360.0
		  AND pitch_deg >= 0.0 AND pitch_deg <= 360.0
		  AND roll_deg >= 0.0 AND roll_deg <= 360.0
		  AND damage >= 0.0 AND damage <= 10000.0
		  AND interact IN ['none', 'togglePause']
		  AND trigger_w > 0.0 AND trigger_w <= 100.0
		  AND trigger_h > 0.0 AND trigger_h <= 100.0
		  AND trigger_l > 0.0 AND trigger_l <= 100.0
		  AND trigger_alpha >= 0.0 AND trigger_alpha <= 1.0;
DEFINE FIELD rid ON dungeon_piece TYPE string;
DEFINE FIELD shape ON dungeon_piece TYPE string;
DEFINE FIELD x ON dungeon_piece TYPE float;
DEFINE FIELD z ON dungeon_piece TYPE float;
DEFINE FIELD elevation ON dungeon_piece TYPE float;
DEFINE FIELD rot_deg ON dungeon_piece TYPE float;
DEFINE FIELD pitch_deg ON dungeon_piece TYPE float;
DEFINE FIELD roll_deg ON dungeon_piece TYPE float;
DEFINE FIELD dynamic ON dungeon_piece TYPE bool;
DEFINE FIELD damage ON dungeon_piece TYPE float;
DEFINE FIELD interact ON dungeon_piece TYPE string;
DEFINE FIELD trigger_w ON dungeon_piece TYPE float;
DEFINE FIELD trigger_h ON dungeon_piece TYPE float;
DEFINE FIELD trigger_l ON dungeon_piece TYPE float;
DEFINE FIELD trigger_alpha ON dungeon_piece TYPE float;
DEFINE INDEX piece_rid ON dungeon_piece FIELDS rid UNIQUE;
DEFINE INDEX piece_shape ON dungeon_piece FIELDS shape;

-- ---------------------------------------------------------------------------
-- platform_path: mover track for one piece (piece_rid = its dungeon_piece)
-- ---------------------------------------------------------------------------
DEFINE TABLE platform_path SCHEMAFULL
	PERMISSIONS FOR select
		WHERE __AUTH_ROW_PREDICATE__
		FOR create, update, delete
		WHERE __AUTH_ROW_PREDICATE__
		  AND mode IN ['pingpong', 'loop', 'oneshot']
		  AND speed >= 0.0 AND speed <= 30.0
		  AND array::len(waypoints) >= 2 AND array::len(waypoints) <= 16;
DEFINE FIELD piece_rid ON platform_path TYPE string;
DEFINE FIELD mode ON platform_path TYPE string;
DEFINE FIELD speed ON platform_path TYPE float;
DEFINE FIELD paused ON platform_path TYPE bool;
DEFINE FIELD waypoints ON platform_path TYPE array;
DEFINE INDEX path_piece ON platform_path FIELDS piece_rid UNIQUE;

-- ---------------------------------------------------------------------------
-- level_meta: singleton lobby row (rid 'main')
-- ---------------------------------------------------------------------------
DEFINE TABLE level_meta SCHEMAFULL
	PERMISSIONS FOR select
		WHERE __AUTH_ROW_PREDICATE__
		FOR create, update, delete
		WHERE __AUTH_ROW_PREDICATE__
		  AND string::len(name) > 0
		  AND string::len(name) <= 80;
DEFINE FIELD rid ON level_meta TYPE string;
DEFINE FIELD name ON level_meta TYPE string;

-- ---------------------------------------------------------------------------
-- dungeon_log: append-only feed (joins, kills, edits, chat)
-- ---------------------------------------------------------------------------
DEFINE TABLE dungeon_log SCHEMAFULL
	PERMISSIONS FOR select
		WHERE __AUTH_ROW_PREDICATE__
		FOR create, update, delete
		WHERE __AUTH_ROW_PREDICATE__
		  AND at_ms > 0
		  AND string::len(text) <= 2000
		  AND string::len(string::trim(text)) > 0;
DEFINE FIELD rid ON dungeon_log TYPE string;
DEFINE FIELD at_ms ON dungeon_log TYPE int;
DEFINE FIELD kind ON dungeon_log TYPE string;
DEFINE FIELD actor ON dungeon_log TYPE string;
DEFINE FIELD text ON dungeon_log TYPE string;
DEFINE INDEX log_rid ON dungeon_log FIELDS rid UNIQUE;
DEFINE INDEX log_time ON dungeon_log FIELDS at_ms;
