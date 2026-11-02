//! overlay systems for the level editor.

use super::*;

// ---------------------------------------------------------------------------
// Palette and status
// ---------------------------------------------------------------------------

pub(super) fn update_palette(
    editor: Res<Editor>,
    mut buttons: Query<(&PaletteButton, &mut BackgroundColor, &mut BorderColor)>,
) {
    for (button, mut bg, mut border) in &mut buttons {
        let on = match button.0 {
            PaletteAction::Build | PaletteAction::Play | PaletteAction::View => false,
            PaletteAction::Shape(index) => editor.tool == Some(index),
            PaletteAction::Character => editor.actor_tool == Some(ActorTool::Place),
            PaletteAction::Path => editor.actor_tool == Some(ActorTool::Path),
            PaletteAction::Plan => editor.actor_tool == Some(ActorTool::Plan),
            PaletteAction::Walk => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && walk_enabled(
                        editor.plan.as_ref(),
                        editor.route.as_ref(),
                        editor.walk.is_some(),
                    )
            }
            PaletteAction::Smooth => {
                editor.actor_tool == Some(ActorTool::Plan)
                    && editor.plan.as_ref().is_some_and(|plan| !plan.is_empty())
            }
            PaletteAction::Course => false,
            PaletteAction::CopyJson => false,
        };
        bg.0 = if on { BUTTON_ON } else { BUTTON_BG };
        *border = BorderColor::all(if on { BORDER_ON } else { BORDER });
    }
}

pub(super) fn sync_path_actions(
    editor: Res<Editor>,
    mut row: Single<&mut Node, With<PathActions>>,
) {
    let display = if editor.actor_tool == Some(ActorTool::Plan) {
        Display::Flex
    } else {
        Display::None
    };
    if row.display != display {
        row.display = display;
    }
}

pub(super) fn update_status(
    editor: Res<Editor>,
    cursor: Res<Cursor>,
    orbit: Res<Orbit>,
    outliner: Res<OutlinerState>,
    mut text: Single<&mut Text, With<StatusText>>,
) {
    let hex = if orbit.mode == ViewMode::First {
        editor.character.map(|c| {
            let (q, r) = model::pixel_to_hex(c.pos);
            format!(
                "hex {q},{r}  y {:.2} {}",
                c.feet,
                if c.grounded { "grounded" } else { "air" }
            )
        })
    } else {
        cursor.world.map(|p| {
            let (q, r) = model::pixel_to_hex(p);
            format!("hex {q},{r}")
        })
    }
    .unwrap_or_else(|| "-".to_string());

    let tool = editor.tool.map(|t| shapes()[t].name).unwrap_or("select");

    let tool = match editor.actor_tool {
        Some(ActorTool::Place) => "character",
        Some(ActorTool::Path) => "path",
        Some(ActorTool::Plan) => "plan",
        Some(ActorTool::Platform) => "motion",
        None => tool,
    };

    let plan = editor
        .plan
        .as_ref()
        .map(|plan| {
            format!(
                "   |   plan {}wp sel {} {}",
                plan.waypoints.len(),
                plan.selected
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                if editor.walk.is_some() {
                    "walking"
                } else {
                    "stopped"
                }
            )
        })
        .unwrap_or_default();

    let selection = if let Some(id) = editor.selected {
        editor.node_label(GraphNode::Piece(id))
    } else {
        match outliner.selection {
            GraphNode::Group(_) => editor.node_label(outliner.selection),
            GraphNode::Character => "character".to_owned(),
            _ => "none".to_owned(),
        }
    };

    let snap = editor
        .ghost
        .map(|g| match g.kind {
            SnapKind::Anchor => "anchor",
            SnapKind::Grid => "grid",
            SnapKind::Free => "free",
        })
        .unwrap_or("none");

    text.0 = format!(
        "{}   |   {} placed   |   mode: {}   |   tool: {}   |   view: {}{}{}   |   selected: {}   |   snap {}   |   {:.0} m   |   {}",
        hex,
        editor.pieces.len(),
        editor.mode.label(),
        tool,
        orbit.mode.label(),
        match editor.route.as_ref() {
            Some(route) if route.valid => format!(" (route {} hexes)", route.hexes),
            Some(_) => " (route invalid)".to_string(),
            None => String::new(),
        },
        plan,
        selection,
        snap,
        orbit.distance,
        editor.note,
    );
}
