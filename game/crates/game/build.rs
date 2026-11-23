// Build stamp plumbing.
//
// `scripts/build-web.sh` picks one stamp per build and uses it for two things:
// it is written into `web/pkg/build.json` (the pages read it to choose the
// immutable `pkg/<stamp>/` directory) and it is compiled into the wasm as
// `net_client::CLIENT_BUILD` (printed in the in-game HUD, so a screenshot says
// which build the browser is actually running). Both come from this one
// variable, so they cannot drift — the old scheme had to hand-edit a constant
// in the Rust source and a matching one in `game.html`, and a forgotten bump
// would either report the wrong build or, now that the package directory is
// cached as immutable, serve a stale wasm.
//
// Native/dev builds (plain `cargo run`, tests) do not set the variable, so
// `CLIENT_BUILD` falls back to "dev".
fn main() {
    println!("cargo:rerun-if-env-changed=PARTY_DUNGEON_BUILD");
    if let Ok(stamp) = std::env::var("PARTY_DUNGEON_BUILD") {
        // Empty would be worse than absent: the fallback is clearer.
        if !stamp.trim().is_empty() {
            println!("cargo:rustc-env=PARTY_DUNGEON_BUILD={}", stamp.trim());
        }
    }
}
