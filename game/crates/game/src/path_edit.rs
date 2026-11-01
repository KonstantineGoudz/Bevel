//! Editable path state: the ordered waypoint plan and the mouse/touch
//! gesture state machine used to tap, select and drag handles.
//!
//! Everything here is pure so it can be tested without a window or an ECS
//! world. The Bevy integration lives in `editor.rs`.

use std::collections::HashSet;

use bevy::math::Vec2;
use bevy::picking::pointer::PointerId;

use crate::path;

/// A tap may drift this many logical pixels and still count as a tap.
pub const TAP_MAX_MOVE: f32 = 14.0;
/// A tap must finish within this many seconds.
pub const TAP_MAX_TIME: f64 = 0.35;
/// A waypoint is considered reached slightly before its exact arc length,
/// matching the walker's arrival tolerance.
pub const PRUNE_AHEAD: f32 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TangentSide {
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathHandle {
    Waypoint(u32),
    Tangent { waypoint: u32, side: TangentSide },
}

#[derive(Clone, Copy, Debug)]
pub struct PathDrag {
    pub pointer: PointerId,
    pub handle: PathHandle,
}

/// The editable waypoints of the active route, in order.
#[derive(Clone, Default)]
pub struct Plan {
    pub waypoints: Vec<path::Waypoint>,
    pub selected: Option<u32>,
    next_id: u32,
}

impl Plan {
    pub fn replace(&mut self, pos: Vec2, cell: path::Hex, height: f32) -> u32 {
        self.waypoints.clear();
        self.selected = None;
        self.append(pos, cell, height)
    }

    pub fn append(&mut self, pos: Vec2, cell: path::Hex, height: f32) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        self.waypoints.push(path::Waypoint {
            id,
            pos,
            cell,
            height,
            incoming: None,
            outgoing: None,
        });
        self.selected = Some(id);
        id
    }

    pub fn delete(&mut self, id: u32) -> bool {
        let Some(index) = self.waypoints.iter().position(|waypoint| waypoint.id == id) else {
            return false;
        };
        self.waypoints.remove(index);
        if self.selected == Some(id) {
            self.selected = None;
        }
        true
    }

    pub fn delete_selected(&mut self) -> bool {
        let Some(id) = self.selected else {
            return false;
        };
        self.delete(id)
    }

    pub fn select(&mut self, id: Option<u32>) {
        self.selected = id;
    }

    pub fn get(&self, id: u32) -> Option<&path::Waypoint> {
        self.waypoints.iter().find(|waypoint| waypoint.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut path::Waypoint> {
        self.waypoints.iter_mut().find(|waypoint| waypoint.id == id)
    }

    /// Drop leading waypoints the walker has already reached. Returns how many
    /// were removed so the caller can trim the matching distance/control data.
    pub fn prune(&mut self, distances: &[f32], walked: f32) -> usize {
        let mut removed = 0;
        while removed < self.waypoints.len() {
            let Some(distance) = distances.get(removed) else {
                break;
            };
            if *distance > walked + PRUNE_AHEAD {
                break;
            }
            removed += 1;
        }

        if removed > 0 {
            let selected_removed = self.selected.is_some_and(|id| {
                self.waypoints[..removed]
                    .iter()
                    .any(|waypoint| waypoint.id == id)
            });
            self.waypoints.drain(..removed);
            if selected_removed {
                self.selected = None;
            }
        }
        removed
    }

    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }
}

#[derive(Clone, Copy)]
struct Press {
    pointer: PointerId,
    screen: Vec2,
    time: f64,
}

/// Mouse/touch gesture ownership: taps are classified on release, and a
/// handle press owns the gesture until every pointer lifts.
#[derive(Default)]
pub struct Gesture {
    pressed: HashSet<PointerId>,
    multi: bool,
    press: Option<Press>,
    drag: Option<PathDrag>,
    last: Option<Vec2>,
    moved: f32,
    /// The curve edit becomes active only after intentional movement.
    active: bool,
    /// Camera/gesture capture lasts until every pointer lifts, even if the
    /// handle owner releases first.
    capture: bool,
}

impl Gesture {
    /// Record a press. Returns a drag when the press landed on a handle.
    pub fn press(
        &mut self,
        pointer: PointerId,
        screen: Vec2,
        time: f64,
        handle: Option<PathHandle>,
    ) -> Option<PathDrag> {
        let first = self.pressed.is_empty();
        self.pressed.insert(pointer);

        if self.pressed.len() > 1 {
            self.multi = true;
            self.press = None;
        }

        if self.drag.is_some() {
            return self.drag;
        }

        if let Some(handle) = handle {
            let drag = PathDrag { pointer, handle };
            self.drag = Some(drag);
            self.press = None;
            self.moved = 0.0;
            self.last = Some(screen);
            self.active = false;
            self.capture = true;
            return Some(drag);
        }

        if first {
            self.press = Some(Press {
                pointer,
                screen,
                time,
            });
            self.moved = 0.0;
            self.last = Some(screen);
        }

        self.drag
    }

    /// Update the active drag or tap candidate with a pointer movement.
    pub fn drag(&mut self, pointer: PointerId, screen: Vec2, delta: Vec2) {
        let owns_drag = self.drag.is_some_and(|drag| drag.pointer == pointer);
        let owns_press = self.press.is_some_and(|press| press.pointer == pointer);
        if owns_drag || owns_press {
            self.last = Some(screen);
            self.moved += delta.length();
            if owns_drag && self.moved > TAP_MAX_MOVE {
                self.active = true;
            }
        }
    }

    /// End a pointer. Returns the tap position only for a qualifying tap.
    pub fn release(&mut self, pointer: PointerId, time: f64) -> Option<Vec2> {
        self.pressed.remove(&pointer);

        let mut tap = None;
        if self.drag.is_some_and(|drag| drag.pointer == pointer) {
            self.drag = None;
            self.active = false;
        } else if let Some(press) = self.press
            && press.pointer == pointer
        {
            self.press = None;
            if !self.multi && self.moved <= TAP_MAX_MOVE && time - press.time <= TAP_MAX_TIME {
                tap = Some(press.screen);
            }
        }

        if self.pressed.is_empty() {
            self.reset();
        }
        tap
    }

    /// End a pointer without a tap (touch cancellation, lost focus).
    pub fn cancel(&mut self, pointer: PointerId) {
        self.pressed.remove(&pointer);
        if self.drag.is_some_and(|drag| drag.pointer == pointer) {
            self.drag = None;
            self.active = false;
        }
        if self.press.is_some_and(|press| press.pointer == pointer) {
            self.press = None;
        }
        if self.pressed.is_empty() {
            self.reset();
        }
    }

    /// True from a handle press until every pointer lifts. Camera gestures
    /// stay suppressed for the whole capture, even after the owner releases.
    pub fn dragging(&self) -> bool {
        self.capture || self.drag.is_some()
    }

    /// The handle is edited only after the pointer moves past the tap limit;
    /// a tap selects without changing an automatic tangent.
    pub fn drag_target(&self) -> Option<PathDrag> {
        self.active.then_some(self.drag).flatten()
    }

    pub fn screen(&self) -> Option<Vec2> {
        self.last
    }

    fn reset(&mut self) {
        self.multi = false;
        self.press = None;
        self.last = None;
        self.moved = 0.0;
        self.active = false;
        self.capture = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_clears_and_append_keeps_order() {
        let mut plan = Plan::default();
        let a = plan.replace(Vec2::new(1.0, 0.0), (0, 0), 0.0);
        let b = plan.append(Vec2::new(3.0, 0.0), (1, 0), 0.9);
        assert_eq!(
            plan.waypoints.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![a, b]
        );
        let c = plan.replace(Vec2::new(5.0, 0.0), (2, 0), 0.0);
        assert_eq!(plan.waypoints.len(), 1);
        assert_eq!(plan.waypoints[0].id, c);
        assert_ne!(c, a);
        assert_ne!(c, b);
    }

    #[test]
    fn delete_selected_removes_only_that_waypoint() {
        let mut plan = Plan::default();
        let a = plan.replace(Vec2::ZERO, (0, 0), 0.0);
        plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
        plan.select(Some(a));
        assert!(plan.delete_selected());
        assert!(!plan.waypoints.iter().any(|w| w.id == a));
        assert_eq!(plan.waypoints.len(), 1);
        assert_eq!(plan.selected, None);
    }

    #[test]
    fn deleting_the_only_waypoint_empties_the_plan() {
        let mut plan = Plan::default();
        let only = plan.replace(Vec2::ZERO, (0, 0), 0.0);
        assert!(plan.delete(only));
        assert!(plan.is_empty());
        assert_eq!(plan.selected, None);
    }

    #[test]
    fn prune_drops_passed_waypoints_at_the_arrival_tolerance() {
        let mut plan = Plan::default();
        plan.replace(Vec2::ZERO, (0, 0), 0.0);
        plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
        assert_eq!(plan.prune(&[1.0, 2.0], 0.0), 0);
        assert_eq!(plan.prune(&[1.0, 2.0], 1.0), 1);
        assert_eq!(plan.prune(&[2.0], 1.88), 1);
        assert!(plan.is_empty());
    }

    #[test]
    fn a_short_still_release_is_a_tap() {
        let mut g = Gesture::default();
        let p = PointerId::Touch(1);
        assert!(g.press(p, Vec2::new(10.0, 10.0), 1.0, None).is_none());
        g.drag(p, Vec2::new(12.0, 10.0), Vec2::new(2.0, 0.0));
        assert_eq!(g.release(p, 1.2), Some(Vec2::new(10.0, 10.0)));
    }

    #[test]
    fn a_moved_or_slow_release_is_not_a_tap() {
        let mut g = Gesture::default();
        let p = PointerId::Touch(1);
        g.press(p, Vec2::ZERO, 1.0, None);
        g.drag(p, Vec2::new(30.0, 0.0), Vec2::new(30.0, 0.0));
        assert_eq!(g.release(p, 1.1), None);

        let mut g = Gesture::default();
        g.press(p, Vec2::ZERO, 1.0, None);
        assert_eq!(g.release(p, 1.5), None);
    }

    #[test]
    fn a_second_finger_cancels_the_tap() {
        let mut g = Gesture::default();
        let one = PointerId::Touch(1);
        let two = PointerId::Touch(2);
        g.press(one, Vec2::ZERO, 1.0, None);
        g.press(two, Vec2::new(20.0, 0.0), 1.0, None);
        assert_eq!(g.release(one, 1.1), None);
        assert_eq!(g.release(two, 1.1), None);
    }

    #[test]
    fn a_handle_drag_never_emits_a_tap() {
        let mut g = Gesture::default();
        let p = PointerId::Touch(1);
        let drag = g
            .press(p, Vec2::ZERO, 1.0, Some(PathHandle::Waypoint(7)))
            .expect("drag");
        assert_eq!(drag.handle, PathHandle::Waypoint(7));
        g.drag(p, Vec2::new(2.0, 0.0), Vec2::new(2.0, 0.0));
        assert_eq!(g.release(p, 1.1), None);
        assert!(!g.dragging());
    }

    #[test]
    fn a_handle_tap_does_not_activate_the_drag() {
        let mut g = Gesture::default();
        let p = PointerId::Touch(1);
        g.press(p, Vec2::ZERO, 1.0, Some(PathHandle::Waypoint(7)));
        g.drag(p, Vec2::new(2.0, 0.0), Vec2::new(2.0, 0.0));
        assert!(
            g.drag_target().is_none(),
            "a tap must only select the waypoint"
        );
        g.drag(p, Vec2::new(30.0, 0.0), Vec2::new(28.0, 0.0));
        assert!(g.drag_target().is_some(), "a real drag edits the handle");
    }

    #[test]
    fn camera_capture_lasts_until_every_pointer_lifts() {
        let mut g = Gesture::default();
        let owner = PointerId::Touch(1);
        let other = PointerId::Touch(2);
        g.press(owner, Vec2::ZERO, 1.0, Some(PathHandle::Waypoint(7)));
        g.press(other, Vec2::new(20.0, 0.0), 1.0, None);
        g.release(owner, 1.1);
        assert!(g.dragging(), "the other finger still owns the gesture");
        assert!(g.drag_target().is_none());
        g.release(other, 1.1);
        assert!(!g.dragging());
    }

    #[test]
    fn a_cancelled_gesture_does_not_tap() {
        let mut g = Gesture::default();
        let p = PointerId::Touch(1);
        g.press(p, Vec2::ZERO, 1.0, None);
        g.cancel(p);
        assert!(!g.dragging());
    }
}
