//! Party Dungeon MCP server.
//!
//! Query world data, edit pieces, plan paths and control the player — over
//! MCP stdio. The single shared world lives in SurrealDB (the sim is
//! authoritative and hot-reloads edits); edits are also forwarded to the
//! game's live bridge when it is reachable so a running editor applies them
//! instantly.
//!
//! Environment (SurrealDB service role — same vars as the server crate):
//! - `DUNGEON_SURREAL_URL` / `DUNGEON_SURREAL_NS` / `DUNGEON_SURREAL_DB`
//! - `DUNGEON_SERVICE_USER` / `DUNGEON_SERVICE_PASS`
//! - `PARTY_DUNGEON_BRIDGE`: `127.0.0.1:6559` by default; empty disables live.

mod store;

use std::collections::HashSet;

use base64::Engine as _;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::stdio;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};

use party_dungeon::model;
use party_dungeon::path;
use party_dungeon::platforms::{self, PlatformMode};
use party_dungeon::session::{
    self, SavedCharacter, SavedGrass, SavedPiece, SavedPlatform, Snapshot,
};

use store::{Source, Store};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ok_text(value: serde_json::Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into()),
    )]))
}

fn tool_error(message: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

fn invalid(message: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(message.to_string(), None)
}

fn parse_source(source: Option<String>) -> Source {
    Source::parse(source.as_deref())
}

fn level_summary(snapshot: &Snapshot, live: bool) -> serde_json::Value {
    let pieces = snapshot.pieces.len();
    let bounds = snapshot.pieces.iter().fold(None::<[f32; 4]>, |acc, p| {
        Some(match acc {
            None => [p.x, p.z, p.x, p.z],
            Some([min_x, min_z, max_x, max_z]) => [
                min_x.min(p.x),
                min_z.min(p.z),
                max_x.max(p.x),
                max_z.max(p.z),
            ],
        })
    });
    let mut by_shape: std::collections::BTreeMap<&str, usize> = Default::default();
    for piece in &snapshot.pieces {
        *by_shape.entry(piece.shape.as_str()).or_default() += 1;
    }
    serde_json::json!({
        "world": WORLD_ID,
        "live": live,
        "version": snapshot.version,
        "nextId": snapshot.next_id,
        "pieces": pieces,
        "bounds": bounds,
        "byShape": by_shape,
        "platforms": snapshot.platforms.len(),
        "grassCells": snapshot.grass.len(),
        "grassSlows": snapshot.grass_slows,
        "groups": snapshot.groups.len(),
        "hasCharacter": snapshot.character.is_some(),
        "hasCamera": snapshot.camera.is_some(),
        "undoSteps": snapshot.undo.len(),
        "redoSteps": snapshot.redo.len(),
    })
}

fn piece_json(saved: &SavedPiece) -> serde_json::Value {
    serde_json::to_value(saved).unwrap_or(serde_json::Value::Null)
}

/// The single shared world as MCP tools report it.
const WORLD_ID: &str = "surrealdb:party_dungeon/main";

fn bridge_note(addr: Option<&str>) -> &'static str {
    if addr.is_some() {
        " (live bridge reachable)"
    } else {
        " (live bridge unreachable; db only)"
    }
}

// ---------------------------------------------------------------------------
// Tool params
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LevelArgs {
    /// "auto" (default) prefers the live game, "db" forces the database, "live" requires the bridge.
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListPiecesArgs {
    #[serde(default)]
    source: Option<String>,
    /// Only pieces of this shape id (e.g. "wall", "block", "spikes").
    #[serde(default)]
    shape: Option<String>,
    /// Only pieces in this catalogue category (Surface, Barrier, Traversal, ...).
    #[serde(default)]
    category: Option<String>,
    /// Bounding box filter [min_x, min_z, max_x, max_z].
    #[serde(default)]
    bbox: Option<[f32; 4]>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetPieceArgs {
    /// Piece id.
    id: u32,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema, Default)]
struct PieceFields {
    shape: Option<String>,
    x: Option<f32>,
    z: Option<f32>,
    elevation: Option<f32>,
    rot_deg: Option<f32>,
    pitch_deg: Option<f32>,
    roll_deg: Option<f32>,
    dynamic: Option<bool>,
    damage: Option<f32>,
    interact: Option<String>,
    trigger_width: Option<f32>,
    trigger_length: Option<f32>,
    trigger_height: Option<f32>,
    trigger_shape: Option<String>,
    trigger_alpha: Option<f32>,
    trigger_color: Option<String>,
    trigger_visible: Option<bool>,
    name: Option<String>,
    group: Option<u32>,
    locked: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddPieceArgs {
    /// Shape id, see list_shapes (e.g. "wall", "floor", "block", "ramp", "spikes").
    shape: String,
    /// World x in metres.
    x: f32,
    /// World z in metres.
    z: f32,
    #[serde(flatten)]
    extra: PieceFields,
    /// Forward to the live game too when reachable (default true).
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct UpdatePieceArgs {
    id: u32,
    #[serde(flatten)]
    patch: PieceFields,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DeletePiecesArgs {
    ids: Vec<u32>,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SetCharacterArgs {
    x: Option<f32>,
    z: Option<f32>,
    feet: Option<f32>,
    yaw_deg: Option<f32>,
    hp: Option<f32>,
    max_hp: Option<f32>,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PlayerControlArgs {
    /// teleport | damage | heal | set_hp | respawn | walk_to | stop
    action: String,
    x: Option<f32>,
    z: Option<f32>,
    feet: Option<f32>,
    yaw_deg: Option<f32>,
    amount: Option<f32>,
    hp: Option<f32>,
    max_hp: Option<f32>,
    /// For walk_to: stops in order, e.g. [{"x":1.0,"z":2.0}].
    #[serde(default)]
    waypoints: Option<Vec<WaypointArg>>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct WaypointArg {
    x: f32,
    z: f32,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PlatformArgs {
    /// Piece id carrying the path.
    piece: u32,
    /// Waypoints in order, e.g. [{"x":0.0,"z":0.0},{"x":4.0,"z":0.0}].
    waypoints: Vec<WaypointArg>,
    /// pingpong (default) | loop | oneshot
    #[serde(default)]
    mode: Option<String>,
    /// Travel speed m/s (default 1.5).
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    paused: Option<bool>,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ClearPlatformArgs {
    piece: u32,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct FindPathArgs {
    /// Start world position.
    from: WaypointArg,
    /// Goal world position.
    to: WaypointArg,
    /// Start height (feet) for ramp/block-top resolution. Default 0.
    #[serde(default)]
    from_height: Option<f32>,
    /// Goal height. Default 0.
    #[serde(default)]
    to_height: Option<f32>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PlanRouteArgs {
    /// Start world position (usually the character).
    start: WaypointArg,
    /// Waypoint stops in order.
    waypoints: Vec<WaypointArg>,
    #[serde(default)]
    start_height: Option<f32>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PaintGrassArgs {
    /// Cells to paint: [{"q":1,"r":2,"density":3}]. Density 1..=4; 0 erases.
    cells: Vec<GrassCellArg>,
    /// When true, grass slows movement and bends routes.
    #[serde(default)]
    grass_slows: Option<bool>,
    /// Erase every painted cell instead.
    #[serde(default)]
    clear_all: Option<bool>,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct GrassCellArg {
    q: i32,
    r: i32,
    density: u8,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ImportArgs {
    /// Path to a snapshot JSON or debug level.json file to load into the world.
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ExportArgs {
    #[serde(default)]
    source: Option<String>,
    /// Where to write the debug level.json. Defaults to `./world.level.json`.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ClearLevelArgs {
    /// Keep the character when clearing. Default false.
    #[serde(default)]
    keep_character: Option<bool>,
    #[serde(default)]
    live: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ScreenshotArgs {
    /// Optional file path to also save the PNG to (parent dirs are created).
    /// The frame is always returned inline as an image content block.
    #[serde(default)]
    path: Option<String>,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PartyDungeonMcp {
    store: Store,
    /// Serializes every level-file read-modify-write so pipelined tool
    /// calls cannot claim the same id or clobber each other.
    io: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[tool_router]
impl PartyDungeonMcp {
    fn new() -> Self {
        Self {
            store: Store::from_env(),
            io: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    async fn effective(
        &self,
        source: Source,
    ) -> Result<
        (
            Snapshot,
            bool,
            tokio::sync::MutexGuard<'_, ()>,
        ),
        McpError,
    > {
        let guard = self.io.lock().await;
        store::load_effective(&self.store, source)
            .await
            .map(|(snapshot, live)| (snapshot, live, guard))
            .map_err(invalid)
    }

    // -- queries --------------------------------------------------------

    #[tool(
        description = "List every placeable shape in the catalogue: ids, categories, snap modes, sizes."
    )]
    fn list_shapes(&self) -> String {
        let shapes: Vec<serde_json::Value> = model::shapes()
            .iter()
            .map(|shape| {
                let (geom_kind, size) = match &shape.geom {
                    model::Geometry::Hex { r } => ("hex", serde_json::json!({"r": r})),
                    model::Geometry::Bar { len, thick } => {
                        ("bar", serde_json::json!({"len": len, "thick": thick}))
                    }
                    model::Geometry::Rect { w, h } => ("rect", serde_json::json!({"w": w, "h": h})),
                    model::Geometry::Circle { r } => ("circle", serde_json::json!({"r": r})),
                    model::Geometry::Diamond { r } => ("diamond", serde_json::json!({"r": r})),
                    model::Geometry::Cross { r } => ("cross", serde_json::json!({"r": r})),
                    model::Geometry::Flag { r } => ("flag", serde_json::json!({"r": r})),
                    model::Geometry::Wedge { w, d, h } => {
                        ("wedge", serde_json::json!({"w": w, "d": d, "h": h}))
                    }
                };
                serde_json::json!({
                    "id": shape.id,
                    "name": shape.name,
                    "category": shape.category,
                    "snap": match shape.mode {
                        model::SnapMode::Edge => "edge",
                        model::SnapMode::Center => "center",
                    },
                    "geom": geom_kind,
                    "size": size,
                    "height": shape.height,
                    "base": shape.base,
                    "top": shape.top(),
                    "damage": shape.damage,
                    "isTrigger": shape.is_trigger,
                    "style": format!("{:?}", shape.style),
                })
            })
            .collect();
        serde_json::json!({
            "hexSize": model::HEX_SIZE,
            "categories": model::CATEGORIES,
            "shapes": shapes,
        })
        .to_string()
    }

    #[tool(
        description = "Get the shared world: summary counts plus every piece, platforms, and grass."
    )]
    async fn get_world(
        &self,
        Parameters(args): Parameters<LevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let mut doc = serde_json::to_value(&snapshot).map_err(invalid)?;
        // Undo/redo history is an authoring detail; report counts, not blobs.
        if let Some(obj) = doc.as_object_mut() {
            let undo_len = snapshot.undo.len();
            let redo_len = snapshot.redo.len();
            obj.insert("undo".to_owned(), serde_json::json!({"steps": undo_len}));
            obj.insert("redo".to_owned(), serde_json::json!({"steps": redo_len}));
        }
        ok_text(serde_json::json!({
            "summary": level_summary(&snapshot, live),
            "world": doc,
        }))
    }

    #[tool(description = "List pieces, optionally filtered by shape, category, or a bounding box.")]
    async fn list_pieces(
        &self,
        Parameters(args): Parameters<ListPiecesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let category_shapes: Option<HashSet<usize>> = args.category.as_deref().map(|category| {
            model::shapes()
                .iter()
                .enumerate()
                .filter(|(_, shape)| shape.category.eq_ignore_ascii_case(category))
                .map(|(index, _)| index)
                .collect()
        });
        let mut out = Vec::new();
        for piece in &snapshot.pieces {
            if let Some(shape) = &args.shape {
                if piece.shape != *shape {
                    continue;
                }
            }
            if let Some(allowed) = &category_shapes {
                let index = model::shape_index(&piece.shape).unwrap_or(usize::MAX);
                if !allowed.contains(&index) {
                    continue;
                }
            }
            if let Some([min_x, min_z, max_x, max_z]) = args.bbox {
                if piece.x < min_x || piece.z < min_z || piece.x > max_x || piece.z > max_z {
                    continue;
                }
            }
            out.push(piece_json(piece));
        }
        ok_text(serde_json::json!({"live": live, "count": out.len(), "pieces": out}))
    }

    #[tool(description = "Get one piece by id, with its shape info.")]
    async fn get_piece(
        &self,
        Parameters(args): Parameters<GetPieceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let Some(piece) = snapshot.pieces.iter().find(|p| p.id == args.id) else {
            return tool_error(format!("no piece with id {}", args.id));
        };
        let shape_info = model::shape_index(&piece.shape).map(|index| {
            let shape = &model::shapes()[index];
            serde_json::json!({
                "id": shape.id, "name": shape.name, "category": shape.category,
                "top": shape.top(), "damage": shape.damage, "isTrigger": shape.is_trigger,
            })
        });
        ok_text(
            serde_json::json!({"live": live, "piece": piece_json(piece), "shapeInfo": shape_info}),
        )
    }

    #[tool(description = "Get the player character (position, feet height, yaw, health).")]
    async fn get_character(
        &self,
        Parameters(args): Parameters<LevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        ok_text(serde_json::json!({"live": live, "character": snapshot.character}))
    }

    #[tool(description = "List moving-platform paths with their waypoint tracks.")]
    async fn get_platforms(
        &self,
        Parameters(args): Parameters<LevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let items: Vec<serde_json::Value> = snapshot
            .platforms
            .iter()
            .map(|platform| {
                let waypoints: Vec<platforms::PlatformWaypoint> = platform
                    .waypoints
                    .iter()
                    .map(|w| platforms::PlatformWaypoint {
                        pos: bevy_math::Vec2::new(w.x, w.z),
                        incoming: w.incoming.map(|[x, y]| bevy_math::Vec2::new(x, y)),
                        outgoing: w.outgoing.map(|[x, y]| bevy_math::Vec2::new(x, y)),
                    })
                    .collect();
                let closed = platform.mode == "loop";
                let length = if waypoints.len() >= 2 {
                    platforms::build_platform_track(&waypoints, closed).total()
                } else {
                    0.0
                };
                serde_json::json!({
                    "piece": platform.piece,
                    "mode": platform.mode,
                    "speed": platform.speed,
                    "paused": platform.paused,
                    "trackLength": length,
                    "waypoints": platform.waypoints,
                })
            })
            .collect();
        ok_text(serde_json::json!({"live": live, "count": items.len(), "platforms": items}))
    }

    #[tool(
        description = "Find the shortest walkable route between two world points (A*, walls/ramps/blocks aware)."
    )]
    async fn find_path(
        &self,
        Parameters(args): Parameters<FindPathArgs>,
    ) -> Result<CallToolResult, McpError> {
        if !args.from.x.is_finite()
            || !args.from.z.is_finite()
            || !args.to.x.is_finite()
            || !args.to.z.is_finite()
        {
            return Err(invalid("from/to must be finite"));
        }
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let pieces = store::pieces_for_nav(&snapshot);
        let from = bevy_math::Vec2::new(args.from.x, args.from.z);
        let to = bevy_math::Vec2::new(args.to.x, args.to.z);
        let from_height = args.from_height.unwrap_or(0.0);
        let to_height = args.to_height.unwrap_or(0.0);
        let start_cell = path::cell_under(&pieces, from, from_height)
            .unwrap_or_else(|| model::pixel_to_hex(from));
        let goal_cell =
            path::cell_under(&pieces, to, to_height).unwrap_or_else(|| model::pixel_to_hex(to));
        let (edges, hexes) = path::blocked(&pieces);
        let grass = store::grass_map(&snapshot);
        let slows = snapshot.grass_slows;
        let route = path::find_path_cost(start_cell, goal_cell, &edges, &hexes, |hex| {
            store::grass_step_cost(slows, &grass, hex)
        });
        match route {
            None => tool_error(format!(
                "no route from {start_cell:?} (cell of {:.2},{:.2}) to {goal_cell:?} (cell of {:.2},{:.2})",
                args.from.x, args.from.z, args.to.x, args.to.z
            )),
            Some(hexes) => {
                let points = path::waypoints(from, to, &hexes);
                ok_text(serde_json::json!({
                    "live": live,
                    "startCell": start_cell, "goalCell": goal_cell,
                    "hexes": hexes.iter().map(|(q, r)| serde_json::json!({"q": q, "r": r})).collect::<Vec<_>>(),
                    "waypoints": points.iter().map(|p| serde_json::json!({"x": p.x, "z": p.y})).collect::<Vec<_>>(),
                }))
            }
        }
    }

    #[tool(
        description = "Plan a smoothed multi-leg route through waypoint stops. Returns the walkable track."
    )]
    async fn plan_route(
        &self,
        Parameters(args): Parameters<PlanRouteArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.waypoints.is_empty() {
            return Err(invalid("waypoints must not be empty"));
        }
        for waypoint in std::iter::once(&args.start).chain(args.waypoints.iter()) {
            if !waypoint.x.is_finite() || !waypoint.z.is_finite() {
                return Err(invalid("waypoint coordinates must be finite"));
            }
        }
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let pieces = store::pieces_for_nav(&snapshot);
        let start = bevy_math::Vec2::new(args.start.x, args.start.z);
        let height = args.start_height.unwrap_or(0.0);
        let start_cell =
            path::cell_under(&pieces, start, height).unwrap_or_else(|| model::pixel_to_hex(start));
        let targets: Vec<path::Waypoint> = args
            .waypoints
            .iter()
            .map(|waypoint| {
                let pos = bevy_math::Vec2::new(waypoint.x, waypoint.z);
                let cell =
                    path::cell_under(&pieces, pos, 0.0).unwrap_or_else(|| model::pixel_to_hex(pos));
                path::Waypoint {
                    id: 0,
                    pos,
                    cell,
                    height: 0.0,
                    incoming: None,
                    outgoing: None,
                }
            })
            .collect();
        let (edges, hexes) = path::blocked(&pieces);
        let grass = store::grass_map(&snapshot);
        let slows = snapshot.grass_slows;
        match path::plan_route_cost(start, start_cell, &targets, &edges, &hexes, |hex| {
            store::grass_step_cost(slows, &grass, hex)
        }) {
            None => tool_error("at least one route leg is walled off"),
            Some(plan) => {
                let points: Vec<serde_json::Value> = plan
                    .track
                    .points()
                    .iter()
                    .step_by(6)
                    .map(|p| serde_json::json!({"x": p.x, "z": p.y}))
                    .collect();
                ok_text(serde_json::json!({
                    "live": live,
                    "length": plan.track.total(),
                    "hexes": plan.hexes,
                    "waypointDistances": plan.distances,
                    "track": points,
                }))
            }
        }
    }

    #[tool(
        description = "Validate the shared world: unknown shapes, bad geometry, unreachable history. Reports errors."
    )]
    async fn validate_world(
        &self,
        Parameters(args): Parameters<LevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let (pieces, platforms, has_character, undo, redo) = (
            snapshot.pieces.len(),
            snapshot.platforms.len(),
            snapshot.character.is_some(),
            snapshot.undo.len(),
            snapshot.redo.len(),
        );
        match snapshot.clone().validate() {
            Ok(_) => ok_text(serde_json::json!({
                "ok": true, "live": live, "world": WORLD_ID.to_string(),
                "pieces": pieces,
                "platforms": platforms,
                "hasCharacter": has_character,
                "undoSteps": undo,
                "redoSteps": redo,
            })),
            Err(error) => tool_error(format!("invalid world {}: {error:?}", WORLD_ID)),
        }
    }

    #[tool(description = "Describe the single shared world: piece and mover counts, live status.")]
    async fn describe_world(&self) -> Result<CallToolResult, McpError> {
        let _guard = self.io.lock().await;
        let summary = self.store.world_summary().await.map_err(invalid)?;
        ok_text(serde_json::json!({
            "world": WORLD_ID,
            "live": summary.live,
            "pieces": summary.pieces,
            "platforms": summary.platforms,
        }))
    }

    // -- mutations ------------------------------------------------------

    fn check_piece_fields(fields: &PieceFields) -> Result<(), McpError> {
        for (value, name) in [
            (fields.x, "x"),
            (fields.z, "z"),
            (fields.elevation, "elevation"),
            (fields.rot_deg, "rot_deg"),
            (fields.pitch_deg, "pitch_deg"),
            (fields.roll_deg, "roll_deg"),
        ] {
            if let Some(value) = value {
                if !value.is_finite() {
                    return Err(invalid(format!("{name} must be finite")));
                }
            }
        }
        if let Some(shape) = &fields.shape {
            if model::shape_index(shape).is_none() {
                return Err(invalid(format!("unknown shape: {shape}")));
            }
        }
        if let Some(damage) = fields.damage {
            if !damage.is_finite() || damage < 0.0 || damage > 10000.0 {
                return Err(invalid("damage must be within 0..=10000"));
            }
        }
        if let Some(interact) = &fields.interact {
            if model::InteractAction::parse(interact).is_none() {
                return Err(invalid(format!("unknown interact action: {interact}")));
            }
        }
        if let Some(shape) = &fields.trigger_shape {
            if model::TriggerShape::parse(shape).is_none() {
                return Err(invalid(format!(
                    "unknown trigger_shape: {shape} (box|sphere|cylinder)"
                )));
            }
        }
        if let Some(color) = &fields.trigger_color {
            if model::trigger_color_parse_hex(color).is_none() {
                return Err(invalid("trigger_color must be #RRGGBB"));
            }
        }
        Ok(())
    }

    fn apply_piece_fields(piece: &mut SavedPiece, fields: &PieceFields) {
        if let Some(shape) = &fields.shape {
            piece.shape = shape.clone();
        }
        if let Some(x) = fields.x {
            piece.x = x;
        }
        if let Some(z) = fields.z {
            piece.z = z;
        }
        if let Some(elevation) = fields.elevation {
            piece.elevation = elevation;
        }
        if let Some(rot) = fields.rot_deg {
            piece.rot_deg = rot;
        }
        if let Some(pitch) = fields.pitch_deg {
            piece.pitch_deg = pitch;
        }
        if let Some(roll) = fields.roll_deg {
            piece.roll_deg = roll;
        }
        if let Some(dynamic) = fields.dynamic {
            piece.dynamic = dynamic;
        }
        if let Some(damage) = fields.damage {
            piece.damage = Some(damage);
        }
        if let Some(interact) = &fields.interact {
            piece.interact = if interact.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(interact.clone())
            };
        }
        if let Some(w) = fields.trigger_width {
            piece.trigger_width = Some(w);
        }
        if let Some(l) = fields.trigger_length {
            piece.trigger_length = Some(l);
        }
        if let Some(h) = fields.trigger_height {
            piece.trigger_height = Some(h);
        }
        if let Some(shape) = &fields.trigger_shape {
            piece.trigger_shape = Some(shape.clone());
        }
        if let Some(alpha) = fields.trigger_alpha {
            piece.trigger_alpha = Some(alpha);
        }
        if let Some(color) = &fields.trigger_color {
            piece.trigger_color = Some(color.clone());
        }
        if let Some(visible) = fields.trigger_visible {
            piece.trigger_visible = Some(visible);
        }
        if let Some(name) = &fields.name {
            piece.name = if name.trim().is_empty() {
                None
            } else {
                Some(name.clone())
            };
        }
        if let Some(group) = fields.group {
            piece.group = Some(group);
        }
        if let Some(locked) = fields.locked {
            piece.locked = locked;
        }
    }

    #[tool(
        description = "Add an object to the shared world (any catalogue shape). Returns the created piece."
    )]
    async fn add_piece(
        &self,
        Parameters(args): Parameters<AddPieceArgs>,
    ) -> Result<CallToolResult, McpError> {
        if model::shape_index(&args.shape).is_none() {
            return Err(invalid(format!("unknown shape: {}", args.shape)));
        }
        Self::check_piece_fields(&args.extra)?;
        if !args.x.is_finite() || !args.z.is_finite() {
            return Err(invalid("x/z must be finite"));
        }
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        if let Some(group) = args.extra.group {
            if !snapshot.groups.iter().any(|g| g.id == group) {
                return Err(invalid(format!("unknown group id: {group}")));
            }
        }
        let id = store::claim_id(&snapshot);
        let mut piece = SavedPiece {
            id,
            shape: args.shape.clone(),
            x: args.x,
            z: args.z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: None,
            interact: None,
            trigger_width: None,
            trigger_length: None,
            trigger_height: None,
            trigger_shape: None,
            trigger_alpha: None,
            trigger_color: None,
            trigger_visible: None,
            name: None,
            group: None,
            locked: false,
        };
        Self::apply_piece_fields(&mut piece, &args.extra);
        snapshot.pieces.push(piece.clone());
        store::reassign_next_id(&mut snapshot);
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        // Forward live so a running game spawns it instantly.
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command = serde_json::json!({"op":"add_piece","piece": piece});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        let mut doc =
            serde_json::json!({"piece": piece_json(&piece), "world": WORLD_ID.to_string()});
        doc["note"] =
            serde_json::Value::String(format!("added {}{}", store::describe_piece(&piece), suffix));
        ok_text(doc)
    }

    #[tool(
        description = "Move, rotate, or reconfigure a piece by id. Only supplied fields change."
    )]
    async fn update_piece(
        &self,
        Parameters(args): Parameters<UpdatePieceArgs>,
    ) -> Result<CallToolResult, McpError> {
        Self::check_piece_fields(&args.patch)?;
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        if let Some(group) = args.patch.group {
            if !snapshot.groups.iter().any(|g| g.id == group) {
                return Err(invalid(format!("unknown group id: {group}")));
            }
        }
        let Some(piece) = snapshot.pieces.iter_mut().find(|p| p.id == args.id) else {
            return tool_error(format!("no piece with id {}", args.id));
        };
        Self::apply_piece_fields(piece, &args.patch);
        let updated = piece.clone();
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command =
                    serde_json::json!({"op":"update_piece","id": args.id, "piece": updated});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        ok_text(serde_json::json!({
            "piece": piece_json(&updated),
            "note": format!("updated {}{}", store::describe_piece(&updated), suffix),
        }))
    }

    #[tool(description = "Delete one or more pieces by id (also drops their platform paths).")]
    async fn delete_pieces(
        &self,
        Parameters(args): Parameters<DeletePiecesArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.ids.is_empty() {
            return Err(invalid("ids must not be empty"));
        }
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        let before = snapshot.pieces.len();
        let ids: HashSet<u32> = args.ids.iter().copied().collect();
        snapshot.pieces.retain(|p| !ids.contains(&p.id));
        snapshot.platforms.retain(|p| !ids.contains(&p.piece));
        let removed = before - snapshot.pieces.len();
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command = serde_json::json!({"op":"delete_pieces","ids": args.ids});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        ok_text(
            serde_json::json!({"removed": removed, "note": format!("deleted {removed} piece(s){suffix}")}),
        )
    }

    #[tool(
        description = "Remove every piece (and platform path). Keeps the character unless asked otherwise."
    )]
    async fn clear_world(
        &self,
        Parameters(args): Parameters<ClearLevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        let removed = snapshot.pieces.len();
        snapshot.pieces.clear();
        snapshot.platforms.clear();
        snapshot.grass.clear();
        if !args.keep_character.unwrap_or(false) {
            snapshot.character = None;
        }
        store::reassign_next_id(&mut snapshot);
        let note = self.store
            .save_world(&snapshot, "mcp")
            .await
            .map_err(invalid)?;
        let _ = args.live;
        ok_text(
            serde_json::json!({"removed": removed, "note": format!("{note}; live games pick it up via the sim within seconds")}),
        )
    }

    #[tool(
        description = "Place or move the player character: position, feet height, facing, health."
    )]
    async fn set_character(
        &self,
        Parameters(args): Parameters<SetCharacterArgs>,
    ) -> Result<CallToolResult, McpError> {
        for (value, name) in [
            (args.x, "x"),
            (args.z, "z"),
            (args.feet, "feet"),
            (args.yaw_deg, "yaw_deg"),
        ] {
            if let Some(value) = value {
                if !value.is_finite() {
                    return Err(invalid(format!("{name} must be finite")));
                }
            }
        }
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        let mut character = snapshot.character.clone().unwrap_or(SavedCharacter {
            x: 0.0,
            z: 0.0,
            feet: 0.0,
            yaw_deg: 0.0,
            hp: Some(100.0),
            max_hp: Some(100.0),
        });
        if let Some(x) = args.x {
            character.x = x;
        }
        if let Some(z) = args.z {
            character.z = z;
        }
        if let Some(feet) = args.feet {
            character.feet = feet;
        }
        if let Some(yaw) = args.yaw_deg {
            character.yaw_deg = yaw;
        }
        if let Some(max_hp) = args.max_hp {
            if !max_hp.is_finite() || max_hp <= 0.0 {
                return Err(invalid("max_hp must be positive"));
            }
            character.max_hp = Some(max_hp);
        }
        if let Some(hp) = args.hp {
            if !hp.is_finite() {
                return Err(invalid("hp must be finite"));
            }
            character.hp = Some(hp);
        }
        snapshot.character = Some(character.clone());
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command = serde_json::json!({"op":"set_character","character": character});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        ok_text(serde_json::json!({
            "character": character,
            "note": format!("character at ({:.2}, {:.2}){suffix}", character.x, character.z),
        }))
    }

    #[tool(description = "Remove the player character.")]
    async fn remove_character(
        &self,
        Parameters(args): Parameters<LevelArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        snapshot.character = None;
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        if let Some(addr) = self.store.bridge_addr() {
            let command = serde_json::json!({"op":"remove_character"});
            let _ = store::bridge_command_json(addr, &command).await;
        }
        ok_text(serde_json::json!({"removed": true}))
    }

    #[tool(
        description = "Control the player: teleport, damage, heal, set_hp, respawn, walk_to (live), stop. File edits apply to the saved character."
    )]
    async fn player_control(
        &self,
        Parameters(args): Parameters<PlayerControlArgs>,
    ) -> Result<CallToolResult, McpError> {
        let action = args.action.to_ascii_lowercase();
        let addr = self.store.bridge_addr().map(|s| s.to_owned());
        // Live-first actions.
        if let Some(addr) = &addr {
            let command = match action.as_str() {
                "teleport" => serde_json::json!({
                    "op": "teleport", "x": args.x, "z": args.z,
                    "feet": args.feet, "yaw_deg": args.yaw_deg,
                }),
                "damage" => serde_json::json!({
                    "op": "damage",
                    "amount": args.amount.unwrap_or(10.0),
                }),
                "heal" => serde_json::json!({
                    "op": "heal",
                    "amount": args.amount.unwrap_or(25.0),
                }),
                "set_hp" => serde_json::json!({
                    "op": "set_hp", "hp": args.hp, "max_hp": args.max_hp,
                }),
                "respawn" => serde_json::json!({"op": "respawn"}),
                "walk_to" => {
                    let waypoints = args.waypoints.clone().unwrap_or_default();
                    if waypoints.is_empty() {
                        return Err(invalid("walk_to needs waypoints"));
                    }
                    serde_json::json!({"op": "walk_to", "waypoints": waypoints})
                }
                "stop" => serde_json::json!({"op": "stop"}),
                _ => {
                    return Err(invalid(
                        "action must be teleport|damage|heal|set_hp|respawn|walk_to|stop",
                    ));
                }
            };
            match store::bridge_command_json(addr, &command).await {
                Ok(note) => {
                    // Mirror teleports/health into the file so they persist.
                    if matches!(
                        action.as_str(),
                        "teleport" | "damage" | "heal" | "set_hp" | "respawn"
                    ) {
                        if let Ok((mut snapshot, _, _guard)) =
                            self.effective(Source::Db).await
                        {
                            let mut character =
                                snapshot.character.clone().unwrap_or(SavedCharacter {
                                    x: 0.0,
                                    z: 0.0,
                                    feet: 0.0,
                                    yaw_deg: 0.0,
                                    hp: Some(100.0),
                                    max_hp: Some(100.0),
                                });
                            match action.as_str() {
                                "teleport" => {
                                    if let Some(x) = args.x {
                                        character.x = x;
                                    }
                                    if let Some(z) = args.z {
                                        character.z = z;
                                    }
                                    if let Some(feet) = args.feet {
                                        character.feet = feet;
                                    }
                                    if let Some(yaw) = args.yaw_deg {
                                        character.yaw_deg = yaw;
                                    }
                                }
                                "damage" => {
                                    let amount = args.amount.unwrap_or(10.0);
                                    let max = character.max_hp.unwrap_or(100.0);
                                    let current = character.hp.unwrap_or(max) - amount.max(0.0);
                                    character.hp = Some(current.max(0.0));
                                }
                                "heal" => {
                                    let amount = args.amount.unwrap_or(25.0);
                                    let max = character.max_hp.unwrap_or(100.0);
                                    let current = character.hp.unwrap_or(max) + amount.max(0.0);
                                    character.hp = Some(current.min(max));
                                }
                                "set_hp" => {
                                    if let Some(max_hp) = args.max_hp {
                                        character.max_hp = Some(max_hp);
                                    }
                                    if let Some(hp) = args.hp {
                                        character.hp = Some(hp);
                                    }
                                }
                                "respawn" => {
                                    character.hp = character.max_hp;
                                }
                                _ => {}
                            }
                            snapshot.character = Some(character);
                            let _ = self.store.save_world(&snapshot, "mcp").await;
                        }
                    }
                    return ok_text(
                        serde_json::json!({"live": true, "action": action, "note": note}),
                    );
                }
                Err(error) => {
                    if action == "walk_to" || action == "stop" {
                        return tool_error(format!(
                            "live bridge unreachable ({error}); walk_to/stop need the game running"
                        ));
                    }
                    // Fall through to the DB for persistent character state.
                }
            }
        } else if action == "walk_to" || action == "stop" {
            return tool_error("walk_to/stop need the live game (bridge disabled or unreachable)");
        }
        // DB fallback: persistent character state.
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        let mut character = snapshot.character.clone().unwrap_or(SavedCharacter {
            x: 0.0,
            z: 0.0,
            feet: 0.0,
            yaw_deg: 0.0,
            hp: Some(100.0),
            max_hp: Some(100.0),
        });
        match action.as_str() {
            "teleport" => {
                if let Some(x) = args.x {
                    if !x.is_finite() {
                        return Err(invalid("x must be finite"));
                    }
                    character.x = x;
                }
                if let Some(z) = args.z {
                    if !z.is_finite() {
                        return Err(invalid("z must be finite"));
                    }
                    character.z = z;
                }
                if let Some(feet) = args.feet {
                    character.feet = feet;
                }
                if let Some(yaw) = args.yaw_deg {
                    character.yaw_deg = yaw;
                }
            }
            "damage" => {
                let amount = args.amount.unwrap_or(10.0);
                let max = character.max_hp.unwrap_or(100.0);
                character.hp = Some((character.hp.unwrap_or(max) - amount.max(0.0)).max(0.0));
            }
            "heal" => {
                let amount = args.amount.unwrap_or(25.0);
                let max = character.max_hp.unwrap_or(100.0);
                character.hp = Some((character.hp.unwrap_or(max) + amount.max(0.0)).min(max));
            }
            "set_hp" => {
                if let Some(max_hp) = args.max_hp {
                    if !max_hp.is_finite() || max_hp <= 0.0 {
                        return Err(invalid("max_hp must be positive"));
                    }
                    character.max_hp = Some(max_hp);
                }
                if let Some(hp) = args.hp {
                    if !hp.is_finite() {
                        return Err(invalid("hp must be finite"));
                    }
                    character.hp = Some(hp);
                }
            }
            "respawn" => {
                character.hp = character.max_hp;
            }
            _ => {
                return Err(invalid(
                    "action must be teleport|damage|heal|set_hp|respawn|walk_to|stop",
                ));
            }
        }
        snapshot.character = Some(character.clone());
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        ok_text(serde_json::json!({
            "live": false,
            "action": action,
            "character": character,
            "note": format!("applied to file{}; game picks it up on load", bridge_note(addr.as_deref())),
        }))
    }

    #[tool(
        description = "Set a moving-platform path on a piece: waypoints, travel mode, speed, pause."
    )]
    async fn set_platform_path(
        &self,
        Parameters(args): Parameters<PlatformArgs>,
    ) -> Result<CallToolResult, McpError> {
        if args.waypoints.is_empty() {
            return Err(invalid("waypoints must not be empty"));
        }
        for waypoint in &args.waypoints {
            if !waypoint.x.is_finite() || !waypoint.z.is_finite() {
                return Err(invalid("waypoint coordinates must be finite"));
            }
        }
        let mode = args.mode.clone().unwrap_or_else(|| "pingpong".into());
        if PlatformMode::parse(&mode).is_none() {
            return Err(invalid("mode must be pingpong|loop|oneshot"));
        }
        let speed = args.speed.unwrap_or(1.5);
        if !speed.is_finite() || speed <= 0.0 {
            return Err(invalid("speed must be positive"));
        }
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        if !snapshot.pieces.iter().any(|p| p.id == args.piece) {
            return tool_error(format!("no piece with id {}", args.piece));
        }
        let platform = SavedPlatform {
            piece: args.piece,
            mode: mode.clone(),
            speed: Some(speed),
            paused: args.paused.unwrap_or(false),
            waypoints: args
                .waypoints
                .iter()
                .map(|w| session::SavedPlatformPoint {
                    x: w.x,
                    z: w.z,
                    incoming: None,
                    outgoing: None,
                })
                .collect(),
        };
        store::validate_platform_path(args.piece, &platform).map_err(invalid)?;
        if let Some(existing) = snapshot
            .platforms
            .iter_mut()
            .find(|p| p.piece == args.piece)
        {
            *existing = platform.clone();
        } else {
            snapshot.platforms.push(platform.clone());
        }
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command = serde_json::json!({"op":"set_platform","platform": platform});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        ok_text(serde_json::json!({
            "platform": store::platform_summary(&platform),
            "note": format!("platform path on piece #{}{}", args.piece, suffix),
        }))
    }

    #[tool(description = "Remove the moving-platform path from a piece (the piece itself stays).")]
    async fn clear_platform_path(
        &self,
        Parameters(args): Parameters<ClearPlatformArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (mut snapshot, _, _guard) =
            self.effective(Source::Db).await?;
        let before = snapshot.platforms.len();
        snapshot.platforms.retain(|p| p.piece != args.piece);
        self.store
            .save_world(&snapshot, "mcp").await
            .map_err(invalid)?;
        let mut suffix = String::new();
        if args.live.unwrap_or(true) {
            if let Some(addr) = self.store.bridge_addr() {
                let command = serde_json::json!({"op":"clear_platform","piece": args.piece});
                match store::bridge_command_json(addr, &command).await {
                    Ok(note) => suffix = format!(" live: {note}"),
                    Err(error) => suffix = format!(" live bridge not applied ({error})"),
                }
            }
        }
        ok_text(serde_json::json!({
            "removed": before - snapshot.platforms.len(),
            "note": format!("cleared platform path on piece #{}{}", args.piece, suffix),
        }))
    }

    #[tool(
        description = "Paint grass (currently editor-session-local only; the shared world does not persist grass yet — paint in the running editor)."
    )]
    async fn paint_grass(
        &self,
        Parameters(_args): Parameters<PaintGrassArgs>,
    ) -> Result<CallToolResult, McpError> {
        let _ = self.store.bridge_addr();
        Err(invalid(
            "grass is editor-session-local in the single-world MMO (see the shared world via describe_world); paint it in the running editor",
        ))
    }

    #[tool(
        description = "Import a snapshot JSON or debug level.json file into the shared world (validates first). This is how the world gets seeded or replaced."
    )]
    async fn import_level_json(
        &self,
        Parameters(args): Parameters<ImportArgs>,
    ) -> Result<CallToolResult, McpError> {
        let _guard = self.io.lock().await;
        let text = std::fs::read_to_string(&args.path)
            .map_err(|error| invalid(format!("could not read {}: {error}", args.path)))?;
        let snapshot = party_dungeon::session::parse_any_snapshot_text(&text)
            .map_err(|error| invalid(format!("invalid snapshot: {error:?}")))?;
        let note = self.store.save_world(&snapshot, "mcp-import").await.map_err(invalid)?;
        ok_text(serde_json::json!({
            "world": WORLD_ID.to_string(),
            "pieces": snapshot.pieces.len(),
            "platforms": snapshot.platforms.len(),
            "note": note,
        }))
    }

    #[tool(
        description = "Export the shared world in the debug level.json shape."
    )]
    async fn export_level_json(
        &self,
        Parameters(args): Parameters<ExportArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (snapshot, live, _guard) = self
            .effective(parse_source(args.source))
            .await?;
        let doc = store::export_level_json(&snapshot);
        let dest = match args.path {
            Some(path) => std::path::PathBuf::from(path),
            None => std::path::PathBuf::from("world.level.json"),
        };
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(invalid)?;
        }
        let pretty = serde_json::to_string_pretty(&doc).map_err(invalid)?;
        std::fs::write(&dest, pretty).map_err(invalid)?;
        ok_text(
            serde_json::json!({"live": live, "world": WORLD_ID.to_string(), "pieces": snapshot.pieces.len()}),
        )
    }

    #[tool(description = "Check whether the live game bridge is reachable.")]
    async fn bridge_status(&self) -> Result<CallToolResult, McpError> {
        let addr = self.store.bridge_addr().map(|addr| addr.to_owned());
        match addr {
            None => ok_text(
                serde_json::json!({"enabled": false, "note": "PARTY_DUNGEON_BRIDGE is empty"}),
            ),
            Some(addr) => {
                let snapshot = store::bridge_state(&addr).await;
                // `/health` reports headless render info (size/fps/frames and
                // whether `/screenshot` is available). Keep it null when the
                // bridge is down; never fail the probe because of it.
                let health = store::bridge_health(&addr).await;
                match snapshot {
                    Some(snapshot) => ok_text(serde_json::json!({
                        "enabled": true,
                        "reachable": true,
                        "addr": addr,
                        "pieces": snapshot.pieces.len(),
                        "hasCharacter": snapshot.character.is_some(),
                        "health": health,
                    })),
                    None => ok_text(serde_json::json!({
                        "enabled": true,
                        "reachable": false,
                        "addr": addr,
                        "health": health,
                        "note": "start the game with PARTY_DUNGEON_BRIDGE=1 (native)",
                    })),
                }
            }
        }
    }

    #[tool(
        description = "Capture a frame from the game's headless renderer as an image (needs the game started with --headless); optionally save the PNG to `path`. The first screenshot right after startup can take several seconds while the renderer warms up (longer on a software renderer)."
    )]
    async fn screenshot(
        &self,
        Parameters(args): Parameters<ScreenshotArgs>,
    ) -> Result<CallToolResult, McpError> {
        let Some(addr) = self.store.bridge_addr().map(|addr| addr.to_owned()) else {
            return tool_error(
                "screenshot: the live bridge is disabled (PARTY_DUNGEON_BRIDGE is empty); \
                 start the game with --headless (the bridge is on by default at 127.0.0.1:6559)",
            );
        };
        let png = match store::bridge_screenshot(&addr).await {
            Ok(png) => png,
            Err(error) => return tool_error(format!("screenshot failed: {error}")),
        };
        let mut saved: Option<String> = None;
        if let Some(path) = args
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let dest = std::path::PathBuf::from(path);
            if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent).map_err(invalid)?;
            }
            std::fs::write(&dest, &png).map_err(invalid)?;
            saved = Some(dest.display().to_string());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
        let summary = serde_json::json!({
            "ok": true,
            "addr": addr,
            "bytes": png.len(),
            "contentType": "image/png",
            "path": saved,
        });
        Ok(CallToolResult::success(vec![
            ContentBlock::text(
                serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".into()),
            ),
            ContentBlock::image(encoded, "image/png"),
        ]))
    }
}

// ---------------------------------------------------------------------------
// Resources + server impl
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for PartyDungeonMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("world://active", "active-world"),
            Resource::new("world://shapes", "shape-catalogue"),
            Resource::new("world://platforms", "platform-paths"),
            Resource::new("world://character", "player-character"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.to_string();
        let text = match uri.as_str() {
            "world://shapes" => {
                let shapes: Vec<serde_json::Value> = model::shapes()
                    .iter()
                    .map(|shape| {
                        serde_json::json!({"id": shape.id, "name": shape.name, "category": shape.category})
                    })
                    .collect();
                serde_json::to_string_pretty(&shapes).unwrap_or_default()
            }
            "world://active" | "world://platforms" | "world://character" => {
                let (snapshot, _, _guard) = self
                    .effective(Source::Auto)
                    .await
                    .map_err(|_| McpError::resource_not_found("world_not_found", None))?;
                let value = match uri.as_str() {
                    "world://platforms" => {
                        serde_json::to_value(&snapshot.platforms).unwrap_or_default()
                    }
                    "world://character" => {
                        serde_json::to_value(&snapshot.character).unwrap_or_default()
                    }
                    _ => serde_json::to_value(&snapshot).unwrap_or_default(),
                };
                serde_json::to_string_pretty(&value).unwrap_or_default()
            }
            _ => return Err(McpError::resource_not_found("unknown_resource", None)),
        };
        Ok(ReadResourceResponse::Complete(ReadResourceResult::new(
            vec![ResourceContents::text(text, uri)],
        )))
    }
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = PartyDungeonMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_dungeon::session::Snapshot;

    fn piece(id: u32, shape: &str, x: f32, z: f32) -> SavedPiece {
        SavedPiece {
            id,
            shape: shape.to_string(),
            x,
            z,
            elevation: 0.0,
            rot_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            dynamic: false,
            damage: None,
            interact: None,
            trigger_width: None,
            trigger_length: None,
            trigger_height: None,
            trigger_shape: None,
            trigger_alpha: None,
            trigger_color: None,
            trigger_visible: None,
            name: None,
            group: None,
            locked: false,
        }
    }

    fn snapshot(pieces: Vec<SavedPiece>) -> Snapshot {
        Snapshot {
            version: session::SNAPSHOT_VERSION,
            next_id: Some(pieces.len() as u32 + 1),
            pieces,
            ..Default::default()
        }
    }

    #[test]
    fn unknown_shapes_fail_validation() {
        let snapshot = snapshot(vec![piece(1, "cloud", 0.0, 0.0)]);
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn piece_round_trips_through_validation() {
        let snapshot = snapshot(vec![piece(1, "block", 1.0, 2.0)]);
        let converted = store::saved_to_piece(&snapshot.pieces[0]).expect("converts");
        let loaded = snapshot.validate().expect("validates");
        assert_eq!(loaded.pieces.len(), 1);
        assert_eq!(converted.id, 1);
    }

    /// Full save/load round trip against a live SurrealDB in an isolated
    /// namespace. Skips loudly when no database is reachable (mirrors the
    /// server live tests). Needs root creds to create the throwaway
    /// namespace + service user; reads SURREAL_ROOT_* (dev defaults match
    /// the local compose stack).
    #[tokio::test]
    async fn world_round_trips_through_surrealdb() {
        use party_dungeon_server::{AuthConfig, Backend, SurrealConfig};

        let url = std::env::var("DUNGEON_TEST_URL")
            .unwrap_or_else(|_| "ws://127.0.0.1:6000".to_string());
        let root_user =
            std::env::var("SURREAL_ROOT_USER").unwrap_or_else(|_| "root".to_string());
        let root_pass =
            std::env::var("SURREAL_ROOT_PASS").unwrap_or_else(|_| "devrootpass".to_string());
        let ns = format!(
            "mcp_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0)
        );
        let auth = AuthConfig::hs256_for_tests("mcp-test-secret");
        let cfg = SurrealConfig::new(url.clone(), ns.clone(), "main".to_string(), auth);
        let root = match Backend::connect_root(&cfg, &root_user, &root_pass).await {
            Ok(db) => db,
            Err(error) => {
                eprintln!("SKIP mcp db test: no SurrealDB at {url}: {error}");
                return;
            }
        };
        let template = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/schema.surql"
        ))
        .expect("schema.surql");
        let rendered = AuthConfig::hs256_for_tests("mcp-test-secret").render_schema(&template);
        if let Err(error) = root.apply_schema(&rendered).await {
            eprintln!("SKIP mcp db test: schema apply failed: {error}");
            return;
        }
        // Service user for the store's service-role path.
        if root
            .apply_schema("DEFINE USER mcp_test ON DATABASE PASSWORD 'mcp-test-pass' ROLES OWNER;")
            .await
            .is_err()
        {
            eprintln!("SKIP mcp db test: cannot create service user");
            return;
        }
        unsafe {
            std::env::set_var("DUNGEON_SURREAL_NS", &ns);
            std::env::set_var("DUNGEON_SURREAL_DB", "main");
            std::env::set_var("DUNGEON_SERVICE_USER", "mcp_test");
            std::env::set_var("DUNGEON_SERVICE_PASS", "mcp-test-pass");
            std::env::set_var("PARTY_DUNGEON_BRIDGE", "");
        }
        let store = Store::from_env();
        let snapshot = snapshot(vec![
            piece(1, "block", 1.0, 2.0),
            piece(2, "floor", 0.0, 0.0),
        ]);
        let note = store.save_world(&snapshot, "mcp-test").await.expect("saves");
        assert!(note.contains("2 pieces"), "unexpected note: {note}");
        let (loaded, live) = store.load_world(Source::Db).await.expect("loads");
        assert!(!live);
        assert_eq!(loaded.pieces.len(), 2);
        assert_eq!(loaded.platforms.len(), 0);
        let ids: std::collections::HashSet<u32> =
            loaded.pieces.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 2);
    }
}
