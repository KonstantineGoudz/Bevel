//! Headless GPU mode: the real game with no window, rendering to an offscreen
//! image.
//!
//! Selected on native builds by `--headless` (or `PARTY_DUNGEON_HEADLESS=1`).
//! The window is gone but the *renderer is not*: Bevy still creates the wgpu
//! device and runs every GPU system (PBR, UI, glTF, the grass compute
//! pipeline), just into an [`Image`] render target instead of a swapchain.
//! That is what makes the mode useful for driving the game without a browser:
//! the live MCP bridge keeps working, and `GET /screenshot` on the bridge can
//! return the current frame as a PNG.
//!
//! The mode is intentionally a thin shell around the normal app:
//!
//! * `WinitPlugin` is disabled and `ScheduleRunnerPlugin` becomes the runner, so
//!   no window, display server, or event loop is ever created.
//! * Every camera whose target is a window is retargeted to the offscreen image
//!   ([`HeadlessPlugin`], `PostStartup`). The minimap camera is dropped instead:
//!   its viewport and order are derived from the primary window every frame.
//! * The auth overlay is suppressed (there is no keyboard to log in with) and
//!   `DUNGEON_AUTH_REQUIRED` defaults to off unless the environment says
//!   otherwise — see `crate::auth::auth_required`.
//!
//! Configuration (a command-line flag wins over its environment variable):
//!
//! | flag | env | default |
//! |------|-----|---------|
//! | `--headless` | `PARTY_DUNGEON_HEADLESS` | off |
//! | `--headless-size 1280x720` | `PARTY_DUNGEON_HEADLESS_SIZE` | `1280x720` |
//! | `--headless-fps 60` | `PARTY_DUNGEON_HEADLESS_FPS` | `60` |
//! | `--headless-frames 300` | `PARTY_DUNGEON_HEADLESS_FRAMES` | none (run until quit) |
//! | `--headless-seconds 60` | `PARTY_DUNGEON_HEADLESS_SECONDS` | none (run until quit) |
//!
//! `--headless=1600x900` is accepted as shorthand for both flags at once. The
//! two bounds are independent: whichever comes first ends the run, which lets
//! CI cap wall-clock time on a slow (software-rendered) machine while still
//! allowing a frame count on a normal GPU.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::camera::{Camera, RenderTarget};
use bevy::diagnostic::FrameCount;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::window::screenshot::{Screenshot, ScreenshotCaptured};

use crate::editor::minimap::MinimapCamera;

/// Default offscreen viewport width, in physical pixels.
pub const DEFAULT_WIDTH: u32 = 1280;
/// Default offscreen viewport height, in physical pixels.
pub const DEFAULT_HEIGHT: u32 = 720;
/// Default simulation/render rate of the headless loop.
pub const DEFAULT_FPS: u32 = 60;
/// Smallest accepted render-target dimension.
pub const MIN_DIMENSION: u32 = 16;
/// Largest accepted render-target dimension (8K, with room to spare).
pub const MAX_DIMENSION: u32 = 8192;
/// Highest accepted loop rate.
pub const MAX_FPS: u32 = 240;
/// How long an armed capture waits for its frame's readback. Generous because
/// a software renderer (llvmpipe) can spend seconds compiling pipelines on the
/// first frames; on a real GPU this finishes in well under a frame.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the bridge keeps a `/screenshot` HTTP request open. Covers the
/// startup window before the first frame (captures are only armed once the
/// renderer has drawn one) plus [`CAPTURE_TIMEOUT`].
pub const CAPTURE_BRIDGE_TIMEOUT: Duration = Duration::from_secs(12);
/// Most screenshots that may queue up before the bridge starts refusing them.
pub const MAX_PENDING_CAPTURES: usize = 8;

/// Process-wide marker set once at boot, so systems layered *above* the boot
/// path (`auth_required`, the auth UI) can adapt without threading a `Boot`
/// resource through every call site. Pure parsing uses [`HeadlessConfig::parse`]
/// and does not touch it.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether this process was started in headless GPU mode.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Headless render/loop configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessConfig {
    /// Offscreen render-target width in physical pixels.
    pub width: u32,
    /// Offscreen render-target height in physical pixels.
    pub height: u32,
    /// Target loop rate for the `ScheduleRunnerPlugin`.
    pub fps: u32,
    /// Exit after this many rendered frames (`None` = no frame bound).
    /// Handy for smoke tests and CI, where waiting forever is not an option.
    pub exit_after_frames: Option<u64>,
    /// Exit after this many wall-clock seconds (`None` = no time bound). The
    /// frame rate depends on the GPU, so a wall-clock cap keeps CI bounded on
    /// software renderers; whichever bound is reached first wins.
    pub exit_after_seconds: Option<u32>,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            fps: DEFAULT_FPS,
            exit_after_frames: None,
            exit_after_seconds: None,
        }
    }
}

impl HeadlessConfig {
    /// Build a config, clamping the size and rate into the accepted range so a
    /// hand-written value can never create a degenerate render target.
    pub fn new(width: u32, height: u32, fps: u32) -> Self {
        Self {
            width: width.clamp(MIN_DIMENSION, MAX_DIMENSION),
            height: height.clamp(MIN_DIMENSION, MAX_DIMENSION),
            fps: fps.clamp(1, MAX_FPS),
            exit_after_frames: None,
            exit_after_seconds: None,
        }
    }

    /// Parse the process environment and command line. Returns `None` when the
    /// mode is not enabled; marks [`is_active`] when it is.
    pub fn from_env() -> Option<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let config = Self::parse(&args, |key| std::env::var(key).ok());
        if config.is_some() {
            ACTIVE.store(true, Ordering::Relaxed);
        }
        config
    }

    /// Pure parser: `args` excludes the program name, `env` looks up a variable.
    /// Invalid values are ignored (the previous/default value is kept) and
    /// warned about, so a typo never silently disables the mode.
    pub fn parse(args: &[String], env: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let mut config = Self::default();
        let mut enabled = env("PARTY_DUNGEON_HEADLESS").is_some_and(|v| truthy(&v));

        if let Some(value) = env("PARTY_DUNGEON_HEADLESS_SIZE") {
            apply_size(&mut config, &value, "PARTY_DUNGEON_HEADLESS_SIZE");
        }
        if let Some(value) = env("PARTY_DUNGEON_HEADLESS_FPS") {
            apply_fps(&mut config, &value, "PARTY_DUNGEON_HEADLESS_FPS");
        }
        if let Some(value) = env("PARTY_DUNGEON_HEADLESS_FRAMES") {
            apply_frames(&mut config, &value, "PARTY_DUNGEON_HEADLESS_FRAMES");
        }
        if let Some(value) = env("PARTY_DUNGEON_HEADLESS_SECONDS") {
            apply_seconds(&mut config, &value, "PARTY_DUNGEON_HEADLESS_SECONDS");
        }

        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_owned())),
                None => (arg, None),
            };
            match flag {
                "--headless" => {
                    enabled = true;
                    // `--headless=1600x900` is a size shorthand.
                    if let Some(value) = inline {
                        apply_size(&mut config, &value, "--headless");
                    }
                }
                "--headless-size" => {
                    if let Some(value) = take_value(args, &mut index, inline) {
                        apply_size(&mut config, &value, "--headless-size");
                    }
                }
                "--headless-fps" => {
                    if let Some(value) = take_value(args, &mut index, inline) {
                        apply_fps(&mut config, &value, "--headless-fps");
                    }
                }
                "--headless-frames" => {
                    if let Some(value) = take_value(args, &mut index, inline) {
                        apply_frames(&mut config, &value, "--headless-frames");
                    }
                }
                "--headless-seconds" => {
                    if let Some(value) = take_value(args, &mut index, inline) {
                        apply_seconds(&mut config, &value, "--headless-seconds");
                    }
                }
                _ => {}
            }
            index += 1;
        }

        enabled.then_some(config)
    }
}

fn take_value(args: &[String], index: &mut usize, inline: Option<String>) -> Option<String> {
    match inline {
        Some(value) => Some(value),
        None => {
            *index += 1;
            args.get(*index).cloned()
        }
    }
}

fn apply_size(config: &mut HeadlessConfig, value: &str, source: &str) {
    match parse_size(value) {
        Ok((width, height)) => {
            config.width = width;
            config.height = height;
        }
        Err(error) => warn!("headless: ignoring {source}={value:?}: {error}"),
    }
}

fn apply_fps(config: &mut HeadlessConfig, value: &str, source: &str) {
    match parse_fps(value) {
        Ok(fps) => config.fps = fps,
        Err(error) => warn!("headless: ignoring {source}={value:?}: {error}"),
    }
}

fn apply_frames(config: &mut HeadlessConfig, value: &str, source: &str) {
    match parse_frames(value) {
        Ok(frames) => config.exit_after_frames = frames,
        Err(error) => warn!("headless: ignoring {source}={value:?}: {error}"),
    }
}

fn apply_seconds(config: &mut HeadlessConfig, value: &str, source: &str) {
    match parse_seconds(value) {
        Ok(seconds) => config.exit_after_seconds = seconds,
        Err(error) => warn!("headless: ignoring {source}={value:?}: {error}"),
    }
}

/// Parse `WIDTHxHEIGHT` (either case for the `x`). Bounds are enforced here so
/// an out-of-range value is reported instead of quietly clamped.
pub fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let value = value.trim();
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| "expected WIDTHxHEIGHT, e.g. 1280x720".to_owned())?;
    let width: u32 = width
        .trim()
        .parse()
        .map_err(|_| format!("bad width in {value:?}"))?;
    let height: u32 = height
        .trim()
        .parse()
        .map_err(|_| format!("bad height in {value:?}"))?;
    if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&width) {
        return Err(format!(
            "width {width} is outside {MIN_DIMENSION}..={MAX_DIMENSION}"
        ));
    }
    if !(MIN_DIMENSION..=MAX_DIMENSION).contains(&height) {
        return Err(format!(
            "height {height} is outside {MIN_DIMENSION}..={MAX_DIMENSION}"
        ));
    }
    Ok((width, height))
}

/// Parse a loop rate; `0` and absurd values are rejected rather than clamped.
pub fn parse_fps(value: &str) -> Result<u32, String> {
    let fps: u32 = value
        .trim()
        .parse()
        .map_err(|_| format!("bad fps {value:?}"))?;
    if !(1..=MAX_FPS).contains(&fps) {
        return Err(format!("fps {fps} is outside 1..={MAX_FPS}"));
    }
    Ok(fps)
}

/// Parse an optional frame budget. `0` means "no budget" (run until quit).
pub fn parse_frames(value: &str) -> Result<Option<u64>, String> {
    let frames: u64 = value
        .trim()
        .parse()
        .map_err(|_| format!("bad frame count {value:?}"))?;
    Ok((frames > 0).then_some(frames))
}

/// Parse an optional wall-clock budget in seconds. `0` means "no budget".
pub fn parse_seconds(value: &str) -> Result<Option<u32>, String> {
    let seconds: u32 = value
        .trim()
        .parse()
        .map_err(|_| format!("bad second count {value:?}"))?;
    Ok((seconds > 0).then_some(seconds))
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// One pending frame request: the game encodes the frame and sends it back.
pub type CaptureReply = mpsc::Sender<Result<Vec<u8>, String>>;
/// Requests waiting to be rendered, shared with the live bridge's HTTP thread.
pub type CaptureQueue = Arc<Mutex<VecDeque<CaptureReply>>>;

/// Ticket attached to an armed `Screenshot`. Captures are tagged so a frame
/// that completes after its request already timed out can never be delivered
/// to the next request's reply channel.
#[derive(Component)]
struct CaptureTicket(u64);

/// Everything the headless plugin owns.
#[derive(Resource)]
pub struct Headless {
    /// Effective configuration.
    pub config: HeadlessConfig,
    /// Offscreen target every camera renders into.
    pub target: Handle<Image>,
    /// Frames requested by the live bridge but not rendered yet.
    pub queue: CaptureQueue,
    /// The in-flight capture (ticket, reply), if a frame is being read back.
    in_flight: Option<(u64, CaptureReply)>,
    /// When the in-flight capture started, for the timeout.
    in_flight_since: Option<Instant>,
    /// Next capture ticket to hand out.
    next_ticket: u64,
}

/// Adds the offscreen render target, retargets cameras, and swaps the app runner
/// for a fixed-rate headless loop.
pub struct HeadlessPlugin {
    config: HeadlessConfig,
}

impl HeadlessPlugin {
    pub fn new(config: HeadlessConfig) -> Self {
        Self { config }
    }
}

impl Plugin for HeadlessPlugin {
    fn build(&self, app: &mut App) {
        let config = self.config.clone();
        let target = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            images.add(Image::new_target_texture(
                config.width,
                config.height,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ))
        };
        info!(
            "headless: rendering {}x{} at {} Hz to an offscreen target (no window)",
            config.width, config.height, config.fps
        );

        app.insert_resource(Headless {
            config: config.clone(),
            target,
            queue: CaptureQueue::default(),
            in_flight: None,
            in_flight_since: None,
            next_ticket: 0,
        })
        .add_systems(PostStartup, retarget_cameras)
        .add_systems(Update, (capture_requested_frame, headless_progress))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / config.fps as f64,
        )));
    }
}

/// Point every window-targeted camera at the offscreen image, and drop the
/// minimap camera. Runs once the editor has spawned its cameras (main +
/// minimap), in `PostStartup`.
fn retarget_cameras(
    mut commands: Commands,
    headless: Res<Headless>,
    mut cameras: Query<(Entity, &mut RenderTarget, Has<MinimapCamera>), With<Camera>>,
) {
    let mut retargeted = 0;
    for (entity, mut target, is_minimap) in &mut cameras {
        if is_minimap {
            // The minimap only exists to sit in a corner of the primary
            // window: `sync_minimap` derives its viewport, order and UI from
            // that window every frame, and with no window that sync never
            // runs. Left alone it would render full-screen on top of the main
            // view (same default order, same target), so drop it.
            commands.entity(entity).despawn();
            continue;
        }
        // Only replace window targets: a camera deliberately rendering to
        // another image (or to no color target) keeps its choice.
        if matches!(*target, RenderTarget::Window(_)) {
            *target = RenderTarget::Image(headless.target.clone().into());
            retargeted += 1;
        }
    }
    if retargeted > 0 {
        info!("headless: {retargeted} camera(s) now render to the offscreen target");
    }
}

/// Serve at most one queued frame per rendered frame. `Screenshot` captures a
/// single render target per frame, so requests are serialized; the bridge HTTP
/// thread blocks on its reply channel until the frame is ready.
fn capture_requested_frame(
    mut commands: Commands,
    mut headless: ResMut<Headless>,
    frames: Option<Res<FrameCount>>,
) {
    // Don't arm a capture before the renderer has drawn anything: on the first
    // frames the render app is still compiling pipelines (seconds on software
    // Vulkan), and a screenshot armed then just sits in the render graph
    // waiting. The request stays queued and the bridge's wait is sized for it.
    if frames.as_ref().is_none_or(|frames| frames.0 == 0) {
        return;
    }
    if headless.in_flight.is_some() {
        if headless
            .in_flight_since
            .is_some_and(|started| started.elapsed() > CAPTURE_TIMEOUT)
        {
            if let Some((_, reply)) = headless.in_flight.take() {
                let _ = reply.send(Err("frame capture timed out".to_owned()));
            }
            headless.in_flight_since = None;
        } else {
            return;
        }
    }
    let next = headless
        .queue
        .lock()
        .ok()
        .and_then(|mut queue| queue.pop_front());
    let Some(reply) = next else {
        return;
    };
    let ticket = headless.next_ticket;
    headless.next_ticket = headless.next_ticket.wrapping_add(1);
    headless.in_flight = Some((ticket, reply));
    headless.in_flight_since = Some(Instant::now());
    commands
        .spawn((
            Screenshot::image(headless.target.clone()),
            CaptureTicket(ticket),
        ))
        .observe(on_screenshot_captured);
}

/// Encodes the captured frame and hands the PNG bytes back to the bridge.
fn on_screenshot_captured(
    captured: On<ScreenshotCaptured>,
    tickets: Query<&CaptureTicket>,
    mut headless: ResMut<Headless>,
) {
    let Ok(ticket) = tickets.get(captured.entity) else {
        return;
    };
    // A frame whose request already timed out is stale: dropping it here keeps
    // it from answering the *next* request with an old image.
    if headless
        .in_flight
        .as_ref()
        .is_none_or(|(current, _)| *current != ticket.0)
    {
        return;
    }
    let Some((_, reply)) = headless.in_flight.take() else {
        return;
    };
    headless.in_flight_since = None;
    let result = encode_png(&captured.image);
    if let Err(error) = &result {
        warn!("headless: could not read back a frame: {error}");
    }
    let _ = reply.send(result);
}

/// Screenshot frames are `Rgba8UnormSrgb`, which `try_into_dynamic` turns into
/// a plain RGBA8 image; encode it losslessly so the bridge can stream it.
fn encode_png(image: &Image) -> Result<Vec<u8>, String> {
    let dynamic = image
        .clone()
        .try_into_dynamic()
        .map_err(|error| format!("frame is not a readable image: {error}"))?;
    let mut bytes = Vec::new();
    dynamic
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    if bytes.is_empty() {
        return Err("PNG encode produced no bytes".to_owned());
    }
    Ok(bytes)
}

/// Log progress sparsely and honour the optional frame / wall-clock budgets for
/// bounded runs (`--headless-frames`, `--headless-seconds`).
fn headless_progress(
    headless: Res<Headless>,
    frames: Option<Res<FrameCount>>,
    mut exit: MessageWriter<AppExit>,
    mut started: Local<Option<Instant>>,
) {
    let started = *started.get_or_insert_with(Instant::now);
    let Some(frames) = frames else {
        return;
    };
    let rendered = frames.0;
    if rendered > 0 && rendered.is_multiple_of(600) {
        debug!(
            "headless: {rendered} frames rendered ({}x{} @ {} Hz)",
            headless.config.width, headless.config.height, headless.config.fps
        );
    }
    let frames_done = headless
        .config
        .exit_after_frames
        .is_some_and(|limit| u64::from(rendered) >= limit);
    let time_done = headless
        .config
        .exit_after_seconds
        .is_some_and(|seconds| started.elapsed() >= Duration::from_secs(u64::from(seconds)));
    if frames_done || time_done {
        info!(
            "headless: budget reached ({rendered} frames, {:.1}s), exiting",
            started.elapsed().as_secs_f64()
        );
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str], env: &[(&str, &str)]) -> Option<HeadlessConfig> {
        let owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        HeadlessConfig::parse(&owned, |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn stays_off_without_a_flag_or_env() {
        assert_eq!(parse_args(&[], &[]), None);
        assert_eq!(parse_args(&["--viewer"], &[]), None);
        assert_eq!(
            parse_args(&[], &[("PARTY_DUNGEON_HEADLESS", "0")]),
            None,
            "an explicit false is still off"
        );
    }

    #[test]
    fn defaults_are_1280x720_at_60hz() {
        let config = parse_args(&["--headless"], &[]).expect("enabled");
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
        assert_eq!(config.fps, 60);
        assert_eq!(config.exit_after_frames, None);
        assert_eq!(config.exit_after_seconds, None);
    }

    #[test]
    fn env_enables_the_mode() {
        for value in ["1", "true", "YES", "on"] {
            assert!(
                parse_args(&[], &[("PARTY_DUNGEON_HEADLESS", value)]).is_some(),
                "{value:?} should enable headless"
            );
        }
    }

    #[test]
    fn size_accepts_both_delimiters_and_cli_wins() {
        let config = parse_args(
            &["--headless", "--headless-size", "1920x1080"],
            &[("PARTY_DUNGEON_HEADLESS_SIZE", "800x600")],
        )
        .expect("enabled");
        assert_eq!((config.width, config.height), (1920, 1080));

        let config = parse_args(&["--headless", "--headless=1024X768"], &[]).expect("enabled");
        assert_eq!((config.width, config.height), (1024, 768));
    }

    #[test]
    fn inline_flag_values_work() {
        let config = parse_args(
            &["--headless", "--headless-size=640x480", "--headless-fps=30"],
            &[],
        )
        .expect("enabled");
        assert_eq!((config.width, config.height), (640, 480));
        assert_eq!(config.fps, 30);
    }

    #[test]
    fn invalid_values_keep_the_defaults() {
        let config = parse_args(
            &[
                "--headless",
                "--headless-size",
                "nonsense",
                "--headless-fps",
                "0",
                "--headless-frames",
                "nope",
            ],
            &[],
        )
        .expect("enabled");
        assert_eq!((config.width, config.height), (1280, 720));
        assert_eq!(config.fps, 60);
        assert_eq!(config.exit_after_frames, None);
    }

    #[test]
    fn frame_budget_is_parsed_and_zero_means_unbounded() {
        let config = parse_args(&["--headless", "--headless-frames", "300"], &[]).expect("enabled");
        assert_eq!(config.exit_after_frames, Some(300));
        let config = parse_args(&["--headless", "--headless-frames", "0"], &[]).expect("enabled");
        assert_eq!(config.exit_after_frames, None);
    }

    #[test]
    fn seconds_budget_is_parsed_from_cli_and_env() {
        let config = parse_args(&["--headless", "--headless-seconds", "45"], &[]).expect("enabled");
        assert_eq!(config.exit_after_seconds, Some(45));
        let config = parse_args(
            &["--headless"],
            &[("PARTY_DUNGEON_HEADLESS_SECONDS", "90")],
        )
        .expect("enabled");
        assert_eq!(config.exit_after_seconds, Some(90));
        let config = parse_args(&["--headless", "--headless-seconds", "0"], &[]).expect("enabled");
        assert_eq!(config.exit_after_seconds, None);
    }

    #[test]
    fn parse_seconds_bounds() {
        assert_eq!(parse_seconds("60"), Ok(Some(60)));
        assert_eq!(parse_seconds("0"), Ok(None));
        assert!(parse_seconds("-1").is_err());
        assert!(parse_seconds("soon").is_err());
    }

    #[test]
    fn parse_size_bounds() {
        assert_eq!(parse_size("1280x720"), Ok((1280, 720)));
        assert_eq!(parse_size(" 1280X720 "), Ok((1280, 720)));
        assert!(parse_size("1280").is_err());
        assert!(parse_size("0x720").is_err());
        assert!(parse_size("1280x0").is_err());
        assert!(parse_size("9000x720").is_err());
        assert!(parse_size("a x b").is_err());
    }

    #[test]
    fn parse_fps_bounds() {
        assert_eq!(parse_fps("60"), Ok(60));
        assert!(parse_fps("0").is_err());
        assert!(parse_fps("241").is_err());
    }

    #[test]
    fn constructor_clamps_instead_of_panicking() {
        let config = HeadlessConfig::new(0, 0, 0);
        assert_eq!(config.width, MIN_DIMENSION);
        assert_eq!(config.height, MIN_DIMENSION);
        assert_eq!(config.fps, 1);
    }
}
