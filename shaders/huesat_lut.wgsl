// Camera-profile color pass (DNG HueSatDelta/look tables + RGB 3-D LUT).
//
// Input/output: rgba16float, scene-linear Rec.2020. The pass runs twice:
// stage 0 applies the WB-resolved DNG HueSat map before the tone curve;
// stage 1 applies the DNG look table and fitted Reference Match RGB LUT after
// the tone curve. All 3-D interpolation is explicit so rgba16float need not
// be filterable on the adapter.

struct Params {
    // x=hue divisions, y=sat, z=value, w=active.
    huesat_dims: vec4<u32>,
    look_dims: vec4<u32>,
    // x=edge size, w=active.
    rgb_dims: vec4<u32>,
    stage: u32,
    out_w: u32,
    out_h: u32,
    _pad: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;
// DCP textures are laid out x=saturation, y=hue, z=value.
@group(0) @binding(3) var huesat_tex: texture_3d<f32>;
@group(0) @binding(4) var look_tex: texture_3d<f32>;
// RGB LUT is laid out x=blue, y=green, z=red.
@group(0) @binding(5) var rgb_tex: texture_3d<f32>;

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if d > 1e-8 {
        if mx == c.r { h = ((c.g - c.b) / d) % 6.0; }
        else if mx == c.g { h = (c.b - c.r) / d + 2.0; }
        else { h = (c.r - c.g) / d + 4.0; }
        h *= 60.0;
        if h < 0.0 { h += 360.0; }
    }
    return vec3<f32>(h, select(0.0, d / mx, mx > 1e-8), mx);
}

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = ((hsv.x % 360.0) + 360.0) % 360.0;
    let c = hsv.z * hsv.y;
    let hp = h / 60.0;
    let x = c * (1.0 - abs(hp % 2.0 - 1.0));
    var rgb = vec3<f32>(0.0);
    if hp < 1.0 { rgb = vec3<f32>(c, x, 0.0); }
    else if hp < 2.0 { rgb = vec3<f32>(x, c, 0.0); }
    else if hp < 3.0 { rgb = vec3<f32>(0.0, c, x); }
    else if hp < 4.0 { rgb = vec3<f32>(0.0, x, c); }
    else if hp < 5.0 { rgb = vec3<f32>(x, 0.0, c); }
    else { rgb = vec3<f32>(c, 0.0, x); }
    return rgb + vec3<f32>(hsv.z - c);
}

fn dcp_load(use_look: bool, s: u32, h: u32, v: u32) -> vec3<f32> {
    let p = vec3<i32>(i32(s), i32(h), i32(v));
    return select(textureLoad(huesat_tex, p, 0).rgb, textureLoad(look_tex, p, 0).rgb, use_look);
}

fn apply_dcp(rgb_in: vec3<f32>, dims: vec4<u32>, use_look: bool) -> vec3<f32> {
    if dims.w == 0u { return rgb_in; }
    let peak = max(rgb_in.r, max(rgb_in.g, rgb_in.b));
    let scale = max(peak, 1.0);
    let hsv = rgb_to_hsv(rgb_in / scale);
    let hf = hsv.x / 360.0 * f32(dims.x);
    let sf = hsv.y * f32(max(dims.y - 1u, 1u));
    let vf = hsv.z * f32(max(dims.z - 1u, 1u));
    let h0 = u32(hf) % dims.x;
    let h1 = (h0 + 1u) % dims.x;
    let s0 = min(u32(sf), dims.y - 1u);
    let s1 = min(s0 + 1u, dims.y - 1u);
    let v0 = min(u32(vf), dims.z - 1u);
    let v1 = min(v0 + 1u, dims.z - 1u);
    let c000 = dcp_load(use_look, s0, h0, v0);
    let c001 = dcp_load(use_look, s1, h0, v0);
    let c010 = dcp_load(use_look, s0, h1, v0);
    let c011 = dcp_load(use_look, s1, h1, v0);
    let c100 = dcp_load(use_look, s0, h0, v1);
    let c101 = dcp_load(use_look, s1, h0, v1);
    let c110 = dcp_load(use_look, s0, h1, v1);
    let c111 = dcp_load(use_look, s1, h1, v1);
    let c0 = mix(mix(c000, c001, fract(sf)), mix(c010, c011, fract(sf)), fract(hf));
    let c1 = mix(mix(c100, c101, fract(sf)), mix(c110, c111, fract(sf)), fract(hf));
    let delta = mix(c0, c1, fract(vf));
    return hsv_to_rgb(vec3<f32>(
        hsv.x + delta.x,
        clamp(hsv.y * delta.y, 0.0, 1.0),
        max(hsv.z * delta.z, 0.0),
    )) * scale;
}

fn rgb_load(b: u32, g: u32, r: u32) -> vec3<f32> {
    return textureLoad(rgb_tex, vec3<i32>(i32(b), i32(g), i32(r)), 0).rgb;
}

fn apply_rgb_lut(rgb_in: vec3<f32>) -> vec3<f32> {
    if params.rgb_dims.w == 0u { return rgb_in; }
    let peak = max(rgb_in.r, max(rgb_in.g, rgb_in.b));
    let scale = max(peak, 1.0);
    let size = params.rgb_dims.x;
    let f = clamp(rgb_in / scale, vec3<f32>(0.0), vec3<f32>(1.0)) * f32(size - 1u);
    let lo = vec3<u32>(floor(f));
    let hi = min(lo + vec3<u32>(1u), vec3<u32>(size - 1u));
    let t = fract(f);
    let c000 = rgb_load(lo.b, lo.g, lo.r);
    let c001 = rgb_load(hi.b, lo.g, lo.r);
    let c010 = rgb_load(lo.b, hi.g, lo.r);
    let c011 = rgb_load(hi.b, hi.g, lo.r);
    let c100 = rgb_load(lo.b, lo.g, hi.r);
    let c101 = rgb_load(hi.b, lo.g, hi.r);
    let c110 = rgb_load(lo.b, hi.g, hi.r);
    let c111 = rgb_load(hi.b, hi.g, hi.r);
    let c0 = mix(mix(c000, c001, t.b), mix(c010, c011, t.b), t.g);
    let c1 = mix(mix(c100, c101, t.b), mix(c110, c111, t.b), t.g);
    return mix(c0, c1, t.r) * scale;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h { return; }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = textureLoad(src_tex, p, 0).rgb;
    if params.stage == 0u {
        rgb = apply_dcp(rgb, params.huesat_dims, false);
    } else {
        rgb = apply_dcp(rgb, params.look_dims, true);
        rgb = apply_rgb_lut(rgb);
    }
    textureStore(out_tex, p, vec4<f32>(rgb, 1.0));
}
