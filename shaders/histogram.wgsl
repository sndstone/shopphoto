// Live histogram plus explicit multi-scale fringe analysis.
// Planes: R, G, B, luminance, purple score, green score, purple width,
// green width. The last four are populated only by Auto Defringe.

struct Params {
    src_w: u32,
    src_h: u32,
    fringe_analysis: u32,
    working_w: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>, 2048>;
@group(0) @binding(2) var<uniform> params: Params;

fn srgb_oetf(x: f32) -> f32 {
    if x <= 0.0031308 { return 12.92 * max(x, 0.0); }
    return 1.055 * pow(max(x, 0.0), 1.0 / 2.4) - 0.055;
}

fn bin_of(x: f32) -> u32 {
    if x <= 1.0 {
        return u32(clamp(srgb_oetf(x) * 223.0, 0.0, 223.0));
    }
    return 224u + u32(clamp(log2(x) * (31.0 / 4.0), 0.0, 31.0));
}

const LUMA: vec3<f32> = vec3<f32>(0.2627, 0.6780, 0.0593);

fn luma(rgb: vec3<f32>) -> f32 {
    return dot(max(rgb, vec3<f32>(0.0)), LUMA);
}

fn saturation(rgb: vec3<f32>) -> f32 {
    let hi = max(rgb.r, max(rgb.g, rgb.b));
    let lo = min(rgb.r, min(rgb.g, rgb.b));
    return (hi - lo) / max(hi, 0.025);
}

fn purple_excess(rgb: vec3<f32>) -> f32 {
    return max(min(rgb.r, rgb.b) - rgb.g * 1.055, 0.0) / max(luma(rgb), 0.025);
}

fn green_excess(rgb: vec3<f32>) -> f32 {
    return max(rgb.g - max(rgb.r, rgb.b) * 1.055, 0.0) / max(luma(rgb), 0.025);
}

fn sample_clamped(p: vec2<i32>) -> vec3<f32> {
    let hi = vec2<i32>(i32(params.src_w) - 1, i32(params.src_h) - 1);
    return max(textureLoad(src_tex, clamp(p, vec2<i32>(0), hi), 0).rgb, vec3<f32>(0.0));
}

fn confidence_bin(value: f32) -> u32 {
    return u32(round(clamp(value, 0.0, 1.0) * 255.0));
}

fn analyze_fringe_at(p: vec2<i32>, center: vec3<f32>, radius: i32) -> vec2<f32> {
    let center_y = luma(center);
    let center_sat = saturation(center);
    let diagonal = max(i32(round(f32(radius) * 0.7071)), 1);
    let offsets = array<vec2<i32>, 8>(
        vec2<i32>( radius, 0), vec2<i32>(-radius, 0),
        vec2<i32>(0, radius), vec2<i32>(0, -radius),
        vec2<i32>(diagonal, diagonal), vec2<i32>(-diagonal, diagonal),
        vec2<i32>(diagonal, -diagonal), vec2<i32>(-diagonal, -diagonal)
    );
    var brightest_y = center_y;
    var darkest_y = center_y;
    var reference_sum = vec3<f32>(0.0);
    var reference_weight = 0.0;
    for (var i = 0; i < 8; i = i + 1) {
        let neighbor = sample_clamped(p + offsets[i]);
        let ny = luma(neighbor);
        let ns = saturation(neighbor);
        brightest_y = max(brightest_y, ny);
        darkest_y = min(darkest_y, ny);
        let neutral = 1.0 - smoothstep(0.10, 0.50, ns);
        let different = smoothstep(0.025, 0.15, abs(ny - center_y) / max(max(ny, center_y), 0.08));
        let weight = (0.03 + 0.97 * neutral) * (0.10 + 0.90 * different) * (0.08 + ny);
        reference_sum = reference_sum + neighbor * weight;
        reference_weight = reference_weight + weight;
    }
    if reference_weight < 0.12 { return vec2<f32>(0.0); }
    let reference = reference_sum / reference_weight;
    let edge = (brightest_y - darkest_y) / max(brightest_y, 0.06);
    let edge_gate = smoothstep(0.19, 0.34, edge);
    let highlight_gate = smoothstep(0.16, 0.62, brightest_y);
    let outlier_gate = smoothstep(0.035, 0.20, center_sat - saturation(reference));
    let spatial = edge_gate * highlight_gate * outlier_gate;
    let purple = spatial * smoothstep(0.035, 0.20, purple_excess(center) - purple_excess(reference));
    let green = spatial * smoothstep(0.035, 0.20, green_excess(center) - green_excess(reference));
    return vec2<f32>(purple, green);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.src_w || gid.y >= params.src_h { return; }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let rgb = textureLoad(src_tex, p, 0).rgb;
    let idx = vec3<u32>(bin_of(rgb.r), bin_of(rgb.g), bin_of(rgb.b));
    atomicAdd(&bins[idx.r], 1u);
    atomicAdd(&bins[256u + idx.g], 1u);
    atomicAdd(&bins[512u + idx.b], 1u);
    atomicAdd(&bins[768u + bin_of(luma(rgb))], 1u);

    if params.fringe_analysis != 0u && (gid.x & 1u) == 0u && (gid.y & 1u) == 0u {
        let center = max(rgb, vec3<f32>(0.0));
        let radii = array<i32, 5>(2, 6, 14, 30, 56);
        var best_purple = 0.0;
        var best_green = 0.0;
        var purple_radius = 0.0;
        var green_radius = 0.0;
        for (var i = 0; i < 5; i = i + 1) {
            let score = analyze_fringe_at(p, center, radii[i]);
            if score.x > best_purple {
                best_purple = score.x;
                purple_radius = f32(radii[i]);
            }
            if score.y > best_green {
                best_green = score.y;
                green_radius = f32(radii[i]);
            }
        }
        let full_scale = f32(params.working_w) / max(f32(params.src_w), 1.0);
        let purple_width = select(0.0, clamp(purple_radius * full_scale / 256.0, 0.0, 1.0), best_purple >= 0.12);
        let green_width = select(0.0, clamp(green_radius * full_scale / 256.0, 0.0, 1.0), best_green >= 0.12);
        atomicAdd(&bins[1024u + confidence_bin(best_purple)], 1u);
        atomicAdd(&bins[1280u + confidence_bin(best_green)], 1u);
        atomicAdd(&bins[1536u + confidence_bin(purple_width)], 1u);
        atomicAdd(&bins[1792u + confidence_bin(green_width)], 1u);
    }
}
