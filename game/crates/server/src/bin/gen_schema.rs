//! Print the rendered `schema.surql` for the current `AuthConfig`.
//! Used by `scripts/bootstrap.py` so the Python side never hard-codes a
//! provider. Run: `cargo run -p party-dungeon-server --bin gen-schema`

use party_dungeon_server::AuthConfig;

fn main() {
    let cfg = AuthConfig::from_env().unwrap_or_else(|e| {
        eprintln!("gen-schema: {e}");
        std::process::exit(1);
    });
    let template_path = concat!(env!("CARGO_MANIFEST_DIR"), "/schema.surql");
    let template = std::fs::read_to_string(template_path).expect("schema.surql");
    let rendered = cfg.render_schema(&template);
    print!("{rendered}");
}
