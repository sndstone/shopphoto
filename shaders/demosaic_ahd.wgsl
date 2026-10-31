// Adaptive homogeneity-directed Bayer demosaic for high-detail 1:1 and
// export rendering. Two directional color-difference interpolations are
// evaluated in CIELAB after white balance and the camera matrix; the more
// homogeneous 3x3 neighbourhood wins. The output remains linear camera RGB.

struct Params {
    out_w: u32,
    out_h: u32,
    cfa: u32,
    _pad0: u32,
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    wb: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn cfa_color(p: vec2<i32>) -> u32 {
    let idx = u32(p.y & 1) * 2u + u32(p.x & 1);
    switch params.cfa {
        case 0u: { return select(select(0u, 1u, idx == 1u || idx == 2u), 2u, idx == 3u); }
        case 1u: { return select(select(2u, 1u, idx == 1u || idx == 2u), 0u, idx == 3u); }
        case 2u: {
            switch idx {
                case 0u: { return 1u; }
                case 1u: { return 0u; }
                case 2u: { return 2u; }
                default: { return 1u; }
            }
        }
        default: {
            switch idx {
                case 0u: { return 1u; }
                case 1u: { return 2u; }
                case 2u: { return 0u; }
                default: { return 1u; }
            }
        }
    }
}

fn mirror(i: i32, n: i32) -> i32 {
    if n < 2 { return 0; }
    let period = 2 * (n - 1);
    var value = ((i % period) + period) % period;
    if value >= n { value = period - value; }
    return value;
}

fn tap(p: vec2<i32>) -> f32 {
    return textureLoad(
        src_tex,
        vec2<i32>(mirror(p.x, i32(params.out_w)), mirror(p.y, i32(params.out_h))),
        0,
    ).r;
}

fn green_at(p: vec2<i32>, horizontal: bool) -> f32 {
    if cfa_color(p) == 1u { return tap(p); }
    let d = select(vec2<i32>(0, 1), vec2<i32>(1, 0), horizontal);
    let near = 0.5 * (tap(p - d) + tap(p + d));
    let correction = 0.25 * (2.0 * tap(p) - tap(p - 2 * d) - tap(p + 2 * d));
    return max(near + correction, 0.0);
}

fn color_at_green(p: vec2<i32>, wanted: u32) -> f32 {
    var d = vec2<i32>(1, 0);
    if cfa_color(p - d) != wanted { d = vec2<i32>(0, 1); }
    let near = 0.5 * (tap(p - d) + tap(p + d));
    let correction = 0.25 * (2.0 * tap(p) - tap(p - 2 * d) - tap(p + 2 * d));
    return max(near + correction, 0.0);
}

fn opposite_delta(p: vec2<i32>, horizontal: bool) -> f32 {
    return tap(p) - green_at(p, horizontal);
}

fn candidate(p: vec2<i32>, horizontal: bool) -> vec3<f32> {
    let color = cfa_color(p);
    if color == 1u {
        return vec3<f32>(color_at_green(p, 0u), tap(p), color_at_green(p, 2u));
    }
    let green = green_at(p, horizontal);
    let opposite = max(green + 0.25 * (
        opposite_delta(p + vec2<i32>(-1, -1), horizontal) +
        opposite_delta(p + vec2<i32>( 1, -1), horizontal) +
        opposite_delta(p + vec2<i32>(-1,  1), horizontal) +
        opposite_delta(p + vec2<i32>( 1,  1), horizontal)
    ), 0.0);
    if color == 0u { return vec3<f32>(tap(p), green, opposite); }
    return vec3<f32>(opposite, green, tap(p));
}

fn lab_f(value: f32) -> f32 {
    if value > 0.008856 { return pow(value, 1.0 / 3.0); }
    return 7.787 * value + 16.0 / 116.0;
}

fn cielab(rgb: vec3<f32>) -> vec3<f32> {
    let balanced = max(rgb * params.wb.xyz, vec3<f32>(0.0));
    let xyz = max(vec3<f32>(
        dot(params.m0.xyz, balanced),
        dot(params.m1.xyz, balanced),
        dot(params.m2.xyz, balanced),
    ), vec3<f32>(0.0));
    let f = vec3<f32>(
        lab_f(xyz.x / 0.95047),
        lab_f(xyz.y),
        lab_f(xyz.z / 1.08883),
    );
    return vec3<f32>(
        (116.0 * f.y - 16.0) / 100.0,
        500.0 * (f.x - f.y) / 128.0,
        200.0 * (f.y - f.z) / 128.0,
    );
}

fn homogeneity(p: vec2<i32>, horizontal: bool, l_eps: f32, c_eps: f32) -> u32 {
    let center = cielab(candidate(p, horizontal));
    var score = 0u;
    for (var oy = -1; oy <= 1; oy++) {
        for (var ox = -1; ox <= 1; ox++) {
            if ox == 0 && oy == 0 { continue; }
            let neighbour = cielab(candidate(p + vec2<i32>(ox, oy), horizontal));
            if abs(neighbour.x - center.x) <= l_eps && length(neighbour.yz - center.yz) <= c_eps {
                score += 1u;
            }
        }
    }
    return score;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h { return; }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));

    let h_left = cielab(candidate(p + vec2<i32>(-1, 0), true));
    let h_right = cielab(candidate(p + vec2<i32>(1, 0), true));
    let v_up = cielab(candidate(p + vec2<i32>(0, -1), false));
    let v_down = cielab(candidate(p + vec2<i32>(0, 1), false));
    let l_eps = min(abs(h_left.x - h_right.x), abs(v_up.x - v_down.x)) + 0.015;
    let c_eps = min(length(h_left.yz - h_right.yz), length(v_up.yz - v_down.yz)) + 0.015;
    let h_score = homogeneity(p, true, l_eps, c_eps);
    let v_score = homogeneity(p, false, l_eps, c_eps);
    let horizontal = candidate(p, true);
    let vertical = candidate(p, false);
    var rgb = 0.5 * (horizontal + vertical);
    if h_score > v_score { rgb = horizontal; }
    if v_score > h_score { rgb = vertical; }
    textureStore(out_tex, p, vec4<f32>(max(rgb, vec3<f32>(0.0)), 1.0));
}
