//! Next-gen 3D grass material extension for Bevy 0.20.
//!
//! Implements the core rendering techniques from Nils Arenz's
//! "Creating Next-Gen Grass in UE4" adapted for Bevy's `ExtendedMaterial`:
//! - **Zero alpha-card overdraw**: designed for true 3D multi-segment tapered
//!   blade geometry (no opacity mask / cutout quad overdraw).
//! - **Multi-layer world-space wind**: rolling directional gusts, cross-waves,
//!   and high-frequency tip flutter weighted quadratically from root to tip,
//!   with approximate arc-length preservation.
//! - **Dynamic character interaction**: radial bending & depression around the
//!   player's world position.
//! - **World-space color variation & root AO**: multi-octave noise breaks up
//!   tiling across large fields, while root-to-tip gradients + root AO anchor
//!   blades into the turf.
//! - **Backlit subsurface translucency**: view/sun-dependent glow through thin
//!   grass tips.

use bevy::asset::{load_internal_asset, uuid_handle};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::{Shader, ShaderRef};

const GRASS_SHADER_HANDLE: Handle<Shader> = uuid_handle!("8e5d7a14-2c3f-4b9e-9a71-6f3c8d1e4a02");

pub type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassExt>;

/// Global wind and character-interaction parameters pushed to all `GrassMaterial`
/// instances each frame.
#[derive(Resource, Debug, Clone)]
pub struct GrassWindConfig {
    pub wind_dir: Vec2,
    pub wind_speed: f32,
    pub wind_strength: f32,
    pub gust_scale: f32,
    pub flutter_strength: f32,
    pub flutter_speed: f32,
    pub interact_radius: f32,
    pub bend_strength: f32,
    pub squash_strength: f32,
}

impl Default for GrassWindConfig {
    fn default() -> Self {
        Self {
            wind_dir: Vec2::new(0.85, 0.52).normalize(),
            wind_speed: 2.2,
            wind_strength: 0.14,
            gust_scale: 0.45,
            flutter_strength: 0.045,
            flutter_speed: 6.5,
            interact_radius: 0.85,
            bend_strength: 0.55,
            squash_strength: 0.25,
        }
    }
}

#[derive(ShaderType, Clone, Copy, Debug, Reflect)]
pub struct GrassUniforms {
    /// `(time, wind_speed, wind_strength, gust_scale)`
    pub wind_params: Vec4,
    /// `(dir_x, dir_z, flutter_strength, flutter_speed)`
    pub wind_dir: Vec4,
    /// `(x, y, z, interact_radius)`
    pub player_pos: Vec4,
    /// `(bend_strength, squash_strength, unused, unused)`
    pub interact_params: Vec4,
    pub root_color: LinearRgba,
    pub tip_color: LinearRgba,
    pub subsurface_color: LinearRgba,
    /// `(patch_scale, patch_variation, ao_strength, subsurface_strength)`
    pub color_params: Vec4,
}

impl Default for GrassUniforms {
    fn default() -> Self {
        Self {
            wind_params: Vec4::new(0.0, 2.2, 0.14, 0.45),
            wind_dir: Vec4::new(0.85, 0.52, 0.045, 6.5),
            player_pos: Vec4::ZERO,
            interact_params: Vec4::new(0.55, 0.25, 0.0, 0.0),
            root_color: LinearRgba::rgb(0.45, 0.58, 0.35),
            tip_color: LinearRgba::rgb(1.15, 1.22, 0.92),
            subsurface_color: LinearRgba::rgb(0.85, 1.0, 0.25),
            color_params: Vec4::new(0.18, 0.22, 0.38, 0.65),
        }
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct GrassExt {
    #[uniform(100)]
    pub uniforms: GrassUniforms,
}

impl GrassExt {
    /// Warm sunlit meadow grass (chartreuse tips, mossy roots).
    pub fn meadow_sun() -> Self {
        Self {
            uniforms: GrassUniforms {
                root_color: LinearRgba::rgb(0.42, 0.54, 0.32),
                tip_color: LinearRgba::rgb(1.18, 1.24, 0.88),
                subsurface_color: LinearRgba::rgb(0.95, 1.05, 0.28),
                color_params: Vec4::new(0.16, 0.24, 0.42, 0.72),
                ..default()
            },
        }
    }

    /// Deeper lush meadow grass for shaded pockets.
    pub fn meadow_lush() -> Self {
        Self {
            uniforms: GrassUniforms {
                root_color: LinearRgba::rgb(0.34, 0.48, 0.28),
                tip_color: LinearRgba::rgb(1.02, 1.16, 0.82),
                subsurface_color: LinearRgba::rgb(0.72, 0.95, 0.22),
                color_params: Vec4::new(0.18, 0.22, 0.46, 0.58),
                ..default()
            },
        }
    }

    /// Paintable hex grass tuft material.
    pub fn painted_tuft(bright: bool) -> Self {
        if bright {
            Self {
                uniforms: GrassUniforms {
                    root_color: LinearRgba::rgb(0.40, 0.55, 0.30),
                    tip_color: LinearRgba::rgb(1.20, 1.26, 0.88),
                    subsurface_color: LinearRgba::rgb(0.92, 1.04, 0.26),
                    color_params: Vec4::new(0.22, 0.22, 0.36, 0.70),
                    ..default()
                },
            }
        } else {
            Self {
                uniforms: GrassUniforms {
                    root_color: LinearRgba::rgb(0.36, 0.50, 0.30),
                    tip_color: LinearRgba::rgb(1.06, 1.18, 0.86),
                    subsurface_color: LinearRgba::rgb(0.75, 0.96, 0.24),
                    color_params: Vec4::new(0.22, 0.22, 0.38, 0.62),
                    ..default()
                },
            }
        }
    }
}

impl MaterialExtension for GrassExt {
    fn vertex_shader() -> ShaderRef {
        GRASS_SHADER_HANDLE.into()
    }

    fn fragment_shader() -> ShaderRef {
        GRASS_SHADER_HANDLE.into()
    }
}

/// Registers the embedded grass WGSL shader and `GrassMaterial` asset/pipeline.
pub struct GrassShaderPlugin;

impl Plugin for GrassShaderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GrassWindConfig>();
        if app.world().contains_resource::<Assets<Shader>>() {
            load_internal_asset!(
                app,
                GRASS_SHADER_HANDLE,
                "shaders/grass.wgsl",
                Shader::from_wgsl
            );
            app.add_plugins(MaterialPlugin::<GrassMaterial>::default());
        } else {
            app.init_asset::<GrassMaterial>();
        }
    }
}

/// Updates time, wind, and active player position on all `GrassMaterial` assets.
pub fn update_grass_uniforms(
    time: Res<Time>,
    config: Res<GrassWindConfig>,
    mut materials: ResMut<Assets<GrassMaterial>>,
    player_pos: Option<Vec3>,
) {
    let t = time.elapsed_secs();
    let p = match player_pos {
        Some(pos) => Vec4::new(pos.x, pos.y, pos.z, config.interact_radius),
        None => Vec4::ZERO,
    };
    let wind_dir = config.wind_dir.normalize_or(Vec2::X);
    for (_, mat) in materials.iter_mut() {
        let u = &mut mat.extension.uniforms;
        u.wind_params = Vec4::new(t, config.wind_speed, config.wind_strength, config.gust_scale);
        u.wind_dir = Vec4::new(
            wind_dir.x,
            wind_dir.y,
            config.flutter_strength,
            config.flutter_speed,
        );
        u.player_pos = p;
        u.interact_params = Vec4::new(config.bend_strength, config.squash_strength, 0.0, 0.0);
    }
}
