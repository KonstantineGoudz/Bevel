# party-dungeon-server

The authoritative backend for Party Dungeon, hosted on a **standalone
SurrealDB** (server `v4.0.1`, Rust client `4.0.1` — one pinned version,
bumped deliberately).

Clients hold **direct, authenticated connections** to the database. There is
no API tier in the write path — but there is also no free-form access. Every
mutation goes through a **reducer** defined once in `src/reducers.rs`:

```
                          src/reducers.rs  (single source of truth)
                           name · doc · params · SurrealQL · result
                                     │
              ┌──────────────────────┼──────────────────────┐
              ▼                      ▼                      ▼
   Backend (service role)   generated client      row rules (schema.surql)
   hand-written, full       (party-dungeon-client,  every statement additionally
   validation + identity    byte-identical calls)   checked against iss/aud,
                                                    ownership, value shapes
```

- **Backend** (`src/backend.rs`): the trusted sidecar. Connects as a database
  user (bypasses row rules), validates input for good errors, executes the
  registry's statements. Used by tests, seeding, imports, and any trusted
  service.
- **Generated client** (`clients/rust/`, crate `party-dungeon-client`): the
  player package. Rendered from the same registry by
  `cargo run -p party-dungeon-server --bin codegen` — check the output in,
  never edit it. Same statements, same canonicalization; the row rules are
  the only difference in enforcement.
- **Freshness + conformance keep them honest**: `generated_client_is_fresh`
  fails if the checked-in package drifts from the generator, and
  `tests/client.rs` drives the generated package through the real row rules
  against a live database.

## The reducer contract

- One reducer = one named mutation with typed params, ordered SurrealQL
  statements, and a result mapping. Multi-statement reducers run in a single
  `BEGIN`/`COMMIT` batch; a mutation that writes nothing is a denial or a
  miss, told apart by a probe read (`PermissionDenied` vs `NotFound`).
- `$rid` / `$rid2` / `$now` are injected, never caller-supplied — unless the
  parameter list names them (updates address a caller-chosen row). No
  computed bindings: every `$var` is either a caller param or injected.
- `connect_player` is the one custom reducer (SELECT-then-branch); both
  consumers orchestrate it from the same statements. `import_snapshot` and
  `ensure_seed` are service-only (bulk admin ops, not player reducers).
- Validation is split deliberately: the service pre-validates for error
  quality; the database re-enforces every value rule for direct clients.
  Anything the schema cannot see (e.g. damage *amounts*) is safe anyway —
  outcomes are clamped by the stored data (`hp` floors at 0).
- Names that look dynamic are static or direct params: feed lines carry fixed
  text or the actor/target id, never formatted strings.

## Quickstart

```sh
cp .env.example .env   # set SURREAL_ROOT_PASS + auth provider vars (see .env.example)
docker compose up -d
./scripts/bootstrap.sh # renders schema.surql for your provider, loads it, creates the service user
cargo test -p party-dungeon-server
```

`cargo test` always runs the unit tests. The live suites (`live.rs`,
`rules.rs`, `client.rs`) need the stack up and skip loudly without one; each
uses an isolated database, so parallel runs never interfere. Against compose,
point them with `DUNGEON_TEST_URL=127.0.0.1:6000` (the default).

Manual play without an external IdP:

```sh
./scripts/dev-user.sh alice s3cret   # RECORD signup, prints a JWT
```

## Auth setup — swap the provider without a refactor

All provider specifics live in `src/auth.rs` (`AuthConfig`). The rest of the
system — `Backend`, `HostNet`, `NetClient`, `schema.surql` — only sees
`AuthClaims { iss, aud, sub }` and `SurrealConfig.auth`. Swap via env:

```sh
# Firebase (still the default when DUNGEON_FIREBASE_PROJECT_ID is set)
DUNGEON_AUTH_PROVIDER=firebase DUNGEON_FIREBASE_PROJECT_ID=my-proj

# Any OIDC provider (Auth0, Clerk, Supabase Auth, Okta, …)
DUNGEON_AUTH_PROVIDER=oidc \
  DUNGEON_OIDC_ISSUER=https://my-tenant.auth0.com/ \
  DUNGEON_OIDC_AUDIENCE=my-api \
  DUNGEON_OIDC_JWKS_URL=https://my-tenant.auth0.com/.well-known/jwks.json

# Local HS256 (tests, dev without an external IdP)
DUNGEON_AUTH_PROVIDER=hs256 DUNGEON_JWT_SECRET=dev-secret \
  DUNGEON_JWT_ISSUER=my-iss DUNGEON_JWT_AUDIENCE=my-aud
```

1. Put the provider's vars in `.env` (see `.env.example`) and re-run
   `./scripts/bootstrap.sh`. It renders `schema.surql` with
   `AuthConfig::render_schema` — the `DEFINE ACCESS` and every row predicate's
   `iss`/`aud` check are derived from the same config, so a token from any
   other tenant verifies its signature but matches no rows.
2. **Routing claims (required, same for every provider).** SurrealDB selects
   the access method from the token's own claims, and most IdPs only mint
   `iss`/`aud`/`sub`. Stamp every player once with the routing claims
   `{ ns, db, ac }` via that IdP's hook:
   - Firebase: `scripts/firebase-claims.mjs` with the Admin SDK
     ```sh
     cd scripts/firebase && npm install
     GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json \
       DUNGEON_FIREBASE_PROJECT_ID=<id> \
       node ../firebase-claims.mjs <uid> [<uid> ...]   # sets { ns, db, ac }
     ```
   - Auth0/Clerk/etc.: an Action/hook that adds `ns, db, ac` to the token.
   Users must refresh their token afterwards (sign out/in, or
   `getIdToken(true)` / `getAccessTokenSilently()`). Without these claims the
   token cannot authenticate at all — verified against the server source and
   live.
3. Clients sign in with that IdP's SDK as usual and pass the **JWT**
   (Firebase ID token, Auth0 access token, etc.) to `Client::connect_with_token`.
   The database fetches the provider's JWKS itself (or checks the HS256 secret).
4. `decode_claims_unverified` is display-only ("signed in as …", routing).
   Authorization decisions belong to the row rules, which are also derived from
   `AuthConfig`.

## Auth model

| Caller | Authenticates as | May touch |
|---|---|---|
| Players | `platform_jwt` access (provider JWT + routing claims) | Game tables, **only** when `iss`/`aud` match the provider's expected values (`AuthConfig`). Player rows are additionally bound to the caller's own `rid = $token.sub` — a spoofed `sub` parameter is useless. |
| Local dev / tests | `dev_login` RECORD access (`./scripts/dev-user.sh`) | Nothing on game tables (no `iss`/`aud` claims) — proves the rules instead of bypassing them. |
| Service layer | database user (`DEFINE USER … ROLES OWNER`, root in dev) | Everything; game logic in code is the enforcement. |
| Tests | throwaway HS256 JWT access (created per test run, random key) | Whatever the test mints — see `tests/rules.rs`. |

Connection strings take `host:port` (`127.0.0.1:6000`, no scheme needed;
`ws://` prefixes are stripped — the SDK mishandles them).

## Operations

- **Backups:** `./scripts/backup.sh` writes a timestamped SurrealQL export
  to `backups/` via `surreal export` inside the compose container. Nightly
  host cron line is in the script header. Restore with `surreal import`
  against a fresh database, then re-run bootstrap.
- **Upgrades:** the server image and the Rust client are pinned together
  (`v4.0.1` / `4.0.1`). Bump both, regenerate the client, run the suite,
  export before you switch.
- **Health:** compose healthcheck and `scripts/bootstrap.sh` both poll
  `is-ready`; `/health` is the load-balancer probe later.
- **Config:** everything is environment-driven (`DUNGEON_*`, `SURREAL_*`);
  nothing bakes endpoints or credentials into a binary.
- **Regenerating the client:** edit `src/reducers.rs`, run
  `cargo run -p party-dungeon-server --bin codegen`, commit the result.
  The freshness test fails the build if you forget.

## Production and Kubernetes-later

Today: single node, surrealkv file backend on a named volume, plus exports.
That covers "survives restarts, restorable from backup". For replicated
production, in increasing order of commitment:

1. Managed SurrealDB (failover + backups outsourced).
2. Self-hosted HA when the 4.x story for it hardens — re-evaluate before
   you need it; do not design around it now.
3. Kubernetes: this crate is already shaped for it — stateless clients,
   env-only config, `is-ready`/`/health` probes, and exports map 1:1 onto
   a StatefulSet + PVC + CronJob. Nothing in `src/` assumes compose.

## Schema evolution

`schema.surql` is the whole migration story for v1 and it is re-runnable:
`DEFINE` is idempotent, and `apply_schema` treats "already exists" as
success. Changing a rule means editing the file and re-running bootstrap
(or `Backend::apply_schema`). Additive changes (new tables/fields with
defaults) apply live; renames need REMOVE + re-DEFINE plus a data copy —
write that migration as a script, not by hand.

## License note

SurrealDB server is Business Source License 1.1 (verified against the
`v4.0.1` release text), converting to Apache 2.0 on its change date. That
permits running and modifying it as this game's backend; it forbids offering
SurrealDB itself as a hosted database service. Accepted as the project
decision; revisit if the licensing requirement changes.

## Layout

- `schema.surql` — tables, indexes, accesses, row rules (template with `__AUTH_*` placeholders, rendered at bootstrap from `AuthConfig`)
- `src/lib.rs` — config (`SurrealConfig` + `AuthConfig`), row types, validators, generic `AuthClaims`, subscription events
- `src/auth.rs` — **auth provider abstraction** (`AuthProvider`, `AuthConfig`, `AuthClaims`, `AUDIENCE` handling, `render_schema`). **Swap the provider without touching any other file.**
- `src/reducers.rs` — **the reducer registry: every mutation and read, once**
- `src/backend.rs` — service-role execution of the registry
- `src/codegen.rs` + `src/bin/codegen.rs` — renders `clients/rust/`
- `clients/rust/` — generated `party-dungeon-client` (checked in, never hand-edited)
- `tests/` — `live.rs` (Backend), `rules.rs` (row rules), `client.rs` (generated client)
- `docker-compose.yml`, `.env.example` — local stack
- `scripts/` — `bootstrap.sh` (provider-agnostic, calls `gen-schema`), `backup.sh`, `dev-user.sh`, `firebase-claims.mjs` (Firebase example; other providers use their own hook), `firebase/` (Admin SDK helper)
