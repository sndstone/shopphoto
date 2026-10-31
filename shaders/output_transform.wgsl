// Output transform: working space → display encoding.
//
// Input:  rgba16float, scene-referred linear Rec.2020.
// Output: rgba8unorm, sRGB-ENCODED values (the texture is Rgba8Unorm, not
//         -Srgb, because egui's native-texture registration requires it;
//         we therefore encode in the shader and egui samples/blits as-is).
//
// SDR path: Rec.2020 → Rec.709 primaries, highlight clamp, sRGB OETF,
// then interleaved-gradient-noise dither (±0.5 LSB) to suppress 8-bit
// banding in smooth gradients. Phase 6 adds PQ / extended-linear variants
// behind the same pass id.

struct Params {
    out_w: u32,
    out_h: u32,
    mode: u32,
    debug_mode: u32,
    hdr: vec4<f32>,
    gamut0: vec4<f32>,
    gamut1: vec4<f32>,
    gamut2: vec4<f32>,
    vignette: vec4<f32>,
    grain: vec4<f32>,
    lens_geometry: vec4<f32>,
    lateral_ca: vec4<f32>,
    crop: vec4<f32>,
    image_rect: vec4<f32>,
    proof_flags: vec4<u32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var proof_tex: texture_3d<f32>;

fn apply_proof_lut(rgb_in: vec3<f32>) -> vec3<f32> {
    if params.proof_flags.y == 0u { return rgb_in; }
    let peak = max(rgb_in.r, max(rgb_in.g, rgb_in.b));
    let scale = max(peak, 1.0);
    let size = params.proof_flags.y;
    let f = clamp(rgb_in / scale, vec3<f32>(0.0), vec3<f32>(1.0)) * f32(size - 1u);
    let lo = vec3<u32>(floor(f));
    let hi = min(lo + vec3<u32>(1u), vec3<u32>(size - 1u));
    let t = fract(f);
    let c000 = textureLoad(proof_tex, vec3<i32>(i32(lo.b), i32(lo.g), i32(lo.r)), 0).rgb;
    let c001 = textureLoad(proof_tex, vec3<i32>(i32(hi.b), i32(lo.g), i32(lo.r)), 0).rgb;
    let c010 = textureLoad(proof_tex, vec3<i32>(i32(lo.b), i32(hi.g), i32(lo.r)), 0).rgb;
    let c011 = textureLoad(proof_tex, vec3<i32>(i32(hi.b), i32(hi.g), i32(lo.r)), 0).rgb;
    let c100 = textureLoad(proof_tex, vec3<i32>(i32(lo.b), i32(lo.g), i32(hi.r)), 0).rgb;
    let c101 = textureLoad(proof_tex, vec3<i32>(i32(hi.b), i32(lo.g), i32(hi.r)), 0).rgb;
    let c110 = textureLoad(proof_tex, vec3<i32>(i32(lo.b), i32(hi.g), i32(hi.r)), 0).rgb;
    let c111 = textureLoad(proof_tex, vec3<i32>(i32(hi.b), i32(hi.g), i32(hi.r)), 0).rgb;
    let c0 = mix(mix(c000, c001, t.b), mix(c010, c011, t.b), t.g);
    let c1 = mix(mix(c100, c101, t.b), mix(c110, c111, t.b), t.g);
    return mix(c0, c1, t.r) * scale;
}

fn lens_load(uv: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(src_tex));
    let local = (uv + vec2<f32>(1.0)) * 0.5 * size - params.image_rect.xy - vec2<f32>(0.5);
    return textureLoad(src_tex, clamp(vec2<i32>(round(local)), vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb;
}

fn load_lens_corrected(p: vec2<i32>) -> vec3<f32> {
    if params.lens_geometry.w < 0.5 ||
       (all(abs(params.lens_geometry.xyz) < vec3<f32>(1e-8)) && all(abs(params.lateral_ca.xy) < vec2<f32>(1e-8))) {
        return textureLoad(src_tex, p, 0).rgb;
    }
    let size = max(params.image_rect.zw, vec2<f32>(1.0));
    let global = vec2<f32>(p) + params.image_rect.xy + vec2<f32>(0.5);
    var centered = global / size * 2.0 - vec2<f32>(1.0);
    if params.lens_geometry.w > 1.5 {
        let edge_radial = abs(1.0 + params.lens_geometry.x * 2.0 + params.lens_geometry.y * 4.0 + params.lens_geometry.z * 8.0);
        let edge_ca = max(abs(1.0 + params.lateral_ca.x * 2.0), abs(1.0 + params.lateral_ca.y * 2.0));
        centered /= max(1.0, edge_radial * edge_ca);
    }
    let r2 = dot(centered, centered);
    let radial = 1.0 + params.lens_geometry.x * r2 + params.lens_geometry.y * r2 * r2 + params.lens_geometry.z * r2 * r2 * r2;
    let green = lens_load(centered * radial, size);
    let red = lens_load(centered * radial * (1.0 + params.lateral_ca.x * r2), size).r;
    let blue = lens_load(centered * radial * (1.0 + params.lateral_ca.y * r2), size).b;
    return vec3<f32>(red, green.g, blue);
}

fn gamut_matrix() -> mat3x3<f32> {
    return mat3x3<f32>(
        vec3<f32>(params.gamut0.x, params.gamut1.x, params.gamut2.x),
        vec3<f32>(params.gamut0.y, params.gamut1.y, params.gamut2.y),
        vec3<f32>(params.gamut0.z, params.gamut1.z, params.gamut2.z));
}

fn apply_vignette(rgb: vec3<f32>, gid: vec2<u32>) -> vec3<f32> {
    if abs(params.vignette.x) < 1e-6 { return rgb; }
    let global_px = vec2<f32>(f32(gid.x), f32(gid.y)) + params.image_rect.xy;
    let uv = (global_px + vec2<f32>(0.5)) / max(params.image_rect.zw, vec2<f32>(1.0));
    let crop_size = max(params.crop.zw - params.crop.xy, vec2<f32>(1e-4));
    let local = (uv - params.crop.xy) / crop_size;
    let centered = (local - vec2<f32>(0.5)) * 2.0;
    let circular = length(centered) / 1.41421356;
    let rectangular = max(abs(centered.x), abs(centered.y));
    let radius = mix(rectangular, circular, params.vignette.z / 100.0);
    let midpoint = mix(0.15, 0.8, params.vignette.y / 100.0);
    let edge = midpoint + max(0.02, (1.0 - midpoint) * params.vignette.w / 100.0);
    let mask = smoothstep(midpoint, edge, radius);
    let gain = exp2(params.vignette.x / 100.0 * 1.5 * mask);
    let luma = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    let highlight_priority = 1.0 / (1.0 + max(luma - 1.0, 0.0) * 2.0);
    return rgb * mix(1.0, gain, highlight_priority);
}

// Interleaved gradient noise (Jimenez 2014): cheap, blue-noise-like
// spatial distribution, stable per pixel.
fn ign(px: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * px.x + 0.00583715 * px.y));
}

fn apply_grain(rgb: vec3<f32>, gid: vec2<u32>) -> vec3<f32> {
    if params.grain.x <= 0.0 { return rgb; }
    let global = vec2<f32>(gid) + params.image_rect.xy;
    let cell = floor(global / max(params.grain.y, 0.5));
    let fine = ign(cell + vec2<f32>(params.grain.w, params.grain.w * 0.37)) - 0.5;
    let coarse = ign(floor(cell * 0.5) + vec2<f32>(params.grain.w * 1.7)) - 0.5;
    let noise = mix(fine, coarse, params.grain.z / 100.0);
    let luma = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    let tonal = 0.35 + 0.65 * (1.0 - smoothstep(0.65, 1.2, luma));
    return max(rgb + vec3<f32>(noise * params.grain.x / 100.0 * 0.06 * tonal), vec3<f32>(0.0));
}

fn srgb_oetf(x: f32) -> f32 {
    if x <= 0.0031308 {
        return 12.92 * x;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

// proof_flags.z selects the SDR transfer: 0 = sRGB, 1 = Adobe RGB (1998)
// pure gamma 563/256. Must match sp_core::color::Transfer::shader_id.
fn output_oetf(x: f32) -> f32 {
    if params.proof_flags.z == 1u {
        return pow(max(x, 0.0), 1.0 / 2.19921875);
    }
    return srgb_oetf(x);
}

fn pq_oetf(x: f32) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 32.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 128.0;
    let c3 = 2392.0 / 128.0;
    let p = pow(max(x, 0.0), m1);
    return pow((c1 + c2 * p) / (1.0 + c3 * p), m2);
}

fn hdr_to_sdr(x: vec3<f32>) -> vec3<f32> {
    let rgb = max(x, vec3<f32>(0.0));
    // This runs in the scene-linear Rec.2020 working space, before the
    // selected proof/export gamut transform.
    let luma = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    let softness = clamp(params.hdr.y / 100.0, 0.0, 1.0);
    let knee = mix(0.90, 0.72, softness);
    let span = max(1.0 - knee, 1e-4);
    let u = max(luma - knee, 0.0) / span;
    let mapped_luma = select(luma, knee + span * tanh(u), luma > knee);
    return rgb * select(1.0, mapped_luma / luma, luma > knee);
}

// Same scene-linear, hue-preserving shoulder as the EDR preview. Reference
// white stays near 1.0; real values above it are progressively compressed
// into the selected content headroom before PQ encoding.
fn tone_map_hdr(rgb_in: vec3<f32>) -> vec3<f32> {
    let ceiling = exp2(clamp(params.hdr.x, 0.0, 4.0));
    if ceiling <= 1.001 {
        return clamp(rgb_in, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let rgb = max(rgb_in, vec3<f32>(0.0));
    let luma = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    let softness = clamp(params.hdr.y / 100.0, 0.0, 1.0);
    let knee = mix(1.0, 0.82, softness);
    let span = max(ceiling - knee, 1e-4);
    let curved_span = span * 0.999;
    let u = max(luma - knee, 0.0) / curved_span;
    let shoulder = knee + curved_span * tanh(u);
    let mapped_luma = select(luma, shoulder, luma > knee);
    var out_rgb = rgb * select(1.0, mapped_luma / luma, luma > knee);
    let peak = max(max(out_rgb.r, out_rgb.g), out_rgb.b);
    out_rgb *= min(1.0, ceiling / max(peak, 1e-6));
    return out_rgb;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    var rec2020 = load_lens_corrected(p);
    rec2020 = apply_vignette(rec2020, gid.xy);
    rec2020 = apply_grain(rec2020, gid.xy);
    let scene_white_ev = select(0.0, params.hdr.z, params.mode != 0u);
    rec2020 *= exp2(-scene_white_ev);
    rec2020 = apply_proof_lut(rec2020);

    let white_target = select(1.0, exp2(params.hdr.x), params.mode != 0u);
    if params.debug_mode != 0u {
        if params.debug_mode != 1u && any(rec2020 >= vec3<f32>(white_target)) {
            textureStore(out_tex, p, vec4<f32>(1.0, 0.0, 0.0, 1.0));
        } else if params.debug_mode != 2u && any(rec2020 <= vec3<f32>(0.001)) {
            textureStore(out_tex, p, vec4<f32>(0.0, 0.0, 1.0, 1.0));
        } else {
            textureStore(out_tex, p, vec4<f32>(0.0, 0.0, 0.0, 1.0));
        }
        return;
    }

    if params.mode == 2u {
        // Rec.2100 PQ, reference diffuse white = 203 cd/m².
        let absolute = tone_map_hdr(rec2020) * (203.0 / 10000.0);
        let encoded_pq = vec3<f32>(pq_oetf(absolute.r), pq_oetf(absolute.g), pq_oetf(absolute.b));
        textureStore(out_tex, p, vec4<f32>(clamp(encoded_pq, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
        return;
    }
    if params.mode == 1u {
        rec2020 = hdr_to_sdr(rec2020);
    }
    var proof_rgb = gamut_matrix() * rec2020;
    if params.proof_flags.x == 1u && any(proof_rgb < vec3<f32>(0.0)) {
        textureStore(out_tex, p, vec4<f32>(1.0, 0.0, 1.0, 1.0));
        return;
    }
    if params.mode == 1u {
        proof_rgb = max(proof_rgb, vec3<f32>(0.0));
        let peak = max(max(proof_rgb.r, proof_rgb.g), proof_rgb.b);
        proof_rgb *= min(1.0, 1.0 / max(peak, 1e-6));
    } else {
        proof_rgb = clamp(proof_rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let encoded = vec3<f32>(
        output_oetf(proof_rgb.r),
        output_oetf(proof_rgb.g),
        output_oetf(proof_rgb.b),
    );
    let dither = (ign(vec2<f32>(f32(gid.x), f32(gid.y)) + params.image_rect.xy) - 0.5) / 255.0;
    textureStore(out_tex, p, vec4<f32>(clamp(encoded + dither, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
