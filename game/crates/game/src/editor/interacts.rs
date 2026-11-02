//! Player-triggered interactions with pieces (E key / Interact button).
//!
//! While collision triggers fire automatically on touch, an interaction fires
//! only when the player presses the interact key while standing near an
//! interactable piece. Each placed piece carries its own [`model::InteractAction`];
//! dispatch lives in [`apply_interact`], so adding a new effect means adding a
//! variant plus an arm there (and a `SavedPiece` entry plus a properties-panel
//! control), without touching the proximity test or the input plumbing.

use super::*;

/// How close (2D metres, piece centre to character) the character must be to
/// interact with a solid piece.
pub const INTERACT_RADIUS: f32 = 2.5;
/// Vertical slack for the reach check, in metres. The piece spans
/// `[elevation + base, elevation + top]`; the capsule spans
/// `[feet, feet + CHARACTER_HEIGHT]`.
pub const INTERACT_VERTICAL_SLACK: f32 = 1.0;

/// Nearest interactable piece within reach of `(pos, feet)`, if any.
///
/// Trigger-volume pieces use their own overlap test so large zones stay
/// usable; solid pieces use a flat centre-distance check plus a vertical-span
/// overlap. Returns the piece id and its action.
pub(crate) fn interactable_at(
    pos: Vec2,
    feet: f32,
    pieces: &[Piece],
) -> Option<(u32, InteractAction)> {
    let head = feet + CHARACTER_HEIGHT;
    let mut best: Option<(f32, u32, InteractAction)> = None;
    for piece in pieces {
        if piece.interact.is_none() {
            continue;
        }
        let Some(shape) = shapes().get(piece.shape) else {
            continue;
        };
        let within_reach = if shape.is_trigger {
            model::trigger_overlaps(piece, shape, pos, feet, head)
        } else {
            let bottom = piece.elevation + shape.base - INTERACT_VERTICAL_SLACK;
            let top = piece.elevation + shape.top() + INTERACT_VERTICAL_SLACK;
            if head < bottom || feet > top {
                false
            } else {
                piece.pos.distance(pos) <= INTERACT_RADIUS
            }
        };
        if !within_reach {
            continue;
        }
        let distance = piece.pos.distance(pos);
        let replace = best
            .as_ref()
            .is_none_or(|(best_distance, ..)| distance < *best_distance);
        if replace {
            best = Some((distance, piece.id, piece.interact));
        }
    }
    best.map(|(_, id, action)| (id, action))
}

/// Run one interaction and report the note to show. Pure over `editor` +
/// `movers` (no Bevy queries) so it stays unit-testable without a window.
pub(crate) fn apply_interact(
    editor: &mut Editor,
    movers: &mut Movers,
    piece_id: u32,
    action: InteractAction,
) -> &'static str {
    match action {
        InteractAction::None => "nothing to interact with",
        // Future actions land here as new arms, e.g.:
        // `InteractAction::Teleport { ... } => ...`
        InteractAction::TogglePause => {
            let Some(mover) = movers.map.get_mut(&piece_id) else {
                return "no motion path";
            };
            mover.paused = !mover.paused;
            editor.hard_dirty = true;
            if mover.paused {
                "motion paused"
            } else {
                "motion resumed"
            }
        }
    }
}

/// Play-mode interaction: E on desktop, the Interact touch button on phones.
/// Finds the nearest interactable within reach and runs its action.
pub(super) fn handle_interact(
    input: PlayerInput,
    mut touch: ResMut<TouchState>,
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
    focus: Res<InputFocus>,
    menu: Res<AddMenu>,
) {
    // Interactions are gameplay. Build mode configures them in the properties
    // panel instead.
    if editor.mode != Mode::Play {
        touch.interact = false;
        return;
    }
    // A focused property field or the open Add menu owns the keyboard.
    if focus.get().is_some() || menu.open {
        touch.interact = false;
        return;
    }
    let pressed = input.just_pressed(Action::Interact) || touch.interact;
    touch.interact = false;
    if !pressed {
        return;
    }
    let Some(character) = editor.character else {
        return;
    };
    if character.health.is_dead() {
        return;
    }
    let Some((id, action)) = interactable_at(character.pos, character.feet, &editor.pieces) else {
        editor.note = "nothing to interact with";
        return;
    };
    let note = apply_interact(&mut editor, &mut movers, id, action);
    editor.note = note;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::PlatformWaypoint;

    fn block_piece(id: u32, pos: Vec2) -> Piece {
        Piece {
            id,
            shape: model::shape_index("block").unwrap(),
            pos,
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
            interact: crate::model::InteractAction::None,
        }
    }

    fn interactable_block(id: u32, pos: Vec2) -> Piece {
        let mut piece = block_piece(id, pos);
        piece.interact = crate::model::InteractAction::TogglePause;
        piece
    }

    fn mover_with_path() -> Mover {
        let mut mover = Mover::new(Vec2::ZERO);
        mover.waypoints.push(PlatformWaypoint::new(Vec2::X * 4.0));
        mover.rebuild();
        mover
    }

    #[test]
    fn pieces_without_an_action_are_never_interactable() {
        let pieces = vec![block_piece(1, Vec2::ZERO)];
        assert_eq!(interactable_at(Vec2::ZERO, 0.0, &pieces), None);
        assert_eq!(interactable_at(Vec2::ZERO, 0.0, &[]), None);
    }

    #[test]
    fn a_nearby_interactable_is_found() {
        let pieces = vec![interactable_block(1, Vec2::ZERO)];
        assert_eq!(
            interactable_at(Vec2::ZERO, 0.0, &pieces),
            Some((1, crate::model::InteractAction::TogglePause))
        );
    }

    #[test]
    fn far_away_is_out_of_reach() {
        let pieces = vec![interactable_block(1, Vec2::ZERO)];
        assert_eq!(
            interactable_at(Vec2::new(INTERACT_RADIUS + 1.0, 0.0), 0.0, &pieces),
            None
        );
    }

    #[test]
    fn well_above_the_piece_is_out_of_reach() {
        let pieces = vec![interactable_block(1, Vec2::ZERO)];
        assert_eq!(interactable_at(Vec2::ZERO, 10.0, &pieces), None);
    }

    #[test]
    fn the_nearest_interactable_wins() {
        let pieces = vec![
            interactable_block(1, Vec2::new(2.0, 0.0)),
            interactable_block(2, Vec2::new(0.5, 0.0)),
        ];
        assert_eq!(
            interactable_at(Vec2::ZERO, 0.0, &pieces),
            Some((2, crate::model::InteractAction::TogglePause))
        );
    }

    #[test]
    fn toggle_pause_flips_both_ways_and_marks_the_session_dirty() {
        let mut editor = Editor::default();
        editor.pieces.push(interactable_block(1, Vec2::ZERO));
        let mut movers = Movers::default();
        movers.map.insert(1, mover_with_path());

        assert_eq!(
            apply_interact(
                &mut editor,
                &mut movers,
                1,
                crate::model::InteractAction::TogglePause
            ),
            "motion paused"
        );
        assert!(movers.map.get(&1).unwrap().paused);
        assert!(editor.hard_dirty);

        editor.hard_dirty = false;
        assert_eq!(
            apply_interact(
                &mut editor,
                &mut movers,
                1,
                crate::model::InteractAction::TogglePause
            ),
            "motion resumed"
        );
        assert!(!movers.map.get(&1).unwrap().paused);
        assert!(editor.hard_dirty);
    }

    #[test]
    fn toggle_pause_without_a_path_changes_nothing() {
        let mut editor = Editor::default();
        editor.pieces.push(interactable_block(1, Vec2::ZERO));
        let mut movers = Movers::default();
        assert_eq!(
            apply_interact(
                &mut editor,
                &mut movers,
                1,
                crate::model::InteractAction::TogglePause
            ),
            "no motion path"
        );
        assert!(!editor.hard_dirty);
    }

    #[test]
    fn no_action_reports_nothing_to_do() {
        let mut editor = Editor::default();
        let mut movers = Movers::default();
        assert_eq!(
            apply_interact(
                &mut editor,
                &mut movers,
                1,
                crate::model::InteractAction::None
            ),
            "nothing to interact with"
        );
    }
}
