//! Tiny Glade–inspired procedural building chemistry, 2D footprint booleans,
//! contextual grammar rules, and organic mesh synthesis.
//!
//! Unlike static catalogue blocks, Glade primitives (`glade_tower`, `glade_room`,
//! `glade_wall`) are continuous parametric shapes that react to their
//! surroundings in real time:
//!
//! 1. **2D Footprint Boolean Merging**: Overlapping rooms and towers compute
//!    their 2D polygon intersections, cull interior walls that lie inside
//!    neighbouring buildings, preserve upper facades when a taller structure
//!    rises out of a shorter one, and place dressed stone seam quoins along
//!    the junctions.
//! 2. **Building Chemistry**:
//!    - Paths (`Track` / `Plan`) or threshold pieces (`door`, `gate`, `bridge`)
//!      crossing a wall automatically cut a vaulted semicircular archway
//!      complete with radial voussoirs and a central keystone.
//!    - Lowering a wall below [`FENCE_MAX_HEIGHT`] transforms it into a low
//!      cobblestone garden fence with irregular coping stones and timber posts;
//!      raising it above [`KEEP_MIN_HEIGHT`] adds a flared stone base batter
//!      and crenellated battlements.
//!    - Exposed exterior wall faces automatically populate with multi-story
//!      window bays based on available surface area.
//! 3. **Handcrafted Organic Geometry**: Procedural roofs (Conical flared
//!    witch-hat, Gable with timber fascia, Hipped, and Crenellated Battlements)
//!    with overlapping shingle courses and deterministic spatial vertex jitter.

use std::hash::{Hash, Hasher};

use bevy::asset::RenderAssetUsages;
use bevy::color::Color;
use bevy::math::{Vec2, Vec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::{Mesh, Ray3d};

use crate::model::{self, Piece, Shape, TriggerShape, shapes};

/// Below or at this height (in metres), a Glade wall transitions into a low
/// cobblestone fence with rounded coping stones.
pub const FENCE_MAX_HEIGHT: f32 = 1.15;

/// Below or at this wall height (in metres), a Glade Room or Tower with a
/// pitched roof transforms into a ground-hugging **tent-like house** / A-frame
/// pavilion: eaves drop to ground level (`y = 0`), side windows and corner
/// quoins/brackets are suppressed so nothing clips through the roof slopes, and
/// timber A-frame rafters frame the gable ends.
pub const TENT_MAX_WALL_HEIGHT: f32 = 0.55;

/// Above this height (in metres), a Glade wall or building gains a fortified
/// stone base batter and castle corbelling.
pub const KEEP_MIN_HEIGHT: f32 = 2.85;

/// Minimum width/diameter for a Glade building, in metres.
pub const GLADE_MIN_SPAN: f32 = 0.6;
/// Maximum width/diameter for a Glade building, in metres.
pub const GLADE_MAX_SPAN: f32 = 24.0;
/// Minimum wall height for a Glade piece, in metres (lowered to allow squashing
/// rooms and towers all the way down into tent-like houses).
pub const GLADE_MIN_HEIGHT: f32 = 0.08;
/// Maximum wall height for a Glade piece, in metres (doubled height range).
pub const GLADE_MAX_HEIGHT: f32 = 24.0;
/// Minimum wall thickness, in metres.
pub const GLADE_MIN_THICK: f32 = 0.18;

/// Minimum horizontal span (width/length) for a `glade_terrain` chunk, in metres.
pub const TERRAIN_MIN_SPAN: f32 = 1.6;
/// Maximum horizontal span (width/length) for a `glade_terrain` chunk, in metres.
pub const TERRAIN_MAX_SPAN: f32 = 48.0;
/// Minimum vertical height for a `glade_terrain` chunk, in metres.
pub const TERRAIN_MIN_HEIGHT: f32 = 0.20;
/// Maximum vertical height for a `glade_terrain` chunk, in metres (doubled terrain height range).
pub const TERRAIN_MAX_HEIGHT: f32 = 20.0;
/// Half-extent of the continuous large-scale world meadow terrain, in metres (`280m x 280m`).
pub const WORLD_TERRAIN_HALF_EXTENT: f32 = 140.0;
/// Radius of the central flat buildable meadow clearing before outer hills rise, in metres
/// (~10x the original 15.5m-radius clearing area).
pub const FLAT_CLEARING_RADIUS: f32 = 49.0;
/// Surface normal Y threshold below which a terrain slope transitions from
/// lush grassy sward into steep craggy limestone cliff rock.
pub const TERRAIN_CLIFF_NORMAL_Y: f32 = 0.70;

/// Default warm terracotta shingle roof colour (`0xRRGGBB`).
pub const DEFAULT_ROOF_COLOR: u32 = 0xc45b43;
/// Alias for [`DEFAULT_ROOF_COLOR`].
pub const GLADE_DEFAULT_ROOF_COLOR: u32 = DEFAULT_ROOF_COLOR;
/// Default lush meadow grass colour for `glade_terrain` (`0xRRGGBB`).
pub const DEFAULT_TERRAIN_COLOR: u32 = 0x5ca046;
/// Default normalized roof pitch (`0.5` -> `1.0` slope factor).
pub const GLADE_DEFAULT_ROOF_PITCH: f32 = 0.5;

/// High-level Glade primitive kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GladeKind {
    Tower,
    Room,
    Wall,
    Door,
    Window,
    Terrain,
    Path,
}

impl GladeKind {
    pub fn from_shape(shape: &Shape) -> Option<Self> {
        glade_kind(shape)
    }
}

/// Parametric roof style for Glade buildings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GladeRoofStyle {
    /// Flared conical roof with concentric shingle rings (default for towers).
    Conical,
    /// Dual-pitched gable roof with timber fascia and triangular gable ends.
    Gable,
    /// Four-sloped hip roof with eaves overhang.
    Hipped,
    /// Flat roof deck surrounded by crenellated castle battlements.
    Battlements,
}

impl GladeRoofStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conical => "Conical",
            Self::Gable => "Gable",
            Self::Hipped => "Hipped",
            Self::Battlements => "Battlements",
        }
    }
}

/// Parametric profile style for a `glade_terrain` chunk, controlled by
/// `piece.trigger_shape_override`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GladeTerrainStyle {
    /// Rounded-rectangle terraced plateau with flat top and adjustable slope/cliff sides (`TriggerShape::Box`).
    Plateau,
    /// Smooth elliptical rolling hill mound / dome (`TriggerShape::Sphere`).
    Mound,
    /// Flat-topped circular cliff mesa / craggy rock butte (`TriggerShape::Cylinder`).
    CliffMesa,
}

impl GladeTerrainStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Plateau => "Plateau",
            Self::Mound => "Mound",
            Self::CliffMesa => "Cliff Mesa",
        }
    }
}

/// Contextual height tier for a wall or building facade.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WallTier {
    /// Low stone garden fence (`height <= FENCE_MAX_HEIGHT`).
    LowFence,
    /// Standard masonry/plaster wall.
    StandardWall,
    /// Tall fortified keep wall (`height > KEEP_MIN_HEIGHT`).
    FortifiedWall,
}

/// Classify a shape into its [`GladeKind`], if it belongs to the Glade toolset.
pub fn glade_kind(shape: &Shape) -> Option<GladeKind> {
    match shape.id {
        "glade_tower" => Some(GladeKind::Tower),
        "glade_room" => Some(GladeKind::Room),
        "glade_wall" => Some(GladeKind::Wall),
        "glade_door" => Some(GladeKind::Door),
        "glade_window" => Some(GladeKind::Window),
        "glade_terrain" => Some(GladeKind::Terrain),
        "glade_path" => Some(GladeKind::Path),
        _ => None,
    }
}

/// True when `shape` is a doorway / threshold piece that cuts archways in Glade walls.
pub fn is_door_cutter_shape(shape: &Shape) -> bool {
    matches!(shape.id, "glade_door" | "door" | "gate" | "bridge")
}

/// True when `shape` is a procedural Glade primitive.
pub fn is_glade_shape(shape: &Shape) -> bool {
    glade_kind(shape).is_some()
}

/// Classify wall height into its architectural [`WallTier`].
pub fn wall_tier(height: f32) -> WallTier {
    if height <= FENCE_MAX_HEIGHT {
        WallTier::LowFence
    } else if height > KEEP_MIN_HEIGHT {
        WallTier::FortifiedWall
    } else {
        WallTier::StandardWall
    }
}

/// Natural default parametric extents `(width, wall_height, length)` when a
/// Glade piece is first armed or placed.
pub fn default_size(shape: &Shape) -> Vec3 {
    match glade_kind(shape) {
        Some(GladeKind::Tower) => Vec3::new(2.4, 3.2, 2.4),
        Some(GladeKind::Room) => Vec3::new(3.2, 2.6, 2.4),
        Some(GladeKind::Wall) => Vec3::new(2.8, 2.2, 0.36),
        Some(GladeKind::Door) => Vec3::new(1.15, 2.1, 0.42),
        Some(GladeKind::Window) => Vec3::new(0.72, 0.92, 0.42),
        Some(GladeKind::Terrain) => Vec3::new(11.0, 2.5, 11.0),
        Some(GladeKind::Path) => Vec3::new(1.08, 0.2, 1.08),
        None => model::TRIGGER_DEFAULT_SIZE,
    }
}

/// Sanitize and clamp the parametric extents `(width, height, length)` of a
/// Glade piece.
pub fn piece_extents(piece: &Piece, shape: &Shape) -> Vec3 {
    let fallback = default_size(shape);
    let raw = piece.trigger_size;
    let x = if raw.x.is_finite() && raw.x > 0.05 {
        raw.x
    } else {
        fallback.x
    };
    let y = if raw.y.is_finite() && raw.y > 0.05 {
        raw.y
    } else {
        fallback.y
    };
    let z = if raw.z.is_finite() && raw.z > 0.05 {
        raw.z
    } else {
        fallback.z
    };
    match glade_kind(shape) {
        Some(GladeKind::Tower) => {
            let rx = x.clamp(GLADE_MIN_SPAN, GLADE_MAX_SPAN);
            let rz = z.clamp(GLADE_MIN_SPAN, GLADE_MAX_SPAN);
            let d = (rx + rz) * 0.5;
            Vec3::new(d, y.clamp(GLADE_MIN_HEIGHT, GLADE_MAX_HEIGHT), d)
        }
        Some(GladeKind::Room) => Vec3::new(
            x.clamp(GLADE_MIN_SPAN, GLADE_MAX_SPAN),
            y.clamp(GLADE_MIN_HEIGHT, GLADE_MAX_HEIGHT),
            z.clamp(GLADE_MIN_SPAN, GLADE_MAX_SPAN),
        ),
        Some(GladeKind::Wall) => Vec3::new(
            x.clamp(GLADE_MIN_SPAN, GLADE_MAX_SPAN),
            y.clamp(GLADE_MIN_HEIGHT, GLADE_MAX_HEIGHT),
            z.clamp(GLADE_MIN_THICK, 2.4),
        ),
        Some(GladeKind::Door) => Vec3::new(
            x.clamp(0.6, 5.0),
            y.clamp(0.85, 6.0),
            z.clamp(0.18, 1.6),
        ),
        Some(GladeKind::Window) => Vec3::new(
            x.clamp(0.40, 3.6),
            y.clamp(0.45, 3.6),
            z.clamp(0.18, 1.6),
        ),
        Some(GladeKind::Terrain) => Vec3::new(
            x.clamp(TERRAIN_MIN_SPAN, TERRAIN_MAX_SPAN),
            y.clamp(TERRAIN_MIN_HEIGHT, TERRAIN_MAX_HEIGHT),
            z.clamp(TERRAIN_MIN_SPAN, TERRAIN_MAX_SPAN),
        ),
        Some(GladeKind::Path) => Vec3::new(
            x.clamp(0.4, 4.0),
            y.clamp(0.05, 1.0),
            z.clamp(0.4, 4.0),
        ),
        None => model::sanitize_trigger_size(raw),
    }
}

/// Effective [`GladeTerrainStyle`] for a `glade_terrain` piece.
pub fn terrain_style(piece: &Piece) -> GladeTerrainStyle {
    match piece.trigger_shape_override {
        TriggerShape::Box => GladeTerrainStyle::Plateau,
        TriggerShape::Sphere => GladeTerrainStyle::Mound,
        TriggerShape::Cylinder => GladeTerrainStyle::CliffMesa,
    }
}

/// Normalized terrain falloff / cliff steepness in `0.0..=1.0` for a
/// `glade_terrain` piece, derived from `piece.trigger_alpha`:
/// - `0.0` = wide, gentle grassy hill slope,
/// - `0.5` = terraced hillside with moderate slope,
/// - `1.0` = tight plateau rim with sheer craggy limestone rock cliffs.
pub fn terrain_falloff(piece: &Piece, shape: &Shape) -> f32 {
    if glade_kind(shape) != Some(GladeKind::Terrain) {
        return 0.0;
    }
    let alpha = model::sanitize_trigger_alpha(piece.trigger_alpha);
    if terrain_style(piece) == GladeTerrainStyle::CliffMesa {
        alpha.max(0.75)
    } else {
        alpha
    }
}

/// True when a `glade_room` or `glade_tower` has its walls pulled down into the
/// **tent-like house** tier (`wall_height <= TENT_MAX_WALL_HEIGHT`) with a
/// pitched roof.
pub fn is_tent_house(piece: &Piece, shape: &Shape) -> bool {
    let Some(kind) = glade_kind(shape) else {
        return false;
    };
    if !matches!(kind, GladeKind::Room | GladeKind::Tower) {
        return false;
    }
    if roof_style(piece, shape) == GladeRoofStyle::Battlements {
        return false;
    }
    piece_extents(piece, shape).y <= TENT_MAX_WALL_HEIGHT
}

/// Effective roof pitch factor (`0.0` = flat battlements, `0.7` = default cozy
/// slope, `2.0` = steep spire). Derived from `piece.trigger_alpha` (`0..=1`).
pub fn roof_pitch(piece: &Piece, shape: &Shape) -> f32 {
    if matches!(
        glade_kind(shape),
        Some(GladeKind::Wall | GladeKind::Door | GladeKind::Window | GladeKind::Terrain)
    ) {
        return 0.0;
    }
    let alpha = model::sanitize_trigger_alpha(piece.trigger_alpha);
    alpha * 2.0
}

/// Effective [`GladeRoofStyle`] for a Glade piece.
///
/// If the roof pitch has been pulled down near zero (`<= 0.08`), the roof
/// automatically transitions into flat crenellated castle battlements!
pub fn roof_style(piece: &Piece, shape: &Shape) -> GladeRoofStyle {
    let Some(kind) = glade_kind(shape) else {
        return GladeRoofStyle::Gable;
    };
    if matches!(
        kind,
        GladeKind::Wall
            | GladeKind::Door
            | GladeKind::Window
            | GladeKind::Terrain
            | GladeKind::Path
    ) {
        return GladeRoofStyle::Battlements;
    }
    let pitch = roof_pitch(piece, shape);
    if pitch <= 0.08 {
        return GladeRoofStyle::Battlements;
    }
    match (kind, piece.trigger_shape_override) {
        (GladeKind::Tower, TriggerShape::Box) => GladeRoofStyle::Conical,
        (GladeKind::Tower, TriggerShape::Sphere) => GladeRoofStyle::Hipped,
        (GladeKind::Tower, TriggerShape::Cylinder) => GladeRoofStyle::Battlements,
        (GladeKind::Room, TriggerShape::Box) => GladeRoofStyle::Gable,
        (GladeKind::Room, TriggerShape::Sphere) => GladeRoofStyle::Hipped,
        (GladeKind::Room, TriggerShape::Cylinder) => GladeRoofStyle::Battlements,
        (
            GladeKind::Wall
            | GladeKind::Door
            | GladeKind::Window
            | GladeKind::Terrain
            | GladeKind::Path,
            _,
        ) => GladeRoofStyle::Battlements,
    }
}

/// Height of the roof apex above the top of the walls, in metres.
pub fn roof_rise(piece: &Piece, shape: &Shape) -> f32 {
    if matches!(
        glade_kind(shape),
        Some(
            GladeKind::Wall
                | GladeKind::Door
                | GladeKind::Window
                | GladeKind::Terrain
                | GladeKind::Path
        )
    ) {
        return 0.0;
    }
    let style = roof_style(piece, shape);
    if style == GladeRoofStyle::Battlements {
        return 0.28;
    }
    let ext = piece_extents(piece, shape);
    let pitch = roof_pitch(piece, shape);
    let span = ext.x.min(ext.z) * 0.5;
    let min_rise = if is_tent_house(piece, shape) {
        0.95
    } else {
        0.15
    };
    (span * pitch).clamp(min_rise, 10.0)
}

/// Total vertical height of the Glade piece (wall height + roof rise), in metres.
pub fn total_height(piece: &Piece, shape: &Shape) -> f32 {
    let ext = piece_extents(piece, shape);
    ext.y + roof_rise(piece, shape)
}

/// Eaves overhang beyond the wall face, in metres.
pub fn roof_overhang(piece: &Piece, shape: &Shape) -> f32 {
    let ext = piece_extents(piece, shape);
    let span = ext.x.min(ext.z);
    let base = (span * 0.11).clamp(0.16, 0.48);
    if is_tent_house(piece, shape) {
        (base * 1.35).clamp(0.24, 0.65)
    } else {
        base
    }
}

/// Roof shingle colour for a Glade building, or lush meadow grass colour for
/// a `glade_terrain` chunk.
pub fn roof_color(piece: &Piece) -> Color {
    let is_terrain = shapes()
        .get(piece.shape)
        .and_then(glade_kind)
        .is_some_and(|k| k == GladeKind::Terrain);
    let hex = if piece.trigger_color == model::TRIGGER_DEFAULT_COLOR
        || (is_terrain && piece.trigger_color == DEFAULT_ROOF_COLOR)
    {
        if is_terrain {
            DEFAULT_TERRAIN_COLOR
        } else {
            DEFAULT_ROOF_COLOR
        }
    } else {
        piece.trigger_color
    };
    model::trigger_color_to_bevy(hex)
}

/// Whether procedural windows, quoins, shingle details, and cliff boulders are enabled.
pub fn details_enabled(piece: &Piece) -> bool {
    piece.trigger_visible
}

/// Local-space 2D footprint outline (counter-clockwise on the XZ ground plane)
/// reflecting the piece's live parametric extents.
pub fn local_footprint(piece: &Piece, shape: &Shape) -> Vec<Vec2> {
    let ext = piece_extents(piece, shape);
    match glade_kind(shape) {
        Some(GladeKind::Tower) => {
            let rx = ext.x * 0.5;
            let rz = ext.z * 0.5;
            let segments = 24;
            (0..segments)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / segments as f32;
                    Vec2::new(a.cos() * rx, a.sin() * rz)
                })
                .collect()
        }
        Some(GladeKind::Terrain) => {
            let rx = ext.x * 0.5;
            let rz = ext.z * 0.5;
            let segments = 24;
            let is_plateau = terrain_style(piece) == GladeTerrainStyle::Plateau;
            (0..segments)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / segments as f32;
                    terrain_contour_point(rx, rz, a, is_plateau)
                })
                .collect()
        }
        Some(GladeKind::Room | GladeKind::Wall | GladeKind::Door | GladeKind::Window) => {
            let hw = ext.x * 0.5;
            let hd = ext.z * 0.5;
            vec![
                Vec2::new(-hw, -hd),
                Vec2::new(hw, -hd),
                Vec2::new(hw, hd),
                Vec2::new(-hw, hd),
            ]
        }
        Some(GladeKind::Path) => Vec::new(),
        None => model::shape_points(shape),
    }
}

/// World-space 2D footprint polygon on the `(x, z)` ground plane.
pub fn world_footprint(piece: &Piece, shape: &Shape) -> Vec<Vec2> {
    let local = local_footprint(piece, shape);
    let (sin, cos) = piece.rot.sin_cos();
    local
        .into_iter()
        .map(|p| {
            piece.pos
                + Vec2::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos)
        })
        .collect()
}

/// Transform a world-space 2D point into the piece's local 2D ground frame.
pub fn world_to_local_2d(piece: &Piece, world: Vec2) -> Vec2 {
    model::to_local(piece, world)
}

/// Transform a local-space 2D point into world 2D `(x, z)`.
pub fn local_to_world_2d(piece: &Piece, local: Vec2) -> Vec2 {
    let (sin, cos) = piece.rot.sin_cos();
    piece.pos + Vec2::new(local.x * cos - local.y * sin, local.x * sin + local.y * cos)
}

// ---------------------------------------------------------------------------
// Tiny Glade–Style Parametric Terrain Heightfield & Multi-Chunk Blending
// ---------------------------------------------------------------------------

/// Point on the unit-normalized or scaled contour of a terrain chunk at polar
/// angle `angle`. For `Plateau` (`is_plateau == true`), uses a superellipse
/// (`p = 3.2`) so plateaus have soft, organic rounded-rectangle corners.
pub fn terrain_contour_point(rx: f32, rz: f32, angle: f32, is_plateau: bool) -> Vec2 {
    let (s, c) = angle.sin_cos();
    if is_plateau {
        let p = 3.2_f32;
        let denom = (c.abs().powf(p) + s.abs().powf(p)).powf(1.0 / p).max(1e-4);
        Vec2::new(rx * c / denom, rz * s / denom)
    } else {
        Vec2::new(rx * c, rz * s)
    }
}

/// Normalized footprint distance `d` (`0.0` at chunk centre, `1.0` on outer
/// base perimeter, `> 1.0` outside) for a `glade_terrain` piece at `world_pt`.
pub fn terrain_chunk_normalized_dist(piece: &Piece, shape: &Shape, world_pt: Vec2) -> f32 {
    let ext = piece_extents(piece, shape);
    let rx = (ext.x * 0.5).max(0.4);
    let rz = (ext.z * 0.5).max(0.4);
    let local = world_to_local_2d(piece, world_pt);
    let ux = (local.x / rx).abs();
    let uz = (local.y / rz).abs();
    if terrain_style(piece) == GladeTerrainStyle::Plateau {
        let p = 3.2_f32;
        (ux.powf(p) + uz.powf(p)).powf(1.0 / p)
    } else {
        (ux * ux + uz * uz).sqrt()
    }
}

/// Evaluate the height (in world Y metres) contributed by a single
/// `glade_terrain` chunk at `world_pt`, or `None` if `world_pt` lies outside
/// its footprint.
pub fn terrain_chunk_height_at(piece: &Piece, shape: &Shape, world_pt: Vec2) -> Option<f32> {
    if glade_kind(shape) != Some(GladeKind::Terrain) {
        return None;
    }
    let d = terrain_chunk_normalized_dist(piece, shape, world_pt);
    if d >= 1.0 {
        return None;
    }
    let ext = piece_extents(piece, shape);
    let falloff = terrain_falloff(piece, shape);
    let style = terrain_style(piece);

    // Flat plateau radius fraction: higher falloff pushes the rim outward so
    // the top stays flat and the outer margin drops off as a steep cliff!
    let plateau_r = match style {
        GladeTerrainStyle::Plateau => 0.26 + 0.54 * falloff,
        GladeTerrainStyle::Mound => 0.08 + 0.42 * falloff,
        GladeTerrainStyle::CliffMesa => (0.58 + 0.25 * falloff).clamp(0.58, 0.85),
    };

    let profile = if d <= plateau_r {
        if style == GladeTerrainStyle::Mound && plateau_r > 0.01 {
            // Subtle dome crown on Mound tops
            let u = d / plateau_r;
            1.0 - 0.05 * u * u
        } else {
            1.0
        }
    } else {
        let t = ((d - plateau_r) / (1.0 - plateau_r).max(0.06)).clamp(0.0, 1.0);
        // Smoothstep S-curve
        let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
        if falloff > 0.55 {
            // Sharpen cliff shoulder near the upper rim when steepness is high
            smooth.powf(1.0 - (falloff - 0.55) * 0.45)
        } else {
            smooth
        }
    };

    if piece.elevation < -0.05 {
        return Some(0.0);
    }
    Some((piece.elevation + ext.y * profile).max(0.0))
}

/// Evaluate how much a carve-mode `glade_terrain` piece (`piece.elevation < -0.05`)
/// lowers the terrain at `world_pt`.
fn terrain_chunk_carve_at(piece: &Piece, shape: &Shape, world_pt: Vec2) -> f32 {
    if glade_kind(shape) != Some(GladeKind::Terrain) || piece.elevation >= -0.05 {
        return 0.0;
    }
    let d = terrain_chunk_normalized_dist(piece, shape, world_pt);
    if d >= 1.0 {
        return 0.0;
    }
    let ext = piece_extents(piece, shape);
    let smooth = 1.0 - d * d * (3.0 - 2.0 * d);
    ext.y * smooth
}

/// Polynomial smooth-maximum (`smax`) so overlapping `glade_terrain` chunks
/// blend organically into continuous rolling ridges and multi-tiered hills.
fn smooth_max_height(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1e-4 {
        return a.max(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.max(b) + h * h * k * 0.25
}

/// Blended world-space terrain elevation at `world_pt` across all
/// `glade_terrain` chunks in `all_pieces` (`0.0` when on flat ground).
pub fn terrain_height_at(world_pt: Vec2, all_pieces: &[Piece]) -> f32 {
    let mut blended: Option<f32> = None;
    let mut carved = 0.0_f32;
    for p in all_pieces {
        let Some(shape) = shapes().get(p.shape) else {
            continue;
        };
        if p.elevation < -0.05 {
            carved = carved.max(terrain_chunk_carve_at(p, shape, world_pt));
            continue;
        }
        if let Some(h) = terrain_chunk_height_at(p, shape, world_pt) {
            blended = Some(match blended {
                None => h,
                Some(prev) => smooth_max_height(prev, h, 0.42),
            });
        }
    }
    (blended.unwrap_or(0.0) - carved).max(0.0)
}

/// Built-in large-scale meadow & hillside relief for the continuous `280m x 280m`
/// world terrain:
/// - Keeps the central spawn/course clearing (`r <= FLAT_CLEARING_RADIUS`)
///   flat at `y = 0.0` so flat-ground courses and tests start level,
/// - Raises a picturesque Tiny Glade plateau hill & craggy limestone cliff
///   escarpment in the `(-X, -Z)` view (`center = (-7.6, -6.8)`) right in front
///   of the default orbit camera,
/// - Surrounds the outer glade (`r > FLAT_CLEARING_RADIUS`) with gentle rolling hills and
///   rocky ridges out to `WORLD_TERRAIN_HALF_EXTENT`.
pub fn base_meadow_height_at(world_pt: Vec2) -> f32 {
    let mut h = 0.0_f32;

    // 1. Scenic Tiny Glade hillside plateau & craggy cliff escarpment in (-X, -Z)
    let hill_center = Vec2::new(-7.6, -6.8);
    let rel = world_pt - hill_center;
    // Rotate slightly (22 deg) so the cliff faces the camera diagonally like ref.jpg
    let (s, c) = 0.38_f32.sin_cos();
    let lx = rel.x * c - rel.y * s;
    let lz = rel.x * s + rel.y * c;
    let rx = 5.8_f32;
    let rz = 4.8_f32;
    let ux = (lx / rx).abs();
    let uz = (lz / rz).abs();
    let d_hill = (ux.powf(3.2) + uz.powf(3.2)).powf(1.0 / 3.2);
    if d_hill < 1.0 {
        let plateau_r = 0.62;
        let profile = if d_hill <= plateau_r {
            1.0
        } else {
            let t = ((d_hill - plateau_r) / (1.0 - plateau_r)).clamp(0.0, 1.0);
            let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
            smooth.powf(0.82)
        };
        h = smooth_max_height(h, 2.35 * profile, 0.35);
    }

    // Secondary lower rocky spur extending toward (-2.8, -5.2) (like the foreground rock seam in ref.jpg)
    let spur_center = Vec2::new(-3.6, -6.2);
    let d_spur = ((world_pt.x - spur_center.x) / 3.4).hypot((world_pt.y - spur_center.y) / 2.4);
    if d_spur < 1.0 {
        let t = d_spur.clamp(0.0, 1.0);
        let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
        h = smooth_max_height(h, 0.95 * smooth, 0.30);
    }

    // 2. Outer rolling countryside hills framing the glade (`r > FLAT_CLEARING_RADIUS`)
    let r = world_pt.length();
    if r > FLAT_CLEARING_RADIUS {
        let rim_t = ((r - FLAT_CLEARING_RADIUS) / 28.0).clamp(0.0, 1.0);
        let smooth_rim = rim_t * rim_t * (3.0 - 2.0 * rim_t);
        let wave = (world_pt.x * 0.14).sin() * (world_pt.y * 0.13).cos() * 0.5 + 0.5;
        let ridge = ((world_pt.x * 0.09 - world_pt.y * 0.11).sin()).abs();
        let outer_h = smooth_rim * (1.1 + 2.4 * wave + 1.2 * ridge);
        h = smooth_max_height(h, outer_h, 0.45);
    }

    h
}

/// Unified world-space terrain elevation at `world_pt`, combining the
/// built-in large-scale meadow/hillside relief with all user-sculpted
/// `glade_terrain` landforms (including carve/lower strokes).
pub fn world_terrain_height_at(world_pt: Vec2, all_pieces: &[Piece]) -> f32 {
    let base = base_meadow_height_at(world_pt);
    let mut blended = base;
    let mut carved = 0.0_f32;
    for p in all_pieces {
        let Some(shape) = shapes().get(p.shape) else {
            continue;
        };
        if p.elevation < -0.05 {
            carved = carved.max(terrain_chunk_carve_at(p, shape, world_pt));
            continue;
        }
        if let Some(h) = terrain_chunk_height_at(p, shape, world_pt) {
            blended = smooth_max_height(blended, h, 0.42);
        }
    }
    (blended - carved).max(0.0)
}

/// Outward unit surface normal of the unified world terrain heightfield at `world_pt`.
pub fn world_terrain_normal_at(world_pt: Vec2, all_pieces: &[Piece]) -> Vec3 {
    let eps = 0.16;
    let hx1 = world_terrain_height_at(world_pt + Vec2::new(eps, 0.0), all_pieces);
    let hx0 = world_terrain_height_at(world_pt - Vec2::new(eps, 0.0), all_pieces);
    let hz1 = world_terrain_height_at(world_pt + Vec2::new(0.0, eps), all_pieces);
    let hz0 = world_terrain_height_at(world_pt - Vec2::new(0.0, eps), all_pieces);
    let dh_dx = (hx1 - hx0) / (2.0 * eps);
    let dh_dz = (hz1 - hz0) / (2.0 * eps);
    Vec3::new(-dh_dx, 1.0, -dh_dz).normalize_or_zero()
}

/// Outward unit surface normal of the blended terrain heightfield at `world_pt`.
pub fn terrain_normal_at(world_pt: Vec2, all_pieces: &[Piece]) -> Vec3 {
    let eps = 0.14;
    let hx1 = terrain_height_at(world_pt + Vec2::new(eps, 0.0), all_pieces);
    let hx0 = terrain_height_at(world_pt - Vec2::new(eps, 0.0), all_pieces);
    let hz1 = terrain_height_at(world_pt + Vec2::new(0.0, eps), all_pieces);
    let hz0 = terrain_height_at(world_pt - Vec2::new(0.0, eps), all_pieces);
    let dh_dx = (hx1 - hx0) / (2.0 * eps);
    let dh_dz = (hz1 - hz0) / (2.0 * eps);
    Vec3::new(-dh_dx, 1.0, -dh_dz).normalize_or_zero()
}

/// Build the piece-local triangle mesh `(vertices, triangles)` for a
/// `glade_terrain` chunk's walkable upper surface. Used by both visual mesh
/// synthesis and Rapier 3D trimesh collider generation (`colliders.rs`).
pub fn terrain_surface_vertices_and_indices(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let ext = piece_extents(piece, shape);
    let rx = (ext.x * 0.5).max(0.4);
    let rz = (ext.z * 0.5).max(0.4);
    let is_plateau = terrain_style(piece) == GladeTerrainStyle::Plateau;
    let rings: usize = 12;
    let segments: usize = 28;

    // Vertex 0 is the centre peak; followed by `rings` concentric rings of `segments` vertices.
    let mut verts = Vec::with_capacity(1 + rings * segments);
    let center_h = terrain_chunk_height_at(piece, shape, piece.pos)
        .unwrap_or(piece.elevation + ext.y)
        .max(terrain_height_at(piece.pos, all_pieces))
        - piece.elevation;
    verts.push(Vec3::new(0.0, center_h.max(0.0), 0.0));

    for r in 1..=rings {
        let rf = r as f32 / rings as f32;
        for s in 0..segments {
            let angle = std::f32::consts::TAU * s as f32 / segments as f32;
            let contour = terrain_contour_point(rx * rf, rz * rf, angle, is_plateau);
            let world_pt = local_to_world_2d(piece, contour);
            let my_h = terrain_chunk_height_at(piece, shape, world_pt).unwrap_or(piece.elevation);
            let local_y = if r == rings {
                0.0
            } else {
                (my_h - piece.elevation).max(0.0)
            };
            verts.push(Vec3::new(contour.x, local_y, contour.y));
        }
    }

    let mut tris = Vec::with_capacity(segments + (rings - 1) * segments * 2);
    // Inner fan around vertex 0
    for s in 0..segments {
        let next_s = (s + 1) % segments;
        let i0 = 1 + s as u32;
        let i1 = 1 + next_s as u32;
        // Counter-clockwise from above (+Y normal): (0, i1, i0) because angle increases from +X toward +Z
        tris.push([0, i1, i0]);
    }
    // Concentric ring strips
    for r in 1..rings {
        let ring0 = 1 + (r - 1) * segments;
        let ring1 = 1 + r * segments;
        for s in 0..segments {
            let next_s = (s + 1) % segments;
            let v00 = (ring0 + s) as u32;
            let v10 = (ring0 + next_s) as u32;
            let v11 = (ring1 + next_s) as u32;
            let v01 = (ring1 + s) as u32;
            tris.push([v00, v10, v11]);
            tris.push([v00, v11, v01]);
        }
    }

    (verts, tris)
}

// ---------------------------------------------------------------------------
// 2D Polygon Boolean Merging & Internal Wall Culling
// ---------------------------------------------------------------------------

/// Point-in-polygon test in 2D (`eps` shrinks or expands the boundary slightly).
pub fn point_in_polygon_2d(point: Vec2, polygon: &[Vec2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        if (pi.y > point.y) != (pj.y > point.y)
            && point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Parametric intersection `t` along segment `a -> b` (`t in 0..=1`) and `u`
/// along `c -> d` (`u in 0..=1`).
pub fn segment_intersection_2d(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<(f32, f32)> {
    let r = b - a;
    let s = d - c;
    let denom = r.x * s.y - r.y * s.x;
    if denom.abs() < 1e-6 {
        return None;
    }
    let qp = c - a;
    let t = (qp.x * s.y - qp.y * s.x) / denom;
    let u = (qp.x * r.y - qp.y * r.x) / denom;
    if (-1e-4..=1.0001).contains(&t) && (-1e-4..=1.0001).contains(&u) {
        Some((t.clamp(0.0, 1.0), u.clamp(0.0, 1.0)))
    } else {
        None
    }
}

/// Shortest distance from point `p` to segment `a -> b`, and the parameter
/// `t in 0..=1` of the closest point.
pub fn point_to_segment_2d(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-8 {
        return (p.distance(a), 0.0);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    (p.distance(closest), t)
}

/// One exposed exterior wall sub-segment of a Glade piece, in the piece's
/// local 2D coordinate system.
#[derive(Clone, Debug, PartialEq)]
pub struct ExposedWallSegment {
    /// Start point in piece-local XZ space.
    pub p0: Vec2,
    /// End point in piece-local XZ space.
    pub p1: Vec2,
    /// Outward unit normal in piece-local XZ space.
    pub normal: Vec2,
    /// Bottom height in piece-local Y space (above `piece.elevation`).
    pub base_y: f32,
    /// Top height in piece-local Y space (above `piece.elevation`).
    pub top_y: f32,
    /// True when `p0` is an original corner of a rectangular room or wall end.
    pub corner_start: bool,
    /// True when `p1` is an original corner of a rectangular room or wall end.
    pub corner_end: bool,
    /// True when `p0` sits on a boolean intersection seam with another building.
    pub seam_start: bool,
    /// True when `p1` sits on a boolean intersection seam with another building.
    pub seam_end: bool,
}

impl ExposedWallSegment {
    pub fn length(&self) -> f32 {
        self.p0.distance(self.p1)
    }

    pub fn height(&self) -> f32 {
        (self.top_y - self.base_y).max(0.0)
    }
}

/// Neighbor footprint cached in world space for boolean clipping.
struct NeighborFootprint {
    id: u32,
    poly_world: Vec<Vec2>,
    bottom_world: f32,
    top_world: f32,
    bounds_min: Vec2,
    bounds_max: Vec2,
}

/// Compute the exposed exterior wall segments of `piece` after 2D boolean
/// union / clipping against all overlapping Glade buildings (`Tower` and `Room`).
///
/// - Wall portions buried inside a neighbor of equal or greater height are
///   culled completely (no internal walls).
/// - When two buildings of identical height share an overlapping region, id
///   tie-breaking ensures a deterministic union.
/// - When a taller tower or room overlaps a shorter room, the lower buried
///   wall is culled while the upper facade (`neighbor_top..piece_top`) remains
///   exposed!
pub fn compute_exposed_wall_segments(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
) -> Vec<ExposedWallSegment> {
    let Some(kind) = glade_kind(shape) else {
        return Vec::new();
    };
    let ext = piece_extents(piece, shape);
    let local_poly = local_footprint(piece, shape);
    if local_poly.len() < 3 {
        return Vec::new();
    }

    let my_bottom = piece.elevation;
    let my_top = piece.elevation + ext.y;

    // Gather overlapping Glade Room/Tower footprints in world space.
    let mut neighbors = Vec::new();
    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        let Some(other_kind) = glade_kind(other_shape) else {
            continue;
        };
        // Only solid buildings (Tower / Room) cull interior walls. Free-standing
        // walls get trimmed when entering a building too!
        if !matches!(other_kind, GladeKind::Tower | GladeKind::Room) {
            continue;
        }
        let other_ext = piece_extents(other, other_shape);
        let other_bottom = other.elevation;
        let other_top = other.elevation + other_ext.y;
        // Must overlap vertically.
        if other_top <= my_bottom + 0.04 || other_bottom >= my_top - 0.04 {
            continue;
        }
        let poly_world = world_footprint(other, other_shape);
        let mut bmin = Vec2::splat(f32::MAX);
        let mut bmax = Vec2::splat(f32::MIN);
        for &pt in &poly_world {
            bmin = bmin.min(pt);
            bmax = bmax.max(pt);
        }
        neighbors.push(NeighborFootprint {
            id: other.id,
            poly_world,
            bottom_world: other_bottom,
            top_world: other_top,
            bounds_min: bmin,
            bounds_max: bmax,
        });
    }

    let is_sharp_poly = matches!(kind, GladeKind::Room | GladeKind::Wall);
    let mut out = Vec::new();

    for i in 0..local_poly.len() {
        let a_local = local_poly[i];
        let b_local = local_poly[(i + 1) % local_poly.len()];
        let edge_vec = b_local - a_local;
        let edge_len = edge_vec.length();
        if edge_len < 1e-4 {
            continue;
        }
        // Counter-clockwise outline in (x, z) -> outward normal is (dz, -dx).
        let normal = Vec2::new(edge_vec.y, -edge_vec.x) / edge_len;

        let a_world = local_to_world_2d(piece, a_local);
        let b_world = local_to_world_2d(piece, b_local);
        let seg_min = a_world.min(b_world) - Vec2::splat(0.05);
        let seg_max = a_world.max(b_world) + Vec2::splat(0.05);

        // Collect all intersection parameters t in [0, 1] with neighbor polygon edges.
        let mut splits = vec![0.0_f32, 1.0_f32];
        for nb in &neighbors {
            if seg_max.x < nb.bounds_min.x
                || seg_min.x > nb.bounds_max.x
                || seg_max.y < nb.bounds_min.y
                || seg_min.y > nb.bounds_max.y
            {
                continue;
            }
            for j in 0..nb.poly_world.len() {
                let c = nb.poly_world[j];
                let d = nb.poly_world[(j + 1) % nb.poly_world.len()];
                if let Some((t, _)) = segment_intersection_2d(a_world, b_world, c, d) {
                    splits.push(t);
                }
                let (dist_c, t_c) = point_to_segment_2d(c, a_world, b_world);
                if dist_c < 0.025 {
                    splits.push(t_c);
                }
            }
        }

        splits.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        splits.dedup_by(|x, y| (*x - *y).abs() < 1e-3);

        for w in splits.windows(2) {
            let (t0, t1) = (w[0], w[1]);
            if (t1 - t0) * edge_len < 0.04 {
                continue;
            }
            let p0 = a_local.lerp(b_local, t0);
            let p1 = a_local.lerp(b_local, t1);
            let mid_local = (p0 + p1) * 0.5;
            let mid_world = local_to_world_2d(piece, mid_local);
            let outward_probe = local_to_world_2d(piece, mid_local + normal * 0.03);
            let inward_probe = local_to_world_2d(piece, mid_local - normal * 0.03);

            // Check if this sub-segment's exterior side lies inside any overlapping neighbor,
            // or if it is a duplicate co-directional collinear wall.
            let mut max_covering_top: Option<f32> = None;
            let mut completely_culled = false;

            for nb in &neighbors {
                if mid_world.x < nb.bounds_min.x - 0.05
                    || mid_world.x > nb.bounds_max.x + 0.05
                    || mid_world.y < nb.bounds_min.y - 0.05
                    || mid_world.y > nb.bounds_max.y + 0.05
                {
                    continue;
                }
                let outward_inside = point_in_polygon_2d(outward_probe, &nb.poly_world);
                let mut on_boundary = false;
                if !outward_inside {
                    for j in 0..nb.poly_world.len() {
                        let c = nb.poly_world[j];
                        let d = nb.poly_world[(j + 1) % nb.poly_world.len()];
                        if point_to_segment_2d(mid_world, c, d).0 < 0.025 {
                            on_boundary = true;
                            break;
                        }
                    }
                }
                let co_directional_shared = on_boundary
                    && nb.id < piece.id
                    && point_in_polygon_2d(inward_probe, &nb.poly_world);

                if outward_inside || co_directional_shared {
                    if nb.bottom_world <= my_bottom + 0.1 {
                        if nb.top_world >= my_top - 0.04 {
                            completely_culled = true;
                            break;
                        } else {
                            let cov = nb.top_world - my_bottom;
                            max_covering_top =
                                Some(max_covering_top.map_or(cov, |prev: f32| prev.max(cov)));
                        }
                    }
                }
            }

            if completely_culled {
                continue;
            }

            let base_y = max_covering_top.unwrap_or(0.0).clamp(0.0, ext.y);
            if ext.y - base_y < 0.08 {
                continue;
            }

            out.push(ExposedWallSegment {
                p0,
                p1,
                normal,
                base_y,
                top_y: ext.y,
                corner_start: is_sharp_poly && t0 < 1e-3,
                corner_end: is_sharp_poly && t1 > 1.0 - 1e-3,
                seam_start: t0 >= 1e-3,
                seam_end: t1 <= 1.0 - 1e-3,
            });
        }
    }

    out
}

/// Compute the merged 2D exterior boundary segments `(world_p0, world_p1)` for
/// all Glade pieces in `pieces`. Useful for visualizing the unified footprint
/// and verifying boolean union topology.
pub fn boolean_union_outline(pieces: &[Piece]) -> Vec<(Vec2, Vec2)> {
    let mut world_segments = Vec::new();
    for piece in pieces {
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        if !is_glade_shape(shape) {
            continue;
        }
        for seg in compute_exposed_wall_segments(piece, shape, pieces) {
            if seg.base_y <= 0.05 {
                world_segments.push((
                    local_to_world_2d(piece, seg.p0),
                    local_to_world_2d(piece, seg.p1),
                ));
            }
        }
    }
    world_segments
}

/// Group Glade pieces into connected clusters whose 2D footprints overlap or touch.
pub fn find_connected_clusters(pieces: &[Piece]) -> Vec<Vec<u32>> {
    let glade_pieces: Vec<(&Piece, &Shape, Vec<Vec2>)> = pieces
        .iter()
        .filter_map(|p| {
            let s = shapes().get(p.shape)?;
            is_glade_shape(s).then(|| (p, s, world_footprint(p, s)))
        })
        .collect();

    let n = glade_pieces.len();
    let mut visited = vec![false; n];
    let mut clusters = Vec::new();

    for i in 0..n {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let mut queue = vec![i];
        let mut cluster = Vec::new();
        while let Some(curr) = queue.pop() {
            cluster.push(glade_pieces[curr].0.id);
            for next in 0..n {
                if !visited[next]
                    && footprints_overlap_or_touch(&glade_pieces[curr].2, &glade_pieces[next].2)
                {
                    visited[next] = true;
                    queue.push(next);
                }
            }
        }
        clusters.push(cluster);
    }
    clusters
}

/// True when two 2D polygons overlap or share a boundary within `0.08m`.
pub fn footprints_overlap_or_touch(poly_a: &[Vec2], poly_b: &[Vec2]) -> bool {
    if poly_a.is_empty() || poly_b.is_empty() {
        return false;
    }
    if poly_a.iter().any(|&p| point_in_polygon_2d(p, poly_b))
        || poly_b.iter().any(|&p| point_in_polygon_2d(p, poly_a))
    {
        return true;
    }
    for i in 0..poly_a.len() {
        let a0 = poly_a[i];
        let a1 = poly_a[(i + 1) % poly_a.len()];
        for j in 0..poly_b.len() {
            let b0 = poly_b[j];
            let b1 = poly_b[(j + 1) % poly_b.len()];
            if segment_intersection_2d(a0, a1, b0, b1).is_some()
                || point_to_segment_2d(a0, b0, b1).0 < 0.08
                || point_to_segment_2d(b0, a0, a1).0 < 0.08
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Building Chemistry: Archways & Contextual Windows
// ---------------------------------------------------------------------------

/// A vaulted archway cut through a wall segment where a path or threshold crosses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArchOpening {
    /// Distance from segment start `p0` to the arch centre, in metres.
    pub center_dist: f32,
    /// Clear opening width of the archway, in metres.
    pub width: f32,
    /// Total height from the segment base to the top of the semicircular crown.
    pub crown_height: f32,
}

/// Snap a door piece (`glade_door`, `door`, `gate`) tangentially onto the nearest
/// exposed wall of any Glade building, curved Glade wall, or standard wall.
///
/// Returns `Some((snapped_world_pos, wall_tangent_rot))`.
pub fn snap_door_to_glade_wall(piece: &Piece, others: &[Piece]) -> Option<(Vec2, f32)> {
    snap_door_to_glade_wall_with_ray(piece, others, None, &[])
}

/// Raycast and proximity snap for a door piece (`glade_door`, `door`, `gate`)
/// against 3D tower cylinders, room facades, curved path-following walls, and
/// standard bar walls.
///
/// - When `ray` is provided (3D viewport cursor/finger ray), directly hitting a
///   vertical wall face in 3D takes top priority so pointing mid-wall in a
///   perspective view snaps to the front wall under the cursor rather than the
///   ground plane behind it.
/// - On cylindrical towers (`GladeKind::Tower`), position and rotation are
///   computed from the exact smooth ellipse/circle tangent at the hit angle.
/// - On rooms, curved path walls, and bar walls, position and rotation align
///   tangentially with the wall segment.
pub fn snap_door_to_glade_wall_with_ray(
    piece: &Piece,
    others: &[Piece],
    ray: Option<Ray3d>,
    wall_spines: &[(u32, Vec<Vec2>)],
) -> Option<(Vec2, f32)> {
    snap_door_to_glade_wall_with_elevation(piece, others, ray, wall_spines)
        .map(|(pos, rot, _)| (pos, rot))
}

/// Like [`snap_door_to_glade_wall_with_ray`], but also returns the host wall's
/// `elevation` so doors placed or dragged onto hillside/elevated buildings
/// match their host wall's base elevation and cut open archways cleanly.
pub fn snap_door_to_glade_wall_with_elevation(
    piece: &Piece,
    others: &[Piece],
    ray: Option<Ray3d>,
    wall_spines: &[(u32, Vec<Vec2>)],
) -> Option<(Vec2, f32, f32)> {
    let piece_shape = shapes().get(piece.shape);
    let is_window = piece_shape.is_some_and(|s| s.id == "glade_window");
    let win_h = if is_window {
        piece_shape.map_or(0.92, |s| piece_extents(piece, s).y)
    } else {
        0.0
    };
    let attachment_elev = |other_elev: f32, wall_h: f32, hit_y: Option<f32>| -> f32 {
        if !is_window {
            return other_elev;
        }
        let min_elev = other_elev + 0.35;
        let max_elev = (other_elev + wall_h - win_h - 0.15).max(min_elev);
        if let Some(y) = hit_y {
            (y - win_h * 0.5).clamp(min_elev, max_elev)
        } else if piece.id != 0
            && piece.elevation >= other_elev + 0.15
            && piece.elevation <= other_elev + wall_h
        {
            piece.elevation.clamp(min_elev, max_elev)
        } else {
            (other_elev + 0.85).clamp(min_elev, max_elev)
        }
    };

    // (priority_score, snapped_world_pos, tangent_rot, host_elevation)
    let mut best: Option<(f32, Vec2, f32, f32)> = None;

    for other in others {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        let kind = glade_kind(other_shape);
        let is_standard_wall = matches!(
            other_shape.id,
            "wall" | "tall_wall" | "fence" | "window" | "arch"
        );
        if !matches!(
            kind,
            Some(GladeKind::Room | GladeKind::Tower | GladeKind::Wall)
        ) && !is_standard_wall
        {
            continue;
        }
        // When placing a fresh door (`id == 0`) or raycasting in 3D, allow
        // snapping to buildings on elevated terrain/hillsides; for existing
        // pieces dragged purely in 2D, keep a generous elevation band.
        if ray.is_none() && piece.id != 0 && (other.elevation - piece.elevation).abs() > 3.5 {
            continue;
        }

        // 1. Smooth tangential snapping onto cylindrical/elliptical Glade Towers.
        if kind == Some(GladeKind::Tower) {
            let ext = piece_extents(other, other_shape);
            let rx = (ext.x * 0.5).max(0.2);
            let rz = (ext.z * 0.5).max(0.2);
            let y0 = other.elevation;
            let y1 = other.elevation + ext.y;
            let exposed = compute_exposed_wall_segments(other, other_shape, others);

            let mut candidate_local: Option<(f32, Vec2, Option<f32>)> = None;

            // Check 3D ray intersection with the tower cylinder first.
            if let Some(r) = ray
                && let Some((s, hit_local)) = ray_cylinder_wall_hit(r, other, rx, rz, y0, y1)
            {
                let hit_y = r.origin.y + r.direction.y * s;
                candidate_local = Some((0.02 + s * 0.002, hit_local, Some(hit_y)));
            }

            // Fallback: 2D ground proximity to the tower perimeter.
            if candidate_local.is_none() {
                let p_local = world_to_local_2d(other, piece.pos);
                let theta = (p_local.y / rz).atan2(p_local.x / rx);
                let on_ring = Vec2::new(rx * theta.cos(), rz * theta.sin());
                let dist = p_local.distance(on_ring);
                if dist <= 2.2 {
                    candidate_local = Some((dist, on_ring, None));
                }
            }

            if let Some((score, ref_local, hit_y)) = candidate_local {
                let theta = (ref_local.y / rz).atan2(ref_local.x / rx);
                let q_local = Vec2::new(rx * theta.cos(), rz * theta.sin());

                // Ensure this side of the tower is exposed (not buried inside a room).
                let is_exposed = exposed.iter().any(|seg| {
                    (is_window || seg.base_y <= 0.25)
                        && ((seg.p0 + seg.p1) * 0.5).distance(q_local) <= 0.55
                });
                if is_exposed {
                    // Counter-clockwise tangent vector on the ellipse `(rx cos θ, rz sin θ)`
                    // is `(-rx sin θ, rz cos θ)`.
                    let t_local = Vec2::new(-rx * theta.sin(), rz * theta.cos());
                    let (sin, cos) = other.rot.sin_cos();
                    let world_tangent = Vec2::new(
                        t_local.x * cos - t_local.y * sin,
                        t_local.x * sin + t_local.y * cos,
                    );
                    let snapped_pos = local_to_world_2d(other, q_local);
                    let rot = model::normalize_angle(world_tangent.y.atan2(world_tangent.x));
                    let elev = attachment_elev(other.elevation, ext.y, hit_y);
                    if best.is_none_or(|(best_score, _, _, _)| score < best_score) {
                        best = Some((score, snapped_pos, rot, elev));
                    }
                }
            }
            continue;
        }

        // 2. Straight or curved wall segments (Glade Room, Glade Wall with optional spine, or standard wall).
        let wall_height = if other_shape.is_glade() {
            piece_extents(other, other_shape).y
        } else {
            other_shape.height.max(1.0)
        };
        let y0 = other.elevation;
        let y1 = other.elevation + wall_height;

        let mut is_curved_spine = false;
        let segments: Vec<(Vec2, Vec2)> = if kind == Some(GladeKind::Wall) {
            if let Some((_, spine)) = wall_spines
                .iter()
                .find(|(id, pts)| *id == other.id && pts.len() >= 2)
            {
                is_curved_spine = spine.len() > 2;
                spine.windows(2).map(|w| (w[0], w[1])).collect()
            } else {
                let ext = piece_extents(other, other_shape);
                let hl = ext.x * 0.5;
                vec![(
                    local_to_world_2d(other, Vec2::new(-hl, 0.0)),
                    local_to_world_2d(other, Vec2::new(hl, 0.0)),
                )]
            }
        } else if kind == Some(GladeKind::Room) {
            compute_exposed_wall_segments(other, other_shape, others)
                .into_iter()
                .filter(|s| is_window || s.base_y <= 0.25)
                .map(|s| (local_to_world_2d(other, s.p0), local_to_world_2d(other, s.p1)))
                .collect()
        } else if let model::Geometry::Bar { len, .. } = other_shape.geom {
            let hl = len * 0.5;
            vec![(
                local_to_world_2d(other, Vec2::new(-hl, 0.0)),
                local_to_world_2d(other, Vec2::new(hl, 0.0)),
            )]
        } else {
            Vec::new()
        };

        for (seg_idx, &(w0, w1)) in segments.iter().enumerate() {
            let seg_vec = w1 - w0;
            let seg_len = seg_vec.length();
            if seg_len < 0.12 {
                continue;
            }

            let mut seg_hit: Option<(f32, f32, Option<f32>)> = None;
            if let Some(r) = ray
                && let Some((s, t_ray)) = ray_segment_wall_hit(r, w0, w1, y0, y1)
            {
                let hit_y = r.origin.y + r.direction.y * s;
                seg_hit = Some((0.02 + s * 0.002, t_ray, Some(hit_y)));
            }

            if seg_hit.is_none() {
                let (dist, t_2d) = point_to_segment_2d(piece.pos, w0, w1);
                if dist <= 2.0 {
                    seg_hit = Some((dist, t_2d, None));
                }
            }

            let Some((score, mut t, hit_y)) = seg_hit else {
                continue;
            };

            // Keep doors away from sharp room corners when the wall is wide enough.
            if !is_curved_spine && seg_len > 1.3 {
                let margin = (0.55 / seg_len).min(0.35);
                t = t.clamp(margin, 1.0 - margin);
            }
            let snapped_pos = w0.lerp(w1, t);

            // Compute smooth tangent direction along curved spines, or exact segment tangent on straight walls.
            let tangent = if is_curved_spine && segments.len() >= 2 {
                let cur_dir = seg_vec / seg_len;
                if t < 0.35 && seg_idx > 0 {
                    let prev_vec = segments[seg_idx - 1].1 - segments[seg_idx - 1].0;
                    let prev_dir = prev_vec.normalize_or_zero();
                    let blend = (0.35 - t) / 0.35 * 0.5;
                    cur_dir.lerp(prev_dir, blend).normalize_or_zero()
                } else if t > 0.65 && seg_idx + 1 < segments.len() {
                    let next_vec = segments[seg_idx + 1].1 - segments[seg_idx + 1].0;
                    let next_dir = next_vec.normalize_or_zero();
                    let blend = (t - 0.65) / 0.35 * 0.5;
                    cur_dir.lerp(next_dir, blend).normalize_or_zero()
                } else {
                    cur_dir
                }
            } else {
                seg_vec / seg_len
            };

            let rot = model::normalize_angle(tangent.y.atan2(tangent.x));
            let elev = attachment_elev(other.elevation, wall_height, hit_y);
            if best.is_none_or(|(best_score, _, _, _)| score < best_score) {
                best = Some((score, snapped_pos, rot, elev));
            }
        }
    }

    best.map(|(_, pos, rot, elev)| (pos, rot, elev))
}

/// Keep doors (`glade_door`, `door`, `gate`) pinned to the outer wall when a
/// Glade building (`glade_room`, `glade_tower`, `glade_wall`) is resized.
pub fn pin_doors_on_glade_resize(old_piece: &Piece, new_piece: &Piece, pieces: &mut [Piece]) {
    let Some(shape) = shapes().get(new_piece.shape) else {
        return;
    };
    let Some(kind) = glade_kind(shape) else {
        return;
    };
    if !matches!(
        kind,
        GladeKind::Room | GladeKind::Tower | GladeKind::Wall
    ) {
        return;
    }

    let old_ext = piece_extents(old_piece, shape);
    let new_ext = piece_extents(new_piece, shape);
    if (old_ext.x - new_ext.x).abs() < 1e-6 && (old_ext.z - new_ext.z).abs() < 1e-6 {
        return;
    }

    for other in pieces.iter_mut() {
        if other.id == new_piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if !matches!(other_shape.id, "glade_door" | "glade_window" | "door" | "gate") {
            continue;
        }
        if other_shape.id == "glade_window" {
            if other.elevation + 0.5 < old_piece.elevation
                || other.elevation > old_piece.elevation + old_ext.y + 0.9
            {
                continue;
            }
        } else if (other.elevation - old_piece.elevation).abs() > 0.9 {
            continue;
        }

        let local = world_to_local_2d(old_piece, other.pos);
        match kind {
            GladeKind::Room => {
                let old_hx = old_ext.x * 0.5;
                let old_hz = old_ext.z * 0.5;
                let new_hx = new_ext.x * 0.5;
                let new_hz = new_ext.z * 0.5;
                if local.x.abs() > old_hx + 0.45 || local.y.abs() > old_hz + 0.45 {
                    continue;
                }
                let d_px = (local.x - old_hx).abs();
                let d_nx = (local.x + old_hx).abs();
                let d_pz = (local.y - old_hz).abs();
                let d_nz = (local.y + old_hz).abs();
                let min_d = d_px.min(d_nx).min(d_pz).min(d_nz);
                if min_d > 0.45 {
                    continue;
                }
                let new_local = if min_d == d_px {
                    let ratio = if old_hz > 0.05 { local.y / old_hz } else { 0.0 };
                    Vec2::new(new_hx, (ratio * new_hz).clamp(-new_hz, new_hz))
                } else if min_d == d_nx {
                    let ratio = if old_hz > 0.05 { local.y / old_hz } else { 0.0 };
                    Vec2::new(-new_hx, (ratio * new_hz).clamp(-new_hz, new_hz))
                } else if min_d == d_pz {
                    let ratio = if old_hx > 0.05 { local.x / old_hx } else { 0.0 };
                    Vec2::new((ratio * new_hx).clamp(-new_hx, new_hx), new_hz)
                } else {
                    let ratio = if old_hx > 0.05 { local.x / old_hx } else { 0.0 };
                    Vec2::new((ratio * new_hx).clamp(-new_hx, new_hx), -new_hz)
                };
                other.pos = local_to_world_2d(new_piece, new_local);
            }
            GladeKind::Tower => {
                let old_rx = (old_ext.x * 0.5).max(0.2);
                let old_rz = (old_ext.z * 0.5).max(0.2);
                let new_rx = (new_ext.x * 0.5).max(0.2);
                let new_rz = (new_ext.z * 0.5).max(0.2);
                let theta = (local.y / old_rz).atan2(local.x / old_rx);
                let on_old = Vec2::new(old_rx * theta.cos(), old_rz * theta.sin());
                if local.distance(on_old) > 0.45 {
                    continue;
                }
                let on_new = Vec2::new(new_rx * theta.cos(), new_rz * theta.sin());
                other.pos = local_to_world_2d(new_piece, on_new);
            }
            GladeKind::Wall => {
                let old_hl = old_ext.x * 0.5;
                let new_hl = new_ext.x * 0.5;
                if local.x.abs() > old_hl + 0.45 || local.y.abs() > 0.45 {
                    continue;
                }
                let ratio = if old_hl > 0.05 { local.x / old_hl } else { 0.0 };
                let new_local = Vec2::new((ratio * new_hl).clamp(-new_hl, new_hl), 0.0);
                other.pos = local_to_world_2d(new_piece, new_local);
            }
            GladeKind::Door | GladeKind::Window | GladeKind::Terrain | GladeKind::Path => {}
        }
    }
}

/// True when `shape` is a Glade building host (`glade_room` or `glade_tower`)
/// that can own attached doors, windows, walls, and fixtures.
pub fn is_building_shape(shape: &Shape) -> bool {
    matches!(glade_kind(shape), Some(GladeKind::Room | GladeKind::Tower))
}

/// Distance from a world-space 2D point `pt` to a Glade building's outer
/// footprint (`0.0` if inside or on the wall).
fn distance_to_building_footprint(pt: Vec2, building: &Piece, building_shape: &Shape) -> f32 {
    let ext = piece_extents(building, building_shape);
    let local = world_to_local_2d(building, pt);
    match glade_kind(building_shape) {
        Some(GladeKind::Room) => {
            let hx = ext.x * 0.5;
            let hz = ext.z * 0.5;
            let dx = (local.x.abs() - hx).max(0.0);
            let dz = (local.y.abs() - hz).max(0.0);
            Vec2::new(dx, dz).length()
        }
        Some(GladeKind::Tower) => {
            let rx = (ext.x * 0.5).max(0.2);
            let rz = (ext.z * 0.5).max(0.2);
            let nr = Vec2::new(local.x / rx, local.y / rz).length();
            ((nr - 1.0) * rx.min(rz)).max(0.0)
        }
        _ => f32::INFINITY,
    }
}

/// True when `fixture` (door, window, wall, or prop) is attached to `building`.
pub fn is_attached_to_building(fixture: &Piece, building: &Piece) -> bool {
    if fixture.id == building.id {
        return false;
    }
    let (Some(fix_shape), Some(bld_shape)) =
        (shapes().get(fixture.shape), shapes().get(building.shape))
    else {
        return false;
    };
    if !is_building_shape(bld_shape) {
        return false;
    }
    // Buildings, terrain, and paths are independent structural/landscape pieces.
    if is_building_shape(fix_shape)
        || matches!(
            glade_kind(fix_shape),
            Some(GladeKind::Terrain | GladeKind::Path)
        )
    {
        return false;
    }

    let bld_ext = piece_extents(building, bld_shape);
    let fix_h = if fix_shape.is_glade() {
        piece_extents(fixture, fix_shape).y
    } else {
        fix_shape.height.max(0.5)
    };
    if fixture.elevation + fix_h < building.elevation - 0.5
        || fixture.elevation > building.elevation + bld_ext.y + 1.2
    {
        return false;
    }

    // Check fixture centre and, for bar/wall pieces, both endpoints.
    let half_len = if fix_shape.is_glade() {
        piece_extents(fixture, fix_shape).x * 0.5
    } else if let model::Geometry::Bar { len, .. } = fix_shape.geom {
        len * 0.5
    } else {
        0.0
    };
    let mut probes = vec![fixture.pos];
    if half_len > 0.1 {
        let dir = Vec2::new(fixture.rot.cos(), fixture.rot.sin());
        probes.push(fixture.pos - dir * half_len);
        probes.push(fixture.pos + dir * half_len);
    }

    probes
        .into_iter()
        .any(|p| distance_to_building_footprint(p, building, bld_shape) <= 0.85)
}

/// Find the closest host building (`glade_room` / `glade_tower`) that `fixture`
/// is attached to, if any.
pub fn find_host_building(fixture: &Piece, pieces: &[Piece]) -> Option<u32> {
    let mut best: Option<(f32, u32)> = None;
    for other in pieces {
        if !is_attached_to_building(fixture, other) {
            continue;
        }
        let Some(bld_shape) = shapes().get(other.shape) else {
            continue;
        };
        let d = distance_to_building_footprint(fixture.pos, other, bld_shape);
        if best.is_none_or(|(best_d, _)| d < best_d) {
            best = Some((d, other.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Intersect a 3D camera ray with a vertical wall quad `(w0 -> w1)` spanning `y0..=y1`.
/// Returns `Some((ray_distance_2d, segment_t))` when the ray hits the wall face.
fn ray_segment_wall_hit(
    ray: Ray3d,
    w0: Vec2,
    w1: Vec2,
    y0: f32,
    y1: f32,
) -> Option<(f32, f32)> {
    let ro = Vec2::new(ray.origin.x, ray.origin.z);
    let rd = Vec2::new(ray.direction.x, ray.direction.z);
    let v = w1 - w0;
    let denom = v.x * rd.y - v.y * rd.x;
    if denom.abs() < 1e-6 {
        return None;
    }
    let qp = ro - w0;
    let t = (qp.x * rd.y - qp.y * rd.x) / denom;
    let s = (qp.x * v.y - qp.y * v.x) / denom;
    if s <= 0.0 || !(-0.08..=1.08).contains(&t) {
        return None;
    }
    let hit_y = ray.origin.y + s * ray.direction.y;
    if hit_y < y0 - 0.35 || hit_y > y1 + 0.35 {
        return None;
    }
    Some((s, t.clamp(0.0, 1.0)))
}

/// Intersect a 3D camera ray with a vertical elliptical cylinder `(x/rx)^2 + (z/rz)^2 = 1`
/// in `tower`'s local space spanning `y0..=y1`.
fn ray_cylinder_wall_hit(
    ray: Ray3d,
    tower: &Piece,
    rx: f32,
    rz: f32,
    y0: f32,
    y1: f32,
) -> Option<(f32, Vec2)> {
    let ro_world = Vec2::new(ray.origin.x, ray.origin.z);
    let rd_world = Vec2::new(ray.direction.x, ray.direction.z);
    let ro_local = world_to_local_2d(tower, ro_world);
    let (sin, cos) = (-tower.rot).sin_cos();
    let rd_local = Vec2::new(
        rd_world.x * cos - rd_world.y * sin,
        rd_world.x * sin + rd_world.y * cos,
    );

    let ou = ro_local.x / rx;
    let ov = ro_local.y / rz;
    let du = rd_local.x / rx;
    let dv = rd_local.y / rz;

    let a = du * du + dv * dv;
    if a < 1e-6 {
        return None;
    }
    let b = 2.0 * (ou * du + ov * dv);
    let c = ou * ou + ov * ov - 1.0;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_d = disc.sqrt();
    for s in [(-b - sqrt_d) / (2.0 * a), (-b + sqrt_d) / (2.0 * a)] {
        if s > 0.0 {
            let hit_y = ray.origin.y + s * ray.direction.y;
            if hit_y >= y0 - 0.35 && hit_y <= y1 + 0.35 {
                return Some((s, ro_local + rd_local * s));
            }
        }
    }
    None
}

/// Detect archway openings along `segment` caused by:
/// 1. Any polyline segment in `path_points` crossing the wall segment, or
/// 2. Any placed door/threshold piece (`glade_door`, `door`, `gate`, `bridge`) overlapping it.
pub fn compute_arch_openings(
    piece: &Piece,
    segment: &ExposedWallSegment,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> Vec<ArchOpening> {
    let seg_len = segment.length();
    let seg_height = segment.height();
    // An archway needs enough wall width and height to form an arch.
    if seg_len < 0.55 || seg_height < 0.95 || segment.base_y > 0.25 {
        return Vec::new();
    }

    let w0 = local_to_world_2d(piece, segment.p0);
    let w1 = local_to_world_2d(piece, segment.p1);
    let mut hits: Vec<(f32, f32, f32)> = Vec::new();
    let default_w = 1.05_f32.min((seg_len - 0.18).max(0.42));
    let default_h = (seg_height - 0.18).clamp(0.85, 2.15);

    // 1. Check path polylines crossing the wall segment.
    for line in path_polylines {
        for pair in line.windows(2) {
            if let Some((t_wall, _)) = segment_intersection_2d(w0, w1, pair[0], pair[1]) {
                hits.push((t_wall, default_w, default_h));
            } else {
                // Also trigger if a path endpoint lands right on the wall face.
                for &pt in pair {
                    let (dist, t_wall) = point_to_segment_2d(pt, w0, w1);
                    if dist < 0.28 {
                        hits.push((t_wall, default_w, default_h));
                    }
                }
            }
        }
    }

    // 2. Check door/threshold pieces (`glade_door`, `door`, `gate`, `bridge`) near the wall segment.
    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if !is_door_cutter_shape(other_shape) {
            continue;
        }
        if (other.elevation - piece.elevation).abs() > 1.15 {
            continue;
        }
        let (dist, t_wall) = point_to_segment_2d(other.pos, w0, w1);
        if dist < 0.85 {
            let (door_w, door_h) = if other_shape.id == "glade_door" {
                let dext = piece_extents(other, other_shape);
                (
                    dext.x.min((seg_len - 0.16).max(0.42)),
                    dext.y.min((seg_height - 0.12).max(0.80)),
                )
            } else {
                (default_w, default_h)
            };
            hits.push((t_wall, door_w, door_h));
        }
    }

    if hits.is_empty() {
        return Vec::new();
    }

    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Merge overlapping arch openings along the same segment.
    let mut openings: Vec<ArchOpening> = Vec::new();
    for (t, arch_width, crown_height) in hits {
        let half_w = arch_width * 0.5;
        let margin = (half_w + 0.08).min(seg_len * 0.5);
        let c = (t * seg_len).clamp(margin, (seg_len - margin).max(margin));
        if openings
            .last()
            .is_some_and(|prev| (c - prev.center_dist).abs() < (arch_width + prev.width) * 0.5 + 0.22)
        {
            continue;
        }
        openings.push(ArchOpening {
            center_dist: c,
            width: arch_width,
            crown_height,
        });
    }
    openings
}

/// A rectangular boolean window opening carved through an exposed wall segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowOpening {
    /// Horizontal distance from segment start `p0` to window centre.
    pub center_dist: f32,
    /// Bottom sill height in piece-local Y.
    pub bottom_y: f32,
    /// Top lintel height in piece-local Y.
    pub top_y: f32,
    /// Clear opening width in metres.
    pub width: f32,
}

/// Compute boolean window openings along `segment` from any placed `glade_window`
/// pieces snapped to or overlapping the wall.
pub fn compute_window_openings(
    piece: &Piece,
    segment: &ExposedWallSegment,
    all_pieces: &[Piece],
) -> Vec<WindowOpening> {
    let seg_len = segment.length();
    let seg_height = segment.height();
    if seg_len < 0.45 || seg_height < 0.45 {
        return Vec::new();
    }

    let w0 = local_to_world_2d(piece, segment.p0);
    let w1 = local_to_world_2d(piece, segment.p1);
    let mut openings = Vec::new();

    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if other_shape.id != "glade_window" {
            continue;
        }
        let (dist, t_wall) = point_to_segment_2d(other.pos, w0, w1);
        if dist >= 0.85 {
            continue;
        }
        let wext = piece_extents(other, other_shape);
        let clear_w = (wext.x - 0.14).max(0.28).min((seg_len - 0.12).max(0.28));
        let half_w = clear_w * 0.5;
        let margin = (half_w + 0.04).min(seg_len * 0.5);
        let center_dist = (t_wall * seg_len).clamp(margin, (seg_len - margin).max(margin));

        let local_bottom = other.elevation - piece.elevation;
        let local_top = local_bottom + wext.y;
        let bottom_y = local_bottom.max(segment.base_y + 0.06);
        let top_y = local_top.min(segment.top_y - 0.06);
        if top_y <= bottom_y + 0.15 {
            continue;
        }

        openings.push(WindowOpening {
            center_dist,
            bottom_y,
            top_y,
            width: clear_w,
        });
    }

    openings.sort_by(|a, b| {
        a.center_dist
            .partial_cmp(&b.center_dist)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.bottom_y
                    .partial_cmp(&b.bottom_y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    openings
}

// ---------------------------------------------------------------------------
// Organic Handcrafted Mesh Generation
// ---------------------------------------------------------------------------

/// Generated meshes for a single Glade piece:
/// - `stone`: Walls, quoins, arch keystones, coping stones, battlements (Material slot 0)
/// - `glass`: Window panes (Material slot 1)
/// - `roof`: Roof slopes, shingle tiers, fascia & ridge beams (Material slot 2)
pub struct GladeMeshParts {
    pub stone: Mesh,
    pub glass: Mesh,
    pub roof: Mesh,
}

#[derive(Default)]
struct MeshBuf {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshBuf {
    fn push_tri(&mut self, mut verts: [Vec3; 3], normal: Vec3) {
        let area2 = (verts[1] - verts[0]).cross(verts[2] - verts[0]);
        if area2.length_squared() < 1e-12 {
            return;
        }
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        if area2.dot(n) < 0.0 {
            verts.swap(1, 2);
        }
        let first = self.positions.len() as u32;
        let ax = n.x.abs();
        let ay = n.y.abs();
        let az = n.z.abs();
        for v in verts {
            self.positions.push(v.to_array());
            self.normals.push(n.to_array());
            // Box-projected UVs in metre-scale texel density so vertical walls,
            // bricks, quoins, and roof slopes all have non-stretched 2D UVs.
            let uv = if ay >= ax && ay >= az {
                [v.x * 0.5, v.z * 0.5]
            } else if ax >= az {
                [v.z * 0.5, v.y * 0.5]
            } else {
                [v.x * 0.5, v.y * 0.5]
            };
            self.uvs.push(uv);
        }
        self.indices.extend([first, first + 1, first + 2]);
    }

    fn push_tri_uv(&mut self, mut verts: [Vec3; 3], mut uvs: [[f32; 2]; 3], normal: Vec3) {
        let area2 = (verts[1] - verts[0]).cross(verts[2] - verts[0]);
        if area2.length_squared() < 1e-12 {
            return;
        }
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        if area2.dot(n) < 0.0 {
            verts.swap(1, 2);
            uvs.swap(1, 2);
        }
        let first = self.positions.len() as u32;
        for (v, uv) in verts.into_iter().zip(uvs) {
            self.positions.push(v.to_array());
            self.normals.push(n.to_array());
            self.uvs.push(uv);
        }
        self.indices.extend([first, first + 1, first + 2]);
    }

    fn push_quad(&mut self, v0: Vec3, v1: Vec3, v2: Vec3, v3: Vec3, normal: Vec3) {
        self.push_tri([v0, v1, v2], normal);
        self.push_tri([v0, v2, v3], normal);
    }

    /// Append an oriented 3D box (used for quoins, coping stones, window sills,
    /// keystones, shingle strips, and battlement merlons).
    fn push_oriented_box(
        &mut self,
        center: Vec3,
        right: Vec3,
        up: Vec3,
        outward: Vec3,
        half_extents: Vec3,
        seed: u32,
        jitter_amount: f32,
    ) {
        let rx = right * half_extents.x;
        let ry = up * half_extents.y;
        let rz = outward * half_extents.z;

        let raw_corners = [
            center - rx - ry - rz,
            center + rx - ry - rz,
            center + rx + ry - rz,
            center - rx + ry - rz,
            center - rx - ry + rz,
            center + rx - ry + rz,
            center + rx + ry + rz,
            center - rx + ry + rz,
        ];
        let c: [Vec3; 8] = std::array::from_fn(|i| {
            if jitter_amount > 0.0 {
                raw_corners[i] + organic_jitter(raw_corners[i], seed.wrapping_add(i as u32)) * jitter_amount
            } else {
                raw_corners[i]
            }
        });

        // Front (+outward)
        self.push_quad(c[4], c[5], c[6], c[7], outward);
        // Back (-outward)
        self.push_quad(c[1], c[0], c[3], c[2], -outward);
        // Right (+right)
        self.push_quad(c[5], c[1], c[2], c[6], right);
        // Left (-right)
        self.push_quad(c[0], c[4], c[7], c[3], -right);
        // Top (+up)
        self.push_quad(c[7], c[6], c[2], c[3], up);
        // Bottom (-up)
        self.push_quad(c[0], c[1], c[5], c[4], -up);
    }

    fn finish(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Deterministic spatial vertex jitter so stones, walls, and roof shingles
/// have Tiny Glade's soft, hand-sculpted silhouette rather than rigid CAD lines.
pub fn organic_jitter(pos: Vec3, seed: u32) -> Vec3 {
    let s = seed as f32 * 1.37;
    let jx = (pos.x * 3.7 + pos.y * 5.1 + pos.z * 2.3 + s).sin()
        * (pos.z * 4.3 - pos.y * 2.9 + s).cos();
    let jy = (pos.y * 4.1 + pos.x * 2.7 - pos.z * 3.5 + s).sin() * 0.6;
    let jz = (pos.z * 3.9 - pos.x * 4.7 + pos.y * 3.1 + s).cos()
        * (pos.x * 3.3 + pos.y * 2.1 + s).sin();
    Vec3::new(jx, jy, jz) * 0.022
}

/// Synthesize all procedural meshes (`stone`, `glass`, `roof`) for `piece`,
/// incorporating 2D footprint boolean culling, archway cutouts, height-based
/// wall-to-fence transitions, procedural windows, quoins, and roof shingles.
pub fn build_glade_meshes(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> GladeMeshParts {
    build_glade_meshes_with_spine(piece, shape, all_pieces, path_polylines, None)
}

/// Synthesize all procedural meshes (`stone`, `glass`, `roof`) for `piece`,
/// optionally extruding a `glade_wall` along a multi-point curved `wall_spine`
/// (in world-space 2D coordinates).
pub fn build_glade_meshes_with_spine(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    wall_spine: Option<&[Vec2]>,
) -> GladeMeshParts {
    let mut stone = MeshBuf::default();
    let mut glass = MeshBuf::default();
    let mut roof = MeshBuf::default();

    let Some(kind) = glade_kind(shape) else {
        return GladeMeshParts {
            stone: stone.finish(),
            glass: glass.finish(),
            roof: roof.finish(),
        };
    };

    let ext = piece_extents(piece, shape);
    let tier = wall_tier(ext.y);
    let details = details_enabled(piece);
    let seed = piece.id.wrapping_mul(747796405).wrapping_add(2891336453);

    match kind {
        GladeKind::Path => {}
        GladeKind::Terrain => {
            build_terrain_mesh(
                piece,
                shape,
                all_pieces,
                path_polylines,
                details,
                seed,
                &mut stone,
                &mut roof,
            );
        }
        GladeKind::Door => {
            build_door_portal_mesh(piece, shape, details, seed, &mut stone, &mut roof);
        }
        GladeKind::Window => {
            build_window_mesh(piece, shape, details, seed, &mut stone, &mut glass, &mut roof);
        }
        GladeKind::Wall => {
            build_free_wall_mesh(
                piece,
                shape,
                all_pieces,
                path_polylines,
                wall_spine,
                tier,
                details,
                seed,
                &mut stone,
                &mut roof,
            );
        }
        GladeKind::Room | GladeKind::Tower => {
            let is_tent = is_tent_house(piece, shape);
            let exposed = compute_exposed_wall_segments(piece, shape, all_pieces);
            build_building_walls(
                piece,
                kind,
                &exposed,
                all_pieces,
                path_polylines,
                tier,
                is_tent,
                details,
                seed,
                &mut stone,
                &mut glass,
            );
            // Floor & ceiling caps so the building is solid from outside and inside
            let outline = local_footprint(piece, shape);
            add_horizontal_cap(&mut stone, &outline, 0.0, Vec3::NEG_Y);
            add_horizontal_cap(&mut stone, &outline, 0.0, Vec3::Y);
            add_horizontal_cap(&mut stone, &outline, ext.y, Vec3::Y);
            add_horizontal_cap(&mut stone, &outline, ext.y, Vec3::NEG_Y);

            // Roof synthesis
            let style = roof_style(piece, shape);
            let overhang = roof_overhang(piece, shape);
            let rise = roof_rise(piece, shape);
            match style {
                GladeRoofStyle::Conical => {
                    build_conical_roof(
                        ext.x * 0.5,
                        ext.z * 0.5,
                        ext.y,
                        rise,
                        overhang,
                        details,
                        seed,
                        &mut roof,
                    );
                }
                GladeRoofStyle::Gable => {
                    build_gable_roof(
                        ext.x,
                        ext.z,
                        ext.y,
                        rise,
                        overhang,
                        details,
                        seed,
                        &mut stone,
                        &mut roof,
                    );
                }
                GladeRoofStyle::Hipped => {
                    build_hipped_roof(
                        ext.x,
                        ext.z,
                        ext.y,
                        rise,
                        overhang,
                        details,
                        seed,
                        &mut roof,
                    );
                }
                GladeRoofStyle::Battlements => {
                    build_battlements(&exposed, ext.y, details, seed, &mut stone, &mut roof);
                }
            }
        }
    }

    GladeMeshParts {
        stone: stone.finish(),
        glass: glass.finish(),
        roof: roof.finish(),
    }
}

/// Deterministic pseudo-random float in `0.0..=1.0` from a 32-bit seed.
#[inline]
fn hash_f32(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B1);
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    (x & 0x00FF_FFFF) as f32 / 16_777_215.0
}

/// Emit a chunky, chiseled, flat-shaded low-poly 3D rock boulder (inspired by
/// Tiny Glade's faceted cliff boulders in `ref.jpg`).
///
/// Builds a 20-vertex closed rock polyhedron (`top`, 3 staggered 6-vertex rings,
/// `bottom`), applies deterministic planar bevel cuts to form angular rock
/// cleavage planes, and emits each of the 36 triangular facets with its own
/// flat geometric face normal so directional sunlight highlights each rock plane.
fn emit_faceted_boulder(
    buf: &mut MeshBuf,
    center: Vec3,
    right: Vec3,
    up: Vec3,
    outward: Vec3,
    half_extents: Vec3,
    seed: u32,
) {
    // 4 deterministic planar bevel cuts `(normal, plane_dist)` in unit rock space
    let planes: [(Vec3, f32); 4] = [
        (
            Vec3::new(
                hash_f32(seed.wrapping_add(3)) - 0.5,
                0.55 + hash_f32(seed.wrapping_add(7)) * 0.4,
                0.45 + hash_f32(seed.wrapping_add(11)) * 0.4,
            )
            .normalize_or_zero(),
            0.52 + hash_f32(seed.wrapping_add(13)) * 0.22,
        ),
        (
            Vec3::new(
                0.65 * if seed & 1 == 0 { 1.0 } else { -1.0 },
                0.25 - hash_f32(seed.wrapping_add(17)) * 0.5,
                0.45 + hash_f32(seed.wrapping_add(19)) * 0.35,
            )
            .normalize_or_zero(),
            0.56 + hash_f32(seed.wrapping_add(23)) * 0.20,
        ),
        (
            Vec3::new(
                -0.55 * if seed & 2 == 0 { 1.0 } else { -1.0 },
                0.45 + hash_f32(seed.wrapping_add(29)) * 0.35,
                -0.25 + hash_f32(seed.wrapping_add(31)) * 0.5,
            )
            .normalize_or_zero(),
            0.58 + hash_f32(seed.wrapping_add(37)) * 0.20,
        ),
        (
            Vec3::new(
                (hash_f32(seed.wrapping_add(41)) - 0.5) * 0.6,
                -0.20 + hash_f32(seed.wrapping_add(43)) * 0.4,
                0.85,
            )
            .normalize_or_zero(),
            0.60 + hash_f32(seed.wrapping_add(47)) * 0.22,
        ),
    ];

    let mut unit_verts = [Vec3::ZERO; 20];
    // Rings 0..3 (6 vertices each = indices 0..18)
    let ring_specs: [(f32, f32, f32); 3] = [
        (0.58, 0.78, 0.0),
        (0.02, 1.06, std::f32::consts::FRAC_PI_6),
        (-0.58, 0.94, 0.0),
    ];
    for (r_idx, &(ry, rad, phase)) in ring_specs.iter().enumerate() {
        for k in 0..6 {
            let idx = r_idx * 6 + k;
            let v_seed = seed.wrapping_add((idx as u32).wrapping_mul(79));
            let angle = phase
                + k as f32 * std::f32::consts::FRAC_PI_3
                + (hash_f32(v_seed) - 0.5) * 0.26;
            let r_jit = rad * (0.82 + hash_f32(v_seed.wrapping_add(5)) * 0.34);
            let y_jit = ry + (hash_f32(v_seed.wrapping_add(9)) - 0.5) * 0.22;
            let (s, c) = angle.sin_cos();
            unit_verts[idx] = Vec3::new(c * r_jit, y_jit, s * r_jit);
        }
    }
    // Top crown (index 18) and bottom base (index 19)
    unit_verts[18] = Vec3::new(
        (hash_f32(seed.wrapping_add(211)) - 0.5) * 0.24,
        0.92 + (hash_f32(seed.wrapping_add(223)) - 0.5) * 0.16,
        (hash_f32(seed.wrapping_add(227)) - 0.5) * 0.24,
    );
    unit_verts[19] = Vec3::new(
        (hash_f32(seed.wrapping_add(233)) - 0.5) * 0.22,
        -0.94,
        (hash_f32(seed.wrapping_add(239)) - 0.5) * 0.22,
    );

    // Apply planar bevel cuts and transform into target space
    let mut verts = [Vec3::ZERO; 20];
    for (i, mut p) in unit_verts.into_iter().enumerate() {
        for &(pn, pd) in &planes {
            let excess = p.dot(pn) - pd;
            if excess > 0.0 {
                p -= pn * (excess * 0.74);
            }
        }
        verts[i] = center
            + right * (p.x * half_extents.x)
            + up * (p.y * half_extents.y)
            + outward * (p.z * half_extents.z);
    }

    let push_flat_tri = |buf: &mut MeshBuf, a: usize, b: usize, c: usize| {
        let va = verts[a];
        let vb = verts[b];
        let vc = verts[c];
        let mut n = (vb - va).cross(vc - va).normalize_or_zero();
        let centroid = (va + vb + vc) * (1.0 / 3.0);
        if n.dot(centroid - center) < 0.0 {
            n = -n;
        }
        buf.push_tri([va, vb, vc], n);
    };

    for k in 0..6 {
        let next = (k + 1) % 6;
        // Top cap -> Ring 0
        push_flat_tri(buf, 18, k, next);
        // Ring 0 <-> Ring 1
        push_flat_tri(buf, k, 6 + k, next);
        push_flat_tri(buf, next, 6 + k, 6 + next);
        // Ring 1 <-> Ring 2
        push_flat_tri(buf, 6 + k, 12 + k, 12 + next);
        push_flat_tri(buf, 6 + k, 12 + next, 6 + next);
        // Ring 2 -> Bottom cap
        push_flat_tri(buf, 12 + k, 19, 12 + next);
    }
}

/// Emit a lush 3D meadow grass clump (`6` tapered, slightly curved 3D grass
/// blades fanned out in azimuth) at `base_pos`.
fn emit_meadow_grass_clump(
    buf: &mut MeshBuf,
    base_pos: Vec3,
    scale: f32,
    seed: u32,
) {
    let blades: [(f32, f32, f32, f32, f32, f32); 6] = [
        (0.00, 0.00, 0.04, -0.05, 0.048, 0.44),
        (0.15, 0.06, 0.12, 0.04, 0.044, 0.36),
        (-0.14, 0.08, -0.11, 0.06, 0.044, 0.38),
        (0.06, -0.15, 0.05, -0.12, 0.040, 0.32),
        (-0.08, -0.13, -0.07, -0.10, 0.040, 0.34),
        (0.19, -0.08, 0.14, -0.05, 0.036, 0.28),
    ];
    let yaw = hash_f32(seed) * std::f32::consts::TAU;
    let (sy, cy) = yaw.sin_cos();
    let wind_x = (base_pos.x * 0.35 + base_pos.z * 0.25).sin() * 0.04 * scale;
    let wind_z = (base_pos.z * 0.31 - base_pos.x * 0.22).cos() * 0.04 * scale;

    for (i, (bx, bz, lx, lz, w, h)) in blades.into_iter().enumerate() {
        let b_seed = seed.wrapping_add((i as u32).wrapping_mul(31));
        let h_var = h * scale * (0.85 + hash_f32(b_seed) * 0.32);
        let w_var = w * scale.clamp(0.75, 1.35);

        let rx = bx * cy - bz * sy;
        let rz = bx * sy + bz * cy;
        let rlx = lx * cy - lz * sy + wind_x;
        let rlz = lx * sy + lz * cy + wind_z;

        // Blade tangent perpendicular to blade azimuth
        let blade_ang = yaw + i as f32 * 0.52;
        let tx = blade_ang.cos() * w_var;
        let tz = blade_ang.sin() * w_var;

        // Upward-biased normal so directional sun lights the meadow grass softly from every angle
        let n = Vec3::new(rlx * 0.35, 0.90, rlz * 0.35).normalize_or_zero();

        // 3 vertical segments (root -> mid1 -> mid2 -> pointed tip) with uv.y in 0.0..=1.0
        // for smooth shader wind/trample bending and root-to-tip shading.
        let p00 = base_pos + Vec3::new(rx * scale - tx, -0.02, rz * scale - tz);
        let p01 = base_pos + Vec3::new(rx * scale + tx, -0.02, rz * scale + tz);
        let p10 = base_pos
            + Vec3::new(
                rx * scale + rlx * scale * 0.16 - tx * 0.72,
                h_var * 0.35,
                rz * scale + rlz * scale * 0.16 - tz * 0.72,
            );
        let p11 = base_pos
            + Vec3::new(
                rx * scale + rlx * scale * 0.16 + tx * 0.72,
                h_var * 0.35,
                rz * scale + rlz * scale * 0.16 + tz * 0.72,
            );
        let p20 = base_pos
            + Vec3::new(
                rx * scale + rlx * scale * 0.52 - tx * 0.38,
                h_var * 0.70,
                rz * scale + rlz * scale * 0.52 - tz * 0.38,
            );
        let p21 = base_pos
            + Vec3::new(
                rx * scale + rlx * scale * 0.52 + tx * 0.38,
                h_var * 0.70,
                rz * scale + rlz * scale * 0.52 + tz * 0.38,
            );
        let tip = base_pos + Vec3::new(rx * scale + rlx * scale, h_var, rz * scale + rlz * scale);

        buf.push_tri_uv([p00, p01, p11], [[0.0, 0.0], [1.0, 0.0], [1.0, 0.35]], n);
        buf.push_tri_uv([p00, p11, p10], [[0.0, 0.0], [1.0, 0.35], [0.0, 0.35]], n);
        buf.push_tri_uv([p10, p11, p21], [[0.0, 0.35], [1.0, 0.35], [1.0, 0.70]], n);
        buf.push_tri_uv([p10, p21, p20], [[0.0, 0.35], [1.0, 0.70], [0.0, 0.70]], n);
        buf.push_tri_uv([p20, p21, tip], [[0.0, 0.70], [1.0, 0.70], [0.5, 1.0]], n);
    }
}

/// Emit a wildflower & broad-leaf meadow clover patch at `base_pos` (like the
/// yellow and white wildflowers in `ref.jpg`).
fn emit_wildflower_cluster(
    clover_buf: &mut MeshBuf,
    flower_buf: &mut MeshBuf,
    base_pos: Vec3,
    scale: f32,
    seed: u32,
) {
    // 3 broad-leaf herb/clover leaves near the base
    for i in 0..3 {
        let ang = seed as f32 * 0.7 + i as f32 * 2.094;
        let (s, c) = ang.sin_cos();
        let dir = Vec3::new(c, 0.35, s).normalize_or_zero();
        let side = Vec3::new(-s, 0.0, c) * (0.075 * scale);
        let p0 = base_pos + Vec3::Y * 0.04;
        let p1 = p0 + dir * (0.14 * scale) + side;
        let p2 = p0 + dir * (0.24 * scale);
        let p3 = p0 + dir * (0.14 * scale) - side;
        clover_buf.push_quad(p0, p1, p2, p3, Vec3::new(c * 0.25, 0.94, s * 0.25));
    }

    // 3 small 3D blossom heads perched above the grass
    for i in 0..3 {
        let f_seed = seed.wrapping_add(i * 47);
        let ox = (hash_f32(f_seed) - 0.5) * 0.34 * scale;
        let oz = (hash_f32(f_seed.wrapping_add(7)) - 0.5) * 0.34 * scale;
        let fy = (0.26 + hash_f32(f_seed.wrapping_add(13)) * 0.16) * scale;
        let center = base_pos + Vec3::new(ox, fy, oz);
        let r = (0.042 + hash_f32(f_seed.wrapping_add(19)) * 0.018) * scale;
        for p in 0..3 {
            let a0 = p as f32 * 2.094 + hash_f32(f_seed) * 0.5;
            let a1 = a0 + 1.45;
            let v0 = center + Vec3::new(0.0, 0.012, 0.0);
            let v1 = center + Vec3::new(a0.cos() * r, 0.0, a0.sin() * r);
            let v2 = center + Vec3::new(a1.cos() * r, 0.0, a1.sin() * r);
            flower_buf.push_tri([v0, v1, v2], Vec3::Y);
        }
    }
}

/// Shortest 2D distance from `world_pt` to any segment in `polylines`.
fn min_distance_to_polylines(world_pt: Vec2, polylines: &[Vec<Vec2>]) -> f32 {
    let mut best = f32::INFINITY;
    for line in polylines {
        for pair in line.windows(2) {
            let (d, _) = point_to_segment_2d(world_pt, pair[0], pair[1]);
            if d < best {
                best = d;
            }
        }
    }
    best
}

/// Synthesize a procedural Tiny Glade terrain chunk (`glade_terrain`) with:
/// - Smooth multi-chunk heightfield blending across overlapping terrain chunks,
/// - Lush meadow grass sward & 3D grass tufts on plateaus and gentle slopes (`roof` buffer),
/// - Chunky, multi-faceted 3D angular cliff boulders, overhanging grassy turf rims,
///   talus boulders, and automatic cliff stone steps where paths cross (`stone` buffer).
#[allow(clippy::too_many_arguments)]
fn build_terrain_mesh(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    if piece.elevation < -0.05 {
        return;
    }
    let ext = piece_extents(piece, shape);
    let rx = (ext.x * 0.5).max(0.4);
    let rz = (ext.z * 0.5).max(0.4);
    let falloff = terrain_falloff(piece, shape);
    let is_plateau = terrain_style(piece) == GladeTerrainStyle::Plateau;

    let rings: usize = 12;
    let segments: usize = 28;

    // Sample local 3D grid `[ring 0..=rings][seg 0..segments]`
    let mut grid = vec![vec![Vec3::ZERO; segments]; rings + 1];
    let mut world_grid = vec![vec![Vec2::ZERO; segments]; rings + 1];

    let center_my_h =
        terrain_chunk_height_at(piece, shape, piece.pos).unwrap_or(piece.elevation + ext.y);
    let center_blended = terrain_height_at(piece.pos, all_pieces);
    let center_y = (center_my_h.max(center_blended * 0.98) - piece.elevation).max(0.0);
    for s in 0..segments {
        grid[0][s] = Vec3::new(0.0, center_y, 0.0);
        world_grid[0][s] = piece.pos;
    }

    for r in 1..=rings {
        let rf = r as f32 / rings as f32;
        for s in 0..segments {
            let angle = std::f32::consts::TAU * s as f32 / segments as f32;
            let contour = terrain_contour_point(rx * rf, rz * rf, angle, is_plateau);
            let world_pt = local_to_world_2d(piece, contour);
            world_grid[r][s] = world_pt;

            let my_h = terrain_chunk_height_at(piece, shape, world_pt).unwrap_or(piece.elevation);
            let blended_h = terrain_height_at(world_pt, all_pieces);
            // Smoothly pull the skirt up toward overlapping sibling chunks in the blend zone
            let target_world_y = if r < rings && (blended_h - my_h).abs() < 0.65 {
                my_h.max(blended_h)
            } else {
                my_h
            };
            let mut local_y = if r == rings {
                0.0
            } else {
                (target_world_y - piece.elevation).max(0.0)
            };

            let mut pt = Vec3::new(contour.x, local_y, contour.y);
            if details && r > 0 && r < rings {
                let j = organic_jitter(
                    Vec3::new(world_pt.x, target_world_y, world_pt.y),
                    seed.wrapping_add((r * 41 + s) as u32),
                );
                local_y = (local_y + j.y * 1.15).max(0.0);
                pt = Vec3::new(
                    contour.x + j.x * 1.35,
                    local_y,
                    contour.y + j.z * 1.35,
                );
            }
            grid[r][s] = pt;
        }
    }

    // Helper to check if a world point on this chunk's slope is deeply buried inside
    // another higher terrain chunk (or an equal-height chunk with lower id).
    let is_buried_by_other = |world_pt: Vec2, my_world_y: f32| -> bool {
        for other in all_pieces {
            if other.id == piece.id || other.elevation < -0.05 {
                continue;
            }
            let Some(other_shape) = shapes().get(other.shape) else {
                continue;
            };
            if let Some(other_h) = terrain_chunk_height_at(other, other_shape, world_pt) {
                if other_h > my_world_y + 0.14
                    || ((other_h - my_world_y).abs() <= 0.04 && other.id < piece.id)
                {
                    return true;
                }
            }
        }
        false
    };

    // Emit concentric rings, classifying each cell into lush grass (`roof`) vs
    // steep limestone cliff (`stone`) based on slope angle and falloff.
    for r in 0..rings {
        for s in 0..segments {
            let next_s = (s + 1) % segments;
            let v00 = grid[r][s];
            let v10 = grid[r][next_s];
            let v11 = grid[r + 1][next_s];
            let v01 = grid[r + 1][s];

            let mid_local = (v00 + v10 + v11 + v01) * 0.25;
            let mid_world = (world_grid[r][s]
                + world_grid[r][next_s]
                + world_grid[r + 1][next_s]
                + world_grid[r + 1][s])
                * 0.25;
            if is_buried_by_other(mid_world, piece.elevation + mid_local.y) {
                continue;
            }

            let e_tangent = if r == 0 { v11 - v01 } else { v10 - v00 };
            let e_down = v01 - v00;
            // `e_tangent` runs counter-clockwise (from +X toward +Z) and `e_down` runs
            // radially outward, so `e_down.cross(e_tangent)` points upward (+Y) and outward!
            let mut cell_normal = e_down.cross(e_tangent).normalize_or_zero();
            if cell_normal.y < 0.0 {
                cell_normal = -cell_normal;
            }
            if cell_normal == Vec3::ZERO {
                cell_normal = Vec3::Y;
            }

            // Determine whether this cell is steep enough to expose craggy cliff rock.
            let cliff_cos = (TERRAIN_CLIFF_NORMAL_Y + (falloff - 0.5) * 0.22).clamp(0.55, 0.85);
            let is_cliff = r > 0 && (cell_normal.y < cliff_cos || (falloff > 0.62 && r >= rings / 2));

            let target_buf: &mut MeshBuf = if is_cliff { stone } else { roof };
            if r == 0 {
                target_buf.push_tri([v00, v11, v01], cell_normal);
            } else {
                target_buf.push_quad(v00, v01, v11, v10, cell_normal);
            }

            let cell_seed = seed.wrapping_add((r * 53 + s * 17) as u32);
            let path_dist = min_distance_to_polylines(mid_world, path_polylines);

            // Procedural Tiny Glade faceted cliff boulders, rim turf, & 3D meadow grass
            if details && is_cliff {
                let tangent = e_tangent.normalize_or_zero();
                let radial_out = Vec3::new(cell_normal.x, 0.0, cell_normal.z).normalize_or_zero();
                let chord = v01.distance(v11).max(0.28);
                let step_h = (v00.y - v01.y).abs().max(0.22);

                // 1. Overhanging lush grassy turf lip & 3D grass tufts at the upper cliff rim
                let prev_ring_was_grass = r == 1 || {
                    let pv00 = grid[r - 1][s];
                    let pv10 = grid[r - 1][next_s];
                    let pe_down = v00 - pv00;
                    let mut pn = pe_down.cross(pv10 - pv00).normalize_or_zero();
                    if pn.y < 0.0 {
                        pn = -pn;
                    }
                    pn.y >= cliff_cos && !(falloff > 0.62 && (r - 1) >= rings / 2)
                };
                if prev_ring_was_grass && path_dist > 0.65 {
                    let rim_mid = (v00 + v10) * 0.5 + radial_out * 0.08 + Vec3::Y * 0.02;
                    roof.push_oriented_box(
                        rim_mid,
                        tangent,
                        Vec3::Y,
                        radial_out,
                        Vec3::new(chord * 0.56, 0.065, 0.14),
                        cell_seed,
                        0.45,
                    );
                    emit_meadow_grass_clump(roof, (v00 + v10) * 0.5, 1.15, cell_seed.wrapping_add(11));
                }

                // 2. If a path crosses this cliff cell, carve a cleft and place stone steps!
                if path_dist <= 0.68 {
                    stone.push_oriented_box(
                        mid_local + radial_out * 0.06,
                        tangent,
                        Vec3::Y,
                        radial_out,
                        Vec3::new(0.46, 0.11, 0.24),
                        cell_seed.wrapping_add(37),
                        0.30,
                    );
                } else if (r + s) % 2 == 0 {
                    // Chunky, multi-faceted 3D angular cliff boulders (`ref.jpg`)
                    let b_scale = Vec3::new(
                        (chord * 0.68).clamp(0.38, 1.15),
                        (step_h * 0.85).clamp(0.36, 1.35),
                        (0.28 + falloff * 0.24).clamp(0.28, 0.65),
                    );
                    emit_faceted_boulder(
                        stone,
                        mid_local + radial_out * (b_scale.z * 0.32),
                        tangent,
                        Vec3::Y,
                        radial_out,
                        b_scale,
                        cell_seed.wrapping_add(19),
                    );
                }

                // 3. Faceted talus boulders and grass tufts resting at the foot of steep cliffs
                if r + 1 == rings && s % 2 == 0 && path_dist > 0.72 {
                    let boulder_pos = (v01 + v11) * 0.5 + radial_out * 0.18 + Vec3::Y * 0.16;
                    emit_faceted_boulder(
                        stone,
                        boulder_pos,
                        tangent,
                        Vec3::Y,
                        radial_out,
                        Vec3::new(0.34, 0.24, 0.30),
                        cell_seed.wrapping_add(73),
                    );
                    emit_meadow_grass_clump(
                        roof,
                        (v01 + v11) * 0.5 + radial_out * 0.32,
                        1.20,
                        cell_seed.wrapping_add(97),
                    );
                }
            } else if details && !is_cliff && path_dist > 0.58 {
                // Lush 3D grass clumps across the upper grassy plateau and gentle slopes
                if (r == 0 && s % 4 == 0) || (r > 0 && (r + s) % 2 == 0) {
                    emit_meadow_grass_clump(roof, mid_local, 1.0, cell_seed.wrapping_add(5));
                }
            }
        }
    }

    // Solid bottom cap at y = 0
    let base_outline = local_footprint(piece, shape);
    add_horizontal_cap(stone, &base_outline, 0.0, Vec3::NEG_Y);
}

// ---------------------------------------------------------------------------
// Large-Scale Continuous World Meadow Terrain (`WorldTerrainMeshes`)
// ---------------------------------------------------------------------------

/// Multi-material batched meshes for the continuous `88m x 88m` Tiny Glade
/// world landscape:
/// - `turf`: Continuous grassy meadow & plateau ground mesh,
/// - `rock_light`: Primary weathered limestone faceted cliff boulders & stone steps,
/// - `rock_dark`: Secondary crevice boulders, dark rock slabs, & step risers,
/// - `grass_sun`: Batched multi-blade 3D sunlit chartreuse grass clumps,
/// - `grass_lush`: Batched multi-blade 3D deep meadow green grass clumps & tower ivy,
/// - `grass_clover`: Broad-leaf meadow clover/weed rosettes & ivy leaves,
/// - `flower_yellow`: 3D yellow buttercup/dandelion wildflower blossoms,
/// - `flower_white`: 3D white daisy wildflower blossoms,
/// - `path_sand`: 3D warm sandy-ochre dirt path ribbons hugging the terrain.
pub struct WorldTerrainMeshes {
    pub turf: Mesh,
    pub rock_light: Mesh,
    pub rock_dark: Mesh,
    pub grass_sun: Mesh,
    pub grass_lush: Mesh,
    pub grass_clover: Mesh,
    pub flower_yellow: Mesh,
    pub flower_white: Mesh,
    pub path_sand: Mesh,
}

/// Compute all active path polylines across the world terrain:
/// 1. User-authored paths (`path_polylines` from `route`, `plan`, and `movers`),
/// 2. The scenic hillside trail climbing the built-in `(-X, -Z)` cliff plateau,
/// 3. Automatic hilltop access trails descending from any building perched on an
///    elevated terrain plateau (`elevation >= 0.75m`) down into the meadow.
/// Collect all 2D world-space path polylines used for sandy meadow paths,
/// automatic cliff stone stairs, and archway cutouts:
/// 1. User-authored route/plan/mover `path_polylines`, plus
/// 2. The built-in scenic hillside path climbing up the `(-X, -Z)` cliff cleft.
pub fn compute_world_terrain_paths(
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> Vec<Vec<Vec2>> {
    let mut out: Vec<Vec<Vec2>> = path_polylines
        .iter()
        .filter(|line| line.len() >= 2)
        .cloned()
        .collect();

    // Built-in scenic hillside path climbing up the (-X, -Z) cliff cleft (like ref.jpg right side)
    let builtin_path = vec![
        Vec2::new(-1.4, -2.2),
        Vec2::new(-2.6, -3.4),
        Vec2::new(-4.1, -4.8),
        Vec2::new(-5.6, -6.0),
        Vec2::new(-7.2, -6.9),
    ];
    let blocked_by_carve = all_pieces.iter().any(|p| {
        p.elevation < -0.05 && p.pos.distance(Vec2::new(-6.0, -6.0)) < 6.0
    });
    if !blocked_by_carve {
        out.push(builtin_path);
    }

    out
}

/// Shortest 2D distance from `world_pt` to the exterior boundary (and interior status)
/// of any solid building footprint (`glade_tower`, `glade_room`, `floor`, `block`).
/// Returns `(is_inside_footprint, min_boundary_dist)`.
fn building_footprint_proximity(world_pt: Vec2, all_pieces: &[Piece]) -> (bool, f32) {
    let mut min_dist = f32::INFINITY;
    let mut inside = false;
    for p in all_pieces {
        let Some(shape) = shapes().get(p.shape) else {
            continue;
        };
        let kind = glade_kind(shape);
        if kind == Some(GladeKind::Terrain) {
            continue;
        }
        let is_solid = matches!(kind, Some(GladeKind::Tower | GladeKind::Room | GladeKind::Wall))
            || matches!(shape.id, "floor" | "block" | "ramp");
        if !is_solid {
            continue;
        }
        let poly = world_footprint(p, shape);
        if poly.len() < 3 {
            continue;
        }
        if point_in_polygon_2d(world_pt, &poly) {
            inside = true;
        }
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let (d, _) = point_to_segment_2d(world_pt, a, b);
            if d < min_dist {
                min_dist = d;
            }
        }
    }
    (inside, min_dist)
}

/// Emit 3D sandy-ochre dirt path ribbons on gentle terrain and automatic
/// chiseled 3D stone steps / rock stairs wherever a path crosses a steep
/// terrain slope or cliff (matching the right side of `ref.jpg`).
fn emit_terrain_paths_and_stairs(
    all_pieces: &[Piece],
    paths: &[Vec<Vec2>],
    path_sand: &mut MeshBuf,
    rock_light: &mut MeshBuf,
    rock_dark: &mut MeshBuf,
) {
    let step_len = 0.34_f32;
    let half_w = 0.54_f32;

    for (p_idx, line) in paths.iter().enumerate() {
        if line.len() < 2 {
            continue;
        }
        // Resample polyline into uniform `step_len` 2D samples
        let mut samples: Vec<Vec2> = Vec::new();
        samples.push(line[0]);
        for pair in line.windows(2) {
            let seg = pair[1] - pair[0];
            let len = seg.length();
            if len < 1e-3 {
                continue;
            }
            let steps = (len / step_len).ceil().max(1.0) as usize;
            for s in 1..=steps {
                samples.push(pair[0].lerp(pair[1], s as f32 / steps as f32));
            }
        }
        if samples.len() < 2 {
            continue;
        }

        let heights: Vec<f32> = samples
            .iter()
            .map(|&pt| world_terrain_height_at(pt, all_pieces))
            .collect();

        for i in 0..(samples.len() - 1) {
            let p0 = samples[i];
            let p1 = samples[i + 1];
            let (inside0, _) = building_footprint_proximity(p0, all_pieces);
            let (inside1, _) = building_footprint_proximity(p1, all_pieces);
            if inside0 && inside1 {
                continue;
            }
            let dir2 = (p1 - p0).normalize_or_zero();
            if dir2 == Vec2::ZERO {
                continue;
            }
            let right2 = Vec2::new(-dir2.y, dir2.x);
            let forward = Vec3::new(dir2.x, 0.0, dir2.y);
            let right = Vec3::new(right2.x, 0.0, right2.y);

            let h0 = heights[i];
            let h1 = heights[i + 1];
            let ds = p0.distance(p1).max(0.10);
            let grade = (h1 - h0).abs() / ds;

            // Always lay a warm sandy-ochre bedding ribbon along the path
            let crown = 0.032;
            let edge_lift = 0.008;
            let c0 = Vec3::new(p0.x, h0 + crown, p0.y);
            let c1 = Vec3::new(p1.x, h1 + crown, p1.y);
            let l0 = Vec3::new(
                p0.x - right2.x * half_w,
                world_terrain_height_at(p0 - right2 * half_w, all_pieces) + edge_lift,
                p0.y - right2.y * half_w,
            );
            let l1 = Vec3::new(
                p1.x - right2.x * half_w,
                world_terrain_height_at(p1 - right2 * half_w, all_pieces) + edge_lift,
                p1.y - right2.y * half_w,
            );
            let r0 = Vec3::new(
                p0.x + right2.x * half_w,
                world_terrain_height_at(p0 + right2 * half_w, all_pieces) + edge_lift,
                p0.y + right2.y * half_w,
            );
            let r1 = Vec3::new(
                p1.x + right2.x * half_w,
                world_terrain_height_at(p1 + right2 * half_w, all_pieces) + edge_lift,
                p1.y + right2.y * half_w,
            );
            path_sand.push_quad(l0, c0, c1, l1, Vec3::new(-right2.x * 0.12, 0.98, -right2.y * 0.12));
            path_sand.push_quad(c0, r0, r1, c1, Vec3::new(right2.x * 0.12, 0.98, right2.y * 0.12));

            // Where the path climbs a steep terrain slope or cliff (`grade > 0.26`),
            // automatically place chiseled stone steps / flagstones (`ref.jpg` right)!
            if grade > 0.26 {
                let step_seed = ((p_idx * 131 + i * 37) as u32).wrapping_add(911);
                let mid2 = (p0 + p1) * 0.5;
                let step_top_y = h0.max(h1) + 0.055;
                let step_bot_y = h0.min(h1) - 0.06;
                let step_h = (step_top_y - step_bot_y).clamp(0.14, 0.42);
                let step_center = Vec3::new(mid2.x, step_top_y - step_h * 0.5, mid2.y);

                // Alternate between a wide chiseled flagstone and paired side-by-side stone slabs
                if i % 2 == 0 {
                    rock_light.push_oriented_box(
                        step_center,
                        right,
                        Vec3::Y,
                        forward,
                        Vec3::new(half_w * 0.86, step_h * 0.5, ds * 0.56),
                        step_seed,
                        0.32,
                    );
                } else {
                    for side in [-1.0_f32, 1.0_f32] {
                        let target = if (i + (side > 0.0) as usize) % 2 == 0 {
                            &mut *rock_light
                        } else {
                            &mut *rock_dark
                        };
                        target.push_oriented_box(
                            step_center + right * (side * half_w * 0.44),
                            right,
                            Vec3::Y,
                            forward,
                            Vec3::new(half_w * 0.40, step_h * 0.5, ds * 0.54),
                            step_seed.wrapping_add(if side < 0.0 { 13 } else { 29 }),
                            0.35,
                        );
                    }
                }

                // Flanking cheek-boulders framing the stone staircase cleft
                if i % 2 == 0 {
                    for side in [-1.0_f32, 1.0_f32] {
                        let cheek_pos =
                            step_center + right * (side * (half_w + 0.28)) + Vec3::Y * 0.08;
                        let target = if side < 0.0 {
                            &mut *rock_dark
                        } else {
                            &mut *rock_light
                        };
                        emit_faceted_boulder(
                            target,
                            cheek_pos,
                            right,
                            Vec3::Y,
                            forward,
                            Vec3::new(0.28, step_h * 0.72 + 0.14, 0.26),
                            step_seed.wrapping_add(if side < 0.0 { 53 } else { 71 }),
                        );
                    }
                }
            }
        }
    }
}

/// Emit 3D climbing ivy vines and leaf clusters clinging to the exterior masonry
/// walls of `glade_tower` and `glade_room` pieces (like the ivy on the hilltop
/// stone tower in `ref.jpg`).
fn emit_building_ivy(
    all_pieces: &[Piece],
    grass_lush: &mut MeshBuf,
    grass_clover: &mut MeshBuf,
) {
    for p in all_pieces {
        if !details_enabled(p) {
            continue;
        }
        let Some(shape) = shapes().get(p.shape) else {
            continue;
        };
        let Some(kind) = glade_kind(shape) else {
            continue;
        };
        if !matches!(kind, GladeKind::Tower | GladeKind::Room) || is_tent_house(p, shape) {
            continue;
        }
        let exposed = compute_exposed_wall_segments(p, shape, all_pieces);
        let seed = p.id.wrapping_mul(1_103_515_245).wrapping_add(12_345);

        for (seg_idx, seg) in exposed.iter().enumerate() {
            if seg.base_y > 0.15 || seg.height() < 1.1 {
                continue;
            }
            // Grow ivy vines on ~1/3 of tower segments or ~2 vines per room wall
            let v_seed = seed.wrapping_add(seg_idx as u32 * 97);
            if kind == GladeKind::Tower && seg_idx % 5 != 0 && seg_idx % 7 != 0 {
                continue;
            }
            let w0 = local_to_world_2d(p, seg.p0);
            let w1 = local_to_world_2d(p, seg.p1);
            let seg_len = w0.distance(w1);
            if seg_len < 0.25 {
                continue;
            }
            let out2 = (local_to_world_2d(p, seg.p0 + seg.normal) - w0).normalize_or_zero();
            let right2 = (w1 - w0) / seg_len;
            let outward = Vec3::new(out2.x, 0.0, out2.y);
            let right = Vec3::new(right2.x, 0.0, right2.y);

            let vine_count = if kind == GladeKind::Room {
                ((seg_len / 1.8).floor() as usize).clamp(1, 3)
            } else {
                1
            };
            for v in 0..vine_count {
                let u_frac = (v as f32 + 0.5) / vine_count as f32;
                let base2 = w0.lerp(w1, u_frac);
                let ground_y = world_terrain_height_at(base2, all_pieces).max(p.elevation);
                let max_climb = (p.elevation + seg.top_y - ground_y - 0.25).clamp(0.6, 2.35);
                let climb_h = max_climb * (0.55 + hash_f32(v_seed.wrapping_add(v as u32 * 19)) * 0.45);
                let leaves = ((climb_h / 0.18).ceil() as usize).clamp(4, 14);
                for l in 0..leaves {
                    let lf = l as f32 / leaves as f32;
                    let sway = (lf * 5.5 + v_seed as f32).sin() * 0.14;
                    let ly = ground_y + 0.10 + lf * climb_h;
                    let pos = Vec3::new(base2.x, ly, base2.y)
                        + right * sway
                        + outward * (0.045 + (1.0 - lf) * 0.02);
                    let leaf_r = 0.085 * (1.15 - lf * 0.35);
                    let target = if (l + v) % 2 == 0 {
                        &mut *grass_clover
                    } else {
                        &mut *grass_lush
                    };
                    target.push_quad(
                        pos - right * leaf_r,
                        pos + outward * (leaf_r * 0.7) - Vec3::Y * (leaf_r * 0.6),
                        pos + right * leaf_r,
                        pos + Vec3::Y * leaf_r,
                        (outward + Vec3::Y * 0.45).normalize_or_zero(),
                    );
                }
            }
        }
    }
}

/// Build all batched meshes (`WorldTerrainMeshes`) for the continuous `88m x 88m`
/// Tiny Glade landscape, integrating:
/// - Smooth meadow & plateau heightfield surface (`turf`),
/// - Multi-faceted 3D angular cliff boulders (`rock_light` & `rock_dark`),
/// - Dense 3D grass blades (`grass_sun` & `grass_lush`), clover (`grass_clover`),
///   and wildflowers (`flower_yellow` & `flower_white`),
/// - 3D sandy paths (`path_sand`) and automatic cliff stone stairs,
/// - Climbing ivy vines on stone towers and rooms.
pub fn build_world_terrain_meshes(
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> WorldTerrainMeshes {
    let mut turf = MeshBuf::default();
    let mut rock_light = MeshBuf::default();
    let mut rock_dark = MeshBuf::default();
    let mut grass_sun = MeshBuf::default();
    let mut grass_lush = MeshBuf::default();
    let mut grass_clover = MeshBuf::default();
    let mut flower_yellow = MeshBuf::default();
    let mut flower_white = MeshBuf::default();
    let mut path_sand = MeshBuf::default();

    let paths = compute_world_terrain_paths(all_pieces, path_polylines);

    // 1. Fine inner grid (`-18.0m .. +18.0m`, 56x56 cells = 0.64m resolution)
    let inner_half = 18.0_f32;
    let inner_n: usize = 56;
    let step = (inner_half * 2.0) / inner_n as f32;

    let mut heights = vec![vec![0.0_f32; inner_n + 1]; inner_n + 1];
    let mut normals = vec![vec![Vec3::Y; inner_n + 1]; inner_n + 1];
    for iz in 0..=inner_n {
        let z = -inner_half + iz as f32 * step;
        for ix in 0..=inner_n {
            let x = -inner_half + ix as f32 * step;
            let pt = Vec2::new(x, z);
            heights[iz][ix] = world_terrain_height_at(pt, all_pieces);
            normals[iz][ix] = world_terrain_normal_at(pt, all_pieces);
        }
    }

    let cliff_cos = TERRAIN_CLIFF_NORMAL_Y;

    for iz in 0..inner_n {
        let z0 = -inner_half + iz as f32 * step;
        let z1 = z0 + step;
        for ix in 0..inner_n {
            let x0 = -inner_half + ix as f32 * step;
            let x1 = x0 + step;

            let h00 = heights[iz][ix];
            let h10 = heights[iz][ix + 1];
            let h11 = heights[iz + 1][ix + 1];
            let h01 = heights[iz + 1][ix];

            let v00 = Vec3::new(x0, h00, z0);
            let v10 = Vec3::new(x1, h10, z0);
            let v11 = Vec3::new(x1, h11, z1);
            let v01 = Vec3::new(x0, h01, z1);

            let cell_n = (normals[iz][ix]
                + normals[iz][ix + 1]
                + normals[iz + 1][ix + 1]
                + normals[iz + 1][ix])
                .normalize_or_zero();
            let min_ny = normals[iz][ix]
                .y
                .min(normals[iz][ix + 1].y)
                .min(normals[iz + 1][ix + 1].y)
                .min(normals[iz + 1][ix].y);
            let max_h = h00.max(h10).max(h11).max(h01);
            let min_h = h00.min(h10).min(h11).min(h01);
            let mid2 = Vec2::new((x0 + x1) * 0.5, (z0 + z1) * 0.5);
            let mid_h = (h00 + h10 + h11 + h01) * 0.25;
            let mid3 = Vec3::new(mid2.x, mid_h, mid2.y);

            let is_cliff = max_h > 0.18 && (cell_n.y < cliff_cos || min_ny < cliff_cos - 0.08);
            if is_cliff {
                rock_dark.push_quad(v00, v01, v11, v10, cell_n);
            } else {
                turf.push_quad(v00, v01, v11, v10, cell_n);
            }

            let cell_seed = ((iz * 197 + ix * 73) as u32).wrapping_add(0x5F37_59DF);
            let path_dist = min_distance_to_polylines(mid2, &paths);
            let (inside_bldg, bldg_dist) = building_footprint_proximity(mid2, all_pieces);

            // 2. Chunky, multi-faceted 3D angular cliff boulders & talus (`ref.jpg` center)
            if is_cliff && !inside_bldg && path_dist > 0.68 {
                let slope_out = Vec3::new(cell_n.x, 0.0, cell_n.z).normalize_or_zero();
                let outward = if slope_out == Vec3::ZERO {
                    Vec3::Z
                } else {
                    slope_out
                };
                let right = Vec3::new(-outward.z, 0.0, outward.x);
                let step_drop = (max_h - min_h).max(0.32);

                // Primary hero faceted boulder
                let b_ext = Vec3::new(
                    step * (0.72 + hash_f32(cell_seed) * 0.42),
                    (step_drop * 0.92).clamp(0.42, 1.35),
                    0.34 + hash_f32(cell_seed.wrapping_add(7)) * 0.32,
                );
                let primary_buf = if (ix + iz) % 3 == 0 {
                    &mut rock_dark
                } else {
                    &mut rock_light
                };
                emit_faceted_boulder(
                    primary_buf,
                    mid3 + outward * (b_ext.z * 0.26),
                    right,
                    Vec3::Y,
                    outward,
                    b_ext,
                    cell_seed,
                );

                // Interlocking secondary crevice boulder
                if (ix + iz) % 2 == 0 {
                    let sub_ext = b_ext * Vec3::new(0.68, 0.65, 0.78);
                    let sub_offset = right * ((hash_f32(cell_seed.wrapping_add(19)) - 0.5) * 0.45)
                        - Vec3::Y * (b_ext.y * 0.32)
                        + outward * (b_ext.z * 0.42);
                    let secondary_buf = if (ix + iz) % 3 == 0 {
                        &mut rock_light
                    } else {
                        &mut rock_dark
                    };
                    emit_faceted_boulder(
                        secondary_buf,
                        mid3 + sub_offset,
                        right,
                        Vec3::Y,
                        outward,
                        sub_ext,
                        cell_seed.wrapping_add(43),
                    );
                }

                // Overhanging grassy turf pillow + tall rim grass tufts at upper cliff edge
                if max_h > 0.9 && min_h > max_h - step_drop * 1.15 && (ix + iz) % 2 == 0 {
                    let rim_pos = Vec3::new(mid2.x, max_h - 0.04, mid2.y) + outward * 0.10;
                    turf.push_oriented_box(
                        rim_pos,
                        right,
                        Vec3::Y,
                        outward,
                        Vec3::new(step * 0.56, 0.075, 0.18),
                        cell_seed.wrapping_add(61),
                        0.45,
                    );
                    emit_meadow_grass_clump(
                        &mut grass_sun,
                        rim_pos + Vec3::Y * 0.06,
                        1.25,
                        cell_seed.wrapping_add(67),
                    );
                }

                // Tumbled talus boulders + tall grass pockets at the foot of the cliff
                if min_h < 0.55 && (ix + iz) % 2 == 1 {
                    let foot_pos =
                        Vec3::new(mid2.x, min_h + 0.14, mid2.y) + outward * (step * 0.58);
                    emit_faceted_boulder(
                        &mut rock_light,
                        foot_pos,
                        right,
                        Vec3::Y,
                        outward,
                        Vec3::new(0.32, 0.22, 0.28),
                        cell_seed.wrapping_add(89),
                    );
                    emit_meadow_grass_clump(
                        &mut grass_lush,
                        Vec3::new(foot_pos.x, min_h, foot_pos.z) + outward * 0.22,
                        1.32,
                        cell_seed.wrapping_add(97),
                    );
                }
            } else if !is_cliff && !inside_bldg {
                // Low rocky spur outcrops emerging through the grass on moderate ridge slopes
                // (like the foreground rock seam in `ref.jpg`)
                if mid_h > 0.18 && mid_h < 1.15 && cell_n.y < 0.92 && (ix + iz) % 3 == 0 && path_dist > 0.72 {
                    let slope_out = Vec3::new(cell_n.x, 0.0, cell_n.z).normalize_or_zero();
                    let outward = if slope_out == Vec3::ZERO { Vec3::Z } else { slope_out };
                    let right = Vec3::new(-outward.z, 0.0, outward.x);
                    emit_faceted_boulder(
                        &mut rock_light,
                        mid3 + Vec3::Y * 0.04,
                        right,
                        Vec3::Y,
                        outward,
                        Vec3::new(0.36, 0.18, 0.28),
                        cell_seed.wrapping_add(113),
                    );
                }

                // 3. Dense 3D Meadow Grass Clumps & Wildflowers (`ref.jpg` foreground & hilltop)
                if path_dist > 0.52 {
                    let jx = (hash_f32(cell_seed.wrapping_add(3)) - 0.5) * step * 0.75;
                    let jz = (hash_f32(cell_seed.wrapping_add(9)) - 0.5) * step * 0.75;
                    let gpt2 = mid2 + Vec2::new(jx, jz);
                    let gh = world_terrain_height_at(gpt2, all_pieces);
                    let gpos = Vec3::new(gpt2.x, gh, gpt2.y);

                    // Contextual lushness: taller along path borders, cliff rims, and building bases!
                    let wave = (gpt2.x * 0.42 + gpt2.y * 0.37).sin() * 0.16;
                    let mut lush_scale = (0.94 + wave).clamp(0.78, 1.18);
                    if path_dist < 1.15 {
                        lush_scale *= 1.24;
                    }
                    if bldg_dist < 0.70 {
                        lush_scale *= 1.32;
                    }
                    if cell_n.y < 0.90 {
                        lush_scale *= 1.22;
                    }

                    let grass_target = if (ix + iz) % 2 == 0 {
                        &mut grass_sun
                    } else {
                        &mut grass_lush
                    };
                    emit_meadow_grass_clump(grass_target, gpos, lush_scale, cell_seed);

                    // Second offset sub-tuft in lush meadow zones for extra Tiny Glade density
                    if lush_scale > 1.02 || gh > 0.5 {
                        let sub_pt2 = mid2 - Vec2::new(jx * 0.7, jz * 0.7);
                        let sub_h = world_terrain_height_at(sub_pt2, all_pieces);
                        let other_grass = if (ix + iz) % 2 == 0 {
                            &mut grass_lush
                        } else {
                            &mut grass_sun
                        };
                        emit_meadow_grass_clump(
                            other_grass,
                            Vec3::new(sub_pt2.x, sub_h, sub_pt2.y),
                            lush_scale * 0.88,
                            cell_seed.wrapping_add(151),
                        );
                    }

                    // Natural wildflower & clover patches (~11% of grassy meadow cells)
                    let flower_noise = (gpt2.x * 0.38).sin() * (gpt2.y * 0.41).cos();
                    if flower_noise > 0.32 && hash_f32(cell_seed.wrapping_add(179)) > 0.42 {
                        let flower_target = if hash_f32(cell_seed.wrapping_add(191)) > 0.32 {
                            &mut flower_yellow
                        } else {
                            &mut flower_white
                        };
                        emit_wildflower_cluster(
                            &mut grass_clover,
                            flower_target,
                            gpos,
                            lush_scale,
                            cell_seed.wrapping_add(199),
                        );
                    }
                }
            }
        }
    }

    // 4. Outer rolling meadow skirt (`-WORLD_TERRAIN_HALF_EXTENT .. +WORLD_TERRAIN_HALF_EXTENT` surrounding `-18.0m .. +18.0m`)
    let outer_half = WORLD_TERRAIN_HALF_EXTENT;
    let outer_n: usize = 80;
    let outer_step = (outer_half * 2.0) / outer_n as f32;
    for iz in 0..outer_n {
        let z0 = -outer_half + iz as f32 * outer_step;
        let z1 = z0 + outer_step;
        for ix in 0..outer_n {
            let x0 = -outer_half + ix as f32 * outer_step;
            let x1 = x0 + outer_step;
            // Skip cells already covered by the fine inner grid
            if x0 >= -inner_half + 0.1
                && x1 <= inner_half - 0.1
                && z0 >= -inner_half + 0.1
                && z1 <= inner_half - 0.1
            {
                continue;
            }
            let p00 = Vec2::new(x0, z0);
            let p10 = Vec2::new(x1, z0);
            let p11 = Vec2::new(x1, z1);
            let p01 = Vec2::new(x0, z1);
            let h00 = world_terrain_height_at(p00, all_pieces) - 0.015;
            let h10 = world_terrain_height_at(p10, all_pieces) - 0.015;
            let h11 = world_terrain_height_at(p11, all_pieces) - 0.015;
            let h01 = world_terrain_height_at(p01, all_pieces) - 0.015;
            let v00 = Vec3::new(x0, h00, z0);
            let v10 = Vec3::new(x1, h10, z0);
            let v11 = Vec3::new(x1, h11, z1);
            let v01 = Vec3::new(x0, h01, z1);
            let mid2 = (p00 + p11) * 0.5;
            let n = world_terrain_normal_at(mid2, all_pieces);
            if n.y < cliff_cos {
                rock_light.push_quad(v00, v01, v11, v10, n);
            } else {
                turf.push_quad(v00, v01, v11, v10, n);
                let cell_seed = ((iz * 97 + ix * 41) as u32).wrapping_add(777);
                if (ix + iz) % 3 == 0 && mid2.length() < 90.0 {
                    let mh = (h00 + h10 + h11 + h01) * 0.25;
                    let target = if (ix + iz) % 4 == 0 {
                        &mut grass_sun
                    } else {
                        &mut grass_lush
                    };
                    emit_meadow_grass_clump(
                        target,
                        Vec3::new(mid2.x, mh, mid2.y),
                        1.15,
                        cell_seed,
                    );
                }
            }
        }
    }

    // 5. 3D sandy meadow paths & automatic cliff stone stairs
    emit_terrain_paths_and_stairs(
        all_pieces,
        &paths,
        &mut path_sand,
        &mut rock_light,
        &mut rock_dark,
    );

    // 6. Climbing ivy vines on stone towers and rooms
    emit_building_ivy(all_pieces, &mut grass_lush, &mut grass_clover);

    WorldTerrainMeshes {
        turf: turf.finish(),
        rock_light: rock_light.finish(),
        rock_dark: rock_dark.finish(),
        grass_sun: grass_sun.finish(),
        grass_lush: grass_lush.finish(),
        grass_clover: grass_clover.finish(),
        flower_yellow: flower_yellow.finish(),
        flower_white: flower_white.finish(),
        path_sand: path_sand.finish(),
    }
}

/// Build the elevated walkable surface triangles `(vertices, indices)` for the
/// unified world terrain (`y > 0.02`), used by Rapier 3D physics so characters
/// and raycasts walk up the hillside plateau and cliff stairs.
pub fn world_terrain_elevated_collider(all_pieces: &[Piece]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let half = 64.0_f32;
    let n: usize = 80;
    let step = (half * 2.0) / n as f32;
    let mut verts = Vec::with_capacity((n + 1) * (n + 1));
    for iz in 0..=n {
        let z = -half + iz as f32 * step;
        for ix in 0..=n {
            let x = -half + ix as f32 * step;
            let h = world_terrain_height_at(Vec2::new(x, z), all_pieces);
            verts.push(Vec3::new(x, h, z));
        }
    }
    let mut tris = Vec::new();
    let stride = n + 1;
    for iz in 0..n {
        for ix in 0..n {
            let i00 = (iz * stride + ix) as u32;
            let i10 = (iz * stride + ix + 1) as u32;
            let i11 = ((iz + 1) * stride + ix + 1) as u32;
            let i01 = ((iz + 1) * stride + ix) as u32;
            let max_y = verts[i00 as usize]
                .y
                .max(verts[i10 as usize].y)
                .max(verts[i11 as usize].y)
                .max(verts[i01 as usize].y);
            if max_y > 0.02 {
                tris.push([i00, i01, i11]);
                tris.push([i00, i11, i10]);
            }
        }
    }
    (verts, tris)
}

/// Compute a 64-bit reactive signature for the unified world terrain so it
/// rebuilds only when sculpted terrain, building footprints, or paths change.
pub fn compute_world_terrain_signature(
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    0x7E44_A190_u64.hash(&mut hasher);
    for p in all_pieces {
        let Some(shape) = shapes().get(p.shape) else {
            continue;
        };
        if is_glade_shape(shape) || matches!(shape.id, "floor" | "block" | "ramp") {
            p.id.hash(&mut hasher);
            p.shape.hash(&mut hasher);
            quantize_vec2(p.pos).hash(&mut hasher);
            quantize_f32(p.elevation).hash(&mut hasher);
            quantize_f32(p.rot).hash(&mut hasher);
            quantize_vec3(p.trigger_size).hash(&mut hasher);
            quantize_f32(p.trigger_alpha).hash(&mut hasher);
            p.trigger_color.hash(&mut hasher);
            p.trigger_visible.hash(&mut hasher);
            (p.trigger_shape_override as u8).hash(&mut hasher);
        }
    }
    for line in path_polylines {
        line.len().hash(&mut hasher);
        for &pt in line {
            quantize_vec2(pt).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Build an open vaulted stone & timber doorway portal for `GladeKind::Door` (`glade_door`).
///
/// The portal frame has dressed stone jambs, a semicircular voussoir arch + keystone,
/// and ajar wooden door leaves on the sides while staying completely open through the
/// center so players and cameras can look and walk right through the cut-out wall!
fn build_door_portal_mesh(
    piece: &Piece,
    shape: &Shape,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    let ext = piece_extents(piece, shape);
    let clear_w = (ext.x - 0.24).max(0.55);
    let height = ext.y.max(1.2);
    let depth = ext.z.max(0.38);
    let jamb_w = 0.14;
    let half_w = clear_w * 0.5;
    let radius = half_w;
    let spring_y = (height - radius).max(0.45);

    // Left and right stone jamb pillars
    for side in [-1.0_f32, 1.0_f32] {
        let jx = side * (half_w + jamb_w * 0.5);
        stone.push_oriented_box(
            Vec3::new(jx, spring_y * 0.5, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(jamb_w * 0.5, spring_y * 0.5, depth * 0.52),
            seed.wrapping_add(if side < 0.0 { 7 } else { 19 }),
            0.35,
        );
    }

    // Semicircular stone voussoir arch ring and keystone over the open doorway
    let steps = 8;
    for i in 0..steps {
        let a0 = std::f32::consts::PI * (1.0 - i as f32 / steps as f32);
        let a1 = std::f32::consts::PI * (1.0 - (i + 1) as f32 / steps as f32);
        let mid_a = (a0 + a1) * 0.5;
        let radial = Vec3::new(mid_a.cos(), mid_a.sin(), 0.0).normalize_or_zero();
        let tangent = Vec3::new(mid_a.sin(), -mid_a.cos(), 0.0).normalize_or_zero();
        let is_keystone = i == steps / 2 || i + 1 == steps / 2;
        let stone_h = if is_keystone { 0.13 } else { 0.095 };
        let chord = (radius * (a0 - a1).abs()) * 0.56;
        let center = Vec3::new(
            mid_a.cos() * (radius + stone_h * 0.65),
            spring_y + mid_a.sin() * (radius + stone_h * 0.65),
            0.0,
        );
        stone.push_oriented_box(
            center,
            tangent,
            radial,
            Vec3::Z,
            Vec3::new(chord, stone_h, depth * 0.56),
            seed.wrapping_add(i as u32 * 23),
            0.35,
        );
    }

    // Slightly ajar timber door leaves (in `roof` material slot so they render in warm wood/shingle tint)
    if details {
        let leaf_w = (clear_w * 0.36).max(0.22);
        let leaf_h = (spring_y * 0.92).max(0.40);
        for side in [-1.0_f32, 1.0_f32] {
            let hinge_x = side * (half_w - 0.03);
            let swing_angle = -side * 0.95; // swung open ~55 degrees invitingly
            let (s, c) = swing_angle.sin_cos();
            let leaf_right = Vec3::new(c * (-side), 0.0, s * (-side)).normalize_or_zero();
            let leaf_out = Vec3::new(-leaf_right.z, 0.0, leaf_right.x).normalize_or_zero();
            let center = Vec3::new(hinge_x, leaf_h * 0.5, 0.0) + leaf_right * (leaf_w * 0.5);
            roof.push_oriented_box(
                center,
                leaf_right,
                Vec3::Y,
                leaf_out,
                Vec3::new(leaf_w * 0.5, leaf_h * 0.5, 0.035),
                seed.wrapping_add(if side < 0.0 { 101 } else { 109 }),
                0.2,
            );
        }
    }
}

/// Build a standalone Glade window (`GladeKind::Window`, `glade_window`) with
/// front & back glass panes, dressed stone jambs and sill, and warm timber lintel
/// & cross-mullions.
fn build_window_mesh(
    piece: &Piece,
    shape: &Shape,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    glass: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    let ext = piece_extents(piece, shape);
    let clear_w = (ext.x - 0.14).max(0.28);
    let height = ext.y.max(0.45);
    let depth = ext.z.max(0.38);
    let jamb_w = 0.065;
    let half_w = clear_w * 0.5;
    let half_d = depth * 0.5;
    let pane_z = 0.012;

    // Front and back glass panes recessed cleanly inside the cut-out wall opening.
    for (z_off, norm) in [(pane_z, Vec3::Z), (-pane_z, Vec3::NEG_Z)] {
        let g0 = Vec3::new(-half_w, 0.0, z_off);
        let g1 = Vec3::new(half_w, 0.0, z_off);
        let g2 = Vec3::new(half_w, height, z_off);
        let g3 = Vec3::new(-half_w, height, z_off);
        if norm.z > 0.0 {
            glass.push_quad(g0, g1, g2, g3, norm);
        } else {
            glass.push_quad(g1, g0, g3, g2, norm);
        }
    }

    // Left and right stone jambs spanning the wall thickness.
    for side in [-1.0_f32, 1.0_f32] {
        let jx = side * (half_w + jamb_w * 0.5);
        stone.push_oriented_box(
            Vec3::new(jx, height * 0.5, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(jamb_w * 0.5, height * 0.5, half_d * 1.04),
            seed.wrapping_add(if side < 0.0 { 11 } else { 23 }),
            0.35,
        );
    }

    // Projecting stone sill below the window.
    stone.push_oriented_box(
        Vec3::new(0.0, -0.025, 0.0),
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(half_w + jamb_w + 0.045, 0.04, half_d * 1.18),
        seed.wrapping_add(37),
        0.35,
    );

    // Timber lintel beam above the window (`roof` material slot for warm wood tint).
    roof.push_oriented_box(
        Vec3::new(0.0, height + 0.04, 0.0),
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(half_w + jamb_w + 0.055, 0.045, half_d * 1.12),
        seed.wrapping_add(53),
        0.25,
    );

    // Timber cross-mullions on both faces when details are enabled.
    if details {
        let mullion_half = 0.022;
        let mullion_d = half_d * 1.02;
        // Vertical center mullion
        roof.push_oriented_box(
            Vec3::new(0.0, height * 0.5, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(mullion_half, height * 0.5, mullion_d),
            seed.wrapping_add(71),
            0.15,
        );
        // Horizontal transom bar
        roof.push_oriented_box(
            Vec3::new(0.0, height * 0.55, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(half_w, mullion_half, mullion_d),
            seed.wrapping_add(89),
            0.15,
        );
    }
}

fn add_horizontal_cap(buf: &mut MeshBuf, outline: &[Vec2], y: f32, normal: Vec3) {
    if outline.len() < 3 {
        return;
    }
    let center = outline.iter().copied().fold(Vec2::ZERO, |a, b| a + b) / outline.len() as f32;
    let c3 = Vec3::new(center.x, y, center.y);
    let flip = normal.y < 0.0;
    for i in 0..outline.len() {
        let p = outline[i];
        let q = outline[(i + 1) % outline.len()];
        let p3 = Vec3::new(p.x, y, p.y);
        let q3 = Vec3::new(q.x, y, q.y);
        if flip {
            buf.push_tri([c3, q3, p3], normal);
        } else {
            buf.push_tri([c3, p3, q3], normal);
        }
    }
}

/// Collect local-space 2D points, widths, and crown heights where doors or paths touch the tower perimeter.
pub fn collect_tower_door_cutters(
    piece: &Piece,
    exposed: &[ExposedWallSegment],
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> Vec<(Vec2, f32, f32)> {
    let mut cutters: Vec<(Vec2, f32, f32)> = Vec::new();
    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if !is_door_cutter_shape(other_shape) {
            continue;
        }
        if (other.elevation - piece.elevation).abs() > 1.15 {
            continue;
        }
        let local_pt = world_to_local_2d(piece, other.pos);
        let (w, h) = if other_shape.id == "glade_door" {
            let dext = piece_extents(other, other_shape);
            (dext.x, dext.y)
        } else {
            (1.05, 1.85)
        };
        cutters.push((local_pt, w, h));
    }

    for line in path_polylines {
        for pair in line.windows(2) {
            for seg in exposed.iter() {
                if seg.base_y > 0.25 {
                    continue;
                }
                let w0 = local_to_world_2d(piece, seg.p0);
                let w1 = local_to_world_2d(piece, seg.p1);
                if let Some((t, _)) = segment_intersection_2d(w0, w1, pair[0], pair[1]) {
                    let hit_local = seg.p0.lerp(seg.p1, t);
                    if cutters
                        .iter()
                        .all(|(c, _, _)| c.distance(hit_local) > 0.85)
                    {
                        cutters.push((hit_local, 1.05, 1.85));
                    }
                }
            }
        }
    }
    cutters
}

/// Carve doorway openings into exposed tower wall segments by raising `base_y` to the arch crown height.
pub fn compute_tower_door_cutouts(
    piece: &Piece,
    exposed: &mut [ExposedWallSegment],
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> Vec<(Vec2, f32, f32)> {
    let cutters = collect_tower_door_cutters(piece, exposed, all_pieces, path_polylines);
    for &(cutter_local, width, crown_h) in &cutters {
        let mut best_seg: Option<(usize, f32, Vec2)> = None;
        for (idx, seg) in exposed.iter().enumerate() {
            if seg.base_y > 0.25 || seg.height() < 1.0 {
                continue;
            }
            let mid = (seg.p0 + seg.p1) * 0.5;
            let d = mid.distance(cutter_local);
            if d < 0.95 && best_seg.is_none_or(|(_, bd, _)| d < bd) {
                best_seg = Some((idx, d, mid));
            }
        }
        let Some((center_idx, _, center_mid)) = best_seg else {
            continue;
        };
        let top_y = exposed[center_idx].top_y;
        let normal_2d = if cutter_local.length_squared() > 1e-4 {
            cutter_local.normalize()
        } else {
            exposed[center_idx].normal
        };
        let tangent_2d = Vec2::new(-normal_2d.y, normal_2d.x);
        let center_ring = if cutter_local.length_squared() > 1e-4 {
            let r = center_mid.length();
            normal_2d * r
        } else {
            center_mid
        };
        let actual_w = width.clamp(0.65, 1.45);
        let actual_crown = crown_h.min(top_y - 0.18).max(0.95);
        let half_w = actual_w * 0.5;

        // Raise `base_y` on tower facets that fall inside the doorway span so the
        // lower wall is carved open!
        for seg in exposed.iter_mut() {
            if seg.base_y > 0.25 {
                continue;
            }
            let mid = (seg.p0 + seg.p1) * 0.5;
            let along = (mid - center_ring).dot(tangent_2d).abs();
            let radial_dot = seg.normal.dot(normal_2d);
            if radial_dot > 0.55 && along < half_w + 0.12 {
                seg.base_y = actual_crown.min(seg.top_y - 0.12);
                seg.seam_start = true;
                seg.seam_end = true;
            }
        }
    }
    cutters
}

/// Carve vaulted archway cutouts into a cylindrical `GladeKind::Tower`'s
/// 24-gon wall segments wherever a door (`glade_door`, `door`, `gate`) or path crosses.
fn apply_tower_door_cutouts(
    piece: &Piece,
    exposed: &mut [ExposedWallSegment],
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    stone: &mut MeshBuf,
    details: bool,
    seed: u32,
) {
    let cutters = compute_tower_door_cutouts(piece, exposed, all_pieces, path_polylines);
    for (c_idx, (cutter_local, width, crown_h)) in cutters.into_iter().enumerate() {
        let mut best_seg: Option<(usize, f32, Vec2)> = None;
        for (idx, seg) in exposed.iter().enumerate() {
            let mid = (seg.p0 + seg.p1) * 0.5;
            let d = mid.distance(cutter_local);
            if d < 0.95 && best_seg.is_none_or(|(_, bd, _)| d < bd) {
                best_seg = Some((idx, d, mid));
            }
        }
        let Some((center_idx, _, center_mid)) = best_seg else {
            continue;
        };
        let top_y = exposed[center_idx].top_y;
        let normal_2d = if cutter_local.length_squared() > 1e-4 {
            cutter_local.normalize()
        } else {
            exposed[center_idx].normal
        };
        let tangent_2d = Vec2::new(-normal_2d.y, normal_2d.x);
        let center_ring = if cutter_local.length_squared() > 1e-4 {
            let r = center_mid.length();
            normal_2d * r
        } else {
            center_mid
        };
        let right = Vec3::new(tangent_2d.x, 0.0, tangent_2d.y);
        let outward = Vec3::new(normal_2d.x, 0.0, normal_2d.y);
        let actual_w = width.clamp(0.65, 1.45);
        let actual_crown = crown_h.min(top_y - 0.18).max(0.95);
        let half_w = actual_w * 0.5;

        // Emit the vaulted archway tunnel & voussoir stones tangent to the tower face.
        let front_origin =
            Vec3::new(center_ring.x, 0.0, center_ring.y) - right * half_w + outward * 0.03;
        emit_archway_span(
            stone,
            front_origin,
            right,
            outward,
            0.0,
            actual_crown,
            0.0,
            actual_w,
            actual_crown,
            0.30,
            true,
            details,
            seed.wrapping_add(c_idx as u32 * 83),
        );
    }
}

/// Extend a battered stone foundation plinth below `y = 0.0` whenever an
/// exposed wall segment sits on sloped terrain or elevated above a
/// `glade_terrain` chunk, so hillside buildings never float in mid-air.
#[allow(clippy::too_many_arguments)]
fn emit_terrain_foundation_skirt(
    piece: &Piece,
    seg: &ExposedWallSegment,
    right: Vec3,
    outward: Vec3,
    wall_depth: f32,
    all_pieces: &[Piece],
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
) {
    if seg.base_y > 0.05 {
        return;
    }
    let w0 = local_to_world_2d(piece, seg.p0);
    let w1 = local_to_world_2d(piece, seg.p1);
    let w_mid = (w0 + w1) * 0.5;
    let h0 = terrain_height_at(w0, all_pieces).max(world_terrain_height_at(w0, all_pieces));
    let h1 = terrain_height_at(w1, all_pieces).max(world_terrain_height_at(w1, all_pieces));
    let hm = terrain_height_at(w_mid, all_pieces).max(world_terrain_height_at(w_mid, all_pieces));
    let min_ground = h0.min(h1).min(hm);
    let max_ground = h0.max(h1).max(hm);

    // Extend skirt if the piece sits above the lowest ground point under this wall
    // (within a 4.5m foundation reach) or if terrain slopes across the wall.
    let drop = (piece.elevation - min_ground).clamp(0.0, 4.5);
    let rise_into_slope = (max_ground - piece.elevation).max(0.0);
    if drop < 0.06 && rise_into_slope < 0.08 {
        return;
    }

    let skirt_bottom_y = -drop.max(0.18);
    let flare = 0.08;
    let b0 = Vec3::new(seg.p0.x, skirt_bottom_y, seg.p0.y) + outward * flare;
    let b1 = Vec3::new(seg.p1.x, skirt_bottom_y, seg.p1.y) + outward * flare;
    let t0 = Vec3::new(seg.p0.x, 0.02, seg.p0.y) + outward * 0.02;
    let t1 = Vec3::new(seg.p1.x, 0.02, seg.p1.y) + outward * 0.02;
    let slope_n = (outward * drop.max(0.2) + Vec3::Y * flare).normalize_or_zero();
    stone.push_quad(b0, b1, t1, t0, slope_n);

    // Interior face of the foundation skirt
    let in_off = -outward * wall_depth;
    stone.push_quad(
        Vec3::new(seg.p1.x, skirt_bottom_y, seg.p1.y) + in_off,
        Vec3::new(seg.p0.x, skirt_bottom_y, seg.p0.y) + in_off,
        Vec3::new(seg.p0.x, 0.02, seg.p0.y) + in_off,
        Vec3::new(seg.p1.x, 0.02, seg.p1.y) + in_off,
        -outward,
    );

    if details {
        let len = seg.length();
        let count = (len / 0.55).floor().max(1.0) as usize;
        let step = len / count as f32;
        let start = Vec3::new(seg.p0.x, skirt_bottom_y * 0.5, seg.p0.y);
        for i in 0..count {
            let u = (i as f32 + 0.5) * step;
            stone.push_oriented_box(
                start + right * u + outward * 0.04,
                right,
                Vec3::Y,
                outward,
                Vec3::new(step * 0.46, (skirt_bottom_y.abs() * 0.45).max(0.08), 0.065),
                seed.wrapping_add(i as u32 * 47),
                0.45,
            );
        }
    }
}

/// Carve rectangular window openings out of exposed 24-gon tower wall segments
/// wherever a `glade_window` piece is placed on the tower.
fn apply_tower_window_cutouts(
    piece: &Piece,
    exposed: &mut Vec<ExposedWallSegment>,
    all_pieces: &[Piece],
) {
    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if other_shape.id != "glade_window" {
            continue;
        }
        let wext = piece_extents(other, other_shape);
        let clear_hw = ((wext.x - 0.14).max(0.28)) * 0.5;
        let win_local = world_to_local_2d(piece, other.pos);
        let win_bottom = other.elevation - piece.elevation;
        let win_top = win_bottom + wext.y;
        let out_local = win_local.normalize_or_zero();

        let mut extra: Vec<ExposedWallSegment> = Vec::new();
        for seg in exposed.iter_mut() {
            if seg.height() < 0.25 {
                continue;
            }
            if out_local != Vec2::ZERO && seg.normal.dot(out_local) < 0.3 {
                continue;
            }
            let mid = (seg.p0 + seg.p1) * 0.5;
            if mid.distance(win_local) > clear_hw + 0.16 {
                continue;
            }
            let b = win_bottom.max(seg.base_y);
            let t = win_top.min(seg.top_y);
            if t <= b + 0.12 {
                continue;
            }
            if seg.top_y > t + 0.05 && seg.base_y < b - 0.05 {
                let mut upper = seg.clone();
                upper.base_y = t;
                extra.push(upper);
                seg.top_y = b;
            } else if seg.base_y < b - 0.05 {
                seg.top_y = b;
            } else if seg.top_y > t + 0.05 {
                seg.base_y = t;
            } else {
                seg.top_y = seg.base_y;
            }
        }
        exposed.extend(extra);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_building_walls(
    piece: &Piece,
    kind: GladeKind,
    exposed: &[ExposedWallSegment],
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    tier: WallTier,
    is_tent: bool,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    _glass: &mut MeshBuf,
) {
    let mut tower_exposed: Vec<ExposedWallSegment>;
    let segments: &[ExposedWallSegment] = if kind == GladeKind::Tower {
        tower_exposed = exposed.to_vec();
        apply_tower_door_cutouts(
            piece,
            &mut tower_exposed,
            all_pieces,
            path_polylines,
            stone,
            details,
            seed,
        );
        apply_tower_window_cutouts(piece, &mut tower_exposed, all_pieces);
        &tower_exposed
    } else {
        exposed
    };

    let wall_depth = 0.26;
    for (seg_idx, seg) in segments.iter().enumerate() {
        let seg_len = seg.length();
        if seg_len < 0.04 || seg.height() < 0.02 {
            continue;
        }
        let tangent_2d = (seg.p1 - seg.p0) / seg_len;
        let right = Vec3::new(tangent_2d.x, 0.0, tangent_2d.y);
        let outward = Vec3::new(seg.normal.x, 0.0, seg.normal.y);

        let openings = if kind == GladeKind::Room {
            compute_arch_openings(piece, seg, all_pieces, path_polylines)
        } else {
            Vec::new()
        };
        let window_openings = if kind == GladeKind::Room {
            compute_window_openings(piece, seg, all_pieces)
        } else {
            Vec::new()
        };

        // Extrude the wall face with archway & window cutouts.
        emit_wall_face_with_arches(
            stone,
            seg,
            right,
            outward,
            wall_depth,
            &openings,
            &window_openings,
            details,
            seed.wrapping_add(seg_idx as u32 * 31),
        );

        // Hillside stone foundation skirt when perched on or above terrain.
        emit_terrain_foundation_skirt(
            piece,
            seg,
            right,
            outward,
            wall_depth,
            all_pieces,
            details,
            seed.wrapping_add(seg_idx as u32 * 61),
            stone,
        );

        // Fortified stone foundation batter on tall walls/towers.
        if tier == WallTier::FortifiedWall && seg.base_y <= 0.05 {
            let batter_h = 0.55_f32.min(seg.height() * 0.3);
            let flare = 0.10;
            let b0 = Vec3::new(seg.p0.x, seg.base_y, seg.p0.y) + outward * flare;
            let b1 = Vec3::new(seg.p1.x, seg.base_y, seg.p1.y) + outward * flare;
            let t0 = Vec3::new(seg.p0.x, seg.base_y + batter_h, seg.p0.y);
            let t1 = Vec3::new(seg.p1.x, seg.base_y + batter_h, seg.p1.y);
            let slope_n = (outward * batter_h + Vec3::Y * flare).normalize_or_zero();
            if openings.is_empty() {
                stone.push_quad(b0, b1, t1, t0, slope_n);
            }
        }

        // Low fence coping stones when lowered below FENCE_MAX_HEIGHT (but not
        // when the building is a roofed tent-like house whose eaves cover the wall!).
        if tier == WallTier::LowFence && !is_tent && details {
            emit_coping_stones(
                stone,
                seg.p0,
                seg.p1,
                seg.top_y,
                right,
                outward,
                0.22,
                seed.wrapping_add(seg_idx as u32 * 97),
            );
        }

        // Corner quoins & boolean intersection seam pilasters (automatically
        // suppressed on tent-like houses so brackets/quoins never poke through low eaves).
        if details && !is_tent && seg.height() > 0.7 {
            if seg.corner_start || seg.seam_start {
                emit_quoin_column(
                    stone,
                    Vec3::new(seg.p0.x, seg.base_y, seg.p0.y),
                    seg.height(),
                    right,
                    outward,
                    seg.seam_start,
                    seed.wrapping_add(seg_idx as u32 * 131),
                );
            }
            if seg.corner_end || seg.seam_end {
                emit_quoin_column(
                    stone,
                    Vec3::new(seg.p1.x, seg.base_y, seg.p1.y),
                    seg.height(),
                    -right,
                    outward,
                    seg.seam_end,
                    seed.wrapping_add(seg_idx as u32 * 193),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_free_wall_mesh(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    wall_spine: Option<&[Vec2]>,
    tier: WallTier,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    let ext = piece_extents(piece, shape);
    let half_len = ext.x * 0.5;
    let height = ext.y;
    let thick = ext.z;

    // Build local-space spine points: either the piece's curved multi-point path
    // or a single straight span from `-half_len` to `+half_len`.
    let local_points: Vec<Vec2> = if let Some(world_pts) = wall_spine.filter(|pts| pts.len() >= 2) {
        world_pts
            .iter()
            .map(|&wp| world_to_local_2d(piece, wp))
            .collect()
    } else {
        vec![Vec2::new(-half_len, 0.0), Vec2::new(half_len, 0.0)]
    };

    let num_segs = local_points.len().saturating_sub(1);
    for (seg_idx, pair) in local_points.windows(2).enumerate() {
        let p0 = pair[0];
        let p1 = pair[1];
        let seg_vec = p1 - p0;
        let seg_len = seg_vec.length();
        if seg_len < 0.05 {
            continue;
        }
        let tangent_2d = seg_vec / seg_len;
        let normal_2d = Vec2::new(-tangent_2d.y, tangent_2d.x);
        let right = Vec3::new(tangent_2d.x, 0.0, tangent_2d.y);
        let outward = Vec3::new(normal_2d.x, 0.0, normal_2d.y);

        let seg = ExposedWallSegment {
            p0,
            p1,
            normal: normal_2d,
            base_y: 0.0,
            top_y: height,
            corner_start: seg_idx == 0,
            corner_end: seg_idx + 1 == num_segs,
            seam_start: seg_idx > 0,
            seam_end: seg_idx + 1 < num_segs,
        };

        let seg_seed = seed.wrapping_add(seg_idx as u32 * 137);
        let openings = compute_arch_openings(piece, &seg, all_pieces, path_polylines);
        let window_openings = compute_window_openings(piece, &seg, all_pieces);

        // Front, back, and end caps of the thick wall segment with arch & window cutouts.
        emit_thick_wall_with_arches(
            stone,
            &seg,
            right,
            outward,
            thick,
            &openings,
            &window_openings,
            details,
            seg_seed,
        );

        // Fill a cylindrical stone joint post at interior curve bends so curved
        // walls look seamlessly connected at every vertex.
        if seg_idx > 0 {
            stone.push_oriented_box(
                Vec3::new(p0.x, height * 0.5, p0.y),
                right,
                Vec3::Y,
                outward,
                Vec3::new(thick * 0.48, height * 0.5, thick * 0.48),
                seg_seed.wrapping_add(5),
                0.25,
            );
        }

        match tier {
            WallTier::LowFence => {
                if details {
                    emit_coping_stones(
                        stone,
                        seg.p0,
                        seg.p1,
                        height,
                        right,
                        outward,
                        thick * 1.15,
                        seg_seed,
                    );
                }
            }
            WallTier::StandardWall => {
                if details {
                    emit_coping_stones(
                        roof,
                        seg.p0,
                        seg.p1,
                        height,
                        right,
                        outward,
                        thick * 1.18,
                        seg_seed.wrapping_add(77),
                    );
                    if seg.corner_start {
                        emit_quoin_column(
                            stone,
                            Vec3::new(p0.x, 0.0, p0.y),
                            height,
                            right,
                            outward,
                            false,
                            seg_seed.wrapping_add(11),
                        );
                    }
                    if seg.corner_end {
                        emit_quoin_column(
                            stone,
                            Vec3::new(p1.x, 0.0, p1.y),
                            height,
                            -right,
                            outward,
                            false,
                            seg_seed.wrapping_add(23),
                        );
                    }
                }
            }
            WallTier::FortifiedWall => {
                let merlon_len = 0.44;
                let gap = 0.32;
                let step = merlon_len + gap;
                let count = (seg_len / step).floor().max(1.0) as usize;
                let actual_step = seg_len / count as f32;
                let start_3d = Vec3::new(p0.x, 0.0, p0.y);
                for i in 0..count {
                    let u = (i as f32 + 0.5) * actual_step;
                    let base_m = start_3d + right * u;
                    stone.push_oriented_box(
                        base_m + Vec3::Y * (height + 0.18),
                        right,
                        Vec3::Y,
                        outward,
                        Vec3::new(merlon_len * 0.5, 0.18, thick * 0.54),
                        seg_seed.wrapping_add(i as u32 * 41),
                        0.4,
                    );
                    if details {
                        roof.push_oriented_box(
                            base_m + Vec3::Y * (height + 0.38),
                            right,
                            Vec3::Y,
                            outward,
                            Vec3::new(merlon_len * 0.55, 0.035, thick * 0.60),
                            seg_seed.wrapping_add(i as u32 * 53),
                            0.3,
                        );
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_wall_face_with_arches(
    stone: &mut MeshBuf,
    seg: &ExposedWallSegment,
    right: Vec3,
    outward: Vec3,
    wall_depth: f32,
    openings: &[ArchOpening],
    windows: &[WindowOpening],
    details: bool,
    seed: u32,
) {
    let seg_len = seg.length();
    let origin = Vec3::new(seg.p0.x, 0.0, seg.p0.y);
    let in_offset = -outward * wall_depth;

    if openings.is_empty() && windows.is_empty() {
        // Subdivide into horizontal courses so organic jitter gives a subtle
        // hand-laid masonry curve.
        let cols = (seg_len / 0.85).ceil().max(1.0) as usize;
        let rows = (seg.height() / 0.85).ceil().max(1.0) as usize;
        for r in 0..rows {
            let y0 = seg.base_y + (r as f32 / rows as f32) * seg.height();
            let y1 = seg.base_y + ((r + 1) as f32 / rows as f32) * seg.height();
            for c in 0..cols {
                let u0 = (c as f32 / cols as f32) * seg_len;
                let u1 = ((c + 1) as f32 / cols as f32) * seg_len;
                let mut v00 = origin + right * u0 + Vec3::Y * y0;
                let mut v10 = origin + right * u1 + Vec3::Y * y0;
                let mut v11 = origin + right * u1 + Vec3::Y * y1;
                let mut v01 = origin + right * u0 + Vec3::Y * y1;
                if details {
                    // Only jitter interior grid points so corners stay watertight
                    if c > 0 && r > 0 {
                        v00 += outward * organic_jitter(v00, seed).x * 0.4;
                    }
                    if c + 1 < cols && r > 0 {
                        v10 += outward * organic_jitter(v10, seed).x * 0.4;
                    }
                    if c + 1 < cols && r + 1 < rows {
                        v11 += outward * organic_jitter(v11, seed).x * 0.4;
                    }
                    if c > 0 && r + 1 < rows {
                        v01 += outward * organic_jitter(v01, seed).x * 0.4;
                    }
                }
                // Exterior face
                stone.push_quad(v00, v10, v11, v01, outward);

                // Interior face facing into the room (-outward)
                let iv00 = v00 + in_offset;
                let iv10 = v10 + in_offset;
                let iv11 = v11 + in_offset;
                let iv01 = v01 + in_offset;
                stone.push_quad(iv10, iv00, iv01, iv11, -outward);
            }
        }
        return;
    }

    emit_wall_openings_spans(
        stone,
        origin,
        right,
        outward,
        seg.base_y,
        seg.top_y,
        seg_len,
        wall_depth,
        openings,
        windows,
        details,
        seed,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_thick_wall_with_arches(
    stone: &mut MeshBuf,
    seg: &ExposedWallSegment,
    right: Vec3,
    outward: Vec3,
    thick: f32,
    openings: &[ArchOpening],
    windows: &[WindowOpening],
    details: bool,
    seed: u32,
) {
    let seg_len = seg.length();
    let half_t = thick * 0.5;
    let origin_center = Vec3::new(seg.p0.x, 0.0, seg.p0.y);
    let front_origin = origin_center + outward * half_t;
    let back_origin = origin_center - outward * half_t;

    // Left & right end caps and top/bottom caps
    let l_f_b = front_origin + Vec3::Y * seg.base_y;
    let l_b_b = back_origin + Vec3::Y * seg.base_y;
    let l_b_t = back_origin + Vec3::Y * seg.top_y;
    let l_f_t = front_origin + Vec3::Y * seg.top_y;
    stone.push_quad(l_b_b, l_f_b, l_f_t, l_b_t, -right);

    let r_f_b = front_origin + right * seg_len + Vec3::Y * seg.base_y;
    let r_b_b = back_origin + right * seg_len + Vec3::Y * seg.base_y;
    let r_b_t = back_origin + right * seg_len + Vec3::Y * seg.top_y;
    let r_f_t = front_origin + right * seg_len + Vec3::Y * seg.top_y;
    stone.push_quad(r_f_b, r_b_b, r_b_t, r_f_t, right);

    // Top cap
    stone.push_quad(l_f_t, r_f_t, r_b_t, l_b_t, Vec3::Y);
    // Bottom cap
    stone.push_quad(l_b_b, r_b_b, r_f_b, l_f_b, Vec3::NEG_Y);

    emit_wall_openings_spans(
        stone,
        front_origin,
        right,
        outward,
        seg.base_y,
        seg.top_y,
        seg_len,
        thick,
        openings,
        windows,
        details,
        seed,
    );
}

enum OpeningSpan<'a> {
    Arch {
        u_start: f32,
        u_end: f32,
        arch: &'a ArchOpening,
        idx: usize,
    },
    WindowCol {
        u_start: f32,
        u_end: f32,
        wins: Vec<&'a WindowOpening>,
    },
}

/// Shared horizontal-span splitter that carves both [`ArchOpening`] doorways
/// and rectangular [`WindowOpening`] cutouts through a double-sided wall.
#[allow(clippy::too_many_arguments)]
fn emit_wall_openings_spans(
    stone: &mut MeshBuf,
    front_origin: Vec3,
    right: Vec3,
    outward: Vec3,
    base_y: f32,
    top_y: f32,
    seg_len: f32,
    depth: f32,
    openings: &[ArchOpening],
    windows: &[WindowOpening],
    details: bool,
    seed: u32,
) {
    let back_origin = front_origin - outward * depth;
    let emit_solid_panel = |stone: &mut MeshBuf, u0: f32, u1: f32, y0: f32, y1: f32| {
        if u1 <= u0 + 0.015 || y1 <= y0 + 0.015 {
            return;
        }
        let fa = front_origin + right * u0;
        let fb = front_origin + right * u1;
        stone.push_quad(
            fa + Vec3::Y * y0,
            fb + Vec3::Y * y0,
            fb + Vec3::Y * y1,
            fa + Vec3::Y * y1,
            outward,
        );
        let ba = back_origin + right * u0;
        let bb = back_origin + right * u1;
        stone.push_quad(
            bb + Vec3::Y * y0,
            ba + Vec3::Y * y0,
            ba + Vec3::Y * y1,
            bb + Vec3::Y * y1,
            -outward,
        );
    };

    let mut spans: Vec<OpeningSpan<'_>> = Vec::new();
    for (idx, arch) in openings.iter().enumerate() {
        let u0 = (arch.center_dist - arch.width * 0.5).clamp(0.0, seg_len);
        let u1 = (arch.center_dist + arch.width * 0.5).clamp(0.0, seg_len);
        if u1 > u0 + 0.05 {
            spans.push(OpeningSpan::Arch {
                u_start: u0,
                u_end: u1,
                arch,
                idx,
            });
        }
    }
    for win in windows {
        let u0 = (win.center_dist - win.width * 0.5).clamp(0.0, seg_len);
        let u1 = (win.center_dist + win.width * 0.5).clamp(0.0, seg_len);
        if u1 <= u0 + 0.05 {
            continue;
        }
        let overlaps_arch = spans.iter().any(|s| {
            matches!(s, OpeningSpan::Arch { u_start, u_end, .. } if u0 < *u_end - 0.02 && u1 > *u_start + 0.02)
        });
        if overlaps_arch {
            continue;
        }
        if let Some(OpeningSpan::WindowCol {
            u_start,
            u_end,
            wins,
        }) = spans.iter_mut().find(|s| {
            matches!(s, OpeningSpan::WindowCol { u_start, u_end, .. } if (u0 - *u_start).abs() < 0.15 && (u1 - *u_end).abs() < 0.15)
        }) {
            *u_start = u_start.min(u0);
            *u_end = u_end.max(u1);
            wins.push(win);
        } else {
            spans.push(OpeningSpan::WindowCol {
                u_start: u0,
                u_end: u1,
                wins: vec![win],
            });
        }
    }

    spans.sort_by(|a, b| {
        let ua = match a {
            OpeningSpan::Arch { u_start, .. } | OpeningSpan::WindowCol { u_start, .. } => *u_start,
        };
        let ub = match b {
            OpeningSpan::Arch { u_start, .. } | OpeningSpan::WindowCol { u_start, .. } => *u_start,
        };
        ua.partial_cmp(&ub).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut cursor_u = 0.0_f32;
    for span in spans {
        match span {
            OpeningSpan::Arch {
                u_start,
                u_end,
                arch,
                idx,
            } => {
                let s0 = u_start.max(cursor_u);
                let s1 = u_end.min(seg_len);
                if s1 <= s0 + 0.05 {
                    continue;
                }
                emit_solid_panel(stone, cursor_u, s0, base_y, top_y);
                emit_archway_span(
                    stone,
                    front_origin,
                    right,
                    outward,
                    base_y,
                    top_y,
                    s0,
                    s1,
                    arch.crown_height,
                    depth,
                    true,
                    details,
                    seed.wrapping_add(idx as u32 * 67),
                );
                cursor_u = s1;
            }
            OpeningSpan::WindowCol {
                u_start,
                u_end,
                mut wins,
            } => {
                let s0 = u_start.max(cursor_u);
                let s1 = u_end.min(seg_len);
                if s1 <= s0 + 0.05 {
                    continue;
                }
                emit_solid_panel(stone, cursor_u, s0, base_y, top_y);
                wins.sort_by(|a, b| {
                    a.bottom_y
                        .partial_cmp(&b.bottom_y)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let mut y_cursor = base_y;
                for w in wins {
                    let wb = w.bottom_y.max(y_cursor);
                    let wt = w.top_y.min(top_y);
                    if wt <= wb + 0.05 {
                        continue;
                    }
                    emit_solid_panel(stone, s0, s1, y_cursor, wb);

                    // Inner window reveal faces (left jamb, right jamb, sill, soffit).
                    let lf_b = front_origin + right * s0 + Vec3::Y * wb;
                    let lb_b = back_origin + right * s0 + Vec3::Y * wb;
                    let lf_t = front_origin + right * s0 + Vec3::Y * wt;
                    let lb_t = back_origin + right * s0 + Vec3::Y * wt;
                    stone.push_quad(lf_b, lb_b, lb_t, lf_t, right);

                    let rf_b = front_origin + right * s1 + Vec3::Y * wb;
                    let rb_b = back_origin + right * s1 + Vec3::Y * wb;
                    let rf_t = front_origin + right * s1 + Vec3::Y * wt;
                    let rb_t = back_origin + right * s1 + Vec3::Y * wt;
                    stone.push_quad(rb_b, rf_b, rf_t, rb_t, -right);

                    // Sill (facing +Y) & Soffit (facing -Y)
                    stone.push_quad(lf_b, rf_b, rb_b, lb_b, Vec3::Y);
                    stone.push_quad(lb_t, rb_t, rf_t, lf_t, Vec3::NEG_Y);

                    y_cursor = wt;
                }
                emit_solid_panel(stone, s0, s1, y_cursor, top_y);
                cursor_u = s1;
            }
        }
    }
    emit_solid_panel(stone, cursor_u, seg_len, base_y, top_y);
}

/// Emit the spandrel wall above a semicircular archway, the inner vaulted
/// arch tunnel soffit, and the radial voussoir & keystone blocks.
#[allow(clippy::too_many_arguments)]
fn emit_archway_span(
    stone: &mut MeshBuf,
    front_origin: Vec3,
    right: Vec3,
    outward: Vec3,
    base_y: f32,
    top_y: f32,
    u_start: f32,
    u_end: f32,
    crown_h: f32,
    depth: f32,
    both_faces: bool,
    details: bool,
    seed: u32,
) {
    let width = (u_end - u_start).max(0.2);
    let radius = width * 0.5;
    let spring_y = (base_y + crown_h - radius).max(base_y + 0.25);
    let u_mid = (u_start + u_end) * 0.5;
    let steps = 8;

    let back_offset = -outward * depth;

    // Inner vertical jamb walls (left and right sides of the portal opening).
    let left_f_b = front_origin + right * u_start + Vec3::Y * base_y;
    let left_b_b = left_f_b + back_offset;
    let left_f_s = front_origin + right * u_start + Vec3::Y * spring_y;
    let left_b_s = left_f_s + back_offset;
    stone.push_quad(left_f_b, left_b_b, left_b_s, left_f_s, right);

    let right_f_b = front_origin + right * u_end + Vec3::Y * base_y;
    let right_b_b = right_f_b + back_offset;
    let right_f_s = front_origin + right * u_end + Vec3::Y * spring_y;
    let right_b_s = right_f_s + back_offset;
    stone.push_quad(right_b_b, right_f_b, right_f_s, right_b_s, -right);

    for i in 0..steps {
        let a0 = std::f32::consts::PI * (1.0 - i as f32 / steps as f32);
        let a1 = std::f32::consts::PI * (1.0 - (i + 1) as f32 / steps as f32);

        let u0 = u_mid + a0.cos() * radius;
        let y0 = spring_y + a0.sin() * radius;
        let u1 = u_mid + a1.cos() * radius;
        let y1 = spring_y + a1.sin() * radius;

        // Front spandrel quad above the arch curve up to top_y
        let f_a0 = front_origin + right * u0 + Vec3::Y * y0;
        let f_a1 = front_origin + right * u1 + Vec3::Y * y1;
        let f_t0 = front_origin + right * u0 + Vec3::Y * top_y;
        let f_t1 = front_origin + right * u1 + Vec3::Y * top_y;
        stone.push_quad(f_a0, f_a1, f_t1, f_t0, outward);

        if both_faces {
            let b_a0 = f_a0 + back_offset;
            let b_a1 = f_a1 + back_offset;
            let b_t0 = f_t0 + back_offset;
            let b_t1 = f_t1 + back_offset;
            stone.push_quad(b_a1, b_a0, b_t0, b_t1, -outward);
        }

        // Vaulted inner arch soffit (ceiling of the archway tunnel)
        let b_a0 = f_a0 + back_offset;
        let b_a1 = f_a1 + back_offset;
        let mid_angle = (a0 + a1) * 0.5;
        let inward_normal = (-right * mid_angle.cos() - Vec3::Y * mid_angle.sin()).normalize_or_zero();
        stone.push_quad(f_a1, f_a0, b_a0, b_a1, inward_normal);

        // Decorative voussoir arch stones & prominent central keystone
        if details {
            let radial = (right * mid_angle.cos() + Vec3::Y * mid_angle.sin()).normalize_or_zero();
            let tangent = (right * mid_angle.sin() - Vec3::Y * mid_angle.cos()).normalize_or_zero();
            let is_keystone = i == steps / 2 || i + 1 == steps / 2;
            let stone_h = if is_keystone { 0.11 } else { 0.08 };
            let stone_protrude = if is_keystone { 0.055 } else { 0.038 };
            let chord = ((u1 - u0).hypot(y1 - y0)) * 0.48;
            let center_arch = (f_a0 + f_a1) * 0.5 + radial * stone_h - outward * (depth * 0.5);
            stone.push_oriented_box(
                center_arch,
                tangent,
                radial,
                outward,
                Vec3::new(chord, stone_h, depth * 0.5 + stone_protrude),
                seed.wrapping_add(i as u32 * 19),
                0.35,
            );
        }
    }
}

/// Emit a vertical stack of alternating long-and-short corner quoins (or seam
/// junction stones where two buildings merge).
fn emit_quoin_column(
    stone: &mut MeshBuf,
    base_pos: Vec3,
    height: f32,
    inward_tangent: Vec3,
    outward: Vec3,
    is_seam: bool,
    seed: u32,
) {
    let block_h = 0.28;
    let count = (height / block_h).floor().max(1.0) as usize;
    let actual_h = height / count as f32;
    for i in 0..count {
        let long = i % 2 == 0;
        let len = if is_seam {
            0.18
        } else if long {
            0.26
        } else {
            0.16
        };
        let cy = (i as f32 + 0.5) * actual_h;
        let center = base_pos
            + Vec3::Y * cy
            + inward_tangent * (len * 0.42)
            + outward * 0.025;
        stone.push_oriented_box(
            center,
            inward_tangent,
            Vec3::Y,
            outward,
            Vec3::new(len * 0.5, actual_h * 0.46, 0.045),
            seed.wrapping_add(i as u32 * 13),
            0.45,
        );
    }
}

/// Emit irregular rounded coping stones along the top of a low fence or wall.
#[allow(clippy::too_many_arguments)]
fn emit_coping_stones(
    buf: &mut MeshBuf,
    p0: Vec2,
    p1: Vec2,
    top_y: f32,
    right: Vec3,
    outward: Vec3,
    width: f32,
    seed: u32,
) {
    let len = p0.distance(p1);
    let stone_len = 0.34;
    let count = (len / stone_len).floor().max(1.0) as usize;
    let step = len / count as f32;
    let start = Vec3::new(p0.x, top_y + 0.045, p0.y);
    for i in 0..count {
        let u = (i as f32 + 0.5) * step;
        let jitter_h = ((seed.wrapping_add(i as u32 * 37) % 11) as f32 - 5.0) * 0.003;
        buf.push_oriented_box(
            start + right * u + Vec3::Y * jitter_h,
            right,
            Vec3::Y,
            outward,
            Vec3::new(step * 0.46, 0.048, width * 0.52),
            seed.wrapping_add(i as u32 * 23),
            0.55,
        );
    }
}

// ---------------------------------------------------------------------------
// Procedural Roof Generators (Conical, Gable, Hipped, Battlements)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_conical_roof(
    rx: f32,
    rz: f32,
    base_y: f32,
    rise: f32,
    overhang: f32,
    details: bool,
    seed: u32,
    roof: &mut MeshBuf,
) {
    let segments = 24;
    let rings = 6;
    let r_x = rx + overhang;
    let r_z = rz + overhang;
    // When lowered into the tent-like house tier, drop the eaves all the way to ground level!
    let eaves_y = if base_y <= TENT_MAX_WALL_HEIGHT {
        0.0
    } else {
        base_y - 0.06
    };
    let apex_y = base_y + rise;

    // Flared witch-hat profile: radius tapers non-linearly so eaves flare out.
    let profile = |t: f32| -> (f32, f32, f32) {
        let flare = (1.0 - t).powf(0.80);
        (r_x * flare, r_z * flare, eaves_y + t * (apex_y - eaves_y))
    };

    for ring in 0..rings {
        let t0 = ring as f32 / rings as f32;
        let t1 = (ring + 1) as f32 / rings as f32;
        let (rx0, rz0, y0) = profile(t0);
        let (rx1, rz1, y1) = profile(t1);

        for i in 0..segments {
            let a0 = std::f32::consts::TAU * i as f32 / segments as f32;
            let a1 = std::f32::consts::TAU * (i + 1) as f32 / segments as f32;

            let p00 = Vec3::new(a0.cos() * rx0, y0, a0.sin() * rz0);
            let p10 = Vec3::new(a1.cos() * rx0, y0, a1.sin() * rz0);
            let p11 = Vec3::new(a1.cos() * rx1, y1, a1.sin() * rz1);
            let p01 = Vec3::new(a0.cos() * rx1, y1, a0.sin() * rz1);

            let mid_a = (a0 + a1) * 0.5;
            let normal = Vec3::new(
                mid_a.cos() * rise,
                (r_x + r_z) * 0.5,
                mid_a.sin() * rise,
            )
            .normalize_or_zero();

            if ring + 1 == rings {
                roof.push_tri([p00, p10, Vec3::new(0.0, y1, 0.0)], normal);
            } else {
                roof.push_quad(p00, p10, p11, p01, normal);
            }

            // Shingle ring lip on lower rings
            if details && ring < rings - 1 && i % 2 == ring % 2 {
                let shingle_mid = (p00 + p10) * 0.5 + normal * 0.025;
                let tangent = (p10 - p00).normalize_or_zero();
                let up_slope = (p01 - p00).normalize_or_zero();
                let chord = p00.distance(p10) * 0.48;
                roof.push_oriented_box(
                    shingle_mid,
                    tangent,
                    up_slope,
                    normal,
                    Vec3::new(chord, 0.09, 0.022),
                    seed.wrapping_add((ring * 31 + i) as u32),
                    0.35,
                );
            }
        }
    }

    // Bottom eaves soffit cap
    let soffit_outline: Vec<Vec2> = (0..segments)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / segments as f32;
            Vec2::new(a.cos() * r_x, a.sin() * r_z)
        })
        .collect();
    add_horizontal_cap(roof, &soffit_outline, eaves_y, Vec3::NEG_Y);
}

#[allow(clippy::too_many_arguments)]
fn build_gable_roof(
    width: f32,
    depth: f32,
    base_y: f32,
    rise: f32,
    overhang: f32,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    let hw = width * 0.5;
    let hd = depth * 0.5;
    let ow = hw + overhang;
    let od = hd + overhang;
    let is_tent = base_y <= TENT_MAX_WALL_HEIGHT;
    let eaves_y = if is_tent { 0.0 } else { base_y - 0.05 };
    let apex_y = base_y + rise;

    // Triangular masonry/timber gable end walls (at -X and +X)
    let gable_base_y = if is_tent { 0.0 } else { base_y };
    stone.push_tri(
        [
            Vec3::new(-hw, gable_base_y, -hd),
            Vec3::new(-hw, gable_base_y, hd),
            Vec3::new(-hw, apex_y, 0.0),
        ],
        Vec3::NEG_X,
    );
    stone.push_tri(
        [
            Vec3::new(-hw, gable_base_y, hd),
            Vec3::new(-hw, gable_base_y, -hd),
            Vec3::new(-hw, apex_y, 0.0),
        ],
        Vec3::X,
    );
    stone.push_tri(
        [
            Vec3::new(hw, gable_base_y, hd),
            Vec3::new(hw, gable_base_y, -hd),
            Vec3::new(hw, apex_y, 0.0),
        ],
        Vec3::X,
    );
    stone.push_tri(
        [
            Vec3::new(hw, gable_base_y, -hd),
            Vec3::new(hw, gable_base_y, hd),
            Vec3::new(hw, apex_y, 0.0),
        ],
        Vec3::NEG_X,
    );

    // Front (+Z) and Back (-Z) sloped roof planes
    let total_drop = (apex_y - eaves_y).max(0.15);
    let n_front = Vec3::new(0.0, od, total_drop).normalize_or_zero();
    let n_back = Vec3::new(0.0, od, -total_drop).normalize_or_zero();

    let f_bl = Vec3::new(-ow, eaves_y, od);
    let f_br = Vec3::new(ow, eaves_y, od);
    let r_r = Vec3::new(ow, apex_y, 0.0);
    let r_l = Vec3::new(-ow, apex_y, 0.0);
    roof.push_quad(f_bl, f_br, r_r, r_l, n_front);
    // Underside of front slope
    roof.push_quad(f_br, f_bl, r_l, r_r, -n_front);

    let b_br = Vec3::new(ow, eaves_y, -od);
    let b_bl = Vec3::new(-ow, eaves_y, -od);
    roof.push_quad(b_br, b_bl, r_l, r_r, n_back);
    roof.push_quad(b_bl, b_br, r_r, r_l, -n_back);

    if details {
        // Ridge beam crest along the top
        roof.push_oriented_box(
            Vec3::new(0.0, apex_y + 0.04, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(ow + 0.04, 0.055, 0.06),
            seed,
            0.35,
        );

        // When lowered into a tent-like house, add angled A-frame timber rafters
        // at both gable ends so the ground-reaching tent structure looks crisp and intentional.
        if is_tent {
            for gx in [-ow + 0.04, ow - 0.04] {
                for side_z in [-1.0_f32, 1.0_f32] {
                    let p_top = Vec3::new(gx, apex_y, 0.0);
                    let p_bot = Vec3::new(gx, eaves_y, side_z * od);
                    let rafter_vec = p_top - p_bot;
                    let rafter_len = rafter_vec.length();
                    if rafter_len > 0.1 {
                        let up_dir = rafter_vec / rafter_len;
                        let out_dir = Vec3::new(0.0, od, side_z * total_drop).normalize_or_zero();
                        roof.push_oriented_box(
                            (p_top + p_bot) * 0.5,
                            Vec3::X,
                            up_dir,
                            out_dir,
                            Vec3::new(0.045, rafter_len * 0.5, 0.045),
                            seed.wrapping_add(if gx < 0.0 { 307 } else { 313 }),
                            0.25,
                        );
                    }
                }
            }
        }

        // Tiered shingle courses on both slopes
        let courses = 4;
        for c in 0..courses {
            let t = (c as f32 + 0.35) / courses as f32;
            let y = eaves_y + t * (apex_y - eaves_y);
            let z = od * (1.0 - t);
            roof.push_oriented_box(
                Vec3::new(0.0, y, z) + n_front * 0.025,
                Vec3::X,
                Vec3::Y,
                n_front,
                Vec3::new(ow * 0.98, 0.035, 0.03),
                seed.wrapping_add(c as u32 * 19),
                0.4,
            );
            roof.push_oriented_box(
                Vec3::new(0.0, y, -z) + n_back * 0.025,
                Vec3::X,
                Vec3::Y,
                n_back,
                Vec3::new(ow * 0.98, 0.035, 0.03),
                seed.wrapping_add(c as u32 * 29),
                0.4,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_hipped_roof(
    width: f32,
    depth: f32,
    base_y: f32,
    rise: f32,
    overhang: f32,
    details: bool,
    seed: u32,
    roof: &mut MeshBuf,
) {
    let ow = width * 0.5 + overhang;
    let od = depth * 0.5 + overhang;
    let eaves_y = if base_y <= TENT_MAX_WALL_HEIGHT {
        0.0
    } else {
        base_y - 0.05
    };
    let apex_y = base_y + rise;

    // Ridge runs along the longer horizontal axis.
    let ridge_half_x = (ow - od).max(0.0);
    let ridge_half_z = (od - ow).max(0.0);

    let c0 = Vec3::new(-ow, eaves_y, -od);
    let c1 = Vec3::new(ow, eaves_y, -od);
    let c2 = Vec3::new(ow, eaves_y, od);
    let c3 = Vec3::new(-ow, eaves_y, od);

    let r0 = Vec3::new(-ridge_half_x, apex_y, -ridge_half_z);
    let r1 = Vec3::new(ridge_half_x, apex_y, ridge_half_z);

    let n_front = Vec3::new(0.0, od, rise).normalize_or_zero();
    let n_back = Vec3::new(0.0, od, -rise).normalize_or_zero();
    let n_right = Vec3::new(rise, ow, 0.0).normalize_or_zero();
    let n_left = Vec3::new(-rise, ow, 0.0).normalize_or_zero();

    if ridge_half_x >= ridge_half_z {
        // Ridge along X
        roof.push_quad(c3, c2, r1, r0, n_front);
        roof.push_quad(c1, c0, r0, r1, n_back);
        roof.push_tri([c2, c1, r1], n_right);
        roof.push_tri([c0, c3, r0], n_left);
    } else {
        // Ridge along Z
        roof.push_quad(c2, c1, r0, r1, n_right);
        roof.push_quad(c0, c3, r1, r0, n_left);
        roof.push_tri([c3, c2, r1], n_front);
        roof.push_tri([c1, c0, r0], n_back);
    }

    if details && (r1 - r0).length() > 0.05 {
        let dir = (r1 - r0).normalize_or_zero();
        let half_len = (r1 - r0).length() * 0.5 + 0.05;
        roof.push_oriented_box(
            (r0 + r1) * 0.5 + Vec3::Y * 0.035,
            dir,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(half_len, 0.045, 0.055),
            seed,
            0.35,
        );
    }
}

fn build_battlements(
    exposed: &[ExposedWallSegment],
    top_y: f32,
    details: bool,
    seed: u32,
    stone: &mut MeshBuf,
    roof: &mut MeshBuf,
) {
    let merlon_len = 0.42;
    let gap = 0.30;
    let step = merlon_len + gap;

    for (seg_idx, seg) in exposed.iter().enumerate() {
        if (seg.top_y - top_y).abs() > 0.05 {
            continue;
        }
        let seg_len = seg.length();
        if seg_len < 0.35 {
            continue;
        }
        let tangent_2d = (seg.p1 - seg.p0) / seg_len;
        let right = Vec3::new(tangent_2d.x, 0.0, tangent_2d.y);
        let outward = Vec3::new(seg.normal.x, 0.0, seg.normal.y);
        let count = (seg_len / step).floor().max(1.0) as usize;
        let actual_step = seg_len / count as f32;
        let start = Vec3::new(seg.p0.x, top_y, seg.p0.y);

        for i in 0..count {
            let u = (i as f32 + 0.5) * actual_step;
            let center = start + right * u + Vec3::Y * 0.16 - outward * 0.05;
            stone.push_oriented_box(
                center,
                right,
                Vec3::Y,
                outward,
                Vec3::new((actual_step * 0.56).min(0.26), 0.16, 0.11),
                seed.wrapping_add((seg_idx * 43 + i) as u32),
                0.35,
            );
            if details {
                roof.push_oriented_box(
                    center + Vec3::Y * 0.175,
                    right,
                    Vec3::Y,
                    outward,
                    Vec3::new((actual_step * 0.60).min(0.28), 0.03, 0.13),
                    seed.wrapping_add((seg_idx * 59 + i) as u32),
                    0.25,
                );
            }
        }
    }
}

/// Build a library of standalone modular brick, stone, quoin, arch, shingle,
/// coping, battlement, and window meshes for glTF kit export (`gltf_export.rs`).
///
/// Each entry returns `(mesh_name, material_name, mesh)` with geometry centred
/// around its local origin so it can be textured, sculpted, or instanced in
/// external 3D modeling software (such as Blender).
pub fn build_glade_modular_kit_meshes() -> Vec<(String, &'static str, Mesh)> {
    let mut items: Vec<(String, &'static str, Mesh)> = Vec::new();

    // 1. Six Individual Brick & Stone Variations
    let brick_specs = [
        (
            "Brick_Var_01_Standard_Ashlar",
            "Mat_Glade_Brick_Dressed",
            Vec3::new(0.19, 0.09, 0.10),
            101_u32,
            0.30_f32,
        ),
        (
            "Brick_Var_02_Long_Stretcher",
            "Mat_Glade_Brick_Dressed",
            Vec3::new(0.27, 0.08, 0.10),
            202,
            0.35,
        ),
        (
            "Brick_Var_03_Short_Header",
            "Mat_Glade_Brick_Dressed",
            Vec3::new(0.11, 0.09, 0.11),
            303,
            0.35,
        ),
        (
            "Brick_Var_04_Rounded_Cobblestone",
            "Mat_Glade_Cobblestone",
            Vec3::new(0.15, 0.08, 0.12),
            404,
            0.65,
        ),
        (
            "Brick_Var_05_Weathered_Castle_Block",
            "Mat_Glade_Masonry_Stone",
            Vec3::new(0.23, 0.13, 0.14),
            505,
            0.55,
        ),
        (
            "Brick_Var_06_Courtyard_Flagstone",
            "Mat_Glade_Cobblestone",
            Vec3::new(0.24, 0.035, 0.21),
            606,
            0.45,
        ),
    ];

    for (name, mat, half_ext, seed, jitter) in brick_specs {
        let mut buf = MeshBuf::default();
        buf.push_oriented_box(
            Vec3::new(0.0, half_ext.y, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            half_ext,
            seed,
            jitter,
        );
        items.push((name.to_owned(), mat, buf.finish()));
    }

    // 2. Staggered Brick Course Wall Clusters (2 variations)
    for (cluster_idx, name) in [
        (0_u32, "Brick_Course_Cluster_Var_01"),
        (1_u32, "Brick_Course_Cluster_Var_02"),
    ] {
        let mut buf = MeshBuf::default();
        let row_h = 0.16;
        for row in 0..3 {
            let stagger = if (row + cluster_idx as usize) % 2 == 1 {
                0.16
            } else {
                0.0
            };
            for col in 0..3 {
                let cx = -0.32 + col as f32 * 0.32 + stagger - 0.08;
                let cy = (row as f32 + 0.5) * row_h;
                buf.push_oriented_box(
                    Vec3::new(cx, cy, 0.0),
                    Vec3::X,
                    Vec3::Y,
                    Vec3::Z,
                    Vec3::new(0.145, row_h * 0.45, 0.09),
                    700 + cluster_idx * 53 + (row * 7 + col) as u32,
                    0.40,
                );
            }
        }
        items.push((name.to_owned(), "Mat_Glade_Brick_Dressed", buf.finish()));
    }

    // 3. Corner Quoin Column & Seam Pilaster Variations
    {
        let mut corner_quoin = MeshBuf::default();
        emit_quoin_column(
            &mut corner_quoin,
            Vec3::ZERO,
            2.2,
            Vec3::X,
            Vec3::Z,
            false,
            801,
        );
        items.push((
            "Quoin_Column_Corner_Alternating".to_owned(),
            "Mat_Glade_Brick_Dressed",
            corner_quoin.finish(),
        ));

        let mut seam_quoin = MeshBuf::default();
        emit_quoin_column(
            &mut seam_quoin,
            Vec3::ZERO,
            2.2,
            Vec3::X,
            Vec3::Z,
            true,
            809,
        );
        items.push((
            "Quoin_Column_Seam_Pilaster".to_owned(),
            "Mat_Glade_Brick_Dressed",
            seam_quoin.finish(),
        ));
    }

    // 4. Archway Voussoir Wedges, Keystone & Complete Arch Vault
    {
        for (idx, name, h, d, seed) in [
            (0, "Arch_Voussoir_Wedge_Var_01", 0.10, 0.22, 901_u32),
            (1, "Arch_Voussoir_Wedge_Var_02", 0.11, 0.24, 902_u32),
            (2, "Arch_Keystone_Central_Crest", 0.15, 0.27, 903_u32),
        ] {
            let mut buf = MeshBuf::default();
            buf.push_oriented_box(
                Vec3::new(0.0, h, 0.0),
                Vec3::X,
                Vec3::Y,
                Vec3::Z,
                Vec3::new(0.11 + idx as f32 * 0.015, h, d),
                seed,
                0.35,
            );
            items.push((name.to_owned(), "Mat_Glade_Brick_Dressed", buf.finish()));
        }

        let mut arch_vault = MeshBuf::default();
        emit_archway_span(
            &mut arch_vault,
            Vec3::new(-0.6, 0.0, 0.18),
            Vec3::X,
            Vec3::Z,
            0.0,
            2.2,
            0.0,
            1.2,
            1.95,
            0.36,
            true,
            true,
            950,
        );
        items.push((
            "Arch_Vault_Portal_Complete".to_owned(),
            "Mat_Glade_Brick_Dressed",
            arch_vault.finish(),
        ));
    }

    // 5. Roof Shingle Tiles, Shingle Strip, Wall Coping & Battlement Merlons
    {
        for (name, half_ext, seed) in [
            (
                "Roof_Shingle_Tile_Var_01_Flat",
                Vec3::new(0.12, 0.022, 0.16),
                1001_u32,
            ),
            (
                "Roof_Shingle_Tile_Var_02_Wide",
                Vec3::new(0.16, 0.024, 0.15),
                1002_u32,
            ),
            (
                "Roof_Shingle_Tile_Var_03_Weathered",
                Vec3::new(0.11, 0.026, 0.18),
                1003_u32,
            ),
        ] {
            let mut buf = MeshBuf::default();
            buf.push_oriented_box(
                Vec3::new(0.0, half_ext.y, 0.0),
                Vec3::X,
                Vec3::Y,
                Vec3::Z,
                half_ext,
                seed,
                0.45,
            );
            items.push((name.to_owned(), "Mat_Glade_Roof_Terracotta", buf.finish()));
        }

        let mut coping = MeshBuf::default();
        emit_coping_stones(
            &mut coping,
            Vec2::new(-0.9, 0.0),
            Vec2::new(0.9, 0.0),
            0.0,
            Vec3::X,
            Vec3::Z,
            0.42,
            1101,
        );
        items.push((
            "Wall_Coping_Stone_Row".to_owned(),
            "Mat_Glade_Cobblestone",
            coping.finish(),
        ));

        let mut merlon = MeshBuf::default();
        merlon.push_oriented_box(
            Vec3::new(0.0, 0.18, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.22, 0.18, 0.19),
            1201,
            0.35,
        );
        items.push((
            "Battlement_Merlon_Body".to_owned(),
            "Mat_Glade_Masonry_Stone",
            merlon.finish(),
        ));

        let mut merlon_cap = MeshBuf::default();
        merlon_cap.push_oriented_box(
            Vec3::new(0.0, 0.035, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.25, 0.035, 0.22),
            1202,
            0.25,
        );
        items.push((
            "Battlement_Merlon_Cap".to_owned(),
            "Mat_Glade_Roof_Terracotta",
            merlon_cap.finish(),
        ));
    }

    // 6. Window Frames & Timber Door Leaves
    {
        for (frame_name, glass_name, w, h, seed) in [
            (
                "Window_Cottage_StoneFrame",
                "Window_Cottage_GlassPane",
                0.48_f32,
                0.68_f32,
                1301_u32,
            ),
            (
                "Window_Tower_Narrow_StoneFrame",
                "Window_Tower_Narrow_GlassPane",
                0.36_f32,
                0.62_f32,
                1302_u32,
            ),
        ] {
            let hw = w * 0.5;
            let hh = h * 0.5;
            let cy = hh + 0.08;
            let mut frame = MeshBuf::default();
            // Sill
            frame.push_oriented_box(
                Vec3::new(0.0, cy - hh - 0.04, 0.045),
                Vec3::X,
                Vec3::Y,
                Vec3::Z,
                Vec3::new(hw + 0.06, 0.04, 0.055),
                seed,
                0.35,
            );
            // Lintel
            frame.push_oriented_box(
                Vec3::new(0.0, cy + hh + 0.045, 0.038),
                Vec3::X,
                Vec3::Y,
                Vec3::Z,
                Vec3::new(hw + 0.07, 0.045, 0.045),
                seed + 17,
                0.35,
            );
            items.push((frame_name.to_owned(), "Mat_Glade_Brick_Dressed", frame.finish()));

            let mut glass = MeshBuf::default();
            glass.push_quad(
                Vec3::new(-hw, cy - hh, 0.018),
                Vec3::new(hw, cy - hh, 0.018),
                Vec3::new(hw, cy + hh, 0.018),
                Vec3::new(-hw, cy + hh, 0.018),
                Vec3::Z,
            );
            items.push((glass_name.to_owned(), "Mat_Window_Glass", glass.finish()));
        }

        let mut door_leaf = MeshBuf::default();
        door_leaf.push_oriented_box(
            Vec3::new(0.0, 0.65, 0.0),
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.24, 0.65, 0.035),
            1401,
            0.20,
        );
        items.push((
            "Door_Timber_Leaf_Planked".to_owned(),
            "Mat_Glade_Timber_Wood",
            door_leaf.finish(),
        ));
    }

    items
}

// ---------------------------------------------------------------------------
// Dirty Signature Hashing for Real-Time Reactive Rebuilds
// ---------------------------------------------------------------------------

/// Compute a 64-bit topology & parameter signature for `piece` that changes
/// whenever:
/// - `piece` itself is resized, rotated, moved, or restyled,
/// - any overlapping Glade building moves or resizes (affecting 2D boolean wall culling),
/// - any path or threshold crossing `piece` moves (affecting archway cutouts).
pub fn compute_piece_signature(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
) -> u64 {
    compute_piece_signature_with_spine(piece, shape, all_pieces, path_polylines, None)
}

/// Compute a 64-bit topology & parameter signature for `piece`, including any
/// curved `wall_spine` waypoints when `piece` is a path-following `glade_wall`.
pub fn compute_piece_signature_with_spine(
    piece: &Piece,
    shape: &Shape,
    all_pieces: &[Piece],
    path_polylines: &[Vec<Vec2>],
    wall_spine: Option<&[Vec2]>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    piece.id.hash(&mut hasher);
    piece.shape.hash(&mut hasher);
    quantize_vec2(piece.pos).hash(&mut hasher);
    quantize_f32(piece.elevation).hash(&mut hasher);
    quantize_f32(piece.rot).hash(&mut hasher);
    quantize_vec3(piece.trigger_size).hash(&mut hasher);
    quantize_f32(piece.trigger_alpha).hash(&mut hasher);
    piece.trigger_color.hash(&mut hasher);
    piece.trigger_visible.hash(&mut hasher);
    (piece.trigger_shape_override as u8).hash(&mut hasher);

    let mut spine_radius = 0.0_f32;
    if let Some(spine) = wall_spine {
        spine.len().hash(&mut hasher);
        for &pt in spine {
            quantize_vec2(pt).hash(&mut hasher);
            spine_radius = spine_radius.max(pt.distance(piece.pos));
        }
    }

    let ext = piece_extents(piece, shape);
    let radius = ext.x.max(ext.z).max(spine_radius * 2.0) + 4.0;
    for other in all_pieces {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        if !(is_glade_shape(other_shape) || is_door_cutter_shape(other_shape)) {
            continue;
        }
        let other_ext = piece_extents(other, other_shape);
        let other_radius = other_ext.x.max(other_ext.z) * 0.5;
        if other.pos.distance(piece.pos) <= radius + other_radius + 4.0 {
            other.id.hash(&mut hasher);
            other.shape.hash(&mut hasher);
            quantize_vec2(other.pos).hash(&mut hasher);
            quantize_f32(other.elevation).hash(&mut hasher);
            quantize_f32(other.rot).hash(&mut hasher);
            quantize_vec3(other.trigger_size).hash(&mut hasher);
            quantize_f32(other.trigger_alpha).hash(&mut hasher);
            (other.trigger_shape_override as u8).hash(&mut hasher);
        }
    }

    for line in path_polylines {
        for &pt in line {
            if pt.distance(piece.pos) <= radius + 2.0 {
                quantize_vec2(pt).hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

fn quantize_f32(v: f32) -> i32 {
    if v.is_finite() {
        (v * 500.0).round() as i32
    } else {
        0
    }
}

fn quantize_vec2(v: Vec2) -> [i32; 2] {
    [quantize_f32(v.x), quantize_f32(v.y)]
}

fn quantize_vec3(v: Vec3) -> [i32; 3] {
    [quantize_f32(v.x), quantize_f32(v.y), quantize_f32(v.z)]
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::Dir3;
    use bevy::mesh::VertexAttributeValues;

    fn make_glade_piece(id: u32, shape_id: &str, pos: Vec2, size: Vec3) -> Piece {
        Piece {
            id,
            shape: model::shape_index(shape_id).expect("glade shape exists"),
            pos,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: model::Trigger::harmless(),
            trigger_size: size,
            trigger_shape_override: TriggerShape::Box,
            trigger_alpha: 0.35,
            trigger_color: model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: true,
            interact: model::InteractAction::None,
        }
    }

    fn vertex_count(mesh: &Mesh) -> usize {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(v)) => v.len(),
            _ => 0,
        }
    }

    #[test]
    fn glade_shapes_are_classified_and_default_to_cozy_proportions() {
        let tower = &shapes()[model::shape_index("glade_tower").unwrap()];
        let room = &shapes()[model::shape_index("glade_room").unwrap()];
        let wall = &shapes()[model::shape_index("glade_wall").unwrap()];

        assert_eq!(glade_kind(tower), Some(GladeKind::Tower));
        assert_eq!(glade_kind(room), Some(GladeKind::Room));
        assert_eq!(glade_kind(wall), Some(GladeKind::Wall));

        let p_tower = make_glade_piece(1, "glade_tower", Vec2::ZERO, default_size(tower));
        assert_eq!(roof_style(&p_tower, tower), GladeRoofStyle::Conical);
        assert!(total_height(&p_tower, tower) > 3.5);

        let p_room = make_glade_piece(2, "glade_room", Vec2::ZERO, default_size(room));
        assert_eq!(roof_style(&p_room, room), GladeRoofStyle::Gable);
    }

    #[test]
    fn lowering_roof_pitch_to_zero_transitions_to_battlements() {
        let room = &shapes()[model::shape_index("glade_room").unwrap()];
        let mut p = make_glade_piece(1, "glade_room", Vec2::ZERO, default_size(room));
        assert_eq!(roof_style(&p, room), GladeRoofStyle::Gable);

        p.trigger_alpha = 0.0;
        assert_eq!(roof_style(&p, room), GladeRoofStyle::Battlements);
    }

    #[test]
    fn wall_height_transitions_between_fence_standard_and_fortified_tiers() {
        assert_eq!(wall_tier(0.8), WallTier::LowFence);
        assert_eq!(wall_tier(1.15), WallTier::LowFence);
        assert_eq!(wall_tier(2.0), WallTier::StandardWall);
        assert_eq!(wall_tier(3.4), WallTier::FortifiedWall);
    }

    #[test]
    fn overlapping_rooms_cull_interior_walls_and_merge_footprints() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        // Single isolated 4x4 room has 4 exposed perimeter wall segments (total length 16m).
        let a = make_glade_piece(1, "glade_room", Vec2::new(-1.0, 0.0), Vec3::new(4.0, 2.6, 4.0));
        let solo_segs = compute_exposed_wall_segments(&a, room_shape, &[a]);
        let solo_len: f32 = solo_segs.iter().map(ExposedWallSegment::length).sum();
        assert!((solo_len - 16.0).abs() < 0.05, "solo perimeter = {solo_len}");

        // Place an overlapping 4x4 room shifted +2m in X (overlapping by 2m).
        // The internal wall portions inside the overlap must be culled!
        let b = make_glade_piece(2, "glade_room", Vec2::new(1.0, 0.0), Vec3::new(4.0, 2.6, 4.0));
        let all = [a, b];
        let exposed_a = compute_exposed_wall_segments(&a, room_shape, &all);
        let exposed_b = compute_exposed_wall_segments(&b, room_shape, &all);
        let merged_len: f32 = exposed_a
            .iter()
            .chain(exposed_b.iter())
            .map(ExposedWallSegment::length)
            .sum();

        // Two 4x4 squares overlapping by 2x4 form a 6x4 rectangle with perimeter 20m
        // (far less than 32m of two unmerged rooms!).
        assert!(
            (merged_len - 20.0).abs() < 0.15,
            "merged outer perimeter should be ~20m, got {merged_len}"
        );
        // And the split edges at the intersection must be flagged as seam junctions
        assert!(
            exposed_a.iter().any(|s| s.seam_start || s.seam_end),
            "intersection points should be marked as seam junctions"
        );

        let clusters = find_connected_clusters(&all);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 2);
    }

    #[test]
    fn taller_tower_overlapping_shorter_room_keeps_its_upper_facade() {
        let tower_shape = &shapes()[model::shape_index("glade_tower").unwrap()];
        let tower = make_glade_piece(1, "glade_tower", Vec2::ZERO, Vec3::new(2.4, 4.5, 2.4));
        let room = make_glade_piece(2, "glade_room", Vec2::new(1.0, 0.0), Vec3::new(3.6, 2.2, 3.6));
        let all = [tower, room];

        let tower_segs = compute_exposed_wall_segments(&tower, tower_shape, &all);
        // Segments inside the room footprint should start at the room's roof base (y = 2.2)
        // rather than being culled all the way to the top!
        assert!(
            tower_segs.iter().any(|s| (s.base_y - 2.2).abs() < 0.05 && (s.top_y - 4.5).abs() < 0.05),
            "tower rising out of shorter room keeps its upper facade: {tower_segs:?}"
        );
    }

    #[test]
    fn path_crossing_a_glade_wall_automatically_cuts_an_archway() {
        let wall_shape = &shapes()[model::shape_index("glade_wall").unwrap()];
        let wall = make_glade_piece(1, "glade_wall", Vec2::ZERO, Vec3::new(3.6, 2.4, 0.36));
        let seg = ExposedWallSegment {
            p0: Vec2::new(-1.8, 0.0),
            p1: Vec2::new(1.8, 0.0),
            normal: Vec2::new(0.0, 1.0),
            base_y: 0.0,
            top_y: 2.4,
            corner_start: true,
            corner_end: true,
            seam_start: false,
            seam_end: false,
        };

        // Path running from (0, -2) to (0, +2) straight through the wall centre.
        let path = vec![vec![Vec2::new(0.0, -2.0), Vec2::new(0.0, 2.0)]];
        let openings = compute_arch_openings(&wall, &seg, &[wall], &path);
        assert_eq!(openings.len(), 1, "path crossing wall creates an archway");
        assert!(
            (openings[0].center_dist - 1.8).abs() < 0.05,
            "archway is centred at the path intersection"
        );

        // Mesh generation with the path builds additional archway & keystone geometry.
        let mesh_no_path = build_glade_meshes(&wall, wall_shape, &[wall], &[]);
        let mesh_with_path = build_glade_meshes(&wall, wall_shape, &[wall], &path);
        assert!(
            vertex_count(&mesh_with_path.stone) > vertex_count(&mesh_no_path.stone),
            "archway adds vaulted soffit and keystone geometry"
        );
    }

    #[test]
    fn buildings_do_not_spawn_automatic_windows_and_glade_window_builds_and_snaps() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        let tower_shape = &shapes()[model::shape_index("glade_tower").unwrap()];
        let window_shape = &shapes()[model::shape_index("glade_window").unwrap()];

        let room = make_glade_piece(1, "glade_room", Vec2::ZERO, Vec3::new(5.2, 4.2, 4.0));
        let tower = make_glade_piece(2, "glade_tower", Vec2::new(8.0, 0.0), Vec3::new(2.8, 3.6, 2.8));
        let room_meshes = build_glade_meshes(&room, room_shape, &[room], &[]);
        let tower_meshes = build_glade_meshes(&tower, tower_shape, &[tower], &[]);

        assert_eq!(
            vertex_count(&room_meshes.glass),
            0,
            "buildings no longer spawn automatic windows"
        );
        assert_eq!(
            vertex_count(&tower_meshes.glass),
            0,
            "towers no longer spawn automatic windows"
        );

        let mut win = make_glade_piece(
            3,
            "glade_window",
            Vec2::new(0.0, 2.5),
            default_size(window_shape),
        );
        let (snapped_pos, _snapped_rot, snapped_elev) =
            snap_door_to_glade_wall_with_elevation(&win, &[room], None, &[])
                .expect("glade_window snaps onto room wall");
        assert!(
            (snapped_pos.y - 2.0).abs() < 0.05,
            "window snaps onto room +Z wall at z=2.0 (got {})",
            snapped_pos.y
        );
        assert!(
            snapped_elev > room.elevation + 0.3,
            "window snaps above ground sill height (got {snapped_elev})"
        );
        win.pos = snapped_pos;
        win.elevation = snapped_elev;

        // Verify the window cuts a boolean opening out of the host room wall.
        let room_segs = compute_exposed_wall_segments(&room, room_shape, &[room, win]);
        let win_openings: usize = room_segs
            .iter()
            .map(|s| compute_window_openings(&room, s, &[room, win]).len())
            .sum();
        assert_eq!(
            win_openings, 1,
            "glade_window cuts a boolean opening in the host room wall"
        );
        let room_with_win = build_glade_meshes(&room, room_shape, &[room, win], &[]);
        assert_ne!(
            vertex_count(&room_with_win.stone),
            vertex_count(&room_meshes.stone),
            "cutting a window opening updates the host room wall mesh"
        );

        // Verify tangential snapping onto a circular tower.
        let tower_probe = make_glade_piece(
            4,
            "glade_window",
            Vec2::new(8.0 + 1.2, 1.2),
            default_size(window_shape),
        );
        let (t_pos, t_rot, _t_elev) =
            snap_door_to_glade_wall_with_elevation(&tower_probe, &[tower], None, &[])
                .expect("glade_window snaps tangentially onto tower");
        let radial = (t_pos - tower.pos).normalize_or_zero();
        let tangent_dir = Vec2::new(t_rot.cos(), t_rot.sin());
        assert!(
            radial.dot(tangent_dir).abs() < 0.05,
            "glade_window aligns tangentially to tower wall (dot={})",
            radial.dot(tangent_dir)
        );

        let win_meshes = build_glade_meshes(&win, window_shape, &[room, win], &[]);
        assert!(vertex_count(&win_meshes.glass) > 0, "glade_window emits glass");
        assert!(vertex_count(&win_meshes.stone) > 0, "glade_window emits stone frame");
        assert!(vertex_count(&win_meshes.roof) > 0, "glade_window emits timber lintel & mullions");
    }

    #[test]
    fn glade_door_snaps_to_walls_and_cuts_open_archways_in_rooms_and_towers() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        let tower_shape = &shapes()[model::shape_index("glade_tower").unwrap()];
        let door_shape = &shapes()[model::shape_index("glade_door").unwrap()];

        let room = make_glade_piece(1, "glade_room", Vec2::ZERO, Vec3::new(4.0, 2.8, 3.0));
        let mut door = make_glade_piece(
            2,
            "glade_door",
            Vec2::new(0.2, 1.85),
            default_size(door_shape),
        );

        // Door within 1.25m of the room's +Z wall (at z = 1.5) snaps directly onto the wall face.
        let (snapped_pos, snapped_rot) =
            snap_door_to_glade_wall(&door, &[room]).expect("door snaps to room wall");
        assert!(
            (snapped_pos.y - 1.5).abs() < 0.05,
            "door snapped onto z=1.5 wall: {snapped_pos:?}"
        );
        door.pos = snapped_pos;
        door.rot = snapped_rot;

        // The room wall segment at z = 1.5 now has a cut-out archway opening!
        let exposed = compute_exposed_wall_segments(&room, room_shape, &[room, door]);
        let total_openings: usize = exposed
            .iter()
            .map(|seg| compute_arch_openings(&room, seg, &[room, door], &[]).len())
            .sum();
        assert_eq!(total_openings, 1, "glade_door cuts an archway in the room wall");

        // And the door itself builds stone jambs/voussoirs and wooden leaves.
        let door_meshes = build_glade_meshes(&door, door_shape, &[room, door], &[]);
        assert!(vertex_count(&door_meshes.stone) > 24);
        assert!(vertex_count(&door_meshes.roof) > 12);

        // Placing a door on a cylindrical tower also carves an archway cutout into the tower wall!
        let tower = make_glade_piece(3, "glade_tower", Vec2::new(6.0, 0.0), Vec3::new(2.4, 4.0, 2.4));
        let tower_door = make_glade_piece(
            4,
            "glade_door",
            Vec2::new(6.0, 1.2),
            default_size(door_shape),
        );
        let tower_solo = build_glade_meshes(&tower, tower_shape, &[tower], &[]);
        let tower_cut = build_glade_meshes(&tower, tower_shape, &[tower, tower_door], &[]);
        assert_ne!(
            vertex_count(&tower_cut.stone),
            vertex_count(&tower_solo.stone),
            "tower wall geometry changes when a cut-out door is placed on its perimeter"
        );
    }

    #[test]
    fn glade_wall_extrudes_along_a_curved_multi_point_path_spine() {
        let wall_shape = &shapes()[model::shape_index("glade_wall").unwrap()];
        let wall = make_glade_piece(1, "glade_wall", Vec2::ZERO, Vec3::new(3.0, 2.2, 0.36));

        let straight = build_glade_meshes_with_spine(&wall, wall_shape, &[wall], &[], None);
        let spine = vec![
            Vec2::new(-3.0, 0.0),
            Vec2::new(-1.0, 1.2),
            Vec2::new(1.2, 1.2),
            Vec2::new(3.2, -0.4),
        ];
        let curved =
            build_glade_meshes_with_spine(&wall, wall_shape, &[wall], &[], Some(&spine));

        assert!(
            vertex_count(&curved.stone) > vertex_count(&straight.stone)
                && vertex_count(&curved.roof) > vertex_count(&straight.roof),
            "multi-segment path wall has more stone and coping vertices than a single straight span ({} vs {})",
            vertex_count(&curved.stone),
            vertex_count(&straight.stone)
        );
    }

    #[test]
    fn door_snaps_strictly_tangent_to_towers_curved_walls_and_3d_wall_rays() {
        let door_shape = &shapes()[model::shape_index("glade_door").unwrap()];
        let tower = make_glade_piece(1, "glade_tower", Vec2::ZERO, Vec3::new(3.0, 4.0, 3.0));

        // 1. Test arbitrary non-facet angles around a cylindrical tower: the snapped
        // door position must lie on the R=1.5 circle and its tangent vector `(cos(rot), sin(rot))`
        // must be strictly perpendicular (`dot == 0`) to the radial vector!
        for &angle in &[0.0_f32, 0.43, 1.12, std::f32::consts::FRAC_PI_2, 2.35, 4.18] {
            let probe = Vec2::new(angle.cos() * 1.9, angle.sin() * 1.9);
            let door = make_glade_piece(2, "glade_door", probe, default_size(door_shape));
            let (snapped_pos, snapped_rot) =
                snap_door_to_glade_wall(&door, &[tower]).expect("snaps to tower");
            assert!(
                (snapped_pos.length() - 1.5).abs() < 1e-3,
                "door snaps to tower radius 1.5m: got {}",
                snapped_pos.length()
            );
            let radial = snapped_pos.normalize();
            let door_tangent = Vec2::new(snapped_rot.cos(), snapped_rot.sin());
            assert!(
                radial.dot(door_tangent).abs() < 1e-4,
                "door tangent {door_tangent:?} must be perpendicular to tower radius {radial:?} at angle {angle}"
            );
        }

        // 2. Test 3D perspective ray aimed at the front (+Z) face of the tower at height y=1.5m,
        // where the ground-plane intersection (y=0) lands far behind the front wall!
        let ray_origin = Vec3::new(0.0, 3.5, 5.5);
        let target_on_front_wall = Vec3::new(0.0, 1.5, 1.5);
        let ray = Ray3d::new(
            ray_origin,
            Dir3::new((target_on_front_wall - ray_origin).normalize()).unwrap(),
        );
        // Even if `piece.pos` (the y=0 projection) is far behind at z = -1.5, the 3D ray hits
        // the front wall at z = +1.5!
        let far_ground_door =
            make_glade_piece(2, "glade_door", Vec2::new(0.0, -1.5), default_size(door_shape));
        let (ray_pos, ray_rot) =
            snap_door_to_glade_wall_with_ray(&far_ground_door, &[tower], Some(ray), &[])
                .expect("3D ray hits front tower wall");
        assert!(
            (ray_pos.y - 1.5).abs() < 0.05 && ray_pos.x.abs() < 0.05,
            "3D ray snaps to front wall (0, 1.5), got {ray_pos:?}"
        );
        let front_tangent = Vec2::new(ray_rot.cos(), ray_rot.sin());
        assert!(
            front_tangent.x.abs() > 0.99,
            "door on +Z tower face is tangent along X axis: {front_tangent:?}"
        );

        // 3. Test snapping tangentially onto a diagonal segment of a curved path-following wall.
        let wall = make_glade_piece(3, "glade_wall", Vec2::new(10.0, 0.0), Vec3::new(3.0, 2.4, 0.36));
        let spine = vec![Vec2::new(8.0, 0.0), Vec2::new(11.0, 3.0)]; // 45-degree segment
        let wall_door = make_glade_piece(
            4,
            "glade_door",
            Vec2::new(9.6, 1.4),
            default_size(door_shape),
        );
        let (_, spine_rot) = snap_door_to_glade_wall_with_ray(
            &wall_door,
            &[wall],
            None,
            &[(wall.id, spine)],
        )
        .expect("snaps to diagonal spine wall");
        let expected_angle = std::f32::consts::FRAC_PI_4;
        assert!(
            (spine_rot - expected_angle).abs() < 1e-3,
            "door aligns tangentially at 45 deg ({expected_angle}), got {spine_rot}"
        );
    }

    #[test]
    fn building_walls_and_caps_generate_both_exterior_and_interior_faces() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        let room = make_glade_piece(1, "glade_room", Vec2::ZERO, Vec3::new(3.0, 2.4, 3.0));
        let meshes = build_glade_meshes(&room, room_shape, &[room], &[]);

        let normals = meshes
            .stone
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("normals")
            .as_float3()
            .expect("float3 normals");

        // Verify that floor is visible from inside (+Y normal at y=0) and below (-Y normal at y=0)
        let has_floor_up = normals
            .iter()
            .any(|&[nx, ny, nz]| ny > 0.95 && nx.abs() < 0.1 && nz.abs() < 0.1);
        let has_floor_down = normals
            .iter()
            .any(|&[nx, ny, nz]| ny < -0.95 && nx.abs() < 0.1 && nz.abs() < 0.1);
        assert!(has_floor_up, "interior floor must be visible with upward normals");
        assert!(has_floor_down, "exterior floor must have downward normals");

        // Walls should have both outward and inward normals (e.g. +Z and -Z, +X and -X)
        let has_wall_pos_z = normals.iter().any(|&[_nx, ny, nz]| nz > 0.95 && ny.abs() < 0.1);
        let has_wall_neg_z = normals.iter().any(|&[_nx, ny, nz]| nz < -0.95 && ny.abs() < 0.1);
        let has_wall_pos_x = normals.iter().any(|&[nx, ny, _nz]| nx > 0.95 && ny.abs() < 0.1);
        let has_wall_neg_x = normals.iter().any(|&[nx, ny, _nz]| nx < -0.95 && ny.abs() < 0.1);

        assert!(
            has_wall_pos_z && has_wall_neg_z,
            "walls must have both outward and inward faces along Z"
        );
        assert!(
            has_wall_pos_x && has_wall_neg_x,
            "walls must have both outward and inward faces along X"
        );
    }

    #[test]
    fn glade_terrain_supports_adjustable_dimensions_doubled_height_and_slope_to_cliff_transitions() {
        let terrain_shape = &shapes()[model::shape_index("glade_terrain").unwrap()];
        assert_eq!(glade_kind(terrain_shape), Some(GladeKind::Terrain));

        // 1. Gentle grassy mound (low falloff alpha = 0.15)
        let mut gentle = make_glade_piece(1, "glade_terrain", Vec2::ZERO, Vec3::new(8.0, 2.8, 8.0));
        gentle.trigger_alpha = 0.15;
        gentle.trigger_shape_override = TriggerShape::Sphere;
        assert_eq!(terrain_style(&gentle), GladeTerrainStyle::Mound);

        // Peak height at centre matches piece height; smoothly drops to 0 outside footprint.
        let center_h = terrain_height_at(Vec2::ZERO, &[gentle]);
        let mid_slope_h = terrain_height_at(Vec2::new(2.2, 0.0), &[gentle]);
        let outside_h = terrain_height_at(Vec2::new(5.0, 0.0), &[gentle]);
        assert!((center_h - 2.8).abs() < 0.2, "center_h = {center_h}");
        assert!(mid_slope_h > 0.3 && mid_slope_h < center_h, "mid_slope_h = {mid_slope_h}");
        assert_eq!(outside_h, 0.0);

        // 2. Steep cliff plateau with doubled height range (height = 16.0m, falloff alpha = 0.90)
        let mut cliff = make_glade_piece(1, "glade_terrain", Vec2::ZERO, Vec3::new(8.0, 16.0, 8.0));
        cliff.trigger_alpha = 0.90;
        cliff.trigger_shape_override = TriggerShape::Box;
        assert_eq!(terrain_style(&cliff), GladeTerrainStyle::Plateau);
        assert!((piece_extents(&cliff, terrain_shape).y - 16.0).abs() < 1e-4);

        let gentle_meshes = build_glade_meshes(&gentle, terrain_shape, &[gentle], &[]);
        let cliff_meshes = build_glade_meshes(&cliff, terrain_shape, &[cliff], &[]);

        // Steep cliff falloff exposes craggy limestone rock faces & ledges (`stone` buffer),
        // whereas gentle hill slope is mostly meadow grass (`roof` buffer).
        assert!(
            vertex_count(&cliff_meshes.stone) > vertex_count(&gentle_meshes.stone) + 200,
            "steep falloff generates craggy cliff rock geometry ({} vs {} stone vertices)",
            vertex_count(&cliff_meshes.stone),
            vertex_count(&gentle_meshes.stone)
        );
        assert!(
            vertex_count(&gentle_meshes.roof) > 100 && vertex_count(&cliff_meshes.roof) > 50,
            "both terrain chunks retain a grassy meadow sward on their upper plateau"
        );
    }

    #[test]
    fn overlapping_terrain_chunks_blend_smoothly_and_buildings_extend_hillside_foundations() {
        let terrain_shape = &shapes()[model::shape_index("glade_terrain").unwrap()];
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];

        let chunk_a = make_glade_piece(
            1,
            "glade_terrain",
            Vec2::new(-1.8, 0.0),
            Vec3::new(6.0, 3.0, 6.0),
        );
        let chunk_b = make_glade_piece(
            2,
            "glade_terrain",
            Vec2::new(1.8, 0.0),
            Vec3::new(6.0, 3.0, 6.0),
        );
        let solo_mid = terrain_height_at(Vec2::ZERO, &[chunk_a]);
        let blended_mid = terrain_height_at(Vec2::ZERO, &[chunk_a, chunk_b]);
        assert!(
            blended_mid > solo_mid,
            "smooth-max blending raises saddle between overlapping terrain chunks ({blended_mid} > {solo_mid})"
        );

        // Place a room perched on the sloped hillside of chunk_a: its walls must extend
        // a battered stone foundation skirt below y = 0.0 down to meet the terrain slope!
        let mut hillside_room =
            make_glade_piece(3, "glade_room", Vec2::new(0.2, 0.0), Vec3::new(3.0, 2.4, 2.6));
        hillside_room.elevation = terrain_height_at(hillside_room.pos, &[chunk_a]);
        let room_meshes = build_glade_meshes(
            &hillside_room,
            room_shape,
            &[chunk_a, hillside_room],
            &[],
        );
        let positions = room_meshes
            .stone
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("float3");
        let min_local_y = positions
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        assert!(
            min_local_y < -0.15,
            "hillside building extends stone foundation skirt below y=0 (min_local_y = {min_local_y})"
        );
    }

    #[test]
    fn lowering_building_height_creates_tent_like_houses_and_suppresses_clipping_details() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        let tower_shape = &shapes()[model::shape_index("glade_tower").unwrap()];

        // Standard cottage room has no automatic windows (`glass` == 0)
        let normal_room =
            make_glade_piece(1, "glade_room", Vec2::ZERO, Vec3::new(3.6, 2.6, 3.0));
        assert!(!is_tent_house(&normal_room, room_shape));
        let normal_meshes = build_glade_meshes(&normal_room, room_shape, &[normal_room], &[]);
        assert_eq!(vertex_count(&normal_meshes.glass), 0);

        // Squash the room's wall height down to 0.25m (<= TENT_MAX_WALL_HEIGHT):
        // it transforms into a tent-like A-frame house!
        let tent_room =
            make_glade_piece(1, "glade_room", Vec2::ZERO, Vec3::new(3.6, 0.25, 3.0));
        assert!(is_tent_house(&tent_room, room_shape));
        let tent_meshes = build_glade_meshes(&tent_room, room_shape, &[tent_room], &[]);

        // Side windows are suppressed so they never poke through the ground-reaching roof slopes
        assert_eq!(
            vertex_count(&tent_meshes.glass),
            0,
            "tent-like house suppresses side wall windows"
        );
        // Roof eaves drop all the way to y = 0.0 (ground level)
        let roof_positions = tent_meshes
            .roof
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("roof positions")
            .as_float3()
            .expect("float3");
        let min_roof_y = roof_positions
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        let max_roof_y = roof_positions
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            min_roof_y <= 0.02,
            "tent-like house roof eaves drop to ground level y=0 (got {min_roof_y})"
        );
        assert!(
            max_roof_y > 1.0,
            "tent-like house retains a walkable peaked roof apex (got {max_roof_y})"
        );

        // Circular tower also supports tent/pavilion mode when squashed down
        let tent_tower =
            make_glade_piece(2, "glade_tower", Vec2::ZERO, Vec3::new(2.8, 0.20, 2.8));
        assert!(is_tent_house(&tent_tower, tower_shape));
        let tower_meshes = build_glade_meshes(&tent_tower, tower_shape, &[tent_tower], &[]);
        assert_eq!(vertex_count(&tower_meshes.glass), 0);
    }

    #[test]
    fn world_terrain_generates_continuous_meadow_faceted_cliff_boulders_3d_grass_flowers_paths_and_stairs() {
        // Central clearing stays level at 0.0 across the expanded 10x flat
        // buildable area (r <= FLAT_CLEARING_RADIUS) while the built-in Tiny
        // Glade hillside rises in the (-X, -Z) quadrant at (-7.6, -6.8).
        assert!((base_meadow_height_at(Vec2::ZERO)).abs() < 1e-4);
        assert!((base_meadow_height_at(Vec2::new(6.0, 0.0))).abs() < 1e-4);
        assert!((base_meadow_height_at(Vec2::new(30.0, 0.0))).abs() < 1e-4);
        assert!((base_meadow_height_at(Vec2::new(0.0, 48.0))).abs() < 1e-4);
        assert!(
            base_meadow_height_at(Vec2::new(75.0, 0.0)) > 0.5,
            "outer hills rise beyond FLAT_CLEARING_RADIUS"
        );
        let hill_h = base_meadow_height_at(Vec2::new(-7.6, -6.8));
        assert!(
            hill_h > 2.0,
            "built-in scenic plateau hillside rises above 2m (got {hill_h})"
        );

        // Place a stone tower on the hilltop plateau and a sculpted terrain ridge
        let mut tower = make_glade_piece(
            1,
            "glade_tower",
            Vec2::new(-7.6, -6.8),
            Vec3::new(2.8, 4.2, 2.8),
        );
        tower.elevation = world_terrain_height_at(tower.pos, &[]);
        let custom_ridge = make_glade_piece(
            2,
            "glade_terrain",
            Vec2::new(4.0, -5.0),
            Vec3::new(11.0, 2.8, 11.0),
        );
        let pieces = vec![tower, custom_ridge];

        let world_meshes = build_world_terrain_meshes(&pieces, &[]);
        assert!(
            vertex_count(&world_meshes.turf) > 5_000,
            "continuous 88m x 88m world turf mesh is generated"
        );
        assert!(
            vertex_count(&world_meshes.rock_light) > 500
                && vertex_count(&world_meshes.rock_dark) > 300,
            "steep cliff slopes and talus emit multi-faceted 3D angular rock boulders"
        );
        assert!(
            vertex_count(&world_meshes.grass_sun) > 2_000
                && vertex_count(&world_meshes.grass_lush) > 1_500
                && vertex_count(&world_meshes.grass_clover) > 200,
            "dense 3D grass blades, clover rosettes, and tower ivy are generated across meadow & plateau"
        );
        assert!(
            vertex_count(&world_meshes.flower_yellow) > 100
                && vertex_count(&world_meshes.flower_white) > 60,
            "3D yellow and white wildflower clusters dot the meadow"
        );
        assert!(
            vertex_count(&world_meshes.path_sand) > 100,
            "3D sandy meadow path ribbon is generated along the hillside trail"
        );

        // Elevated world terrain produces walkable trimesh triangles
        let (verts, tris) = world_terrain_elevated_collider(&pieces);
        assert!(
            !verts.is_empty() && !tris.is_empty(),
            "world terrain generates trimesh collider triangles for elevated hills and plateaus"
        );
    }

    #[test]
    fn buildings_do_not_spawn_automatic_paths_and_doors_cut_into_elevated_buildings() {
        let room_shape = &shapes()[model::shape_index("glade_room").unwrap()];
        let door_shape = &shapes()[model::shape_index("glade_door").unwrap()];

        // Place a building on an elevated plateau away from the built-in hillside path
        let mut room = make_glade_piece(1, "glade_room", Vec2::new(14.0, 14.0), Vec3::new(3.6, 2.6, 3.0));
        room.elevation = 2.4;

        // Placing a building must NOT spawn an automatic path attached to it
        let paths_without_user = compute_world_terrain_paths(&[room], &[]);
        assert!(
            !paths_without_user
                .iter()
                .any(|line| line.iter().any(|pt| pt.distance(room.pos) < 5.0)),
            "buildings must not come with automatic paths"
        );

        // Fresh door ghost (`id == 0`, `elevation == 0.0`) snaps to the elevated room wall
        // and inherits the room's elevation so it cuts open an archway cleanly.
        let raw_door = make_glade_piece(
            0,
            "glade_door",
            Vec2::new(14.0, 14.0 + 1.6),
            default_size(door_shape),
        );
        let (snapped_pos, snapped_rot, wall_elev) =
            snap_door_to_glade_wall_with_elevation(&raw_door, &[room], None, &[])
                .expect("door snaps to elevated building wall");
        assert!(
            (wall_elev - room.elevation).abs() < 1e-4,
            "snapped door inherits host building elevation ({} vs {})",
            wall_elev,
            room.elevation
        );

        let mut placed_door = make_glade_piece(2, "glade_door", snapped_pos, default_size(door_shape));
        placed_door.rot = snapped_rot;
        placed_door.elevation = wall_elev;

        let exposed = compute_exposed_wall_segments(&room, room_shape, &[room, placed_door]);
        let openings: usize = exposed
            .iter()
            .map(|seg| compute_arch_openings(&room, seg, &[room, placed_door], &[]).len())
            .sum();
        assert_eq!(
            openings, 1,
            "door cuts an open archway into the elevated building wall"
        );
    }
}
