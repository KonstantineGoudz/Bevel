//! Character statuses: health points and (later) conditions.
//!
//! Pure data + maths, no Bevy systems, so it stays unit-testable without a
//! window. Start here when adding stamina, poison, shields, etc: one small
//! struct per status, grouped under [`Statuses`] when the character needs
//! more than health.

use bevy::prelude::Component;

/// Full health for a freshly placed character.
pub const DEFAULT_MAX_HEALTH: f32 = 100.0;

/// Health points: `current` is always clamped to `[0, max]`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Default for Health {
    fn default() -> Self {
        Self::full(DEFAULT_MAX_HEALTH)
    }
}

impl Health {
    /// Full health with the given maximum. Non-positive maxima fall back to
    /// [`DEFAULT_MAX_HEALTH`] so a bad value can never make an immortal (or
    /// instantly dead) character.
    pub fn full(max: f32) -> Self {
        let max = if max.is_finite() && max > 0.0 {
            max
        } else {
            DEFAULT_MAX_HEALTH
        };
        Self { current: max, max }
    }

    /// Arbitrary starting health, clamped into range.
    pub fn new(current: f32, max: f32) -> Self {
        let mut health = Self::full(max);
        health.set_current(current);
        health
    }

    /// Fraction remaining in `[0, 1]`; `0` means dead.
    pub fn fraction(self) -> f32 {
        if self.max <= 0.0 {
            0.0
        } else {
            (self.current / self.max).clamp(0.0, 1.0)
        }
    }

    pub fn is_alive(self) -> bool {
        self.current > 0.0
    }

    pub fn is_dead(self) -> bool {
        !self.is_alive()
    }

    /// Subtract damage, flooring at zero. Negative amounts are ignored.
    pub fn damage(&mut self, amount: f32) {
        if amount.is_finite() && amount > 0.0 {
            self.current = (self.current - amount).max(0.0);
        }
    }

    /// Restore health, capping at max. Negative amounts are ignored.
    pub fn heal(&mut self, amount: f32) {
        if amount.is_finite() && amount > 0.0 {
            self.current = (self.current + amount).min(self.max);
        }
    }

    pub fn set_current(&mut self, value: f32) {
        if value.is_finite() {
            self.current = value.clamp(0.0, self.max);
        }
    }

    /// Change the maximum, keeping the current value in range.
    pub fn set_max(&mut self, max: f32) {
        if max.is_finite() && max > 0.0 {
            self.max = max;
            self.current = self.current.clamp(0.0, self.max);
        }
    }

    pub fn revive(&mut self) {
        if self.is_dead() {
            self.current = self.max;
        }
    }
}

/// All statuses attached to one character. Grows as new systems (stamina,
/// poison, shields, ...) land; health is the first.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Statuses {
    pub health: Health,
}

impl Statuses {
    pub fn full_health() -> Self {
        Self {
            health: Health::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_health_starts_at_max() {
        let health = Health::full(100.0);
        assert_eq!(health.current, 100.0);
        assert_eq!(health.max, 100.0);
        assert!(health.is_alive());
        assert!(!health.is_dead());
        assert!((health.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn damage_floors_at_zero_and_reports_death() {
        let mut health = Health::full(100.0);
        health.damage(30.0);
        assert_eq!(health.current, 70.0);
        assert!((health.fraction() - 0.7).abs() < 1e-6);
        health.damage(80.0);
        assert_eq!(health.current, 0.0);
        assert!(health.is_dead());
        assert_eq!(health.fraction(), 0.0);
    }

    #[test]
    fn heal_caps_at_max() {
        let mut health = Health::new(50.0, 100.0);
        health.heal(30.0);
        assert_eq!(health.current, 80.0);
        health.heal(50.0);
        assert_eq!(health.current, 100.0);
    }

    #[test]
    fn negative_or_nan_amounts_are_ignored() {
        let mut health = Health::full(100.0);
        health.damage(-10.0);
        health.heal(-10.0);
        health.damage(f32::NAN);
        assert_eq!(health.current, 100.0);
    }

    #[test]
    fn bad_maxima_fall_back_to_default() {
        assert_eq!(Health::full(0.0).max, DEFAULT_MAX_HEALTH);
        assert_eq!(Health::full(-5.0).max, DEFAULT_MAX_HEALTH);
        assert_eq!(Health::full(f32::INFINITY).max, DEFAULT_MAX_HEALTH);
    }

    #[test]
    fn set_max_keeps_current_in_range() {
        let mut health = Health::full(100.0);
        health.set_max(60.0);
        assert_eq!((health.max, health.current), (60.0, 60.0));
        health.set_max(-10.0);
        assert_eq!(health.max, 60.0, "bad maxima are ignored");
    }

    #[test]
    fn revive_only_works_from_zero() {
        let mut health = Health::new(0.0, 100.0);
        assert!(health.is_dead());
        health.revive();
        assert_eq!(health.current, 100.0);
        health.damage(10.0);
        health.revive();
        assert_eq!(health.current, 90.0, "revive must not heal the living");
    }
}
