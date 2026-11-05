use super::*;

#[test]
fn deleting_removes_every_selected_piece_in_one_step() {
    let mut editor = Editor {
        snap: false,
        ..Default::default()
    };
    for id in 1..=3 {
        editor.pieces.push(test_piece(id));
    }
    editor.selection.set_all([1, 3]);

    delete_piece(&mut editor);

    assert_eq!(
        editor.pieces.iter().map(|piece| piece.id).collect::<Vec<_>>(),
        [2],
        "only the selected pieces go"
    );
    assert!(editor.selection.is_empty());
    assert_eq!(editor.undo.len(), 1, "one delete of a group is one undo step");
}

#[test]
fn deleting_with_nothing_selected_is_a_no_op() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));

    delete_piece(&mut editor);

    assert_eq!(editor.pieces.len(), 1);
    assert!(editor.undo.is_empty(), "no undo step for an empty delete");
}

#[test]
fn rotating_turns_every_selected_piece() {
    let step = model::ANCHOR_STEP;
    let mut editor = Editor::default();
    for id in 1..=3 {
        editor.pieces.push(test_piece(id));
    }
    editor.selection.set_all([1, 3]);

    rotate_piece(&mut editor, step);

    assert!((editor.pieces[0].rot - step).abs() < 1e-6, "primary turned");
    assert!((editor.pieces[1].rot).abs() < 1e-6, "unselected piece kept");
    assert!((editor.pieces[2].rot - step).abs() < 1e-6, "other member turned");
    assert_eq!(editor.undo.len(), 1, "one rotate of a group is one undo step");
}

#[test]
fn copying_copies_every_selected_piece_in_selection_order() {
    let mut editor = Editor::default();
    for id in 1..=3 {
        editor.pieces.push(test_piece(id));
    }
    editor.selection.set_all([3, 1]);

    copy_piece(&mut editor);

    assert_eq!(
        editor.clipboard.iter().map(|piece| piece.id).collect::<Vec<_>>(),
        [3, 1],
        "the clipboard keeps the primary first"
    );
}

#[test]
fn pasting_a_group_keeps_its_layout_and_selects_the_copies() {
    let mut editor = Editor {
        snap: false,
        next_id: 3,
        ..Default::default()
    };
    let mut floor = test_piece(1);
    floor.pos = model::hex_center(0, 0);
    let mut block = test_piece(2);
    block.pos = model::hex_center(1, 0);
    editor.pieces.push(floor);
    editor.pieces.push(block);
    let offset = block.pos - floor.pos;
    editor.selection.set_all([1, 2]);
    copy_piece(&mut editor);

    paste_piece(&mut editor, Some(Vec2::new(5.0, 6.0)));

    assert_eq!(editor.pieces.len(), 4, "both copied pieces pasted");
    let first = editor.pieces[2];
    let second = editor.pieces[3];
    assert_eq!(first.id, 3, "fresh ids in order");
    assert_eq!(second.id, 4);
    assert!(
        (first.pos - Vec2::new(5.0, 6.0)).length() < 1e-5,
        "the primary's copy lands on the cursor: {:?}",
        first.pos
    );
    assert!(
        (second.pos - first.pos).abs_diff_eq(offset, 1e-5),
        "the copied layout keeps its offsets: {:?}",
        second.pos - first.pos
    );
    assert_eq!(
        editor.selection.ids(),
        [3, 4],
        "the pasted group is selected, primary first"
    );
    assert_eq!(editor.undo.len(), 1, "a group paste is one undo step");
    assert_eq!(editor.clipboard.len(), 2, "the clipboard survives a paste");
}

#[test]
fn pasting_without_a_cursor_lands_the_group_beside_the_source() {
    let mut editor = Editor {
        snap: false,
        next_id: 3,
        ..Default::default()
    };
    let mut floor = test_piece(1);
    floor.pos = model::hex_center(0, 0);
    let mut block = test_piece(2);
    block.pos = model::hex_center(1, 0);
    editor.pieces.push(floor);
    editor.pieces.push(block);
    editor.selection.set_all([1, 2]);
    copy_piece(&mut editor);

    paste_piece(&mut editor, None);

    let pasted: Vec<Piece> = editor.pieces[2..].to_vec();
    assert_eq!(pasted.len(), 2);
    assert!(
        (pasted[0].pos - model::hex_center(1, 0)).length() < 1e-5,
        "one hex beside the primary source: {:?}",
        pasted[0].pos
    );
    assert!(
        (pasted[1].pos - model::hex_center(2, 0)).length() < 1e-5,
        "the rest keeps its offset: {:?}",
        pasted[1].pos
    );
}

#[test]
fn a_plain_click_outside_the_selection_replaces_it() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.pieces.push(test_piece(2));
    editor.selection.set_all([1, 2]);

    assert!(click_selection(&mut editor, 3, false), "a drag may start");

    assert_eq!(editor.selection.ids(), [3]);
}

#[test]
fn a_plain_click_inside_a_multi_selection_keeps_the_group() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.pieces.push(test_piece(2));
    editor.selection.set_all([1, 2]);

    assert!(
        click_selection(&mut editor, 2, false),
        "clicking a member still starts a drag"
    );

    assert_eq!(
        editor.selection.ids(),
        [1, 2],
        "the group survives so it can be dragged whole"
    );
}

#[test]
fn a_shift_click_toggles_membership_and_never_drags() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.pieces.push(test_piece(2));
    editor.selection.set(1);

    assert!(!click_selection(&mut editor, 2, true), "no drag on shift-click");
    assert_eq!(editor.selection.ids(), [2, 1], "the new piece joins first");

    assert!(!click_selection(&mut editor, 2, true));
    assert_eq!(editor.selection.ids(), [1], "shift-clicking it again drops it");

    assert!(!click_selection(&mut editor, 1, true));
    assert!(editor.selection.is_empty(), "the last piece can be dropped");
}

#[test]
fn a_drag_from_a_multi_selection_carries_every_other_piece() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.pieces.push(Piece {
        pos: Vec2::new(1.0, 0.0),
        ..test_piece(2)
    });
    editor.pieces.push(Piece {
        pos: Vec2::new(0.0, 2.0),
        ..test_piece(3)
    });
    editor.selection.set_all([2, 1, 3]);

    let anchor = *editor.pieces.iter().find(|piece| piece.id == 2).unwrap();
    let offsets = group_offsets(&editor, 2, anchor.pos);

    assert_eq!(
        offsets,
        vec![
            (1, Vec2::new(-1.0, 0.0)),
            (3, Vec2::new(-1.0, 2.0)),
        ],
        "every other member keeps its offset from the grabbed piece, primary order first"
    );
}

#[test]
fn a_single_selection_has_no_group_to_carry() {
    let mut editor = Editor::default();
    editor.pieces.push(test_piece(1));
    editor.selection.set(1);

    assert!(group_offsets(&editor, 1, Vec2::ZERO).is_empty());
}

#[test]
fn a_plain_click_drops_the_stamp_but_shift_keeps_it_armed() {
    assert!(
        !keeps_stamping(false, false),
        "a plain click hands the pointer back to selection"
    );
    assert!(keeps_stamping(true, false), "Shift keeps stamping");
    assert!(keeps_stamping(false, true), "the sticky touch Add keeps stamping");
}
