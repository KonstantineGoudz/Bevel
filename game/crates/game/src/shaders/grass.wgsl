#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    mesh_functions,
    mesh_view_bindings::{lights, view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    view_transformations::position_world_to_clip,
}

struct GrassUniforms {
    // (time, wind_speed, wind_strength, gust_scale)
    wind_params: vec4<f32>,
    // (dir_x, dir_z, flutter_strength, flutter_speed)
    wind_dir: vec4<f32>,
    // (x, y, z, interact_radius)
    player_pos: vec4<f32>,
    // (bend_strength, squash_strength, unused, unused)
    interact_params: vec4<f32>,
    root_color: vec4<f32>,
    tip_color: vec4<f32>,
    subsurface_color: vec4<f32>,
    // (patch_scale, patch_variation, ao_strength, subsurface_strength)
    color_params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> grass: GrassUniforms;

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm2(p: vec2<f32>) -> f32 {
    var v = 0.0;
    v += 0.65 * value_noise(p);
    v += 0.35 * value_noise(p * 2.17 + vec2<f32>(13.7, 7.3));
    return v;
}

@vertex
fn vertex(vertex_no_morph: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(vertex_no_morph, vertex_no_morph.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_no_morph.instance_index);

    var world_pos = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0)
    );

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex_no_morph.instance_index
    );
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
    let h = clamp(vertex.uv.y, 0.0, 1.0);
#else
    let h = 0.0;
#endif

    // Quadratic bend keeps the root anchored in soil while curving the blade smoothly.
    let bend = h * h;
    let time = grass.wind_params.x;
    let wind_speed = grass.wind_params.y;
    let wind_strength = grass.wind_params.z;
    let gust_scale = grass.wind_params.w;

    let dir = normalize(grass.wind_dir.xy + vec2<f32>(1e-5, 0.0));
    let ortho = vec2<f32>(-dir.y, dir.x);
    let flutter_strength = grass.wind_dir.z;
    let flutter_speed = grass.wind_dir.w;

    // Primary rolling wind gusts + secondary cross-wave + high-frequency tip flutter.
    let gust_phase = dot(world_pos.xz, dir) * gust_scale - time * wind_speed;
    let cross_phase = dot(world_pos.xz, ortho) * (gust_scale * 0.7) - time * (wind_speed * 0.65);
    let gust = sin(gust_phase) * 0.65 + sin(gust_phase * 2.13 + cross_phase) * 0.35;
    let gust_dir = dir * (gust * 0.8 + 0.35) + ortho * (sin(cross_phase) * 0.25);

    let flutter_phase = time * flutter_speed + world_pos.x * 4.7 + world_pos.z * 3.9;
    let flutter = vec2<f32>(sin(flutter_phase), cos(flutter_phase * 1.17)) * flutter_strength * h;

    var disp_xz = (gust_dir * wind_strength + flutter) * bend;

    // Dynamic character interaction: bend blades radially away from player_pos.
    let interact_radius = grass.player_pos.w;
    if interact_radius > 0.0 {
        let to_blade = world_pos.xz - grass.player_pos.xz;
        let dist = length(to_blade);
        let y_diff = abs(world_pos.y - grass.player_pos.y);
        if dist < interact_radius && y_diff < 2.2 {
            let t = 1.0 - dist / interact_radius;
            let push = t * t;
            let push_dir = select(vec2<f32>(1.0, 0.0), to_blade / dist, dist > 0.001);
            disp_xz += push_dir * (push * grass.interact_params.x * bend);
            world_pos.y -= push * grass.interact_params.y * bend;
        }
    }

    // Approximate arc-length preservation so blades arc downward instead of stretching.
    world_pos.x += disp_xz.x;
    world_pos.z += disp_xz.y;
    world_pos.y -= min(dot(disp_xz, disp_xz) * 0.35, 0.22 * h);

    out.world_position = world_pos;
    out.position = position_world_to_clip(out.world_position.xyz);

#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex_no_morph.instance_index
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex_no_morph.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_no_morph.instance_index,
        mesh_world_from_local[3]
    );
#endif

    return out;
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    let in = vertex_output;
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

#ifdef VERTEX_UVS_A
    let h = clamp(in.uv.y, 0.0, 1.0);
#else
    let h = 0.5;
#endif

    // World-space patchiness to break up tiling across large grass fields.
    let patch_scale = grass.color_params.x;
    let patch_var = grass.color_params.y;
    let ao_strength = grass.color_params.z;
    let sss_strength = grass.color_params.w;

    let patch_n = fbm2(in.world_position.xz * patch_scale);
    let patch_shift = (patch_n - 0.5) * 2.0 * patch_var;

    // Root-to-tip gradient + subtle warm/dry vs lush patch tinting.
    let grad_color = mix(grass.root_color.rgb, grass.tip_color.rgb, smoothstep(0.0, 1.0, h));
    let lush_warm = vec3<f32>(1.0 + patch_shift * 0.45, 1.0 + patch_shift * 0.25, 1.0 - patch_shift * 0.35);
    let ao = mix(1.0 - ao_strength, 1.0, smoothstep(0.0, 0.55, h));

    pbr_input.material.base_color = vec4<f32>(
        pbr_input.material.base_color.rgb * grad_color * lush_warm * ao,
        pbr_input.material.base_color.a
    );

    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);

        // Backlit subsurface scattering / translucency through thin grass tips.
        if lights.n_directional_lights > 0u && sss_strength > 0.0 {
            let v_dir = normalize(view.world_position - in.world_position.xyz);
            let l_dir = normalize(lights.directional_lights[0].direction_to_light);
            let backlit = pow(max(dot(v_dir, -l_dir), 0.0), 3.0);
            let tip_mask = smoothstep(0.15, 1.0, h);
            let sss = grass.subsurface_color.rgb * lights.directional_lights[0].color.rgb
                * (backlit * tip_mask * sss_strength * 0.00012);
            out.color = vec4<f32>(out.color.rgb + sss, out.color.a);
        }
    } else {
        out.color = pbr_input.material.base_color;
    }

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
