//! Moving platforms: Bezier paths pieces travel along, carrying riders.
//!
//! Pure data + maths so the motion stays unit-testable without a window.
//! The Bevy wiring (editing UI, per-frame motion, rider carry) lives in
//! `editor::platforms_ui`.

use bevy::math::Vec2;

use crate::model::{self, Piece};
use crate::path;

/// How a platform travels its track.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PlatformMode {
    /// Out and back at constant speed.
    #[default]
    PingPong,
    /// Jump back to the start and repeat.
    Loop,
    /// Travel once and stop at the end.
    OneShot,
}

impl PlatformMode {
    pub fn id(self) -> &'static str {
        match self {
            PlatformMode::PingPong => "pingpong",
            PlatformMode::Loop => "loop",
            PlatformMode::OneShot => "oneshot",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pingpong" => Some(PlatformMode::PingPong),
            "loop" => Some(PlatformMode::Loop),
            "oneshot" => Some(PlatformMode::OneShot),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlatformMode::PingPong => "ping-pong",
            PlatformMode::Loop => "loop",
            PlatformMode::OneShot => "one-shot",
        }
    }
}

/// One control point on a platform path. Handles work like the character
/// route: dragging them reshapes the cubic Bezier through `pos`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatformWaypoint {
    pub pos: Vec2,
    pub incoming: Option<Vec2>,
    pub outgoing: Option<Vec2>,
}

impl PlatformWaypoint {
    pub fn new(pos: Vec2) -> Self {
        Self {
            pos,
            incoming: None,
            outgoing: None,
        }
    }
}

/// Advance along `total` metres of track. Returns `(distance, dir)`.
/// `dir` is only meaningful for ping-pong (+1 / -1); loop and one-shot
/// always travel forward.
pub fn advance_platform(
    distance: f32,
    dir: f32,
    total: f32,
    speed: f32,
    dt: f32,
    mode: PlatformMode,
) -> (f32, f32) {
    if total <= 0.0 || speed <= 0.0 || dt <= 0.0 {
        return (distance.clamp(0.0, total.max(0.0)), if dir >= 0.0 { 1.0 } else { -1.0 });
    }
    match mode {
        PlatformMode::Loop => {
            let mut d = (distance + speed * dt) % total;
            if d < 0.0 {
                d += total;
            }
            (d, 1.0)
        }
        PlatformMode::OneShot => ((distance + speed * dt).min(total), 1.0),
        PlatformMode::PingPong => {
            let step = if dir >= 0.0 { 1.0 } else { -1.0 };
            let mut d = distance + step * speed * dt;
            let mut dir = step;
            // Bounce until back in range (covers huge steps).
            for _ in 0..4 {
                if d > total {
                    d = 2.0 * total - d;
                    dir = -1.0;
                } else if d < 0.0 {
                    d = -d;
                    dir = 1.0;
                } else {
                    break;
                }
            }
            (d.clamp(0.0, total), dir)
        }
    }
}

/// Build a smoothed Bezier track through the waypoints, honouring manual
/// handles. Needs at least two points; fewer yields an empty track.
pub fn build_platform_track(waypoints: &[PlatformWaypoint]) -> path::Track {
    let points: Vec<Vec2> = waypoints.iter().map(|w| w.pos).collect();
    if points.len() < 2 {
        return path::Track::new(points);
    }
    let mut curves = path::smooth(&points);
    for (i, waypoint) in waypoints.iter().enumerate() {
        if let Some(control) = waypoint.outgoing
            && let Some(next) = points.get(i + 1).copied()
            && let Some(curve) = curves.get_mut(i)
        {
            curve.p1 = clamp_handle(points[i], next, control);
        }
        if let Some(control) = waypoint.incoming
            && let Some(curve) = curves.get_mut(i.wrapping_sub(1))
            && i > 0
        {
            curve.p2 = clamp_handle(points[i - 1], points[i], control);
        }
    }
    let sampled = path::sample(&curves, 18);
    path::Track::new(sampled)
}

fn clamp_handle(anchor: Vec2, other: Vec2, control: Vec2) -> Vec2 {
    let limit = anchor.distance(other) * 2.0;
    anchor + (control - anchor).clamp_length_max(limit)
}

/// True when the character stands on the piece: same footprint plus feet
/// near the platform top. The tolerance rides through small physics gaps.
pub fn is_riding(
    character_pos: Vec2,
    feet: f32,
    piece: &Piece,
    shape: &model::Shape,
) -> bool {
    if !model::hit_test(shape, piece, character_pos, 0.25) {
        return false;
    }
    let top = piece.elevation + shape.top();
    (feet - top).abs() < 0.35
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waypoint(x: f32) -> PlatformWaypoint {
        PlatformWaypoint::new(Vec2::new(x, 0.0))
    }

    #[test]
    fn modes_parse_and_label() {
        assert_eq!(PlatformMode::parse("pingpong"), Some(PlatformMode::PingPong));
        assert_eq!(PlatformMode::parse("loop"), Some(PlatformMode::Loop));
        assert_eq!(PlatformMode::parse("oneshot"), Some(PlatformMode::OneShot));
        assert_eq!(PlatformMode::parse("nope"), None);
        assert_eq!(PlatformMode::PingPong.label(), "ping-pong");
    }

    #[test]
    fn loop_wraps_around() {
        let (d, dir) = advance_platform(9.0, 1.0, 10.0, 2.0, 1.0, PlatformMode::Loop);
        assert!((d - 1.0).abs() < 1e-5);
        assert_eq!(dir, 1.0);
    }

    #[test]
    fn one_shot_clamps_at_the_end() {
        let (d, _) = advance_platform(9.0, 1.0, 10.0, 5.0, 1.0, PlatformMode::OneShot);
        assert_eq!(d, 10.0);
        let (d, _) = advance_platform(10.0, 1.0, 10.0, 5.0, 1.0, PlatformMode::OneShot);
        assert_eq!(d, 10.0);
    }

    #[test]
    fn ping_pong_bounces_at_both_ends() {
        let (d, dir) = advance_platform(9.0, 1.0, 10.0, 2.0, 1.0, PlatformMode::PingPong);
        assert!((d - 9.0).abs() < 1e-5, "{d}");
        assert_eq!(dir, -1.0);
        let (d, dir) = advance_platform(1.0, -1.0, 10.0, 2.0, 1.0, PlatformMode::PingPong);
        assert!((d - 1.0).abs() < 1e-5, "{d}");
        assert_eq!(dir, 1.0);
    }

    #[test]
    fn zero_speed_or_empty_track_holds_still() {
        let (d, _) = advance_platform(3.0, 1.0, 10.0, 0.0, 1.0, PlatformMode::Loop);
        assert_eq!(d, 3.0);
        let (d, _) = advance_platform(0.0, 1.0, 0.0, 1.0, 1.0, PlatformMode::Loop);
        assert_eq!(d, 0.0);
    }

    #[test]
    fn a_straight_track_has_the_right_length() {
        let track =
            build_platform_track(&[waypoint(0.0), waypoint(4.0), waypoint(8.0)]);
        assert!((track.total() - 8.0).abs() < 0.05, "total={}", track.total());
        let (pos, _) = track.at(4.0);
        assert!((pos.x - 4.0).abs() < 0.1, "{pos:?}");
    }

    #[test]
    fn manual_handles_reshape_the_curve() {
        let straight = build_platform_track(&[waypoint(0.0), waypoint(4.0)]);
        let bent = build_platform_track(&[
            PlatformWaypoint {
                pos: Vec2::ZERO,
                incoming: None,
                outgoing: Some(Vec2::new(0.0, 3.0)),
            },
            waypoint(4.0),
        ]);
        let (a, _) = straight.at(2.0);
        let (b, _) = bent.at(2.0);
        assert!((b.y - a.y).abs() > 0.2, "handle should bend it: {a:?} vs {b:?}");
    }

    #[test]
    fn rider_check_needs_footprint_and_height() {
        let block = model::shape_index("block").unwrap();
        let shape = &model::shapes()[block];
        let piece = Piece {
            id: 1,
            shape: block,
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
        };
        let top = shape.top();
        assert!(is_riding(Vec2::ZERO, top, &piece, shape));
        assert!(!is_riding(Vec2::new(5.0, 5.0), top, &piece, shape));
        assert!(!is_riding(Vec2::ZERO, top + 2.0, &piece, shape));
    }
}
