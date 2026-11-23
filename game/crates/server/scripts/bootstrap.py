#!/usr/bin/env python3
"""Bootstrap a fresh SurrealDB for Party Dungeon.

Renders `schema.surql` with the configured auth provider (see `src/auth.rs`
— `AuthConfig::render_schema`), waits for the server, loads the schema as
root, then creates the service database user. Idempotent: re-running only
tops up missing definitions.

Swap the auth provider without touching code or schema:

  # Firebase (legacy)
  DUNGEON_AUTH_PROVIDER=firebase DUNGEON_FIREBASE_PROJECT_ID=my-proj
  # Generic OIDC (Auth0, Clerk, Supabase, Okta …)
  DUNGEON_AUTH_PROVIDER=oidc DUNGEON_OIDC_ISSUER=https://my-tenant.auth0.com/ DUNGEON_OIDC_AUDIENCE=my-api DUNGEON_OIDC_JWKS_URL=https://my-tenant.auth0.com/.well-known/jwks.json
  # Local HS256 (tests, dev without external IdP)
  DUNGEON_AUTH_PROVIDER=hs256 DUNGEON_JWT_SECRET=dev-secret

Usage (from game/crates/server):
    cp .env.example .env   # then edit
    docker compose up -d
    ./scripts/bootstrap.sh
"""

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load_env(path):
    values = {}
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    key, _, val = line.partition("=")
                    values[key.strip()] = val.strip()
    except FileNotFoundError:
        pass
    return values


def render_schema_via_cargo(env):
    """Try to render the schema with the Rust binary (provider-agnostic)."""
    # Prefer the compiled binary if it exists, otherwise `cargo run`.
    # We pass the current env (merged file + os) so AuthConfig picks it up.
    cargo_toml = os.path.join(os.path.dirname(HERE), "..", "..", "Cargo.toml")
    # Use `cargo run -p party-dungeon-server --bin gen_schema` from the workspace root.
    # `HERE` is `.../crates/server`, so workspace root is two levels up.
    workspace = os.path.abspath(os.path.join(HERE, "..", ".."))
    try:
        result = subprocess.run(
            ["cargo", "run", "--quiet", "-p", "party-dungeon-server", "--bin", "gen_schema"],
            cwd=workspace,
            env={**os.environ, **env},
            capture_output=True,
            text=True,
            timeout=300,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout
        if result.stderr:
            print(f"gen-schema failed, falling back: {result.stderr[:500]}", file=sys.stderr)
    except Exception as e:
        print(f"gen-schema fallback: {e}", file=sys.stderr)
    return None


def render_schema_fallback(env):
    """Legacy fallback: string-replace for Firebase-only setups."""
    project = env.get("FIREBASE_PROJECT_ID", "") or env.get("DUNGEON_FIREBASE_PROJECT_ID", "")
    if project and project != "your-firebase-project-id":
        with open(os.path.join(HERE, "schema.surql")) as f:
            template = f.read()
        # New template uses __AUTH_* but old fallback can still handle the legacy placeholder
        # for backward compat with un-rebuilt checkouts.
        if "__AUTH_ACCESS_DEFINITION__" in template:
            # If we are here, cargo-based rendering failed but we have a Firebase project.
            # Synthesize a minimal Firebase access definition.
            access = f"DEFINE ACCESS firebase_jwt ON DATABASE TYPE JWT URL 'https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com' AUDIENCE '{project}'"
            row = f"$token.iss == 'https://securetoken.google.com/{project}' AND $token.aud == '{project}'"
            ownership = "rid = $token.sub"
            return (
                template.replace("__AUTH_ACCESS_DEFINITION__", access)
                .replace("__AUTH_ROW_PREDICATE__", row)
                .replace("__AUTH_OWNERSHIP_PREDICATE__", ownership)
                .replace("__FIREBASE_PROJECT_ID__", project)
            )
        else:
            with open(os.path.join(HERE, "schema.surql")) as f:
                return f.read().replace("__FIREBASE_PROJECT_ID__", project)
    return None


def main():
    file_env = load_env(os.path.join(HERE, ".env"))
    env = {**file_env, **os.environ}

    port = env.get("SURREAL_PORT", "6000")
    base = f"http://127.0.0.1:{port}"
    root_user = env.get("SURREAL_ROOT_USER", "root")
    root_pass = env.get("SURREAL_ROOT_PASS", "")
    ns = env.get("DUNGEON_SURREAL_NS", "party_dungeon")
    db = env.get("DUNGEON_SURREAL_DB", "main")
    svc_user = env.get("DUNGEON_SERVICE_USER", "service")
    svc_pass = env.get("DUNGEON_SERVICE_PASS", "")

    # Provider-agnostic missing check: we need a JWT provider config *or* dev_login is enough for local.
    # For bootstrap we just need root + service creds; the provider config is validated when rendering the schema.
    missing = [k for k, v in {
        "SURREAL_ROOT_PASS": root_pass,
        "DUNGEON_SERVICE_PASS": svc_pass,
    }.items() if not v or v in ("change-me-in-prod",)]
    if missing:
        print(f"Refusing to bootstrap with placeholder .env values: {', '.join(missing)}")
        print("Copy .env.example to .env and fill them in.")
        return 1

    # Render the schema via the Rust provider abstraction (handles any DUNGEON_AUTH_*).
    schema = render_schema_via_cargo(env)
    if schema is None:
        schema = render_schema_fallback(env)
    if schema is None:
        print("No auth provider configured. Set one of:")
        print("  DUNGEON_AUTH_PROVIDER=rauthy + DUNGEON_RAUTHY_URL/ISSUER/AUDIENCE/JWKS_URL")
        print("  DUNGEON_AUTH_PROVIDER=firebase + DUNGEON_FIREBASE_PROJECT_ID")
        print("  DUNGEON_AUTH_PROVIDER=oidc + DUNGEON_OIDC_ISSUER/AUDIENCE/JWKS_URL")
        print("  DUNGEON_AUTH_PROVIDER=hs256 + DUNGEON_JWT_SECRET")
        print("See src/auth.rs for the full env table.")
        print("Hint: if gen_schema timed out, pre-build once with:")
        print("  CARGO_HOME=.cargo-home cargo build -p party-dungeon-server --bin gen_schema")
        return 1

    # Wait for readiness (same probe as the compose healthcheck).
    for _ in range(60):
        try:
            with urllib.request.urlopen(f"{base}/health", timeout=3) as r:
                if r.status == 200:
                    break
        except Exception:
            time.sleep(2)
    else:
        print(f"Server at {base} never became ready.")
        return 1

    import base64

    def sql(statements):
        creds = base64.b64encode(f"{root_user}:{root_pass}".encode()).decode()
        req = urllib.request.Request(
            f"{base}/sql",
            data=statements.encode(),
            headers={
                "Content-Type": "application/surql",
                "Authorization": f"Basic {creds}",
                "surreal-ns": ns,
                "surreal-db": db,
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            try:
                body = e.read().decode()[:2000]
            except Exception:
                body = "<unreadable>"
            print(f"SurrealDB rejected the request ({e.code} {e.msg}): {body}")
            raise SystemExit(1)

    print(f"Loading schema into {ns}/{db} ...")
    results = sql(schema)
    problems = [r for r in results if isinstance(r, dict) and r.get("status") != "OK"
                and "already exists" not in str(r.get("result", ""))]
    if problems:
        print("Schema failed:")
        for p in problems:
            print(" ", json.dumps(p)[:400])
        return 1
    print(f"Schema OK ({len(results)} statements).")

    print("Creating service user ...")
    results = sql(
        f"DEFINE USER {svc_user} ON DATABASE PASSWORD '{svc_pass}' ROLES OWNER;"
    )
    problems = [r for r in results if isinstance(r, dict) and r.get("status") != "OK"
                and "already exists" not in str(r.get("result", ""))]
    if problems:
        print("Service user failed:", json.dumps(problems)[:400])
        return 1
    print(f"Service user '{svc_user}' ready.")
    print("Done. Point the app at:")
    print(f"  DUNGEON_SURREAL_URL=127.0.0.1:{port}  DUNGEON_SURREAL_NS={ns}  DUNGEON_SURREAL_DB={db}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
