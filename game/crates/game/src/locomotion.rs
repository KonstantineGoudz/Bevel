//! Player locomotion state machine.
//!
//! Pure data + maths, no Bevy systems, so it stays unit-testable without a
//! window — same pattern as [`crate::statuses`]. Start here when animation,
//! sound, or gameplay needs to know *what the character is doing*:
//! standing still, walking, sprinting, sliding, rising, or falling.
//!
//! The classifier ([`classify`]) maps one physics snapshot to one
//! [`LocomotionState`]. The tracker ([`LocomotionMachine`]) remembers the
//! current state across frames so callers can react to *transitions*
//! (play a land sound, trigger a slide effect, reset a coyote timer).

use bevy::prelude::Component;

/// Below this horizontal speed the character counts as standing still, in
/// m/s. Above sensor noise but below any real walk (`WALK_SPEED` is 2.2).
pub const STATIONARY_MAX_SPEED: f32 = 0.35;

/// Above this horizontal speed grounded movement counts as running, in m/s.
/// Sits between the walk pace (2.2) and the sprint pace (5.5) so a normal
/// walk never flickers into running.
pub const RUN_MIN_SPEED: f32 = 3.2;

/// Vertical speed above this counts as rising, in m/s. A slope push while
/// climbing can read as a small positive `vy`; a real jump leaves at 7.0.
pub const RISE_MIN_SPEED: f32 = 0.5;

/// What the player character is doing right now.
///
/// Priority order (first match wins in [`classify`]): [`Dead`](Self::Dead),
/// then airborne ([`Jumping`](Self::Jumping) / [`Falling`](Self::Falling)),
/// then [`Sliding`](Self::Sliding), then grounded
/// ([`Stationary`](Self::Stationary) / [`Walking`](Self::Walking) /
/// [`Running`](Self::Running)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Component, Default)]
pub enum LocomotionState {
    /// Standing (almost) still on the ground.
    #[default]
    Stationary,
    /// On the ground, moving below [`RUN_MIN_SPEED`].
    Walking,
    /// On the ground, moving at or above [`RUN_MIN_SPEED`].
    Running,
    /// Landing slide: grounded with the slide flag held.
    Sliding,
    /// In the air and still moving up.
    Jumping,
    /// In the air and no longer moving up (apex, falling, descending slope).
    Falling,
    /// Out of health. Overrides every other state.
    Dead,
}

impl LocomotionState {
    /// Short lowercase label for status lines, logs, and debug UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stationary => "stationary",
            Self::Walking => "walking",
            Self::Running => "running",
            Self::Sliding => "sliding",
            Self::Jumping => "jumping",
            Self::Falling => "falling",
            Self::Dead => "dead",
        }
    }

    /// True for [`Jumping`](Self::Jumping) and [`Falling`](Self::Falling).
    pub fn is_airborne(self) -> bool {
        matches!(self, Self::Jumping | Self::Falling)
    }

    /// True while standing on something (including the slide).
    pub fn is_grounded(self) -> bool {
        matches!(
            self,
            Self::Stationary | Self::Walking | Self::Running | Self::Sliding
        )
    }

    /// True while covering ground: walking, running, or sliding.
    pub fn is_moving(self) -> bool {
        matches!(self, Self::Walking | Self::Running | Self::Sliding)
    }
}

impl std::fmt::Display for LocomotionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One physics snapshot to classify. Built from `Character` (or tests) so
/// the state machine never reaches into game resources itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocomotionInput {
    /// `true` while the capsule rests on walkable ground.
    pub grounded: bool,
    /// `true` while the landing-slide flag is active.
    pub sliding: bool,
    /// Horizontal speed in m/s, *including* any platform ride velocity.
    pub horizontal_speed: f32,
    /// Vertical velocity in m/s. Positive is up.
    pub vertical_velocity: f32,
    /// `true` while health is zero.
    pub dead: bool,
}

impl LocomotionInput {
    /// Build a snapshot from raw character fields.
    pub fn new(
        grounded: bool,
        sliding: bool,
        horizontal_speed: f32,
        vertical_velocity: f32,
        dead: bool,
    ) -> Self {
        Self {
            grounded,
            sliding,
            horizontal_speed,
            vertical_velocity,
            dead,
        }
    }
}

/// Map one snapshot to its state. Pure function: same input, same state.
///
/// NaN speeds fall back to the safest bucket (grounded NaN reads as
/// stationary rather than panicking or reporting a sprint).
pub fn classify(input: LocomotionInput) -> LocomotionState {
    if input.dead {
        return LocomotionState::Dead;
    }
    if !input.grounded {
        if input.vertical_velocity > RISE_MIN_SPEED {
            return LocomotionState::Jumping;
        }
        return LocomotionState::Falling;
    }
    if input.sliding {
        return LocomotionState::Sliding;
    }
    let speed = if input.horizontal_speed.is_finite() {
        input.horizontal_speed.max(0.0)
    } else {
        0.0
    };
    if speed < STATIONARY_MAX_SPEED {
        LocomotionState::Stationary
    } else if speed < RUN_MIN_SPEED {
        LocomotionState::Walking
    } else {
        LocomotionState::Running
    }
}

/// One state change, emitted by [`LocomotionMachine::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    /// State being left.
    pub from: LocomotionState,
    /// State being entered.
    pub to: LocomotionState,
}

/// Tracks the character's state across frames: current + previous + how
/// long the current state has held. Feed it [`classify`] output (or raw
/// [`LocomotionInput`]) every frame; when it returns `Some`, the state just
/// changed and gameplay/animation can react once.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct LocomotionMachine {
    /// State as of the last update.
    pub current: LocomotionState,
    /// State before `current`. Equals `current` until the first change.
    pub previous: LocomotionState,
    /// Seconds spent continuously in `current`.
    pub time_in_state: f32,
}

impl Default for LocomotionMachine {
    fn default() -> Self {
        Self::new(LocomotionState::Stationary)
    }
}

impl LocomotionMachine {
    /// Start tracking from a known state.
    pub fn new(initial: LocomotionState) -> Self {
        Self {
            current: initial,
            previous: initial,
            time_in_state: 0.0,
        }
    }

    /// Advance with an already-classified state. Non-finite or negative `dt`
    /// still applies the transition but never moves the clock backwards.
    pub fn update(&mut self, next: LocomotionState, dt: f32) -> Option<Transition> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if next == self.current {
            self.time_in_state += dt;
            return None;
        }
        let transition = Transition {
            from: self.current,
            to: next,
        };
        self.previous = self.current;
        self.current = next;
        self.time_in_state = dt;
        Some(transition)
    }

    /// Classify `input` and advance in one call.
    pub fn observe(&mut self, input: LocomotionInput, dt: f32) -> Option<Transition> {
        self.update(classify(input), dt)
    }

    /// True on the exact frame `state` is entered.
    pub fn just_entered(&self, state: LocomotionState, transition: Option<Transition>) -> bool {
        transition.is_some_and(|t| t.to == state)
    }

    /// Reset to a state, clearing history and the clock. Useful on respawn.
    pub fn reset(&mut self, state: LocomotionState) {
        self.current = state;
        self.previous = state;
        self.time_in_state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(grounded: bool, sliding: bool, speed: f32, vy: f32, dead: bool) -> LocomotionInput {
        LocomotionInput::new(grounded, sliding, speed, vy, dead)
    }

    #[test]
    fn dead_overrides_everything() {
        // Dead while sprinting on the ground, sliding, or flying: still dead.
        assert_eq!(
            classify(input(true, false, 5.0, 0.0, true)),
            LocomotionState::Dead
        );
        assert_eq!(
            classify(input(true, true, 5.0, 0.0, true)),
            LocomotionState::Dead
        );
        assert_eq!(
            classify(input(false, false, 5.0, 4.0, true)),
            LocomotionState::Dead
        );
    }

    #[test]
    fn airborne_splits_into_jumping_and_falling() {
        assert_eq!(
            classify(input(false, false, 1.0, 3.0, false)),
            LocomotionState::Jumping
        );
        assert_eq!(
            classify(input(false, false, 1.0, 0.0, false)),
            LocomotionState::Falling
        );
        assert_eq!(
            classify(input(false, false, 1.0, -4.0, false)),
            LocomotionState::Falling
        );
        // A small slope push does not read as a jump.
        assert_eq!(
            classify(input(false, false, 1.0, RISE_MIN_SPEED, false)),
            LocomotionState::Falling
        );
    }

    #[test]
    fn sliding_beats_grounded_speed() {
        assert_eq!(
            classify(input(true, true, 0.0, 0.0, false)),
            LocomotionState::Sliding
        );
        assert_eq!(
            classify(input(true, true, 9.0, 0.0, false)),
            LocomotionState::Sliding
        );
    }

    #[test]
    fn grounded_speed_buckets_are_stationary_walking_running() {
        assert_eq!(
            classify(input(true, false, 0.0, 0.0, false)),
            LocomotionState::Stationary
        );
        assert_eq!(
            classify(input(true, false, 0.2, 0.0, false)),
            LocomotionState::Stationary
        );
        assert_eq!(
            classify(input(true, false, 2.2, 0.0, false)),
            LocomotionState::Walking
        );
        assert_eq!(
            classify(input(true, false, RUN_MIN_SPEED - 0.01, 0.0, false)),
            LocomotionState::Walking
        );
        assert_eq!(
            classify(input(true, false, RUN_MIN_SPEED, 0.0, false)),
            LocomotionState::Running
        );
        assert_eq!(
            classify(input(true, false, 5.5, 0.0, false)),
            LocomotionState::Running
        );
    }

    #[test]
    fn non_finite_speeds_never_panic() {
        assert_eq!(
            classify(input(true, false, f32::NAN, 0.0, false)),
            LocomotionState::Stationary
        );
        assert_eq!(
            classify(input(true, false, f32::INFINITY, 0.0, false)),
            LocomotionState::Stationary
        );
        assert_eq!(
            classify(input(true, false, -3.0, 0.0, false)),
            LocomotionState::Stationary
        );
    }

    #[test]
    fn labels_and_predicates_cover_every_state() {
        let all = [
            LocomotionState::Stationary,
            LocomotionState::Walking,
            LocomotionState::Running,
            LocomotionState::Sliding,
            LocomotionState::Jumping,
            LocomotionState::Falling,
            LocomotionState::Dead,
        ];
        for state in all {
            assert!(!state.label().is_empty());
            assert_eq!(state.to_string(), state.label());
        }
        assert!(LocomotionState::Jumping.is_airborne());
        assert!(LocomotionState::Falling.is_airborne());
        assert!(!LocomotionState::Running.is_airborne());
        assert!(LocomotionState::Sliding.is_grounded());
        assert!(!LocomotionState::Falling.is_grounded());
        assert!(LocomotionState::Running.is_moving());
        assert!(!LocomotionState::Stationary.is_moving());
        assert!(!LocomotionState::Dead.is_moving());
    }

    #[test]
    fn machine_holds_state_and_accumulates_time() {
        let mut machine = LocomotionMachine::default();
        assert_eq!(machine.current, LocomotionState::Stationary);
        assert!(machine.update(LocomotionState::Stationary, 0.5).is_none());
        assert!((machine.time_in_state - 0.5).abs() < 1e-6);
        assert!(machine.update(LocomotionState::Stationary, 0.5).is_none());
        assert!((machine.time_in_state - 1.0).abs() < 1e-6);
    }

    #[test]
    fn machine_reports_transitions_once() {
        let mut machine = LocomotionMachine::default();
        let transition = machine
            .update(LocomotionState::Walking, 0.1)
            .expect("entering walking reports once");
        assert_eq!(transition.from, LocomotionState::Stationary);
        assert_eq!(transition.to, LocomotionState::Walking);
        assert_eq!(machine.previous, LocomotionState::Stationary);
        assert_eq!(machine.current, LocomotionState::Walking);
        // Same state next frame: no repeat event.
        assert!(machine.update(LocomotionState::Walking, 0.1).is_none());
        let repeat = machine.update(LocomotionState::Walking, 0.1);
        assert!(!machine.just_entered(LocomotionState::Walking, repeat));
    }

    #[test]
    fn machine_observes_raw_input_and_resets() {
        let mut machine = LocomotionMachine::default();
        let t = machine.observe(input(false, false, 0.0, 5.0, false), 0.016);
        assert_eq!(
            t,
            Some(Transition {
                from: LocomotionState::Stationary,
                to: LocomotionState::Jumping
            })
        );
        machine.reset(LocomotionState::Stationary);
        assert_eq!(machine.current, LocomotionState::Stationary);
        assert_eq!(machine.time_in_state, 0.0);
    }

    #[test]
    fn machine_ignores_bad_delta_time() {
        let mut machine = LocomotionMachine::default();
        machine.update(LocomotionState::Stationary, f32::NAN);
        assert_eq!(machine.time_in_state, 0.0);
        machine.update(LocomotionState::Stationary, -1.0);
        assert_eq!(machine.time_in_state, 0.0);
        // A transition still applies; only the clock is clamped.
        let t = machine.update(LocomotionState::Falling, f32::NAN);
        assert!(t.is_some());
        assert_eq!(machine.time_in_state, 0.0);
    }
}
