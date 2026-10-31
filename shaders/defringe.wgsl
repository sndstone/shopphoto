// Targeted longitudinal chromatic-aberration and wide highlight-bloom repair.
// Narrow mode keeps the conservative brighter-neighbour test for ordinary
// lens CA. Wide mode samples both sides of broad, clipped highlight halos.

struct Params {
    fringe: vec4<f32>,
    targets: vec4<f32>,
    out_w: u32,
    out_h: u32,
    wide_bloom: u32,
    _pad0: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(max(rgb, vec3<f32>(0.0)), LUMA);
}

fn saturation(rgb: vec3<f32>) -> f32 {
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    return (hi - lo) / max(hi, 0.025);
}

fn display_luma(rgb: vec3<f32>) -> f32 {
    // A compact display-referred proxy used only to prevent broad repairs
    // from looking brighter after the nonlinear output transform.
    return dot(pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)), LUMA);
}

fn hue(rgb: vec3<f32>) -> f32 {
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    let d = hi - lo;
    if d < 0.00001 { return 0.0; }
    var h = 0.0;
    if hi == rgb.r {
        h = (rgb.g - rgb.b) / d;
    } else if hi == rgb.g {
        h = 2.0 + (rgb.b - rgb.r) / d;
    } else {
        h = 4.0 + (rgb.r - rgb.g) / d;
    }
    return fract(h / 6.0 + 1.0);
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let d = abs(a - b);
    return min(d, 1.0 - d);
}

fn hue_gate(rgb: vec3<f32>, center: f32, half_width: f32) -> f32 {
    let width = max(half_width, 0.0056);
    return 1.0 - smoothstep(width * 0.68, width, hue_distance(hue(rgb), center));
}

fn purple_excess(rgb: vec3<f32>) -> f32 {
    return max(min(rgb.r, rgb.b) - rgb.g * 1.055, 0.0) / max(luma(rgb), 0.025);
}

fn green_excess(rgb: vec3<f32>) -> f32 {
    return max(rgb.g - max(rgb.r, rgb.b) * 1.055, 0.0) / max(luma(rgb), 0.025);
}

fn sample_clamped(p: vec2<i32>) -> vec3<f32> {
    let hi = vec2<i32>(i32(params.out_w) - 1, i32(params.out_h) - 1);
    return max(textureLoad(src_tex, clamp(p, vec2<i32>(0), hi), 0).rgb, vec3<f32>(0.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h { return; }

    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let center4 = textureLoad(src_tex, p, 0);
    let center = max(center4.rgb, vec3<f32>(0.0));
    let center_y = luma(center);
    let center_sat = saturation(center);
    if center_sat < 0.018 {
        textureStore(dst_tex, p, center4);
        return;
    }

    let radius = max(i32(round(params.fringe.z)), 1);
    let half_radius = max(i32(round(f32(radius) * 0.5)), 1);
    let diagonal = max(i32(round(f32(radius) * 0.7071)), 1);
    let half_diagonal = max(i32(round(f32(half_radius) * 0.7071)), 1);
    let offsets = array<vec2<i32>, 16>(
        vec2<i32>( radius, 0), vec2<i32>(-radius, 0),
        vec2<i32>(0,  radius), vec2<i32>(0, -radius),
        vec2<i32>( diagonal,  diagonal), vec2<i32>(-diagonal,  diagonal),
        vec2<i32>( diagonal, -diagonal), vec2<i32>(-diagonal, -diagonal),
        vec2<i32>( half_radius, 0), vec2<i32>(-half_radius, 0),
        vec2<i32>(0,  half_radius), vec2<i32>(0, -half_radius),
        vec2<i32>( half_diagonal,  half_diagonal), vec2<i32>(-half_diagonal,  half_diagonal),
        vec2<i32>( half_diagonal, -half_diagonal), vec2<i32>(-half_diagonal, -half_diagonal)
    );

    let wide = params.wide_bloom != 0u;
    let sample_count = select(8, 16, wide);
    var brightest_y = center_y;
    var darkest_y = center_y;
    var reference_sum = vec3<f32>(0.0);
    var reference_weight = 0.0;
    for (var i = 0; i < sample_count; i = i + 1) {
        let neighbor = sample_clamped(p + offsets[i]);
        let ny = luma(neighbor);
        let ns = saturation(neighbor);
        brightest_y = max(brightest_y, ny);
        darkest_y = min(darkest_y, ny);
        let neutral = 1.0 - smoothstep(0.10, 0.52, ns);
        var weight = 0.0;
        if wide {
            let not_same = smoothstep(0.02, 0.14, abs(ny - center_y) / max(max(ny, center_y), 0.08));
            weight = (0.03 + 0.97 * neutral) * (0.12 + 0.88 * not_same) * (0.08 + ny);
        } else {
            let brighter = smoothstep(center_y * 0.92, center_y + max(0.035, center_y * 0.16), ny);
            weight = brighter * (0.08 + 0.92 * neutral);
        }
        reference_sum = reference_sum + neighbor * weight;
        reference_weight = reference_weight + weight;
    }

    if reference_weight < 0.12 {
        textureStore(dst_tex, p, center4);
        return;
    }

    let reference = reference_sum / reference_weight;
    let reference_y = max(luma(reference), 0.0001);
    let reference_sat = saturation(reference);
    let directional_edge = max(brightest_y - center_y, 0.0) / max(brightest_y, 0.06);
    let wide_edge = (brightest_y - darkest_y) / max(brightest_y, 0.06);
    let edge_signal = select(directional_edge, wide_edge, wide);
    let edge_floor = mix(0.055, 0.30, clamp(params.fringe.w, 0.0, 1.0));
    let edge_gate = smoothstep(edge_floor, edge_floor + 0.13, edge_signal);
    let highlight_gate = smoothstep(0.16, 0.62, brightest_y);
    let outlier_gate = smoothstep(0.025, 0.18, center_sat - reference_sat);

    let purple_target = hue_gate(center, params.targets.x, params.targets.y);
    let green_target = hue_gate(center, params.targets.z, params.targets.w);
    let purple_excess_gate = smoothstep(0.035, 0.20, purple_excess(center) - purple_excess(reference));
    let green_excess_gate = smoothstep(0.035, 0.20, green_excess(center) - green_excess(reference));
    let purple_gate = purple_target * select(purple_excess_gate, 1.0, wide);
    let green_gate = green_target * select(green_excess_gate, 1.0, wide);
    let spatial_mask = edge_gate * highlight_gate * outlier_gate;
    let ceiling = select(0.88, 0.96, wide);
    let correction = clamp(
        spatial_mask * max(purple_gate * params.fringe.x, green_gate * params.fringe.y),
        0.0,
        ceiling
    );

    // Transfer chromaticity only: the repair cannot brighten an edge halo.
    var repair_rgb = max(reference * (center_y / reference_y), vec3<f32>(0.0));
    if wide {
        let perceived_ratio = display_luma(center) / max(display_luma(repair_rgb), 0.0001);
        repair_rgb = repair_rgb * pow(max(perceived_ratio, 0.0), 2.2);
    }
    textureStore(dst_tex, p, vec4<f32>(mix(center, repair_rgb, correction), center4.a));
}
