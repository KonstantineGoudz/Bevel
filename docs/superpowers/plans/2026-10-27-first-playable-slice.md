# First Playable Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Bevy 0.20 workspace that boots a top-down 3D scene with a movable pawn and a portal that starts a new run, building on all six targets: Windows, macOS, Linux, Web, iOS, Android.

**Architecture:** One Cargo workspace at `game/`, one crate (`crates/game`) that is both a `lib` and a `cdylib` so the same entry point serves desktop, web, and mobile. Logic lives in small plugins (`scene`, `player`, `portal`, `hud`) over a pure-math `sim` module that unit-tests without a GPU. The server crate is deliberately absent until the netcode plan; the workspace exists so adding `crates/server` later is a two-line change.

**Tech Stack:** Rust 1.97.1, Bevy 0.20.0, wasm-bindgen + `wasm32-unknown-unknown`, `cargo-ndk` + Gradle (Android, GameActivity), Xcode project shell (iOS), Astro notes site (unchanged, moved to `notes/`).

**Spec:** `notes/src/pages/index.astro`, `notes/src/pages/mechanics/index.astro`, and the four pages under `notes/src/pages/mechanics/` — the design notes this slice starts to make real.

## Global Constraints

- Bevy version is pinned `=0.20.0`; Bevy's MSRV is `1.97.1`, so the toolchain is updated before any build.
- Rust edition `2024`.
- Crate name `party-dungeon`; library name `party_dungeon`; Android application id and iOS bundle id `org.partydungeon.game`; Android `android:value` / `System.loadLibrary` name `party_dungeon`.
- No external art assets in this slice — every mesh is a Bevy primitive, every colour is a literal.
- Tests must run with no window and no GPU: they construct `App::new()`, never `DefaultPlugins`.
- All internal web paths are relative (`./pkg/...`), because the game will be served from a subpath.
- Port 4099 belongs to the notes site. Do not change it.
- Windows, macOS, and iOS cannot be built on this Linux machine; their proof is Task 10's CI matrix. Do not claim those targets work before CI is green.

## Review Focus

- `cargo test` on a headless machine: tests must never construct a renderer or window, or CI dies on the first push.
- Two direction keys held at once: diagonal speed must equal straight-line speed, not 1.41×.
- Standing still on the portal: the run counter must not tick every frame.
- The spawn point and the portal radius: a fresh run must not start already completed.
- The web build served from a subpath: the page must not reference absolute `/pkg/` paths.

---

### Task 1: Repo layout

**Files:**
- Move: `astro.config.mjs`, `package.json`, `package-lock.json`, `src/`, `.gitignore` → `notes/`
- Create: `.gitignore` (root), `docs/` (plan lives here)

**Interfaces:**
- Produces: `notes/` = the Astro design site, runnable with `npm run dev` from inside it; repo root reserved for `game/`, `notes/`, `docs/`.

- [ ] **Step 1: Initialise the repo and commit the current state**

```bash
cd /home/kon/dev/bevy
git init -b main
git add -A
git commit -m "chore: import design notes site"
```

- [ ] **Step 2: Move the notes site**

```bash
cd /home/kon/dev/bevy
mkdir -p notes
git mv astro.config.mjs package.json package-lock.json src .gitignore notes/
mv node_modules dist .astro notes/ 2>/dev/null || true
```

- [ ] **Step 3: Write the root `.gitignore`**

```gitignore
# Rust
/target/
**/target/

# Node (notes/)
notes/node_modules/
notes/dist/
notes/.astro/

# Android
game/android/.gradle/
game/android/local.properties
game/android/app/build/
game/android/app/src/main/jniLibs/

# iOS
game/ios/build/
game/ios/DerivedData/

# Web
game/web/pkg/

# Editors
.idea/
.vscode/
```

- [ ] **Step 4: Verify the notes site still builds from its new home**

Run: `cd notes && npm ci && npm run build`
Expected: `3 page(s) built` (or more — the count is whatever `src/pages` holds) and exit 0.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: move design notes to notes/"
```

---

### Task 2: Workspace and a window that boots

**Files:**
- Create: `game/Cargo.toml`, `game/crates/game/Cargo.toml`
- Create: `game/crates/game/src/lib.rs`, `game/crates/game/src/main.rs`

**Interfaces:**
- Produces: `party_dungeon::main()` — the single entry point, `pub`, annotated `#[bevy_main]`; `party_dungeon::GamePlugin` — a `Plugin` that later tasks add systems to.
- Produces: `cargo run -p party-dungeon` opens a window titled `Party Dungeon`.

- [ ] **Step 1: Install the toolchain and Linux build dependencies**

```bash
rustup update stable
rustc --version

sudo apt-get update
sudo apt-get install -y g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev
```

Expected: `rustc 1.97.1` or newer, then exit 0 from apt.

This machine is missing `libudev-dev` (checked: `pkg-config --exists libudev` fails), and Bevy's default `bevy_gilrs` feature will not compile without it. `sudo` here needs a password, so ask your human partner to run the apt block. If the toolchain cannot be updated, stop and report — nothing else in this plan builds.

- [ ] **Step 2: Create the workspace manifest**

`game/Cargo.toml`:

```toml
[workspace]
members = ["crates/game"]
resolver = "3"

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"

[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3

[profile.release]
strip = true

[profile.wasm-release]
inherits = "release"
opt-level = "s"
lto = true
codegen-units = 1
```

- [ ] **Step 3: Create the crate manifest**

`game/crates/game/Cargo.toml`:

```toml
[package]
name = "party-dungeon"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[lib]
name = "party_dungeon"
crate-type = ["lib", "cdylib"]

[dependencies]
bevy = { version = "=0.20.0", features = ["android-game-activity"] }
```

- [ ] **Step 4: Write the entry point and an empty plugin**

`game/crates/game/src/lib.rs`:

```rust
//! Party Dungeon — top-down 3D multiplayer party crawler.

use bevy::prelude::*;

/// Entry point for every target. `bevy_main` generates the Android glue.
#[bevy_main]
pub fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Party Dungeon".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(GamePlugin)
        .run();
}

/// Everything the game adds to the default app.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, _app: &mut App) {}
}
```

`game/crates/game/src/main.rs`:

```rust
fn main() {
    party_dungeon::main();
}
```

- [ ] **Step 5: Build it**

Run: `cd game && cargo build -p party-dungeon`
Expected: exit 0. First build compiles the engine; 5–15 minutes on this machine is normal.

- [ ] **Step 6: Run it**

Run: `cargo run -p party-dungeon`
Expected: a 1280×720 window titled `Party Dungeon` with a plain clear colour. Close it.

- [ ] **Step 7: Commit**

```bash
git add game
git commit -m "feat: bevy workspace that opens a window"
```

---

### Task 3: The scene

**Files:**
- Create: `game/crates/game/src/scene.rs`
- Modify: `game/crates/game/src/lib.rs` (add `mod scene;`, add `ScenePlugin`)

**Interfaces:**
- Produces: `scene::ScenePlugin`; spawns the camera, the key light, and a 40×40 ground plane in `Startup`.
- Produces: `scene::CAMERA_YAW: f32` (= `0.0`) — the camera's yaw about Y, which Task 4's movement consumes.

- [ ] **Step 1: Write the scene plugin**

`game/crates/game/src/scene.rs`:

```rust
use bevy::prelude::*;

/// Yaw of the fixed top-down camera, in radians. Movement is relative to it.
pub const CAMERA_YAW: f32 = 0.0;

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.08)))
            .add_systems(Startup, spawn_scene);
    }
}

fn spawn_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 14.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(40.0, 40.0))),
        MeshMaterial3d(materials.add(StandardMaterial::from(Color::srgb(0.10, 0.12, 0.16)))),
    ));
}
```

Never set `PerspectiveProjection::aspect_ratio` explicitly; the default follows the window, which is what makes resizing and phone rotation work.

- [ ] **Step 2: Register it**

In `lib.rs`: `mod scene;` and `.add_plugins(scene::ScenePlugin)` after `GamePlugin`.

- [ ] **Step 3: Verify by eye**

Run: `cargo run -p party-dungeon`
Expected: dark ground plane in perspective from above, lit from the upper right. Close it.

- [ ] **Step 4: Commit**

```bash
git add game
git commit -m "feat: top-down 3d scene with ground and key light"
```

---

### Task 4: The pawn moves

**Files:**
- Create: `game/crates/game/src/sim.rs`, `game/crates/game/src/player.rs`
- Modify: `game/crates/game/src/lib.rs` (add `mod sim; mod player;`, add `PlayerPlugin`)

**Interfaces:**
- Produces: `sim::planar_move(input: Vec2, yaw: f32, speed: f32, dt: f32) -> Vec3` — normalized, yaw-rotated, scaled displacement on the XZ plane.
- Produces: `sim::PLAYER_SPEED: f32 = 6.0`.
- Produces: `player::Player` marker component; `player::PlayerPlugin` spawns a capsule at `sim::SPAWN` and moves it from WASD/arrow input.

- [ ] **Step 1: Write the failing tests**

`game/crates/game/src/sim.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn no_input_does_not_move() {
        assert_eq!(planar_move(Vec2::ZERO, 0.0, 6.0, 0.5), Vec3::ZERO);
    }

    #[test]
    fn diagonal_is_not_faster_than_straight() {
        let diag = planar_move(Vec2::new(1.0, 1.0), 0.0, 6.0, 0.5);
        assert!((diag.length() - 3.0).abs() < 1e-5, "got {}", diag.length());
    }

    #[test]
    fn yaw_rotates_the_input() {
        let east = planar_move(Vec2::new(1.0, 0.0), 0.0, 1.0, 1.0);
        let south = planar_move(Vec2::new(1.0, 0.0), std::f32::consts::FRAC_PI_2, 1.0, 1.0);
        assert!((east.x - 1.0).abs() < 1e-5);
        assert!((south.z + 1.0).abs() < 1e-5, "got {south:?}");
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cd game && cargo test -p party-dungeon sim::`
Expected: compile error, `cannot find function planar_move`.

- [ ] **Step 3: Implement the maths**

```rust
use bevy::prelude::*;

pub const PLAYER_SPEED: f32 = 6.0;
pub const SPAWN: Vec3 = Vec3::new(0.0, 1.0, 0.0);

/// `input` is screen-space (x right, y up). Forward is `-Z` before yaw.
pub fn planar_move(input: Vec2, yaw: f32, speed: f32, dt: f32) -> Vec3 {
    let dir = Vec2::new(input.x, -input.y).normalize_or_zero();
    let rotated = Vec2::from_angle(yaw).rotate(dir);
    Vec3::new(rotated.x, 0.0, rotated.y) * speed * dt
}
```

- [ ] **Step 4: Run the tests again**

Run: `cargo test -p party-dungeon sim::`
Expected: 3 passed.

- [ ] **Step 5: Write the plugin**

`player.rs`: `Player` marker; `PlayerPlugin` adds a `Startup` system that spawns
`(Player, Mesh3d(Capsule3d::new(0.35, 1.0)), MeshMaterial3d(amber), Transform::from_translation(sim::SPAWN))`
and an `Update` system:

```rust
fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    let x = axis(&keys, KeyCode::KeyD, KeyCode::KeyA);
    let y = axis(&keys, KeyCode::KeyW, KeyCode::KeyS);
    let delta = sim::planar_move(Vec2::new(x, y), scene::CAMERA_YAW, sim::PLAYER_SPEED, time.delta_secs());
    for mut transform in &mut players {
        transform.translation += delta;
    }
}

fn axis(keys: &ButtonInput<KeyCode>, positive: KeyCode, negative: KeyCode) -> f32 {
    (keys.pressed(positive) as i32 - keys.pressed(negative) as i32) as f32
}
```

- [ ] **Step 6: Verify by hand**

Run: `cargo run -p party-dungeon`
Expected: WASD and arrows move the capsule across the ground; no diagonal speed-up; it does not move while a key is held after window focus is lost.

- [ ] **Step 7: Commit**

```bash
git add game
git commit -m "feat: camera-relative pawn movement"
```

---

### Task 5: The portal starts a new run

**Files:**
- Create: `game/crates/game/src/portal.rs`
- Modify: `game/crates/game/src/lib.rs` (add `mod portal;`, add `PortalPlugin`)
- Modify: `game/crates/game/src/player.rs` (use `sim::SPAWN`; already does)

**Interfaces:**
- Produces: `portal::PORTAL_POSITION: Vec3 = Vec3::new(6.0, 0.05, 0.0)`, `portal::PORTAL_RADIUS: f32 = 1.5`.
- Produces: `portal::portal_touched(player: Vec3, portal: Vec3) -> bool` — ignores Y, true when within `PORTAL_RADIUS`.
- Produces: `portal::RunCount(pub u32)` resource, `portal::PortalPlugin` which teleports the player to `sim::SPAWN` and increments `RunCount` on entry.

- [ ] **Step 1: Write the failing tests**

`game/crates/game/src/portal.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn spawn_is_not_on_the_portal() {
        assert!(!portal_touched(sim::SPAWN, PORTAL_POSITION));
    }

    #[test]
    fn standing_on_the_portal_starts_exactly_one_run() {
        let mut app = App::new();
        app.insert_resource(RunCount(0))
            .add_systems(Update, reset_run);
        let player = app
            .world_mut()
            .spawn((Player, Transform::from_translation(PORTAL_POSITION)))
            .id();

        app.update();
        app.update();

        assert_eq!(app.world().resource::<RunCount>().0, 1);
        assert_eq!(
            app.world().entity(player).get::<Transform>().unwrap().translation,
            sim::SPAWN
        );
    }
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p party-dungeon portal::`
Expected: compile error, `PORTAL_POSITION` not found.

- [ ] **Step 3: Implement the portal**

```rust
pub const PORTAL_POSITION: Vec3 = Vec3::new(6.0, 0.05, 0.0);
pub const PORTAL_RADIUS: f32 = 1.5;

#[derive(Resource, Default)]
pub struct RunCount(pub u32);

pub fn portal_touched(player: Vec3, portal: Vec3) -> bool {
    Vec2::new(player.x, player.z).distance(Vec2::new(portal.x, portal.z)) < PORTAL_RADIUS
}

fn reset_run(mut runs: ResMut<RunCount>, mut players: Query<&mut Transform, With<Player>>) {
    for mut transform in &mut players {
        if portal_touched(transform.translation, PORTAL_POSITION) {
            transform.translation = sim::SPAWN;
            runs.0 += 1;
        }
    }
}
```

`PortalPlugin`: adds `reset_run` to `Update`, initialises `RunCount(0)`, and spawns the visible portal — a `Cylinder::new(PORTAL_RADIUS, 0.05)` disc with an emissive violet material, plus a `Cylinder::new(0.15, 1.2)` post so it reads as a gate.

`portal.rs` imports `crate::player::Player` and `crate::sim`; the test module gets both through `use super::*`.

Order `reset_run` after `player::move_player` with `.after(player::move_player)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p party-dungeon portal::`
Expected: 2 passed. The second one proves the counter does not tick twice: after the first update the player is at `SPAWN`, off the portal.

- [ ] **Step 5: Verify by hand**

Run: `cargo run -p party-dungeon`
Expected: walking onto the violet disc snaps the capsule back to the middle of the ground. Walking around it does nothing.

- [ ] **Step 6: Commit**

```bash
git add game
git commit -m "feat: portal resets the run"
```

---

### Task 6: The run counter is visible

**Files:**
- Create: `game/crates/game/src/hud.rs`
- Modify: `game/crates/game/src/lib.rs` (add `mod hud;`, add `HudPlugin`)

**Interfaces:**
- Produces: `hud::HudPlugin`; a top-left text node reading `Run 0`, rewritten whenever `portal::RunCount` changes.

- [ ] **Step 1: Write the HUD plugin**

`HudPlugin` spawns, in `Startup`:

```rust
commands.spawn((
    Text::new("Run 0"),
    TextFont { font_size: 28.0, ..default() },
    TextColor(Color::srgb(0.90, 0.93, 0.96)),
    Node {
        position_type: PositionType::Absolute,
        top: Val::Px(16.0),
        left: Val::Px(20.0),
        ..default()
    },
    HudLabel,
));
```

and an `Update` system `sync_hud` that runs only `if runs.is_changed()` and rewrites the text to `format!("Run {}", runs.0)`.

- [ ] **Step 2: Verify by hand**

Run: `cargo run -p party-dungeon`
Expected: `Run 0` in the top left; walking through the portal changes it to `Run 1`, then `Run 2`.

- [ ] **Step 3: Commit**

```bash
git add game
git commit -m "feat: run counter hud"
```

---

### Task 7: Web build

**Files:**
- Modify: `game/web/index.html` (landing page already exists — this task mounts the game into it)
- Create: `game/scripts/build-web.sh`, `game/scripts/serve-web.sh` (both may already exist; reconcile rather than overwrite)
- Modify: nothing in Rust

**Interfaces:**
- Produces: `game/scripts/build-web.sh` → `game/web/pkg/party_dungeon.js` + `party_dungeon_bg.wasm`, loadable from any static host with no server config.
- Produces: `game/scripts/serve-web.sh` → the site on `http://localhost:8082/`.

- [ ] **Step 1: Add the wasm target and the bindgen CLI**

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --locked
wasm-bindgen --version
```

Expected: a version line. The CLI version must match the `wasm-bindgen` version Bevy resolves (wasm-bindgen errors loudly if it does not; if that happens, `cargo install wasm-bindgen-cli --version <that-version>`).

- [ ] **Step 2: Mount the game into the landing page**

`game/web/index.html` already exists as a placeholder landing page (title, two inert buttons, footer). Add a full-viewport canvas container and, after the `</main>` hero, a module script that boots the engine into it:

```html
<script type="module">
  import init from "./pkg/party_dungeon.js";
  init();
</script>
```

Bevy's wasm-bindgen glue appends its own `<canvas>` to the document body, so the hero must be a sibling that sits above it, not a wrapper that clips it. Give the hero `position: relative; z-index: 1` and make the canvas `position: fixed; inset: 0`.

The import is `./pkg/...`, not `/pkg/...` — that is what lets it run from a subpath. Keep the two buttons inert; wiring "Play" to start the run is a later task.

- [ ] **Step 3: Write the build script**

`game/scripts/build-web.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --profile wasm-release --target wasm32-unknown-unknown -p party-dungeon
mkdir -p web/pkg
wasm-bindgen \
  --out-name party_dungeon \
  --out-dir web/pkg \
  --target web \
  target/wasm32-unknown-unknown/wasm-release/party_dungeon.wasm
echo "Built web/pkg — serve with: python3 -m http.server 8082 -d web"
```

`chmod +x game/scripts/build-web.sh`.

- [ ] **Step 4: Build and serve**

```bash
./scripts/build-web.sh
python3 -m http.server 8082 -d web &
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8082/            # 200
curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8082/pkg/party_dungeon.js   # 200
```

- [ ] **Step 5: Prove the paths are relative**

Run: `grep -c 'src="/' game/web/index.html game/web/pkg/party_dungeon.js`
Expected: `0` for both files. Any absolute path here breaks the itch.io upload later.

- [ ] **Step 6: Check it in a browser**

Open `http://localhost:8082/`, hold W. Expected: the capsule moves, the frame rate is stable, the console has no errors. WebGL2 is Bevy's default; do not add the `webgpu` feature for this slice.

- [ ] **Step 7: Commit**

```bash
git add game/web game/scripts
git commit -m "feat: web build with relative paths"
```

---

### Task 8: Android shell

**Files:**
- Create: `game/android/settings.gradle`, `game/android/build.gradle`, `game/android/gradle.properties`, `game/android/gradle/libs.versions.toml`, gradle wrapper (`gradlew`, `gradlew.bat`, `gradle/wrapper/*`), `game/android/local.properties`
- Create: `game/android/app/build.gradle`, `game/android/app/src/main/AndroidManifest.xml`, `game/android/app/src/main/java/org/partydungeon/game/MainActivity.java`

**Interfaces:**
- Consumes: `libparty_dungeon.so` produced by `cargo ndk` from Task 2's crate.
- Produces: `game/android/app/build/outputs/apk/debug/app-debug.apk`.

- [ ] **Step 1: Install the NDK driver**

```bash
cargo install cargo-ndk --locked
rustup target add aarch64-linux-android
```

- [ ] **Step 2: Vendor the Bevy 0.20 Gradle shell**

Copy the wrapper and manifests from Bevy's own example, then rename:

```bash
git clone --depth 1 --branch v0.20.0 https://github.com/bevyengine/bevy /tmp/opencode/bevy-src
cp -r /tmp/opencode/bevy-src/examples/mobile/android_example/gradle game/android/
cp /tmp/opencode/bevy-src/examples/mobile/android_example/gradlew* game/android/
cp /tmp/opencode/bevy-src/examples/mobile/android_example/{settings.gradle,build.gradle,gradle.properties} game/android/
```

Then edit, in `game/android`:

- `settings.gradle`: `rootProject.name = "Party Dungeon"`
- `gradle/libs.versions.toml`: unchanged (AGP 8.4.0, games-activity 4.4.0)
- `gradle.properties`: append `org.gradle.java.home=/usr/lib/jvm/java-21-openjdk-amd64`
- `local.properties` (not committed): `sdk.dir=/home/kon/Android/Sdk`

- [ ] **Step 3: Write the app module**

`game/android/app/build.gradle` — start from the example, then change:

```groovy
android {
    namespace 'org.partydungeon.game'
    compileSdk 34
    defaultConfig {
        applicationId "org.partydungeon.game"
        minSdk 31
        targetSdk 33
        versionCode 1
        versionName "0.1.0"
        ndk { abiFilters 'arm64-v8a' }
    }
    // remove the example's `sourceSets { assets.srcDirs ... }` block:
    // this slice has no asset folder.
}
```

`AndroidManifest.xml`: drop `android:icon` and `android:roundIcon` (no launcher icon yet), set `android:label="Party Dungeon"`, `.MainActivity`, and:

```xml
<meta-data android:name="android.app.lib_name" android:value="party_dungeon" />
```

`MainActivity.java`: package `org.partydungeon.game`, `System.loadLibrary("party_dungeon")`, same immersive `hideSystemUi()` body as the example.

Also create an empty `game/android/app/proguard-rules.pro` — the vendored release block references it, and Gradle fails on `assembleRelease` without it even though this task only builds debug.

- [ ] **Step 4: Cross-compile the library**

```bash
cd game
cargo ndk -t arm64-v8a -P 31 -o android/app/src/main/jniLibs build -p party-dungeon
ls android/app/src/main/jniLibs/arm64-v8a/libparty_dungeon.so
```

Expected: the `.so` exists.

- [ ] **Step 5: Build the APK**

```bash
cd game/android
./gradlew assembleDebug
ls app/build/outputs/apk/debug/app-debug.apk
```

Expected: `BUILD SUCCESSFUL` and the APK path prints. The first run downloads Gradle 8.6 and AGP.

- [ ] **Step 6: Install and run, if an emulator or device is attached**

```bash
adb devices
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n org.partydungeon.game/.MainActivity
adb logcat | grep -E 'RustStdoutStderr|bevy|wgpu'
```

Expected: the scene appears full-screen; the capsule moves with a gamepad or touch is **not** wired yet — this step proves the app launches and renders, nothing else.

- [ ] **Step 7: Commit**

```bash
git add game/android
git commit -m "feat: android gradle shell and apk build"
```

---

### Task 9: iOS shell

**Files:**
- Create: `game/ios/PartyDungeon.xcodeproj/project.pbxproj`, `game/ios/PartyDungeon.xcodeproj/xcshareddata/xcschemes/PartyDungeon.xcscheme`
- Create: `game/ios/Info.plist`, `game/ios/Makefile`, `game/ios/build_rust_deps.sh`

**Interfaces:**
- Produces: an Xcode project whose "Run Script" phase invokes `cargo build --target $IOS_TARGETS`, linking `libparty_dungeon.a` from `game/target/<target>/debug/`.

- [ ] **Step 1: Vendor the Bevy example's Xcode project**

```bash
cp -r /tmp/opencode/bevy-src/examples/mobile/bevy_mobile_example.xcodeproj game/ios/PartyDungeon.xcodeproj
cp /tmp/opencode/bevy-src/examples/mobile/ios-src/Info.plist game/ios/Info.plist
cp /tmp/opencode/bevy-src/examples/mobile/build_rust_deps.sh game/ios/
cp /tmp/opencode/bevy-src/examples/mobile/Makefile game/ios/
```

- [ ] **Step 2: Rename every reference**

```bash
cd game/ios
grep -rl bevy_mobile_example . | xargs sed -i 's/bevy_mobile_example/party_dungeon/g'
grep -rl 'BevyExample\|bevy.example' . | xargs sed -i 's/org\.bevyengine\.example/org.partydungeon.game/g'
```

Then hand-edit `Info.plist`: `CFBundleIdentifier` = `org.partydungeon.game`, `CFBundleDisplayName` = `Party Dungeon`, and `build_rust_deps.sh` so the crate path is `../crates/game` and the library is `party_dungeon`.

- [ ] **Step 3: Verify the references (the only check possible on Linux)**

```bash
grep -c party_dungeon game/ios/PartyDungeon.xcodeproj/project.pbxproj
grep -c bevy_mobile_example game/ios/PartyDungeon.xcodeproj/project.pbxproj
```

Expected: a positive count, then `0`. If the second number is not 0, the rename is incomplete.

- [ ] **Step 4: Stop and hand over**

iOS cannot be built on this machine. Do not claim iOS support. The CI job in Task 10 is the proof; until it is green, this task's deliverable is "shell vendored and renamed".

- [ ] **Step 5: Commit**

```bash
git add game/ios
git commit -m "feat: ios xcode shell (unverified until macos ci)"
```

---

### Task 10: CI matrix — the only proof for Windows, macOS, iOS

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: four jobs — `test` (ubuntu: `cargo test`), `desktop` (windows, macos, ubuntu: `cargo build --release`), `wasm` (ubuntu: build-web.sh), `android` (ubuntu: `cargo ndk` + `./gradlew assembleDebug`).

- [ ] **Step 1: Write the workflow**

```yaml
name: ci
on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: sudo apt-get update && sudo apt-get install -y libasound2-dev libudev-dev
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --workspace
        working-directory: game

  desktop:
    strategy:
      matrix:
        os: [windows-latest, macos-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo build --release -p party-dungeon
        working-directory: game

  wasm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - run: cargo install wasm-bindgen-cli --locked
      - run: ./scripts/build-web.sh
        working-directory: game

  android:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: aarch64-linux-android
      - run: cargo install cargo-ndk --locked
      - run: cargo ndk -t arm64-v8a -P 31 -o android/app/src/main/jniLibs build -p party-dungeon
        working-directory: game
      - run: ./gradlew assembleDebug
        working-directory: game/android
```

- [ ] **Step 2: Prove it locally as far as possible**

Run: `cd game && cargo test --workspace`
Expected: all tests pass. The workflow's other jobs cannot run locally; that is the point of the workflow.

- [ ] **Step 3: Commit and push**

```bash
git add .github
git commit -m "ci: build desktop, wasm and android"
```

Pushing needs a remote. If there is none, leave the commit local and report that Windows/macOS/iOS remain unproven.

---

### Task 11: The README says how to run it everywhere

**Files:**
- Create: `game/README.md`
- Modify: `README.md` (root) — three lines: what this repo is, `notes/` is the design, `game/` is the game

**Interfaces:**
- Produces: one command per target that a fresh clone can copy.

- [ ] **Step 1: Write `game/README.md`**

Sections, one command block each, with the prerequisites inline:

- Desktop: `cargo run -p party-dungeon`
- Web: `./scripts/build-web.sh` then `python3 -m http.server 8082 -d web`
- Android: the two commands from Task 8 plus `adb install`
- iOS: `cd ios && make run` (macOS only)
- Tests: `cargo test --workspace`
- Toolchain notes: Bevy 0.20 needs Rust ≥ 1.97.1; `wasm-bindgen-cli` must match; Android needs `sdk.dir` and JDK 21.

- [ ] **Step 2: Verify every code block is copy-pasteable**

For each block, run it from a clean shell in `game/` (skip iOS). Expected: exit 0, or the documented output.

- [ ] **Step 3: Commit**

```bash
git add README.md game/README.md
git commit -m "docs: how to run the game on every target"
```

---

## What this slice deliberately leaves out

- Party, classes, skills, crafting UI, loot, procedural maps, netcode — all later plans.
- Touch and gamepad input (Android/iOS currently render and move only with a keyboard, which mobile does not have).
- A launcher icon, splash screen, and audio.
- Asset pipeline: nothing is loaded from disk, so WASM file-loading quirks cannot bite yet.
