# party-dungeon-client

Generated direct-connect client for Party Dungeon. **Do not edit by hand** —
it is rendered from `party-dungeon-server/src/reducers.rs`:

```sh
cargo run -p party-dungeon-server --bin codegen
```

Every method issues the same SurrealQL as the backend's reducer of the same
name. Connect with an auth token (JWT) from the configured provider
(Firebase, Auth0, Clerk, …) via `Client::connect_with_token`; the
row rules in `schema.surql` constrain what that token may touch.
