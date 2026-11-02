//! The editor's data model: hex maths, the shape catalogue, snapping.
//!
//! No Bevy systems live here — this is plain geometry so it can be unit-tested
//! without a window. Everything is in metres: `Vec2` is the ground plane
//! (`x`, `z`), and each shape carries the vertical extent used to extrude it
//! into a solid in `mesh.rs`.

use std::sync::LazyLock;

use bevy::color::Color;
use bevy::math::{Vec2, Vec3};

/// Circumradius of one hex, in metres. Edge length comes out at ~2.08 m.
pub const HEX_SIZE: f32 = 1.2;
/// Rotation increment offered by the editor.
pub const ANCHOR_STEP: f32 = std::f32::consts::PI / 6.0;
/// Width of the component palette, in logical pixels.
pub const PANEL_WIDTH: f32 = 250.0;
/// How far apart two anchors may be and still snap, in metres.
pub const SNAP_THRESHOLD: f32 = 0.4;
/// Extra slack when clicking a piece, in metres.
pub const HIT_PAD: f32 = 0.1;

const SQRT3: f32 = 1.732_050_8;

/// Length of a hex edge: a wall bar is exactly this long. For a regular
/// hexagon the side length equals the circumradius (not the width across flats).
pub fn edge_len() -> f32 {
    HEX_SIZE
}

// ---------------------------------------------------------------------------
// Hex grid (pointy top, axial q/r, +y up)
// ---------------------------------------------------------------------------

pub fn hex_center(q: i32, r: i32) -> Vec2 {
    Vec2::new(
        HEX_SIZE * SQRT3 * (q as f32 + r as f32 / 2.0),
        HEX_SIZE * 1.5 * r as f32,
    )
}

pub fn hex_corners(center: Vec2) -> [Vec2; 6] {
    let mut out = [Vec2::ZERO; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        let a = std::f32::consts::PI / 3.0 * i as f32 + std::f32::consts::PI / 6.0;
        *slot = center + Vec2::from_angle(a) * HEX_SIZE;
    }
    out
}

pub fn hex_edges(center: Vec2) -> [Vec2; 6] {
    let d = HEX_SIZE * SQRT3 / 2.0;
    let mut out = [Vec2::ZERO; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        let a = std::f32::consts::PI / 3.0 * i as f32;
        *slot = center + Vec2::from_angle(a) * d;
    }
    out
}

pub fn pixel_to_hex(p: Vec2) -> (i32, i32) {
    let r = (2.0 / 3.0) * p.y / HEX_SIZE;
    let q = (SQRT3 / 3.0 * p.x - p.y / 3.0) / HEX_SIZE;
    hex_round(q, r)
}

fn hex_round(q: f32, r: f32) -> (i32, i32) {
    let x = q;
    let z = r;
    let y = -x - z;
    let rx = x.round();
    let ry = y.round();
    let rz = z.round();
    let dx = (rx - x).abs();
    let dy = (ry - y).abs();
    let dz = (rz - z).abs();
    let (rx, rz) = if dx > dy && dx > dz {
        (-ry - rz, rz)
    } else if dy > dz {
        (rx, rz)
    } else {
        (rx, -rx - ry)
    };
    (rx as i32, rz as i32)
}

pub fn nearest_hex(p: Vec2) -> Vec2 {
    let (q, r) = pixel_to_hex(p);
    hex_center(q, r)
}

/// Nearest hex edge midpoint, with its angle folded into `[0, π)` because a
/// bar is symmetric under a half turn.
pub fn nearest_edge(p: Vec2) -> (Vec2, f32) {
    let (q, r) = pixel_to_hex(p);
    let c = hex_center(q, r);
    let mut best = Vec2::ZERO;
    let mut best_d = f32::MAX;
    let mut best_angle = 0.0;
    for (i, mid) in hex_edges(c).iter().enumerate() {
        let d = mid.distance(p);
        if d < best_d {
            best_d = d;
            best = *mid;
            best_angle = std::f32::consts::PI / 3.0 * i as f32;
        }
    }
    // The midpoint's bearing points at the edge; the edge itself runs at a
    // right angle to it.
    (
        best,
        (best_angle + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::PI),
    )
}

// ---------------------------------------------------------------------------
// Anchors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnchorKind {
    Edge,
    End,
    Slot,
}

impl AnchorKind {
    pub fn color(self) -> Color {
        match self {
            AnchorKind::Edge => Color::srgb_u8(0x59, 0xa7, 0xc4),
            AnchorKind::End => Color::srgb_u8(0xe0, 0xa2, 0x4a),
            AnchorKind::Slot => Color::srgb_u8(0xb0, 0x8b, 0xf0),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub local: Vec2,
    pub normal: Vec2,
    pub kind: AnchorKind,
}

fn hex_anchors(r: f32) -> Vec<Anchor> {
    let d = r * SQRT3 / 2.0;
    (0..6)
        .map(|i| {
            let dir = Vec2::from_angle(std::f32::consts::PI / 3.0 * i as f32);
            Anchor {
                local: dir * d,
                normal: dir,
                kind: AnchorKind::Edge,
            }
        })
        .collect()
}

fn end_anchors(len: f32) -> Vec<Anchor> {
    vec![
        Anchor {
            local: Vec2::new(-len / 2.0, 0.0),
            normal: Vec2::NEG_X,
            kind: AnchorKind::End,
        },
        Anchor {
            local: Vec2::new(len / 2.0, 0.0),
            normal: Vec2::X,
            kind: AnchorKind::End,
        },
    ]
}

fn ring_anchors(n: usize, radius: f32, start: f32) -> Vec<Anchor> {
    (0..n)
        .map(|i| {
            let a = start + std::f32::consts::TAU / n as f32 * i as f32;
            let dir = Vec2::from_angle(a);
            Anchor {
                local: dir * radius,
                normal: dir,
                kind: AnchorKind::Slot,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The catalogue: 20 pieces across six categories
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapMode {
    /// Snaps to a hex edge and takes its angle (walls, doors).
    Edge,
    /// Snaps to a hex centre and keeps its rotation (props, floors).
    Center,
}

#[derive(Clone, Debug)]
pub enum Geometry {
    Hex {
        r: f32,
    },
    Bar {
        len: f32,
        thick: f32,
    },
    Rect {
        w: f32,
        h: f32,
    },
    Circle {
        r: f32,
    },
    Diamond {
        r: f32,
    },
    Cross {
        r: f32,
    },
    Flag {
        r: f32,
    },
    /// A wedge rising along local +y (the model's ground axis): width, run, rise.
    Wedge {
        w: f32,
        d: f32,
        h: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Plain,
    Dashed,
    Waves,
    Planks,
    Rail,
    Window,
    Door,
    Gate,
    Secret,
    Rubble,
}

/// Damage dealt on touch. Hard-coded to 10 for spikes; everything else is 0.
pub const SPIKE_DAMAGE: f32 = 10.0;

/// Collision-trigger effects attached to a single placed piece.
///
/// A trigger fires while the character overlaps the piece's solid (see
/// `hazards::hazard_at`). Today the only effect is [`Trigger::damage`];
/// add new fields here (heal, bounce, message, teleport, ...) and handle
/// them in `hazards::apply_triggers` — `Piece` stays `Copy` and old saves
/// keep loading because every new field gets a `#[serde(default)]` entry in
/// `SavedPiece`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Trigger {
    /// Touch damage per hit. Zero means harmless.
    pub damage: f32,
}

impl Trigger {
    /// A trigger with no effects.
    pub fn harmless() -> Self {
        Self { damage: 0.0 }
    }

    /// A damaging trigger, clamped to `>= 0`. Non-finite amounts are ignored.
    pub fn damaging(amount: f32) -> Self {
        Self {
            damage: if amount.is_finite() && amount > 0.0 {
                amount
            } else {
                0.0
            },
        }
    }

    pub fn is_harmless(self) -> bool {
        self.damage <= 0.0
    }

    /// Set damage, clamping negatives to zero and ignoring non-finite input.
    pub fn set_damage(&mut self, amount: f32) {
        if amount.is_finite() {
            self.damage = amount.max(0.0);
        }
    }
}

/// Parametric trigger volume shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TriggerShape {
    #[default]
    Box,
    Sphere,
    Cylinder,
}

impl TriggerShape {
    pub fn id(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Sphere => "sphere",
            Self::Cylinder => "cylinder",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Sphere => "Sphere",
            Self::Cylinder => "Cylinder",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "box" => Some(Self::Box),
            "sphere" => Some(Self::Sphere),
            "cylinder" => Some(Self::Cylinder),
            _ => None,
        }
    }
}

/// Default size for a newly placed trigger, in metres (width, height, length).
pub const TRIGGER_DEFAULT_SIZE: Vec3 = Vec3::new(2.0, 2.0, 2.0);
/// Minimum trigger extents, to avoid degenerate volumes.
pub const TRIGGER_MIN_SIZE: f32 = 0.1;
/// Default transparency for trigger zones (0 = transparent, 1 = opaque).
pub const TRIGGER_DEFAULT_ALPHA: f32 = 0.35;
/// Minimum/maximum alpha for trigger zones.
pub const TRIGGER_MIN_ALPHA: f32 = 0.0;
pub const TRIGGER_MAX_ALPHA: f32 = 1.0;
/// Default RGB color for trigger zones (0xRRGGBB).
pub const TRIGGER_DEFAULT_COLOR: u32 = 0x6b7d93;
/// Whether trigger zones are rendered in edit mode by default.
pub const TRIGGER_DEFAULT_VISIBLE: bool = true;
/// Preset trigger colors shown in the properties panel.
pub const TRIGGER_COLOR_PRESETS: [u32; 8] = [
    0x6b7d93, 0x3fd0b0, 0xe05a52, 0x5a8ce0, 0xf0a63c, 0xb08bf0, 0x62c96b, 0xc0392b,
];

/// Convert a 0xRRGGBB hex to a Bevy [`Color`].
pub fn trigger_color_to_bevy(hex: u32) -> Color {
    Color::srgb_u8(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// Format a hex as `#RRGGBB`.
pub fn trigger_color_to_hex_string(hex: u32) -> String {
    format!("#{:06x}", hex & 0xFFFFFF)
}

/// Parse `#RRGGBB` or `RRGGBB` into a hex. Returns `None` on failure.
pub fn trigger_color_parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    u32::from_str_radix(t, 16).ok().filter(|&v| v <= 0xFFFFFF)
}

/// Trigger color combined with alpha.
pub fn trigger_color_with_alpha(hex: u32, alpha: f32) -> Color {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    Color::srgba(r, g, b, alpha.clamp(0.0, 1.0))
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub mode: SnapMode,
    pub color: Color,
    pub geom: Geometry,
    pub style: Style,
    pub anchors: Vec<Anchor>,
    /// How tall the solid is, in metres.
    pub height: f32,
    /// Where its bottom sits; a floor's top is at zero, so its base is negative.
    pub base: f32,
    /// Touch damage per hit. Zero means harmless.
    pub damage: f32,
    /// True for trigger volumes (parametric, invisible in play, non-collidable).
    pub is_trigger: bool,
    /// Which trigger form this shape represents. Meaningful only when `is_trigger`.
    pub trigger_shape: TriggerShape,
}

impl Shape {
    pub fn top(&self) -> f32 {
        self.base + self.height
    }

    pub fn is_trigger(self: &Shape) -> bool {
        self.is_trigger
    }
}

pub const CATEGORIES: [&str; 9] = [
    "Surface",
    "Barrier",
    "Traversal",
    "Threshold",
    "Fixture",
    "Resource",
    "Hazard",
    "Marker",
    "Trigger",
];

#[allow(clippy::too_many_arguments)]
fn shape(
    id: &'static str,
    name: &'static str,
    category: &'static str,
    mode: SnapMode,
    color: u32,
    geom: Geometry,
    style: Style,
    height: f32,
    base: f32,
    anchors: Vec<Anchor>,
) -> Shape {
    Shape {
        id,
        name,
        category,
        mode,
        color: Color::srgb_u8((color >> 16) as u8, (color >> 8) as u8, color as u8),
        geom,
        style,
        anchors,
        height,
        base,
        damage: 0.0,
        is_trigger: false,
        trigger_shape: TriggerShape::Box,
    }
}

fn trigger_shape(
    id: &'static str,
    name: &'static str,
    form: TriggerShape,
    color: u32,
    geom: Geometry,
    height: f32,
    base: f32,
    anchors: Vec<Anchor>,
) -> Shape {
    Shape {
        id,
        name,
        category: "Trigger",
        mode: SnapMode::Center,
        color: Color::srgb_u8((color >> 16) as u8, (color >> 8) as u8, color as u8),
        geom,
        style: Style::Plain,
        anchors,
        height,
        base,
        damage: 0.0,
        is_trigger: true,
        trigger_shape: form,
    }
}

fn hazard_shape(
    id: &'static str,
    name: &'static str,
    mode: SnapMode,
    color: u32,
    geom: Geometry,
    style: Style,
    height: f32,
    base: f32,
    anchors: Vec<Anchor>,
    damage: f32,
) -> Shape {
    let mut shape = shape(
        id, name, "Hazard", mode, color, geom, style, height, base, anchors,
    );
    shape.damage = damage;
    shape
}

static SHAPES: LazyLock<Vec<Shape>> = LazyLock::new(|| {
    let len = edge_len();
    vec![
        // Surfaces. Tops sit at y = 0 so everything else stands on the floor.
        shape(
            "floor",
            "Floor",
            "Surface",
            SnapMode::Center,
            0x4e6076,
            Geometry::Hex { r: HEX_SIZE },
            Style::Plain,
            0.12,
            -0.12,
            hex_anchors(HEX_SIZE),
        ),
        shape(
            "pit",
            "Pit",
            "Surface",
            SnapMode::Center,
            0x272e38,
            Geometry::Hex { r: HEX_SIZE },
            Style::Dashed,
            0.12,
            -1.4,
            hex_anchors(HEX_SIZE),
        ),
        shape(
            "water",
            "Water",
            "Surface",
            SnapMode::Center,
            0x2a6a86,
            Geometry::Hex { r: HEX_SIZE },
            Style::Waves,
            0.08,
            -0.4,
            hex_anchors(HEX_SIZE),
        ),
        shape(
            "bridge",
            "Bridge",
            "Surface",
            SnapMode::Center,
            0x5c4730,
            Geometry::Hex { r: HEX_SIZE },
            Style::Planks,
            0.18,
            0.0,
            hex_anchors(HEX_SIZE),
        ),
        // Barriers
        shape(
            "wall",
            "Wall",
            "Barrier",
            SnapMode::Edge,
            0x8d9aab,
            Geometry::Bar { len, thick: 0.18 },
            Style::Plain,
            2.8,
            0.0,
            end_anchors(len),
        ),
        shape(
            "fence",
            "Fence",
            "Barrier",
            SnapMode::Edge,
            0x6f7f92,
            Geometry::Bar { len, thick: 0.07 },
            Style::Rail,
            1.05,
            0.0,
            end_anchors(len),
        ),
        shape(
            "window",
            "Window",
            "Barrier",
            SnapMode::Edge,
            0x7fb3c8,
            Geometry::Bar { len, thick: 0.18 },
            Style::Window,
            2.8,
            0.0,
            end_anchors(len),
        ),
        shape(
            "pillar",
            "Pillar",
            "Barrier",
            SnapMode::Center,
            0x8d9aab,
            Geometry::Circle { r: 0.24 },
            Style::Plain,
            2.8,
            0.0,
            ring_anchors(4, 0.3, std::f32::consts::FRAC_PI_4),
        ),
        // Traversal: things you climb and jump on.
        shape(
            "block",
            "Block",
            "Traversal",
            SnapMode::Center,
            0x5c6878,
            Geometry::Rect { w: 1.8, h: 1.8 },
            Style::Plain,
            0.9,
            0.0,
            ring_anchors(4, 1.0, 0.0),
        ),
        shape(
            "ramp",
            "Ramp",
            "Traversal",
            SnapMode::Center,
            0x6f6252,
            Geometry::Wedge {
                w: 1.8,
                d: 2.0,
                h: 0.9,
            },
            Style::Plain,
            0.9,
            0.0,
            ring_anchors(4, 1.0, 0.0),
        ),
        // Thresholds
        shape(
            "door",
            "Door",
            "Threshold",
            SnapMode::Edge,
            0xc9924a,
            Geometry::Bar { len, thick: 0.18 },
            Style::Door,
            2.4,
            0.0,
            end_anchors(len),
        ),
        shape(
            "gate",
            "Gate",
            "Threshold",
            SnapMode::Edge,
            0xb47a3f,
            Geometry::Bar { len, thick: 0.22 },
            Style::Gate,
            2.6,
            0.0,
            end_anchors(len),
        ),
        shape(
            "secret",
            "Secret",
            "Threshold",
            SnapMode::Edge,
            0x9b8fc4,
            Geometry::Bar { len, thick: 0.18 },
            Style::Secret,
            2.8,
            0.0,
            end_anchors(len),
        ),
        shape(
            "rubble",
            "Rubble",
            "Threshold",
            SnapMode::Edge,
            0x7d7261,
            Geometry::Bar { len, thick: 0.36 },
            Style::Rubble,
            0.55,
            0.0,
            end_anchors(len),
        ),
        // Fixtures
        shape(
            "crate",
            "Crate",
            "Fixture",
            SnapMode::Center,
            0x8a6334,
            Geometry::Rect { w: 0.85, h: 0.85 },
            Style::Plain,
            0.85,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
        shape(
            "chest",
            "Chest",
            "Fixture",
            SnapMode::Center,
            0x9c7440,
            Geometry::Rect { w: 1.0, h: 0.62 },
            Style::Plain,
            0.7,
            0.0,
            ring_anchors(4, 0.62, 0.0),
        ),
        shape(
            "altar",
            "Altar",
            "Fixture",
            SnapMode::Center,
            0x6e7f9c,
            Geometry::Rect { w: 0.9, h: 0.55 },
            Style::Plain,
            1.0,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
        shape(
            "brazier",
            "Brazier",
            "Fixture",
            SnapMode::Center,
            0xb4622f,
            Geometry::Circle { r: 0.3 },
            Style::Plain,
            0.85,
            0.0,
            ring_anchors(4, 0.38, std::f32::consts::FRAC_PI_4),
        ),
        // Resources
        shape(
            "herb",
            "Herbs",
            "Resource",
            SnapMode::Center,
            0x5d8a4a,
            Geometry::Circle { r: 0.26 },
            Style::Plain,
            0.3,
            0.0,
            ring_anchors(1, 0.34, -std::f32::consts::FRAC_PI_2),
        ),
        shape(
            "ore",
            "Ore",
            "Resource",
            SnapMode::Center,
            0x6f8fa3,
            Geometry::Diamond { r: 0.32 },
            Style::Plain,
            0.5,
            0.0,
            ring_anchors(1, 0.4, -std::f32::consts::FRAC_PI_2),
        ),
        // Markers
        shape(
            "spawn",
            "Spawn",
            "Marker",
            SnapMode::Center,
            0xb4565c,
            Geometry::Cross { r: 0.35 },
            Style::Plain,
            0.02,
            0.01,
            ring_anchors(1, 0.45, -std::f32::consts::FRAC_PI_2),
        ),
        shape(
            "entry",
            "Entry",
            "Marker",
            SnapMode::Center,
            0x57b39a,
            Geometry::Flag { r: 0.4 },
            Style::Plain,
            0.02,
            0.01,
            ring_anchors(1, 0.5, -std::f32::consts::FRAC_PI_2),
        ),
        // Hazards. Touch damage is hard-coded to 10 for now.
        hazard_shape(
            "spikes",
            "Spikes",
            SnapMode::Center,
            0xc0392b,
            Geometry::Circle { r: 0.45 },
            Style::Plain,
            0.5,
            0.0,
            ring_anchors(4, 0.5, 0.0),
            SPIKE_DAMAGE,
        ),
        // Triggers – parametric volumes, invisible in play, non-collidable, any trigger type.
        trigger_shape(
            "trigger",
            "Trigger",
            TriggerShape::Box,
            0x6b7d93,
            Geometry::Rect { w: 1.0, h: 1.0 },
            1.0,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
        trigger_shape(
            "trigger_box",
            "Trigger Box",
            TriggerShape::Box,
            0x6b7d93,
            Geometry::Rect { w: 1.0, h: 1.0 },
            1.0,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
        trigger_shape(
            "trigger_sphere",
            "Trigger Sphere",
            TriggerShape::Sphere,
            0x6b7d93,
            Geometry::Circle { r: 0.5 },
            1.0,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
        trigger_shape(
            "trigger_cylinder",
            "Trigger Cylinder",
            TriggerShape::Cylinder,
            0x6b7d93,
            Geometry::Circle { r: 0.5 },
            1.0,
            0.0,
            ring_anchors(4, 0.6, 0.0),
        ),
    ]
});

pub fn shapes() -> &'static [Shape] {
    &SHAPES
}

pub fn shape_index(id: &str) -> Option<usize> {
    shapes().iter().position(|s| s.id == id)
}

/// Outline of a shape in local space, on the ground plane.
pub fn shape_points(shape: &Shape) -> Vec<Vec2> {
    match &shape.geom {
        Geometry::Hex { r } => (0..6)
            .map(|i| {
                let a = std::f32::consts::PI / 3.0 * i as f32 + std::f32::consts::PI / 6.0;
                Vec2::from_angle(a) * *r
            })
            .collect(),
        Geometry::Bar { len, thick } => vec![
            Vec2::new(-len / 2.0, -thick / 2.0),
            Vec2::new(len / 2.0, -thick / 2.0),
            Vec2::new(len / 2.0, thick / 2.0),
            Vec2::new(-len / 2.0, thick / 2.0),
        ],
        Geometry::Rect { w, h } => vec![
            Vec2::new(-w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, -h / 2.0),
            Vec2::new(w / 2.0, h / 2.0),
            Vec2::new(-w / 2.0, h / 2.0),
        ],
        Geometry::Circle { r } => (0..24)
            .map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / 24.0) * *r)
            .collect(),
        Geometry::Diamond { r } => vec![
            Vec2::new(0.0, *r),
            Vec2::new(r * 0.7, 0.0),
            Vec2::new(0.0, -*r),
            Vec2::new(-r * 0.7, 0.0),
        ],
        Geometry::Cross { r } => {
            let w = r * 0.34;
            vec![
                Vec2::new(-w, -*r),
                Vec2::new(w, -*r),
                Vec2::new(w, -w),
                Vec2::new(*r, -w),
                Vec2::new(*r, w),
                Vec2::new(w, w),
                Vec2::new(w, *r),
                Vec2::new(-w, *r),
                Vec2::new(-w, w),
                Vec2::new(-*r, w),
                Vec2::new(-*r, -w),
                Vec2::new(-w, -w),
            ]
        }
        Geometry::Wedge { w, d, .. } => vec![
            Vec2::new(-w / 2.0, -d / 2.0),
            Vec2::new(w / 2.0, -d / 2.0),
            Vec2::new(w / 2.0, d / 2.0),
            Vec2::new(-w / 2.0, d / 2.0),
        ],
        // A chevron pointing along +x. Simple (no vertex lands on another
        // edge), which the ear clipper in `mesh.rs` needs.
        Geometry::Flag { r } => vec![
            Vec2::new(-0.6 * r, -*r),
            Vec2::new(*r, 0.0),
            Vec2::new(-0.6 * r, *r),
            Vec2::new(-0.2 * r, 0.0),
        ],
    }
}

// ---------------------------------------------------------------------------
// Pieces, anchors in world space, hit testing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub id: u32,
    pub shape: usize,
    /// Ground position: `x` and `z` in metres.
    pub pos: Vec2,
    /// Yaw in radians, counter-clockwise seen from above.
    pub rot: f32,
    /// Pitch in radians around the piece's local X axis, after yaw.
    pub pitch: f32,
    /// Roll in radians around the piece's local Z axis, after pitch.
    pub roll: f32,
    /// Height above the natural placement, in metres. Zero puts a floor's top
    /// on the ground and a block's base on the ground; raising it lifts the
    /// whole piece, so blocks stack and props float.
    pub elevation: f32,
    /// True when Avian drives this piece with gravity and collisions.
    pub dynamic: bool,
    /// Collision-trigger effects for this instance. Defaults to the catalogue
    /// value (`Shape::damage`) when the piece is placed; editing it never
    /// changes other pieces of the same shape.
    pub trigger: Trigger,
    /// Parametric trigger volume extents: width (x), height (y), length (z) in metres.
    /// Only meaningful when the piece's shape is a trigger (`Shape::is_trigger`).
    /// Stored for every piece for `Copy` simplicity; ignored otherwise.
    pub trigger_size: Vec3,
    /// Which trigger form to use when this piece is a generic `trigger` shape.
    /// For `trigger_box` / `trigger_sphere` / `trigger_cylinder` the shape's own
    /// `trigger_shape` wins, but this field still round-trips for persistence.
    pub trigger_shape_override: TriggerShape,
    /// Transparency/opacity for trigger zones (0 transparent .. 1 opaque).
    /// Only meaningful for triggers; stored for all pieces for `Copy`.
    pub trigger_alpha: f32,
    /// RGB color for trigger zones as 0xRRGGBB. Only meaningful for triggers.
    pub trigger_color: u32,
    /// Whether the trigger zone is rendered in edit mode. When false the zone
    /// is still pickable via the scene graph/outliner but its mesh is hidden.
    pub trigger_visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldAnchor {
    pub pos: Vec2,
    pub normal: Vec2,
    pub kind: AnchorKind,
}

/// Full piece orientation: yaw about world +Y, then pitch about the piece's
/// local X, then roll about its local Z.
pub fn piece_quat(piece: &Piece) -> bevy::math::Quat {
    bevy::math::Quat::from_euler(
        bevy::math::EulerRot::YXZ,
        -piece.rot,
        piece.pitch,
        piece.roll,
    )
}

/// Store a full orientation back into the piece's Euler fields. The gizmo
/// works in world-axis quaternion space, so every rotation drag lands here.
pub fn set_piece_quat(piece: &mut Piece, rotation: bevy::math::Quat) {
    let (yaw, pitch, roll) = rotation.to_euler(bevy::math::EulerRot::YXZ);
    piece.rot = normalize_angle(-yaw);
    piece.pitch = normalize_angle(pitch);
    piece.roll = normalize_angle(roll);
}

/// Effective trigger shape for a piece, respecting the shape's own kind.
pub fn piece_trigger_shape(piece: &Piece, shape: &Shape) -> TriggerShape {
    if shape.is_trigger && shape.id != "trigger" {
        shape.trigger_shape
    } else if shape.is_trigger {
        piece.trigger_shape_override
    } else {
        TriggerShape::Box
    }
}

/// Clamp trigger extents to at least `TRIGGER_MIN_SIZE` and finite.
pub fn sanitize_trigger_size(size: Vec3) -> Vec3 {
    Vec3::new(
        if size.x.is_finite() {
            size.x.max(TRIGGER_MIN_SIZE)
        } else {
            TRIGGER_DEFAULT_SIZE.x
        },
        if size.y.is_finite() {
            size.y.max(TRIGGER_MIN_SIZE)
        } else {
            TRIGGER_DEFAULT_SIZE.y
        },
        if size.z.is_finite() {
            size.z.max(TRIGGER_MIN_SIZE)
        } else {
            TRIGGER_DEFAULT_SIZE.z
        },
    )
}

pub fn default_piece_trigger_size() -> Vec3 {
    TRIGGER_DEFAULT_SIZE
}

pub fn sanitize_trigger_alpha(alpha: f32) -> f32 {
    if !alpha.is_finite() {
        TRIGGER_DEFAULT_ALPHA
    } else {
        alpha.clamp(TRIGGER_MIN_ALPHA, TRIGGER_MAX_ALPHA)
    }
}

/// Whether a world point (xz `p`, feet `feet`) is inside a trigger volume.
pub fn trigger_contains(piece: &Piece, shape: &Shape, pos: Vec2, feet: f32) -> bool {
    if !shape.is_trigger {
        return false;
    }
    let size = sanitize_trigger_size(piece.trigger_size);
    let form = piece_trigger_shape(piece, shape);
    // Trigger centre: piece position on ground, elevated by half height.
    let centre_y = piece.elevation + size.y * 0.5;
    let half_h = size.y * 0.5;
    // Horizontal local point (apply yaw).
    let local = to_local(piece, pos);
    // Vertical: check overlap between character capsule [feet, feet+~1.9) and trigger [centre_y - half_h, centre_y + half_h].
    // We reuse the same generous vertical check as hazards: any overlap counts. For parametric triggers we test
    // the point's feet against the volume; hazards use head/feet range separately, but this is used by trigger_hit_test.
    // For now, test that feet is within height, plus a small overlap tolerance, or that head overlaps.
    // The caller `trigger_hit_test` will handle vertical via head/feet; this helper only does 2D shape test.
    // Keep vertical inclusive here for simple callers.
    let vertical_ok = feet >= centre_y - half_h - 0.2 && feet <= centre_y + half_h;
    // Actually vertical handled by caller; we still require roughly inside for sphere/cylinder.
    // For 3D sphere/ellipsoid we need y distance too.
    match form {
        TriggerShape::Box => {
            let inside_xz =
                local.x.abs() <= size.x * 0.5 + 1e-5 && local.y.abs() <= size.z * 0.5 + 1e-5;
            let inside_y = feet >= centre_y - half_h - 0.05
                && feet + 1.9 > centre_y - half_h
                && feet < centre_y + half_h + 0.2;
            inside_xz && inside_y && vertical_ok || (inside_xz && inside_y)
        }
        TriggerShape::Sphere => {
            // Ellipsoid: (x/rx)^2 + (y/ry)^2 + (z/rz)^2 <=1 where y is vertical distance from centre to character's mid.
            // Use character mid height for test: feet+0.95
            let mid_y = feet + 0.95;
            let dy = mid_y - centre_y;
            let rx = size.x * 0.5;
            let ry = size.y * 0.5;
            let rz = size.z * 0.5;
            if rx < 1e-4 || ry < 1e-4 || rz < 1e-4 {
                return false;
            }
            let nx = local.x / rx;
            let ny = dy / ry;
            let nz = local.y / rz;
            nx * nx + ny * ny + nz * nz <= 1.0 + 1e-5
        }
        TriggerShape::Cylinder => {
            let rx = size.x * 0.5;
            let rz = size.z * 0.5;
            if rx < 1e-4 || rz < 1e-4 {
                return false;
            }
            let nx = local.x / rx;
            let nz = local.y / rz;
            let inside_xz = nx * nx + nz * nz <= 1.0 + 1e-5;
            let inside_y = feet >= centre_y - half_h - 0.05
                && feet + 1.9 > centre_y - half_h
                && feet < centre_y + half_h + 0.2;
            inside_xz && inside_y
        }
    }
}

/// Full trigger overlap test used by `hazards::trigger_at`: 2D footprint plus vertical range `feet`..`head`.
pub fn trigger_overlaps(piece: &Piece, shape: &Shape, pos: Vec2, feet: f32, head: f32) -> bool {
    if !shape.is_trigger {
        return false;
    }
    let size = sanitize_trigger_size(piece.trigger_size);
    let form = piece_trigger_shape(piece, shape);
    let centre_y = piece.elevation + size.y * 0.5;
    let bottom = piece.elevation;
    let top = piece.elevation + size.y;
    // Vertical overlap: same as hazards but using trigger's own height.
    if head <= bottom || feet >= top + 0.2 {
        // Check ellipsoid case separately (needs centre)
        if form != TriggerShape::Sphere {
            return false;
        }
    }
    let local = to_local(piece, pos);
    match form {
        TriggerShape::Box => {
            if local.x.abs() > size.x * 0.5 + 0.15 || local.y.abs() > size.z * 0.5 + 0.15 {
                // Pad like hazards
                return false;
            }
            // Pad already applied via 0.15; vertical already checked
            head > bottom && feet < top + 0.2
        }
        TriggerShape::Cylinder => {
            let rx = size.x * 0.5 + 0.15;
            let rz = size.z * 0.5 + 0.15;
            if rx < 1e-4 || rz < 1e-4 {
                return false;
            }
            let nx = local.x / rx;
            let nz = local.y / rz;
            // For padded test we inflate rx/rz above; use direct distance
            let dist2 = (local.x / (size.x * 0.5 + 0.15)).powi(2)
                + (local.y / (size.z * 0.5 + 0.15)).powi(2);
            dist2 <= 1.0 && head > bottom && feet < top + 0.2
        }
        TriggerShape::Sphere => {
            let mid_y = (feet + head) * 0.5;
            let dy = mid_y - centre_y;
            let rx = size.x * 0.5 + 0.15;
            let ry = size.y * 0.5 + 0.15;
            let rz = size.z * 0.5 + 0.15;
            if rx < 1e-4 || ry < 1e-4 || rz < 1e-4 {
                return false;
            }
            let nx = local.x / rx;
            let ny = dy / ry;
            let nz = local.y / rz;
            nx * nx + ny * ny + nz * nz <= 1.0
        }
    }
}

pub fn world_anchors(piece: &Piece, shape: &Shape) -> Vec<WorldAnchor> {
    let (sin, cos) = piece.rot.sin_cos();
    shape
        .anchors
        .iter()
        .map(|a| {
            let rotated = Vec2::new(
                a.local.x * cos - a.local.y * sin,
                a.local.x * sin + a.local.y * cos,
            );
            let normal = Vec2::new(
                a.normal.x * cos - a.normal.y * sin,
                a.normal.x * sin + a.normal.y * cos,
            );
            WorldAnchor {
                pos: piece.pos + rotated,
                normal,
                kind: a.kind,
            }
        })
        .collect()
}

#[allow(dead_code)] // kept for collision and click refinement
pub fn to_local(piece: &Piece, p: Vec2) -> Vec2 {
    let d = p - piece.pos;
    let (sin, cos) = (-piece.rot).sin_cos();
    Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
}

#[allow(dead_code)]
fn point_in_polygon(points: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let pi = points[i];
        let pj = points[j];
        if (pi.y > p.y) != (pj.y > p.y) && p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y) + pi.x
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[allow(dead_code)] // shape-accurate footprint test, used by tests and gameplay
pub fn hit_test(shape: &Shape, piece: &Piece, p: Vec2, pad: f32) -> bool {
    let local = to_local(piece, p);
    if let Geometry::Circle { r } = shape.geom {
        return local.length() <= r + pad;
    }
    point_in_polygon(&shape_points(shape), local) || local.length() <= pad
}

// ---------------------------------------------------------------------------
// Snapping
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapKind {
    Anchor,
    Grid,
    Free,
}

#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub pos: Vec2,
    pub rot: f32,
    pub kind: SnapKind,
}

/// Translation-only snap to the closest pair of anchors that face each other.
/// The facing test (`dot < -0.6`) is what stops a wall on one hex edge from
/// welding to a wall on the neighbouring edge across the shared corner.
pub fn anchor_snap(piece: &Piece, shape: &Shape, others: &[Piece], threshold: f32) -> Option<Snap> {
    let moving = world_anchors(piece, shape);
    let mut best: Option<(f32, WorldAnchor, WorldAnchor, u32)> = None;

    for other in others {
        if other.id == piece.id {
            continue;
        }
        let Some(other_shape) = shapes().get(other.shape) else {
            continue;
        };
        for target in world_anchors(other, other_shape) {
            for m in &moving {
                if m.kind != target.kind {
                    continue;
                }
                if m.normal.dot(target.normal) > -0.6 {
                    continue;
                }
                let d = target.pos.distance(m.pos);
                if d > threshold {
                    continue;
                }
                if best.as_ref().is_none_or(|(bd, ..)| d < *bd) {
                    best = Some((d, *m, target, other.id));
                }
            }
        }
    }

    best.map(|(_, m, target, _)| Snap {
        pos: piece.pos + (target.pos - m.pos),
        rot: piece.rot,
        kind: SnapKind::Anchor,
    })
}

pub fn grid_snap(piece: &Piece, shape: &Shape) -> Snap {
    match shape.mode {
        SnapMode::Edge => {
            let (pos, rot) = nearest_edge(piece.pos);
            Snap {
                pos,
                rot,
                kind: SnapKind::Grid,
            }
        }
        SnapMode::Center => Snap {
            pos: nearest_hex(piece.pos),
            rot: piece.rot,
            kind: SnapKind::Grid,
        },
    }
}

pub fn snap_piece(
    piece: &Piece,
    shape: &Shape,
    others: &[Piece],
    anchors: bool,
    grid: bool,
    threshold: f32,
) -> Snap {
    if anchors {
        if let Some(snap) = anchor_snap(piece, shape, others, threshold) {
            return snap;
        }
    }
    if grid {
        return grid_snap(piece, shape);
    }
    Snap {
        pos: piece.pos,
        rot: piece.rot,
        kind: SnapKind::Free,
    }
}

pub fn normalize_angle(a: f32) -> f32 {
    let a = a.rem_euclid(std::f32::consts::TAU);
    // A physics-derived quaternion often lands a hair below a full turn.
    // Keep saved and displayed rotations clean instead of showing 359.999°.
    if a > std::f32::consts::TAU - 1e-4 {
        0.0
    } else {
        a
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} !~ {b}");
    }

    fn shape_by_id(id: &str) -> &'static Shape {
        let i = shape_index(id).unwrap_or_else(|| panic!("no shape {id}"));
        &shapes()[i]
    }

    #[test]
    fn hex_centres_are_axial() {
        near(hex_center(0, 0).x, 0.0, 0.01);
        near(hex_center(1, 0).x, HEX_SIZE * SQRT3, 0.01);
        near(hex_center(0, 1).y, HEX_SIZE * 1.5, 0.01);
    }

    #[test]
    fn pixel_to_hex_round_trips() {
        for (q, r) in [(0, 0), (3, -2), (-4, 5), (7, 1)] {
            assert_eq!(pixel_to_hex(hex_center(q, r)), (q, r), "at {q},{r}");
        }
    }

    #[test]
    fn nearest_edge_finds_the_edge_and_folds_its_angle() {
        let d = HEX_SIZE * SQRT3 / 2.0;
        let (pos, angle) = nearest_edge(Vec2::new(d - 0.15, 0.0));
        near(pos.x, d, 0.01);
        // The +x edge is vertical, so the bar runs along +y in model space.
        near(angle, std::f32::consts::FRAC_PI_2, 0.001);
        // The same physical edge from the other side must not flip the bar.
        let (_, angle) = nearest_edge(Vec2::new(d + 0.15, 0.0));
        near(angle, std::f32::consts::FRAC_PI_2, 0.001);
    }

    #[test]
    fn a_wall_snaps_end_to_end() {
        let wall = shape_by_id("wall");
        let a = Piece {
            id: 1,
            shape: shape_index("wall").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        let ghost = Piece {
            id: 2,
            shape: shape_index("wall").unwrap(),
            pos: Vec2::new(edge_len() - 0.05, 0.02),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };

        let snap = anchor_snap(&ghost, wall, &[a], SNAP_THRESHOLD).expect("anchor snap");
        near(snap.pos.x, edge_len(), 0.02);
        near(snap.pos.y, 0.0, 0.02);
        assert_eq!(snap.kind, SnapKind::Anchor);
    }

    #[test]
    fn a_wall_on_the_next_edge_does_not_weld() {
        let wall = shape_by_id("wall");
        let a = Piece {
            id: 1,
            shape: shape_index("wall").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        // Rotate the ghost by 60° and place its left anchor exactly on a's right anchor.
        let off = Vec2::from_angle(std::f32::consts::PI / 3.0) * Vec2::new(-edge_len() / 2.0, 0.0);
        let ghost = Piece {
            id: 2,
            shape: shape_index("wall").unwrap(),
            pos: Vec2::new(edge_len() / 2.0, 0.0) - off,
            rot: std::f32::consts::PI / 3.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        assert!(anchor_snap(&ghost, wall, &[a], SNAP_THRESHOLD).is_none());
    }

    #[test]
    fn a_crate_clicks_into_a_crate() {
        let crate_shape = shape_by_id("crate");
        let a = Piece {
            id: 1,
            shape: shape_index("crate").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        let ghost = Piece {
            id: 2,
            shape: shape_index("crate").unwrap(),
            pos: Vec2::new(1.25, 0.02),
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        let snap = anchor_snap(&ghost, crate_shape, &[a], SNAP_THRESHOLD).expect("crate snap");
        near(snap.pos.x, 1.2, 0.01);
        near(snap.pos.y, 0.0, 0.01);
    }

    #[test]
    fn grid_snap_orients_walls_and_leaves_props_alone() {
        let wall = shape_by_id("wall");
        let d = HEX_SIZE * SQRT3 / 2.0;
        let snap = grid_snap(
            &Piece {
                id: 1,
                shape: 0,
                pos: Vec2::new(d + 0.15, 0.05),
                rot: 1.2,
                pitch: 0.0,
                roll: 0.0,
                elevation: 0.0,
                dynamic: false,
                trigger: crate::model::Trigger::harmless(),
                trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
                trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            },
            wall,
        );
        assert_eq!(snap.kind, SnapKind::Grid);
        near(snap.rot, std::f32::consts::FRAC_PI_2, 0.001);

        let crate_shape = shape_by_id("crate");
        let snap = grid_snap(
            &Piece {
                id: 1,
                shape: 0,
                pos: Vec2::new(0.06, -0.08),
                rot: 0.5,
                pitch: 0.0,
                roll: 0.0,
                elevation: 0.0,
                dynamic: false,
                trigger: crate::model::Trigger::harmless(),
                trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
                trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            },
            crate_shape,
        );
        near(snap.rot, 0.5, 0.001);
        near(snap.pos.x, hex_center(0, 0).x, 0.01);
    }

    #[test]
    fn hit_testing_follows_the_shape() {
        let crate_shape = shape_by_id("crate");
        let p = Piece {
            id: 1,
            shape: shape_index("crate").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        assert!(hit_test(crate_shape, &p, Vec2::ZERO, 0.0));
        assert!(!hit_test(crate_shape, &p, Vec2::new(2.0, 0.0), HIT_PAD));
        assert!(!hit_test(crate_shape, &p, Vec2::new(0.5, 0.5), 0.0));
        assert!(hit_test(crate_shape, &p, Vec2::new(0.3, 0.3), 0.0));
    }

    #[test]
    fn catalogue_has_every_piece() {
        assert_eq!(shapes().len(), 27);
        let mut ids: Vec<_> = shapes().iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 27);
        for cat in CATEGORIES {
            assert!(shapes().iter().any(|s| s.category == cat), "missing {cat}");
        }
        for s in shapes() {
            assert!(!s.anchors.is_empty(), "{} has no anchors", s.id);
        }
    }

    #[test]
    fn anchors_rotate_with_the_piece() {
        let crate_shape = shape_by_id("crate");
        let p = Piece {
            id: 1,
            shape: shape_index("crate").unwrap(),
            pos: Vec2::new(0.2, 0.4),
            rot: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        let a = world_anchors(&p, crate_shape);
        near(a[0].pos.x, 0.2, 0.01);
        near(a[0].pos.y, 1.0, 0.01);
        near(a[0].normal.x, 0.0, 0.01);
        near(a[0].normal.y, 1.0, 0.01);
    }

    #[test]
    fn pieces_have_the_vertical_extent_the_scene_expects() {
        let floor = shape_by_id("floor");
        near(floor.top(), 0.0, 1e-4);
        assert!(floor.base < 0.0);

        let wall = shape_by_id("wall");
        near(wall.base, 0.0, 1e-4);
        assert!(wall.height > 2.0, "a wall should be over two metres");

        let crate_shape = shape_by_id("crate");
        near(crate_shape.base, 0.0, 1e-4);
        assert!(crate_shape.height > 0.5);

        for s in shapes() {
            assert!(s.height > 0.0, "{} has no height", s.id);
            assert!(s.top() > -2.0, "{} is buried too deep", s.id);
        }
    }

    #[test]
    fn walls_are_exactly_one_hex_edge_long() {
        let wall = shape_by_id("wall");
        let Geometry::Bar { len, .. } = wall.geom else {
            panic!("wall is not a bar")
        };
        near(len, edge_len(), 1e-4);
    }

    #[test]
    fn piece_orientation_round_trips_through_euler_fields() {
        let shape = shape_index("block").unwrap();
        for (rot, pitch, roll) in [
            (0.3, 0.4, 0.5),
            (1.2, -0.7, 2.6),
            (-2.0, 0.9, -1.1),
            (0.0, std::f32::consts::FRAC_PI_2, 0.0),
        ] {
            let mut piece = Piece {
                id: 1,
                shape,
                pos: Vec2::ZERO,
                rot,
                pitch,
                roll,
                elevation: 0.0,
                dynamic: false,
                trigger: crate::model::Trigger::harmless(),
                trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
                trigger_shape_override: crate::model::TriggerShape::Box,
                trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
                trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
                trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
            };
            let before = piece_quat(&piece);
            set_piece_quat(&mut piece, before);
            let after = piece_quat(&piece);
            assert!(
                before.dot(after).abs() > 0.99999,
                "orientation drifted: {before:?} vs {after:?}"
            );
        }
    }

    #[test]
    fn a_piece_without_pitch_or_roll_is_still_a_yaw_rotation() {
        let piece = Piece {
            id: 1,
            shape: shape_index("block").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.7,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
            trigger_size: crate::model::TRIGGER_DEFAULT_SIZE,
            trigger_shape_override: crate::model::TriggerShape::Box,
            trigger_alpha: crate::model::TRIGGER_DEFAULT_ALPHA,
            trigger_color: crate::model::TRIGGER_DEFAULT_COLOR,
            trigger_visible: crate::model::TRIGGER_DEFAULT_VISIBLE,
        };
        let expected = bevy::math::Quat::from_rotation_y(-0.7);
        assert!(piece_quat(&piece).dot(expected).abs() > 0.99999);
    }

    #[test]
    fn triggers_default_to_harmless_and_clamp_damage() {
        assert!(Trigger::default().is_harmless());
        assert!(Trigger::harmless().is_harmless());
        assert_eq!(Trigger::damaging(10.0).damage, 10.0);
        assert!(Trigger::damaging(0.0).is_harmless());
        assert!(Trigger::damaging(-5.0).is_harmless());
        assert!(Trigger::damaging(f32::NAN).is_harmless());
        assert!(Trigger::damaging(f32::INFINITY).is_harmless());

        let mut trigger = Trigger::damaging(5.0);
        trigger.set_damage(12.5);
        assert_eq!(trigger.damage, 12.5);
        trigger.set_damage(-3.0);
        assert_eq!(trigger.damage, 0.0, "negatives clamp to harmless");
        trigger.set_damage(f32::NAN);
        assert_eq!(trigger.damage, 0.0, "NaN input is ignored");
    }
}
