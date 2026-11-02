//! Touch triggers from pieces (spikes deal damage by default).
//!
//! A trigger fires while the character overlaps a piece's solid: same 2D
//! footprint (`hit_test`) plus vertical overlap. Damage is per hit with a
//! short cooldown so one touch does not drain the whole bar in a single
//! frame.
//!
//! Each placed piece carries its own [`model::Trigger`]: any object can deal
//! damage, and new effects (heal, bounce, message, ...) slot into
//! [`apply_trigger`] without touching the overlap test or the cooldown.

use super::*;

/// Seconds between damage ticks while staying on a hazard.
pub const DAMAGE_COOLDOWN_SECS: f64 = 1.0;
/// Approximate capsule height, for vertical overlap checks.
pub const CHARACTER_HEIGHT: f32 = 1.9;

#[derive(Resource)]
pub(super) struct DamageClock {
    pub(super) last: f64,
}

impl Default for DamageClock {
    fn default() -> Self {
        // Allow the very first touch to hurt immediately; otherwise a fresh
        // map would grant a free second on the spikes.
        Self {
            last: -DAMAGE_COOLDOWN_SECS * 10.0,
        }
    }
}

pub(super) fn damage_ready(now: f64, last: f64) -> bool {
    now - last >= DAMAGE_COOLDOWN_SECS
}

/// Trigger of the first piece overlapping `(pos, feet)`, if any.
///
/// The check is generous by the capsule radius: the physics collider stops
/// the character at the piece's edge, so an exact-centre test would never
/// fire while walking into it.
pub(crate) fn trigger_at(pos: Vec2, feet: f32, pieces: &[Piece]) -> Option<model::Trigger> {
    let head = feet + CHARACTER_HEIGHT;
    for piece in pieces {
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        if piece.trigger.is_harmless() {
            continue;
        }
        if !model::hit_test(shape, piece, pos, PILL_RADIUS + 0.15) {
            continue;
        }
        let bottom = piece.elevation + shape.base;
        let top = piece.elevation + shape.top();
        // `+ 0.2` keeps standing on top of spikes hurting: feet == top
        // is still touch, while a real jump clears it.
        if head > bottom && feet < top + 0.2 {
            return Some(piece.trigger);
        }
    }
    None
}

/// Damage of the first trigger overlapping `(pos, feet)`, if any. Kept as the
/// narrow query so gameplay code and tests do not need to unpack the trigger.
#[allow(dead_code)] // exercised by tests; live code uses `trigger_at`.
pub(crate) fn hazard_at(pos: Vec2, feet: f32, pieces: &[Piece]) -> Option<f32> {
    trigger_at(pos, feet, pieces).map(|trigger| trigger.damage)
}

/// Apply one trigger's effects to the character. Today this is only damage;
/// add new `Trigger` fields here as extra lines (heal, bounce, ...).
pub(crate) fn apply_trigger(character: &mut Character, trigger: model::Trigger) {
    character.health.damage(trigger.damage);
    // Future effects land here, e.g.: `character.health.heal(trigger.heal);`
}

pub(super) fn hazard_damage(
    time: Res<Time>,
    mut clock: ResMut<DamageClock>,
    mut editor: ResMut<Editor>,
) {
    let now = time.elapsed_secs_f64();
    if !damage_ready(now, clock.last) {
        return;
    }
    let Some(snapshot) = editor.character else {
        return;
    };
    if snapshot.health.is_dead() {
        return;
    }
    let Some(trigger) = trigger_at(snapshot.pos, snapshot.feet, &editor.pieces) else {
        return;
    };
    let Some(character) = editor.character.as_mut() else {
        return;
    };
    apply_trigger(character, trigger);
    clock.last = now;
    editor.note = "ouch!";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SPIKE_DAMAGE;

    fn spikes_piece(id: u32, pos: Vec2, elevation: f32) -> Piece {
        Piece {
            id,
            shape: model::shape_index("spikes").unwrap(),
            pos,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation,
            dynamic: false,
            trigger: crate::model::Trigger::damaging(crate::model::SPIKE_DAMAGE),
        }
    }

    fn floor_piece(id: u32) -> Piece {
        Piece {
            id,
            shape: model::shape_index("floor").unwrap(),
            pos: Vec2::ZERO,
            rot: 0.0,
            pitch: 0.0,
            roll: 0.0,
            elevation: 0.0,
            dynamic: false,
            trigger: crate::model::Trigger::harmless(),
        }
    }

    #[test]
    fn spikes_deal_ten_damage() {
        assert_eq!(SPIKE_DAMAGE, 10.0);
        let shape = &shapes()[model::shape_index("spikes").unwrap()];
        assert_eq!(shape.damage, 10.0);
        assert_eq!(shape.category, "Hazard");
    }

    #[test]
    fn harmless_pieces_deal_nothing() {
        let pieces = vec![floor_piece(1)];
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &pieces), None);
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &[]), None);
    }

    #[test]
    fn standing_on_spikes_hurts() {
        let pieces = vec![spikes_piece(1, Vec2::ZERO, 0.0)];
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &pieces), Some(10.0));
    }

    #[test]
    fn far_from_spikes_is_safe() {
        let pieces = vec![spikes_piece(1, Vec2::ZERO, 0.0)];
        assert_eq!(hazard_at(Vec2::new(5.0, 5.0), 0.0, &pieces), None);
    }

    #[test]
    fn touching_the_edge_still_hurts() {
        // Physics stops the capsule at r + capsule radius (~0.77); the touch
        // check must be that generous or walking into spikes never fires.
        let pieces = vec![spikes_piece(1, Vec2::ZERO, 0.0)];
        assert_eq!(hazard_at(Vec2::new(0.7, 0.0), 0.0, &pieces), Some(10.0));
        assert_eq!(hazard_at(Vec2::new(0.0, -0.7), 0.0, &pieces), Some(10.0));
    }

    #[test]
    fn damage_clock_allows_an_immediate_first_hit() {
        let clock = DamageClock::default();
        assert!(damage_ready(0.0, clock.last));
    }

    #[test]
    fn jumping_over_spikes_is_safe() {
        let pieces = vec![spikes_piece(1, Vec2::ZERO, 0.0)];
        // Spikes are 0.5 tall; well above them is no touch.
        assert_eq!(hazard_at(Vec2::ZERO, 2.0, &pieces), None);
    }

    #[test]
    fn damage_cooldown_blocks_every_frame_hits() {
        assert!(!damage_ready(0.5, 0.0));
        assert!(damage_ready(1.0, 0.0));
        assert!(damage_ready(2.5, 1.0));
    }

    #[test]
    fn any_piece_can_deal_custom_damage() {
        let mut block = floor_piece(1);
        block.shape = model::shape_index("block").unwrap();
        block.trigger = model::Trigger::damaging(5.0);
        let pieces = vec![block];
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &pieces), Some(5.0));
        assert_eq!(
            trigger_at(Vec2::ZERO, 0.0, &pieces),
            Some(model::Trigger::damaging(5.0))
        );
    }

    #[test]
    fn zeroed_spikes_are_harmless() {
        let mut spikes = spikes_piece(1, Vec2::ZERO, 0.0);
        spikes.trigger = model::Trigger::harmless();
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &[spikes]), None);
    }

    #[test]
    fn edited_spike_damage_is_used_verbatim() {
        let mut spikes = spikes_piece(1, Vec2::ZERO, 0.0);
        spikes.trigger.set_damage(25.0);
        assert_eq!(hazard_at(Vec2::ZERO, 0.0, &[spikes]), Some(25.0));
    }

    #[test]
    fn apply_trigger_damages_but_never_heals() {
        let character = |hp: f32| Character {
            pos: Vec2::ZERO,
            feet: 0.0,
            yaw: 0.0,
            vy: 0.0,
            hv: Vec2::ZERO,
            ride_vel: Vec2::ZERO,
            grounded: true,
            health: crate::statuses::Health::new(hp, 100.0),
        };
        let mut hurt = character(100.0);
        apply_trigger(&mut hurt, model::Trigger::damaging(30.0));
        assert_eq!(hurt.health.current, 70.0);
        let mut harmless = character(80.0);
        apply_trigger(&mut harmless, model::Trigger::harmless());
        assert_eq!(harmless.health.current, 80.0);
    }
}
