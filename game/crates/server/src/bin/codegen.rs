//! Render the standalone Rust client package (`clients/rust/`).
//!
//! ```sh
//! cargo run -p party-dungeon-server --bin codegen
//! ```

fn main() {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("clients/rust");
    let files = party_dungeon_server::codegen::render_rust_client();
    for (rel, content) in &files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create client dir");
        }
        std::fs::write(&path, content).expect("write client file");
        println!("wrote {}", path.display());
    }
}
