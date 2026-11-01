use super::*;

#[test]
fn plan_and_path_modes_choose_walk_and_append_correctly() {
    assert!(!click_starts_walk(true, false));
    assert!(click_starts_walk(true, true));
    assert!(click_starts_walk(false, false));
    assert!(click_starts_walk(false, true));
    assert!(append_for(true, false));
    assert!(append_for(false, true));
    assert!(!append_for(false, false));
}

#[test]
fn only_play_routes_on_a_click() {
    // Play in third/top view with a character: any click becomes a
    // waypoint, including one that lands on a block or ramp (Build would
    // select that prop instead).
    assert!(click_routes(true, ViewMode::Third, true));
    assert!(click_routes(true, ViewMode::Top, true));

    // Build must never start a walk from a click: that is selection.
    assert!(!click_routes(false, ViewMode::Third, true));
    assert!(!click_routes(false, ViewMode::Top, true));

    // First person or no character: no route.
    assert!(!click_routes(true, ViewMode::First, true));
    assert!(!click_routes(true, ViewMode::Third, false));
}

#[test]
fn entering_play_holsters_tools_but_keeps_the_level_and_walk() {
    let mut editor = Editor {
        tool: Some(0),
        actor_tool: Some(ActorTool::Path),
        selected: Some(3),
        walk: Some(Walk {
            distance: 0.4,
            stuck: 0.0,
        }),
        plan: Some(Plan::default()),
        ..Default::default()
    };

    set_mode(&mut editor, Mode::Play);

    assert_eq!(editor.mode, Mode::Play);
    assert!(editor.tool.is_none());
    assert!(editor.actor_tool.is_none());
    assert!(editor.selected.is_none());
    assert!(editor.walk.is_some(), "a live walk keeps going in play");
    assert!(editor.plan.is_some(), "the plan survives a mode switch");
}

#[test]
fn the_line_prefix_stops_at_the_current_sample() {
    let track = path::Track::new(vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
    let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
    let prefix = line_prefix(&track, &line, Vec2::new(1.1, 0.0));
    assert_eq!(prefix.len(), 2);
}

#[test]
fn successive_live_edits_keep_the_walked_prefix() {
    let a = Vec3::ZERO;
    let b = Vec3::X;
    let c = Vec3::X * 2.0;
    let d = Vec3::X * 3.0;
    let first_track = path::Track::new(vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
    let trail = walked_trail(&[], &first_track, &[a, b, c], Vec2::new(1.0, 0.0));
    assert_eq!(trail, vec![a, b]);

    let second_track = path::Track::new(vec![
        Vec2::new(1.0, 0.0),
        Vec2::new(2.0, 0.0),
        Vec2::new(3.0, 0.0),
    ]);
    let trail = walked_trail(&trail, &second_track, &[b, c, d], Vec2::new(2.0, 0.0));
    assert_eq!(trail, vec![a, b, c], "the first walked prefix is immutable");
}

#[test]
fn arrival_prunes_the_final_waypoint() {
    let mut editor = Editor::default();
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    editor.plan = Some(plan);
    editor.route = Some(test_route(true));
    prune_plan(&mut editor, 0.95);
    assert!(
        editor.plan.as_ref().is_none_or(|plan| plan.is_empty()),
        "the final waypoint must be pruned on arrival"
    );
    assert!(editor.route.as_ref().unwrap().distances.is_empty());
}

#[test]
fn a_live_rebuild_starts_at_the_current_track_point() {
    let route = Route {
        track: path::Track::new(vec![Vec2::ZERO, Vec2::new(4.0, 0.0)]),
        trail: Vec::new(),
        line: Vec::new(),
        distances: vec![4.0],
        controls: Vec::new(),
        valid: true,
        hexes: 2,
    };
    let walk = Walk {
        distance: 2.0,
        stuck: 0.0,
    };
    assert_eq!(
        rebuild_start(true, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
        Vec2::new(2.0, 0.0)
    );
    assert_eq!(
        rebuild_start(false, Some(&route), Some(&walk), Vec2::new(9.0, 9.0)),
        Vec2::new(9.0, 9.0)
    );
}

#[test]
fn a_refused_edit_keeps_the_old_plan() {
    let mut old = Plan::default();
    old.replace(Vec2::ZERO, (0, 0), 0.0);
    let mut new = old.clone();
    new.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    assert_eq!(
        choose_plan(Some(old.clone()), Some(new.clone()), false)
            .expect("old plan")
            .waypoints
            .len(),
        1
    );
    assert_eq!(
        choose_plan(Some(old), Some(new), true)
            .expect("new plan")
            .waypoints
            .len(),
        2
    );
}

#[test]
fn walk_is_enabled_only_for_a_stopped_valid_plan() {
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    let empty = Plan::default();
    assert!(walk_enabled(Some(&plan), Some(&test_route(true)), false));
    assert!(!walk_enabled(Some(&plan), Some(&test_route(true)), true));
    assert!(!walk_enabled(Some(&plan), Some(&test_route(false)), false));
    assert!(!walk_enabled(Some(&empty), Some(&test_route(true)), false));
    assert!(!walk_enabled(None, None, false));
}

#[test]
fn a_nav_rebuild_never_starts_or_resurrects_a_walk() {
    assert_eq!(rebuild_walk_state(false, true), (true, false));
    assert_eq!(rebuild_walk_state(true, true), (true, true));
    assert_eq!(rebuild_walk_state(false, false), (false, false));
    assert_eq!(rebuild_walk_state(true, false), (false, true));
}

#[test]
fn smoothing_clears_manual_tangents() {
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    plan.append(Vec2::new(2.0, 0.0), (1, 0), 0.0);
    plan.waypoints[0].outgoing = Some(Vec2::new(1.0, 1.0));
    plan.waypoints[1].incoming = Some(Vec2::new(1.0, 1.0));
    clear_tangents(&mut plan);
    assert!(
        plan.waypoints
            .iter()
            .all(|w| w.incoming.is_none() && w.outgoing.is_none())
    );
}

#[test]
fn leaving_plan_keeps_the_plan() {
    let mut editor = Editor::default();
    let mut plan = Plan::default();
    plan.replace(Vec2::ZERO, (0, 0), 0.0);
    editor.plan = Some(plan);
    editor.actor_tool = Some(ActorTool::Plan);
    editor.actor_tool = Some(ActorTool::Path);
    assert_eq!(editor.plan.as_ref().map(|p| p.waypoints.len()), Some(1));
}

#[test]
fn the_course_lines_up_for_a_walk() {
    let (pieces, character) = course();
    assert_eq!(pieces.len(), 4);

    let ramp = pieces
        .iter()
        .find(|p| shapes()[p.shape].id == "ramp")
        .unwrap();
    let block = pieces
        .iter()
        .find(|p| shapes()[p.shape].id == "block")
        .unwrap();

    // Ramp rotated 270 degrees rises along +x; its top edge and the block's
    // near face should meet at the same height with a small gap.
    let ramp_top_x = ramp.pos.x + 1.0;
    let block_near_x = block.pos.x - 0.9;
    assert!(
        (block_near_x - ramp_top_x).abs() < 0.25,
        "ramp top {ramp_top_x} vs block face {block_near_x}"
    );
    assert!((shapes()[ramp.shape].top() - shapes()[block.shape].top()).abs() < 1e-4);

    // Start before the ramp, facing +x.
    assert!(character.pos.x < ramp.pos.x);
    let forward = Vec2::new(-character.yaw.sin(), -character.yaw.cos());
    assert!(
        forward.x > 0.99,
        "course should start facing +x, got {forward}"
    );
}

#[test]
fn the_route_line_bridges_a_seam_but_falls_off_a_real_edge() {
    // Ramp/block tops at 0.9 with a narrow crack at x = 2.0.
    let cracked = |p: Vec2| Some(if (1.9..=2.1).contains(&p.x) { 0.0 } else { 0.9 });
    let seam = lift_heights(
        [
            Vec2::new(1.7, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.3, 0.0),
        ]
        .into_iter(),
        cracked,
        0.9,
    );
    assert_eq!(seam, vec![0.9, 0.9, 0.9], "the seam is bridged");

    // Past the end of the block there is no support within the footprint.
    let edge = |p: Vec2| Some(if p.x < 2.0 { 0.9 } else { 0.0 });
    let dropped = lift_heights(
        [
            Vec2::new(2.3, 0.0),
            Vec2::new(2.5, 0.0),
            Vec2::new(2.7, 0.0),
        ]
        .into_iter(),
        edge,
        0.9,
    );
    assert_eq!(dropped, vec![0.0, 0.0, 0.0], "a real edge still drops");
}

#[test]
fn the_route_line_smooths_the_step_onto_a_block() {
    let smoothed = smooth_heights(&[0.0, 0.0, 0.9, 0.9]);
    assert!((smoothed[0] - 0.0).abs() < 1e-4);
    assert!((smoothed[1] - 0.225).abs() < 1e-4);
    assert!((smoothed[2] - 0.675).abs() < 1e-4);
    assert!((smoothed[3] - 0.9).abs() < 1e-4);
}
