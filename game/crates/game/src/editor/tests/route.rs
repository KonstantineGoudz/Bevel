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
        selection: {
            let mut selection = Selection::default();
            selection.set(3);
            selection
        },
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
    assert!(editor.selection.is_empty());
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

/// Headless camera + viewport math (no renderer needed), mirroring the
/// touch gizmo tests: projection and viewport are consistent, which is all
/// the hit test needs.
fn viewport_camera(app: &mut App) {
    use bevy::camera::{RenderTargetInfo, Viewport};

    let eye = Vec3::new(6.0, 7.0, 6.0);
    let cam_transform = Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y);
    let mut camera = Camera::default();
    camera.viewport = Some(Viewport {
        physical_position: UVec2::ZERO,
        physical_size: UVec2::new(800, 600),
        depth: 0.0..1.0,
    });
    camera.computed.clip_from_view =
        Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 800.0 / 600.0, 0.1, 100.0);
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: UVec2::new(800, 600),
        scale_factor: 1.0,
    });
    app.world_mut()
        .spawn((camera, Camera3d::default(), cam_transform));
}

fn viewport_of(app: &mut App, point: Vec3) -> Vec2 {
    let world = app.world_mut();
    let mut query = world.query::<(&Camera, &Transform)>();
    let (camera, transform) = query.single(&*world).expect("camera");
    let global = GlobalTransform::from(*transform);
    camera
        .world_to_viewport(&global, point)
        .expect("point on screen")
}

#[derive(Resource)]
struct MoverClick {
    screen: Vec2,
    hit: bool,
}

fn probe_select_mover(
    mut editor: ResMut<Editor>,
    mut movers: ResMut<Movers>,
    camera: Single<(&Camera, &Transform)>,
    mut probe: ResMut<MoverClick>,
) {
    let (camera, transform) = *camera;
    let global = GlobalTransform::from(*transform);
    probe.hit = select_mover_handle(
        &mut editor,
        &mut movers,
        camera,
        &global,
        probe.screen,
        false,
    );
}

fn mover_click_hit(app: &mut App, screen: Vec2) -> bool {
    app.insert_resource(MoverClick { screen, hit: false });
    app.world_mut()
        .run_system_once(probe_select_mover)
        .expect("probe");
    app.world().resource::<MoverClick>().hit
}

#[test]
fn a_selected_waypoints_gizmo_shafts_grab_like_the_dot() {
    let mut app = App::new();
    app.init_resource::<Editor>();
    app.init_resource::<Movers>();
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.pieces.push(test_piece(7));
        editor.selection.set(7);
        editor.actor_tool = Some(ActorTool::Platform);
    }
    {
        let mut movers = app.world_mut().resource_mut::<Movers>();
        let mover = movers.mover_mut(7, Vec2::ZERO);
        mover
            .waypoints
            .push(crate::platforms::PlatformWaypoint::new(Vec2::X * 4.0));
        mover.rebuild();
        mover.selected = Some(0);
    }
    viewport_camera(&mut app);

    let y = 0.06;
    let dot_screen = viewport_of(&mut app, Vec3::new(0.0, y, 0.0));
    // Well out on the +X shaft, far past the dot's own hit radius.
    let shaft_screen = viewport_of(&mut app, Vec3::new(MOVER_GIZMO_REACH * 0.9, y, 0.0));
    assert!(
        shaft_screen.distance(dot_screen) > 10.0,
        "the probe must be outside the dot hit so only the shaft can match"
    );

    let world = app.world_mut();
    let mut camera_q = world.query::<&Camera>();
    let mut transform_q = world.query::<&Transform>();
    let hit = mover_handle_at(
        world.resource::<Editor>(),
        world.resource::<Movers>(),
        camera_q.single(world).expect("camera"),
        &GlobalTransform::from(*transform_q.single(world).expect("transform")),
        shaft_screen,
        10.0,
    );
    assert_eq!(
        hit,
        Some(PathHandle::Waypoint(0)),
        "the selected waypoint's gizmo shaft grabs like its dot"
    );
    drop(world);

    // Click-to-select records the waypoint instead of appending. Dots are
    // always live, so an unselected waypoint is picked up by its dot...
    app.world_mut()
        .resource_mut::<Movers>()
        .map
        .get_mut(&7)
        .expect("mover")
        .selected = None;
    assert!(mover_click_hit(&mut app, dot_screen));
    assert_eq!(
        app.world()
            .resource::<Movers>()
            .map
            .get(&7)
            .expect("mover")
            .selected,
        Some(0)
    );
    // ...while the shafts only grab once selected, so unselected waypoints
    // never gain phantom handles from a neighbour's gizmo.
    app.world_mut()
        .resource_mut::<Movers>()
        .map
        .get_mut(&7)
        .expect("mover")
        .selected = None;
    assert!(!mover_click_hit(&mut app, shaft_screen));
    // Empty space selects nothing and must not append.
    assert!(!mover_click_hit(&mut app, Vec2::new(5.0, 5.0)));
}

#[test]
fn play_routes_do_not_create_glade_paths_only_glade_path_tool_does() {
    let mut editor = Editor::default();
    editor.mode = Mode::Play;
    let track = path::Track::new(vec![Vec2::ZERO, Vec2::new(4.0, 0.0)]);
    let line: Vec<Vec3> = track.points().iter().map(|p| Vec3::new(p.x, 0.07, p.y)).collect();
    editor.route = Some(Route {
        track,
        trail: Vec::new(),
        line,
        distances: vec![0.0, 4.0],
        controls: vec![
            path::AnchorControls {
                incoming: None,
                outgoing: None,
            };
            2
        ],
        valid: true,
        hexes: 4,
    });
    let mut plan = Plan::default();
    plan.append(Vec2::ZERO, (0, 0), 0.0);
    plan.append(Vec2::new(4.0, 0.0), (2, 0), 0.0);
    editor.plan = Some(plan);

    // A moving platform also has waypoints in Movers, but is not a glade_path.
    let mut movers = Movers::default();
    let mut block = test_piece(1);
    block.shape = model::shape_index("block").expect("block");
    editor.pieces.push(block);
    let block_mover = movers.mover_mut(1, Vec2::ZERO);
    block_mover
        .waypoints
        .push(crate::platforms::PlatformWaypoint::new(Vec2::new(3.0, 0.0)));

    assert!(
        collect_glade_paths(&editor, &movers).is_empty(),
        "character routes, plans, and moving platforms must not spawn dirt/stone paths"
    );

    // Only a dedicated glade_path piece creates terrain path geometry.
    let mut path_piece = test_piece(2);
    path_piece.shape = model::shape_index("glade_path").expect("glade_path");
    path_piece.pos = Vec2::new(1.0, 1.0);
    editor.pieces.push(path_piece);
    let path_mover = movers.mover_mut(2, Vec2::new(1.0, 1.0));
    path_mover
        .waypoints
        .push(crate::platforms::PlatformWaypoint::new(Vec2::new(5.0, 1.0)));

    let paths = collect_glade_paths(&editor, &movers);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], vec![Vec2::new(1.0, 1.0), Vec2::new(5.0, 1.0)]);
}

#[test]
fn glade_path_exposes_and_drags_bezier_tangent_handles_in_curve_editor() {
    let mut app = physics_app();
    let path_shape = model::shape_index("glade_path").expect("glade_path");
    {
        let world = app.world_mut();
        let mut editor = world.resource_mut::<Editor>();
        let mut piece = test_piece(9);
        piece.shape = path_shape;
        editor.pieces.push(piece);
        editor.selection.set(9);
        editor.actor_tool = Some(ActorTool::Platform);
    }
    {
        let mut movers = app.world_mut().resource_mut::<Movers>();
        let mover = movers.mover_mut(9, Vec2::ZERO);
        mover
            .waypoints
            .push(crate::platforms::PlatformWaypoint::new(Vec2::new(6.0, 0.0)));
        mover.rebuild();
    }
    viewport_camera(&mut app);

    let controls = app
        .world()
        .resource::<Movers>()
        .map
        .get(&9)
        .expect("mover")
        .controls();
    let outgoing = controls[0].outgoing.expect("automatic outgoing tangent");
    let handle_screen = viewport_of(&mut app, Vec3::new(outgoing.x, 0.06, outgoing.y));

    let world = app.world_mut();
    let mut camera_q = world.query::<(&Camera, &Transform)>();
    let (camera, cam_transform) = camera_q.single(world).expect("camera");
    let hit = mover_handle_at(
        world.resource::<Editor>(),
        world.resource::<Movers>(),
        camera,
        &GlobalTransform::from(*cam_transform),
        handle_screen,
        10.0,
    );
    assert_eq!(
        hit,
        Some(PathHandle::Tangent {
            waypoint: 0,
            side: TangentSide::Outgoing,
        }),
        "glade_path tangent handles are grabbable in the curve editor"
    );

    // Drag the outgoing tangent handle off-axis and verify the glade_path spine curves.
    let bent_screen = viewport_of(&mut app, Vec3::new(2.0, 0.06, 3.0));
    let drag = PathDrag {
        pointer: PointerId::Mouse,
        handle: PathHandle::Tangent {
            waypoint: 0,
            side: TangentSide::Outgoing,
        },
    };
    let moved = {
        let world = app.world_mut();
        world.resource_scope(|world, mut editor: Mut<Editor>| {
            world.resource_scope(|world, mut movers: Mut<Movers>| {
                let mut camera_q = world.query::<(&Camera, &Transform)>();
                let (camera, cam_transform) = camera_q.single(world).expect("camera");
                apply_mover_drag(
                    &mut editor,
                    &mut movers,
                    drag,
                    bent_screen,
                    camera,
                    &GlobalTransform::from(*cam_transform),
                )
            })
        })
    };
    assert!(moved);

    let paths = collect_glade_paths(
        app.world().resource::<Editor>(),
        app.world().resource::<Movers>(),
    );
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].iter().any(|p| p.y > 0.4),
        "dragging the curve handle bends the glade_path geometry spine: {:?}",
        paths[0]
    );
}
