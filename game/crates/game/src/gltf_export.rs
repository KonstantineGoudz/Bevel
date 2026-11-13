//! glTF 2.0 (`.glb`) kit exporter for all procedural Glade buildings, modular
//! brick/masonry/shingle variations, and classic dungeon catalogue pieces.
//!
//! Produces a self-contained binary glTF 2.0 file (`assets/glade_pieces_kit.glb`)
//! with clean node hierarchies, semantic mesh/material names, and non-stretched
//! box-projected `TEXCOORD_0` UVs so 3D artists can open the kit in Blender
//! (or the built-in `?viewer`) to author textures and material variations.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use serde_json::{Value, json};

use crate::glade::{
    DEFAULT_ROOF_COLOR, build_glade_meshes_with_spine, build_glade_modular_kit_meshes,
};
use crate::mesh::{extrude, wedge};
use crate::model::{
    Geometry, InteractAction, Piece, Shape, Style, TRIGGER_DEFAULT_ALPHA, Trigger, TriggerShape,
    shape_index, shape_points, shapes,
};

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const GLB_CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const GLB_CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;
const COMPONENT_UNSIGNED_INT: u32 = 5125;
const COMPONENT_FLOAT: u32 = 5126;

/// Extracted raw geometry from a Bevy [`Mesh`] ready for glTF buffer packing.
struct RawMeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

fn extract_raw_mesh(mesh: &Mesh) -> Option<RawMeshData> {
    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION)? {
        VertexAttributeValues::Float32x3(v) => v.clone(),
        _ => return None,
    };
    if positions.is_empty() {
        return None;
    }
    let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(v)) if v.len() == positions.len() => v.clone(),
        _ => vec![[0.0, 1.0, 0.0]; positions.len()],
    };
    let uvs = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(v)) if v.len() == positions.len() => v.clone(),
        _ => positions
            .iter()
            .map(|p| [p[0] * 0.5, p[2] * 0.5])
            .collect(),
    };
    let indices = match mesh.indices() {
        Some(Indices::U32(idx)) if !idx.is_empty() => idx.clone(),
        Some(Indices::U16(idx)) if !idx.is_empty() => idx.iter().map(|&i| i as u32).collect(),
        _ => (0..positions.len() as u32).collect(),
    };
    Some(RawMeshData {
        positions,
        normals,
        uvs,
        indices,
    })
}

/// Incremental builder for a glTF 2.0 document and its single binary buffer.
struct GltfBuilder {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
    materials: Vec<Value>,
    material_lookup: HashMap<String, usize>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
}

impl GltfBuilder {
    fn new() -> Self {
        Self {
            bin: Vec::with_capacity(512 * 1024),
            buffer_views: Vec::new(),
            accessors: Vec::new(),
            materials: Vec::new(),
            material_lookup: HashMap::new(),
            meshes: Vec::new(),
            nodes: Vec::new(),
        }
    }

    fn align4(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn ensure_material(
        &mut self,
        name: &str,
        rgba: [f32; 4],
        roughness: f32,
        metallic: f32,
        blend: bool,
    ) -> usize {
        if let Some(&idx) = self.material_lookup.get(name) {
            return idx;
        }
        let idx = self.materials.len();
        let mut mat = json!({
            "name": name,
            "doubleSided": blend,
            "pbrMetallicRoughness": {
                "baseColorFactor": rgba,
                "roughnessFactor": roughness,
                "metallicFactor": metallic,
            }
        });
        if blend {
            mat["alphaMode"] = json!("BLEND");
        }
        self.materials.push(mat);
        self.material_lookup.insert(name.to_owned(), idx);
        idx
    }

    fn push_buffer_view(&mut self, bytes: &[u8], target: u32, name: &str) -> usize {
        self.align4();
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let length = bytes.len();
        self.align4();
        let idx = self.buffer_views.len();
        self.buffer_views.push(json!({
            "name": name,
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": length,
            "target": target,
        }));
        idx
    }

    fn add_mesh(&mut self, name: &str, mesh: &Mesh, material_idx: usize) -> Option<usize> {
        let raw = extract_raw_mesh(mesh)?;

        // Compute tight bounding box for POSITION (required by glTF 2.0 spec).
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut pos_bytes = Vec::with_capacity(raw.positions.len() * 12);
        for p in &raw.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
                pos_bytes.extend_from_slice(&p[axis].to_le_bytes());
            }
        }

        let mut norm_bytes = Vec::with_capacity(raw.normals.len() * 12);
        for n in &raw.normals {
            for axis in 0..3 {
                norm_bytes.extend_from_slice(&n[axis].to_le_bytes());
            }
        }

        let mut uv_bytes = Vec::with_capacity(raw.uvs.len() * 8);
        for uv in &raw.uvs {
            uv_bytes.extend_from_slice(&uv[0].to_le_bytes());
            uv_bytes.extend_from_slice(&uv[1].to_le_bytes());
        }

        let mut idx_bytes = Vec::with_capacity(raw.indices.len() * 4);
        for &idx in &raw.indices {
            idx_bytes.extend_from_slice(&idx.to_le_bytes());
        }

        let pos_bv = self.push_buffer_view(&pos_bytes, TARGET_ARRAY_BUFFER, &format!("{name}_POS"));
        let norm_bv =
            self.push_buffer_view(&norm_bytes, TARGET_ARRAY_BUFFER, &format!("{name}_NORM"));
        let uv_bv = self.push_buffer_view(&uv_bytes, TARGET_ARRAY_BUFFER, &format!("{name}_UV0"));
        let idx_bv = self.push_buffer_view(
            &idx_bytes,
            TARGET_ELEMENT_ARRAY_BUFFER,
            &format!("{name}_IDX"),
        );

        let pos_acc = self.accessors.len();
        self.accessors.push(json!({
            "name": format!("{name}_POSITION"),
            "bufferView": pos_bv,
            "byteOffset": 0,
            "componentType": COMPONENT_FLOAT,
            "count": raw.positions.len(),
            "type": "VEC3",
            "min": min,
            "max": max,
        }));

        let norm_acc = self.accessors.len();
        self.accessors.push(json!({
            "name": format!("{name}_NORMAL"),
            "bufferView": norm_bv,
            "byteOffset": 0,
            "componentType": COMPONENT_FLOAT,
            "count": raw.normals.len(),
            "type": "VEC3",
        }));

        let uv_acc = self.accessors.len();
        self.accessors.push(json!({
            "name": format!("{name}_TEXCOORD_0"),
            "bufferView": uv_bv,
            "byteOffset": 0,
            "componentType": COMPONENT_FLOAT,
            "count": raw.uvs.len(),
            "type": "VEC2",
        }));

        let idx_acc = self.accessors.len();
        self.accessors.push(json!({
            "name": format!("{name}_INDICES"),
            "bufferView": idx_bv,
            "byteOffset": 0,
            "componentType": COMPONENT_UNSIGNED_INT,
            "count": raw.indices.len(),
            "type": "SCALAR",
        }));

        let mesh_idx = self.meshes.len();
        self.meshes.push(json!({
            "name": name,
            "primitives": [{
                "attributes": {
                    "POSITION": pos_acc,
                    "NORMAL": norm_acc,
                    "TEXCOORD_0": uv_acc,
                },
                "indices": idx_acc,
                "material": material_idx,
                "mode": 4
            }]
        }));
        Some(mesh_idx)
    }

    fn add_node(
        &mut self,
        name: &str,
        translation: [f32; 3],
        mesh: Option<usize>,
        children: &[usize],
    ) -> usize {
        let idx = self.nodes.len();
        let mut node = json!({
            "name": name,
        });
        if translation != [0.0, 0.0, 0.0] {
            node["translation"] = json!(translation);
        }
        if let Some(m) = mesh {
            node["mesh"] = json!(m);
        }
        if !children.is_empty() {
            node["children"] = json!(children);
        }
        self.nodes.push(node);
        idx
    }

    fn finish_glb(mut self, root_nodes: &[usize]) -> Vec<u8> {
        self.align4();
        let bin_len = self.bin.len();

        let root_json = json!({
            "asset": {
                "version": "2.0",
                "generator": "party-dungeon gltf_export (Tiny Glade & Dungeon Kit)",
                "copyright": "Party Dungeon Procedural Kit"
            },
            "scene": 0,
            "scenes": [{
                "name": "Scene_PartyDungeon_Kit",
                "nodes": root_nodes
            }],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "materials": self.materials,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
            "buffers": [{
                "name": "PartyDungeonKitBuffer",
                "byteLength": bin_len
            }]
        });

        let mut json_bytes =
            serde_json::to_vec_pretty(&root_json).expect("serializing glTF JSON cannot fail");
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }

        let total_len = 12 + 8 + json_bytes.len() + 8 + self.bin.len();
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
        out.extend_from_slice(&GLB_VERSION.to_le_bytes());
        out.extend_from_slice(&(total_len as u32).to_le_bytes());

        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&GLB_CHUNK_JSON.to_le_bytes());
        out.extend_from_slice(&json_bytes);

        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&GLB_CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&self.bin);

        out
    }
}

fn color_to_rgba(color: Color) -> [f32; 4] {
    let srgba = color.to_srgba();
    [srgba.red, srgba.green, srgba.blue, srgba.alpha]
}

fn shape_solid_parts(shape: &Shape) -> Vec<(Vec<Vec2>, f32, f32, u8)> {
    let outline = shape_points(shape);
    if shape.style == Style::Window {
        let sill = 0.95;
        let head = 2.2;
        vec![
            (outline.clone(), shape.base, sill, 0),
            (outline.clone(), head, (shape.top() - head).max(0.05), 0),
            (outline, sill, (head - sill).max(0.05), 1),
        ]
    } else {
        vec![(outline, shape.base, shape.height.max(0.02), 0)]
    }
}

fn make_glade_piece(
    id: u32,
    shape_id: &str,
    size: Vec3,
    roof_override: TriggerShape,
    roof_alpha: f32,
) -> Piece {
    Piece {
        id,
        shape: shape_index(shape_id).expect("glade shape exists in catalogue"),
        pos: Vec2::ZERO,
        rot: 0.0,
        pitch: 0.0,
        roll: 0.0,
        elevation: 0.0,
        dynamic: false,
        trigger: Trigger::harmless(),
        trigger_size: size,
        trigger_shape_override: roof_override,
        trigger_alpha: roof_alpha,
        trigger_color: DEFAULT_ROOF_COLOR,
        trigger_visible: true,
        interact: InteractAction::None,
    }
}

/// Builds the complete `.glb` byte buffer containing:
/// 1. `Category_01_Glade_Buildings` — all procedural Glade tower, room, wall,
///    curved path wall, wall-with-archway, and portal door variations split into
///    `<Name>_Stone`, `<Name>_Glass`, and `<Name>_Roof` sub-meshes.
/// 2. `Category_02_Bricks_And_Modular_Details` — 25 named brick variations,
///    staggered brick course clusters, corner/seam quoins, arch voussoirs,
///    keystones, roof shingles, wall coping, merlons, window frames, and timber
///    door leaves.
/// 3. `Category_03_Catalogue_Pieces` — all classic level editor shapes grouped
///    and named by category (`Piece_<Category>_<id>`).
pub fn build_glade_pieces_kit_glb() -> Vec<u8> {
    let mut b = GltfBuilder::new();

    // Register shared semantic materials for Glade and modular kit pieces.
    let mat_masonry = b.ensure_material(
        "Mat_Glade_Masonry_Stone",
        [0.84, 0.80, 0.73, 1.0],
        0.88,
        0.02,
        false,
    );
    let mat_brick = b.ensure_material(
        "Mat_Glade_Brick_Dressed",
        [0.78, 0.69, 0.58, 1.0],
        0.85,
        0.02,
        false,
    );
    let mat_cobble = b.ensure_material(
        "Mat_Glade_Cobblestone",
        [0.64, 0.63, 0.60, 1.0],
        0.92,
        0.0,
        false,
    );
    let mat_terracotta = b.ensure_material(
        "Mat_Glade_Roof_Terracotta",
        [0.78, 0.36, 0.25, 1.0],
        0.78,
        0.02,
        false,
    );
    let mat_slate = b.ensure_material(
        "Mat_Glade_Roof_Slate_Blue",
        [0.32, 0.42, 0.54, 1.0],
        0.72,
        0.05,
        false,
    );
    let mat_timber = b.ensure_material(
        "Mat_Glade_Timber_Wood",
        [0.46, 0.31, 0.19, 1.0],
        0.84,
        0.0,
        false,
    );
    let mat_glass = b.ensure_material(
        "Mat_Window_Glass",
        [0.60, 0.80, 0.90, 0.35],
        0.15,
        0.0,
        true,
    );

    let material_for_tag = |tag: &str| -> usize {
        match tag {
            "Mat_Glade_Masonry_Stone" => mat_masonry,
            "Mat_Glade_Brick_Dressed" => mat_brick,
            "Mat_Glade_Cobblestone" => mat_cobble,
            "Mat_Glade_Roof_Terracotta" => mat_terracotta,
            "Mat_Glade_Roof_Slate_Blue" => mat_slate,
            "Mat_Glade_Timber_Wood" => mat_timber,
            "Mat_Window_Glass" => mat_glass,
            _ => mat_masonry,
        }
    };

    // -----------------------------------------------------------------------
    // 1. Category_01_Glade_Buildings
    // -----------------------------------------------------------------------
    struct GladeSpec {
        name: &'static str,
        piece: Piece,
        all_pieces: Vec<Piece>,
        spine: Option<Vec<Vec2>>,
        roof_mat: usize,
    }

    let wall_host = make_glade_piece(
        100,
        "glade_wall",
        Vec3::new(4.0, 2.2, 0.45),
        TriggerShape::Box,
        TRIGGER_DEFAULT_ALPHA,
    );
    let arch_cutter = make_glade_piece(
        101,
        "glade_door",
        Vec3::new(1.2, 1.65, 0.65),
        TriggerShape::Box,
        TRIGGER_DEFAULT_ALPHA,
    );

    let tower_conical = make_glade_piece(
        1,
        "glade_tower",
        Vec3::new(2.4, 3.2, 2.4),
        TriggerShape::Box,
        0.58,
    );
    let tower_battlements = make_glade_piece(
        2,
        "glade_tower",
        Vec3::new(2.6, 3.4, 2.6),
        TriggerShape::Cylinder,
        0.0,
    );
    let room_gable = make_glade_piece(
        3,
        "glade_room",
        Vec3::new(3.6, 2.4, 2.8),
        TriggerShape::Box,
        0.52,
    );
    let room_hipped = make_glade_piece(
        4,
        "glade_room",
        Vec3::new(3.4, 2.4, 3.0),
        TriggerShape::Sphere,
        0.50,
    );
    let room_battlements = make_glade_piece(
        5,
        "glade_room",
        Vec3::new(3.6, 2.6, 3.0),
        TriggerShape::Cylinder,
        0.0,
    );
    let wall_low_fence = make_glade_piece(
        6,
        "glade_wall",
        Vec3::new(3.6, 0.75, 0.40),
        TriggerShape::Box,
        0.0,
    );
    let wall_standard = make_glade_piece(
        7,
        "glade_wall",
        Vec3::new(3.6, 1.85, 0.42),
        TriggerShape::Box,
        0.0,
    );
    let wall_fortified = make_glade_piece(
        8,
        "glade_wall",
        Vec3::new(3.6, 3.10, 0.48),
        TriggerShape::Box,
        0.0,
    );
    let wall_curved = make_glade_piece(
        9,
        "glade_wall",
        Vec3::new(4.0, 1.95, 0.42),
        TriggerShape::Box,
        0.0,
    );
    let door_portal = make_glade_piece(
        11,
        "glade_door",
        Vec3::new(1.25, 1.85, 0.55),
        TriggerShape::Box,
        0.0,
    );

    let glade_specs = vec![
        GladeSpec {
            name: "Glade_Tower_Conical",
            piece: tower_conical,
            all_pieces: vec![tower_conical],
            spine: None,
            roof_mat: mat_terracotta,
        },
        GladeSpec {
            name: "Glade_Tower_Battlements",
            piece: tower_battlements,
            all_pieces: vec![tower_battlements],
            spine: None,
            roof_mat: mat_slate,
        },
        GladeSpec {
            name: "Glade_Room_Gable",
            piece: room_gable,
            all_pieces: vec![room_gable],
            spine: None,
            roof_mat: mat_slate,
        },
        GladeSpec {
            name: "Glade_Room_Hipped",
            piece: room_hipped,
            all_pieces: vec![room_hipped],
            spine: None,
            roof_mat: mat_terracotta,
        },
        GladeSpec {
            name: "Glade_Room_Battlements",
            piece: room_battlements,
            all_pieces: vec![room_battlements],
            spine: None,
            roof_mat: mat_slate,
        },
        GladeSpec {
            name: "Glade_Wall_LowFence",
            piece: wall_low_fence,
            all_pieces: vec![wall_low_fence],
            spine: None,
            roof_mat: mat_brick,
        },
        GladeSpec {
            name: "Glade_Wall_Standard",
            piece: wall_standard,
            all_pieces: vec![wall_standard],
            spine: None,
            roof_mat: mat_brick,
        },
        GladeSpec {
            name: "Glade_Wall_Fortified",
            piece: wall_fortified,
            all_pieces: vec![wall_fortified],
            spine: None,
            roof_mat: mat_brick,
        },
        GladeSpec {
            name: "Glade_Wall_Curved_Path",
            piece: wall_curved,
            all_pieces: vec![wall_curved],
            spine: Some(vec![
                Vec2::new(-1.8, -0.4),
                Vec2::new(-0.6, 0.55),
                Vec2::new(0.6, -0.55),
                Vec2::new(1.8, 0.4),
            ]),
            roof_mat: mat_brick,
        },
        GladeSpec {
            name: "Glade_Wall_With_Archway",
            piece: wall_host,
            all_pieces: vec![wall_host, arch_cutter],
            spine: None,
            roof_mat: mat_brick,
        },
        GladeSpec {
            name: "Glade_Door_Portal",
            piece: door_portal,
            all_pieces: vec![door_portal],
            spine: None,
            roof_mat: mat_timber,
        },
    ];

    let mut glade_building_nodes = Vec::with_capacity(glade_specs.len());
    for (idx, spec) in glade_specs.iter().enumerate() {
        let shape = &shapes()[spec.piece.shape];
        let meshes = build_glade_meshes_with_spine(
            &spec.piece,
            shape,
            &spec.all_pieces,
            &[],
            spec.spine.as_deref(),
        );
        let mut part_nodes = Vec::new();

        let stone_name = format!("{}_Stone", spec.name);
        if let Some(m_idx) = b.add_mesh(&stone_name, &meshes.stone, mat_masonry) {
            part_nodes.push(b.add_node(&stone_name, [0.0, 0.0, 0.0], Some(m_idx), &[]));
        }

        let glass_name = format!("{}_Glass", spec.name);
        if let Some(m_idx) = b.add_mesh(&glass_name, &meshes.glass, mat_glass) {
            part_nodes.push(b.add_node(&glass_name, [0.0, 0.0, 0.0], Some(m_idx), &[]));
        }

        let roof_name = format!("{}_Roof", spec.name);
        if let Some(m_idx) = b.add_mesh(&roof_name, &meshes.roof, spec.roof_mat) {
            part_nodes.push(b.add_node(&roof_name, [0.0, 0.0, 0.0], Some(m_idx), &[]));
        }

        let x = (idx as f32) * 5.2;
        glade_building_nodes.push(b.add_node(spec.name, [x, 0.0, 0.0], None, &part_nodes));
    }

    let cat1_node = b.add_node(
        "Category_01_Glade_Buildings",
        [0.0, 0.0, 0.0],
        None,
        &glade_building_nodes,
    );

    // -----------------------------------------------------------------------
    // 2. Category_02_Bricks_And_Modular_Details
    // -----------------------------------------------------------------------
    let modular_meshes = build_glade_modular_kit_meshes();
    let mut modular_nodes = Vec::with_capacity(modular_meshes.len());
    for (idx, (name, mat_tag, mesh)) in modular_meshes.iter().enumerate() {
        let mat_idx = material_for_tag(mat_tag);
        if let Some(m_idx) = b.add_mesh(name, mesh, mat_idx) {
            let col = (idx % 7) as f32;
            let row = (idx / 7) as f32;
            let pos = [col * 2.2, 0.0, row * 2.2];
            modular_nodes.push(b.add_node(name, pos, Some(m_idx), &[]));
        }
    }
    let cat2_node = b.add_node(
        "Category_02_Bricks_And_Modular_Details",
        [0.0, 0.0, 6.5],
        None,
        &modular_nodes,
    );

    // -----------------------------------------------------------------------
    // 3. Category_03_Catalogue_Pieces
    // -----------------------------------------------------------------------
    let mut catalogue_nodes = Vec::new();
    for (idx, shape) in shapes().iter().enumerate() {
        let cat_name = shape.category;
        let node_name = format!("Piece_{cat_name}_{}", shape.id);
        let mat_name = format!("Mat_Catalogue_{}", shape.id);
        let base_mat = b.ensure_material(
            &mat_name,
            color_to_rgba(shape.color),
            0.85,
            0.0,
            shape.is_trigger,
        );

        let col = (idx % 8) as f32;
        let row = (idx / 8) as f32;
        let pos = [col * 3.5, 0.0, row * 3.5];

        if shape.is_trigger {
            let mesh: Mesh = match shape.trigger_shape {
                TriggerShape::Box => Cuboid::new(1.0, 1.0, 1.0).into(),
                TriggerShape::Sphere => Sphere::new(0.5).mesh().ico(4).unwrap(),
                TriggerShape::Cylinder => Cylinder::new(0.5, 1.0).into(),
            };
            if let Some(m_idx) = b.add_mesh(&node_name, &mesh, base_mat) {
                catalogue_nodes.push(b.add_node(&node_name, pos, Some(m_idx), &[]));
            }
        } else if let Geometry::Wedge { w, d, h } = &shape.geom {
            let mesh = wedge(*w, *d, *h);
            if let Some(m_idx) = b.add_mesh(&node_name, &mesh, base_mat) {
                catalogue_nodes.push(b.add_node(&node_name, pos, Some(m_idx), &[]));
            }
        } else {
            let parts = shape_solid_parts(shape);
            if parts.len() == 1 {
                let (outline, base, height, slot) = &parts[0];
                let mesh = extrude(outline, *base, *height);
                let mat = if *slot == 1 { mat_glass } else { base_mat };
                if let Some(m_idx) = b.add_mesh(&node_name, &mesh, mat) {
                    catalogue_nodes.push(b.add_node(&node_name, pos, Some(m_idx), &[]));
                }
            } else {
                let mut child_nodes = Vec::with_capacity(parts.len());
                for (p_idx, (outline, base, height, slot)) in parts.iter().enumerate() {
                    let suffix = if *slot == 1 {
                        format!("Glass_{p_idx}")
                    } else {
                        format!("Stone_{p_idx}")
                    };
                    let part_name = format!("{node_name}_{suffix}");
                    let mesh = extrude(outline, *base, *height);
                    let mat = if *slot == 1 { mat_glass } else { base_mat };
                    if let Some(m_idx) = b.add_mesh(&part_name, &mesh, mat) {
                        child_nodes.push(b.add_node(&part_name, [0.0, 0.0, 0.0], Some(m_idx), &[]));
                    }
                }
                catalogue_nodes.push(b.add_node(&node_name, pos, None, &child_nodes));
            }
        }
    }
    let cat3_node = b.add_node(
        "Category_03_Catalogue_Pieces",
        [0.0, 0.0, 15.0],
        None,
        &catalogue_nodes,
    );

    b.finish_glb(&[cat1_node, cat2_node, cat3_node])
}

/// Writes the complete Glade & Dungeon pieces `.glb` kit to `path`.
pub fn write_kit_glb(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = build_glade_pieces_kit_glb();
    let mut file = std::fs::File::create(path)?;
    file.write_all(&bytes)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the JSON chunk out of a `.glb` binary and validates the glTF 2.0 header.
    fn parse_glb_json(glb: &[u8]) -> Value {
        assert!(glb.len() > 28, "GLB must be larger than header + chunks");
        let magic = u32::from_le_bytes(glb[0..4].try_into().unwrap());
        let version = u32::from_le_bytes(glb[4..8].try_into().unwrap());
        let total_len = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(magic, GLB_MAGIC);
        assert_eq!(version, GLB_VERSION);
        assert_eq!(total_len, glb.len());

        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json_type = u32::from_le_bytes(glb[16..20].try_into().unwrap());
        assert_eq!(json_type, GLB_CHUNK_JSON);
        let json_slice = &glb[20..20 + json_len];

        let bin_header_start = 20 + json_len;
        let bin_len =
            u32::from_le_bytes(glb[bin_header_start..bin_header_start + 4].try_into().unwrap())
                as usize;
        let bin_type = u32::from_le_bytes(
            glb[bin_header_start + 4..bin_header_start + 8]
                .try_into()
                .unwrap(),
        );
        assert_eq!(bin_type, GLB_CHUNK_BIN);
        assert_eq!(bin_header_start + 8 + bin_len, glb.len());

        serde_json::from_slice(json_slice).expect("valid glTF 2.0 JSON chunk")
    }

    #[test]
    fn glade_pieces_kit_glb_contains_named_buildings_bricks_and_catalogue_pieces() {
        let glb = build_glade_pieces_kit_glb();
        let doc = parse_glb_json(&glb);

        assert_eq!(doc["asset"]["version"], "2.0");

        let node_names: Vec<&str> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();

        // Category roots
        assert!(node_names.contains(&"Category_01_Glade_Buildings"));
        assert!(node_names.contains(&"Category_02_Bricks_And_Modular_Details"));
        assert!(node_names.contains(&"Category_03_Catalogue_Pieces"));

        // Glade building variations & sub-meshes
        for expected in [
            "Glade_Tower_Conical",
            "Glade_Tower_Conical_Stone",
            "Glade_Tower_Conical_Roof",
            "Glade_Tower_Battlements",
            "Glade_Room_Gable",
            "Glade_Room_Hipped",
            "Glade_Room_Battlements",
            "Glade_Wall_LowFence",
            "Glade_Wall_Standard",
            "Glade_Wall_Fortified",
            "Glade_Wall_Curved_Path",
            "Glade_Wall_With_Archway",
            "Glade_Door_Portal",
            "Glade_Door_Portal_Stone",
            "Glade_Door_Portal_Roof",
        ] {
            assert!(
                node_names.contains(&expected),
                "missing expected Glade node: {expected}"
            );
        }

        // Brick, quoin, arch, shingle, coping, merlon, and window variations
        for expected_brick in [
            "Brick_Var_01_Standard_Ashlar",
            "Brick_Var_02_Long_Stretcher",
            "Brick_Var_03_Short_Header",
            "Brick_Var_04_Rounded_Cobblestone",
            "Brick_Var_05_Weathered_Castle_Block",
            "Brick_Var_06_Courtyard_Flagstone",
            "Brick_Course_Cluster_Var_01",
            "Brick_Course_Cluster_Var_02",
            "Quoin_Column_Corner_Alternating",
            "Quoin_Column_Seam_Pilaster",
            "Arch_Voussoir_Wedge_Var_01",
            "Arch_Voussoir_Wedge_Var_02",
            "Arch_Keystone_Central_Crest",
            "Arch_Vault_Portal_Complete",
            "Roof_Shingle_Tile_Var_01_Flat",
            "Roof_Shingle_Tile_Var_02_Wide",
            "Roof_Shingle_Tile_Var_03_Weathered",
            "Wall_Coping_Stone_Row",
            "Battlement_Merlon_Body",
            "Battlement_Merlon_Cap",
            "Window_Cottage_StoneFrame",
            "Window_Cottage_GlassPane",
            "Window_Tower_Narrow_StoneFrame",
            "Window_Tower_Narrow_GlassPane",
            "Door_Timber_Leaf_Planked",
        ] {
            assert!(
                node_names.contains(&expected_brick),
                "missing expected modular brick/detail node: {expected_brick}"
            );
        }

        // Classic catalogue pieces
        for expected_piece in [
            "Piece_Surface_floor",
            "Piece_Barrier_wall",
            "Piece_Threshold_door",
            "Piece_Barrier_window",
            "Piece_Barrier_window_Stone_0",
            "Piece_Barrier_window_Glass_2",
            "Piece_Traversal_ramp",
            "Piece_Traversal_block",
        ] {
            assert!(
                node_names.contains(&expected_piece),
                "missing expected catalogue piece node: {expected_piece}"
            );
        }

        // Materials
        let mat_names: Vec<&str> = doc["materials"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["name"].as_str())
            .collect();
        for expected_mat in [
            "Mat_Glade_Masonry_Stone",
            "Mat_Glade_Brick_Dressed",
            "Mat_Glade_Cobblestone",
            "Mat_Glade_Roof_Terracotta",
            "Mat_Glade_Roof_Slate_Blue",
            "Mat_Glade_Timber_Wood",
            "Mat_Window_Glass",
        ] {
            assert!(
                mat_names.contains(&expected_mat),
                "missing expected material: {expected_mat}"
            );
        }

        // Every mesh primitive must carry POSITION, NORMAL, TEXCOORD_0, and indices
        let meshes = doc["meshes"].as_array().unwrap();
        assert!(meshes.len() >= 60, "expected at least 60 named meshes");
        for m in meshes {
            let prim = &m["primitives"][0];
            assert!(prim["attributes"]["POSITION"].is_number());
            assert!(prim["attributes"]["NORMAL"].is_number());
            assert!(prim["attributes"]["TEXCOORD_0"].is_number());
            assert!(prim["indices"].is_number());
        }
    }

    #[test]
    fn write_glade_pieces_kit_glb_to_assets_directory() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let assets_glb = manifest_dir.join("../../assets/glade_pieces_kit.glb");
        write_kit_glb(&assets_glb).expect("writing assets/glade_pieces_kit.glb should succeed");
        let meta = std::fs::metadata(&assets_glb).expect("glade_pieces_kit.glb exists on disk");
        assert!(meta.len() > 50_000, "kit GLB should have substantial geometry");
    }
}
