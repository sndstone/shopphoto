// White balance + camera-matrix color transform + exposure.
//
// Input:  rgba16float, linear camera RGB (normalized, from demosaic).
// Output: rgba16float, scene-referred LINEAR REC.2020 working space
//         (BUILD.md §3.2). Values may exceed 1.0 — do not clamp highlights.
//
// The CPU precomputes cam→Rec2020 = XYZ_TO_REC2020 * cam_to_xyz (resolved
// from the active CameraProfile) and normalized WB gains, and folds the
// exposure multiplier (2^ev) in as a separate uniform so Phase 3's tone LUT
// can move it without touching this shader's interface.

struct Params {
    // Rows of the 3x3 cam→Rec2020 matrix (w unused).
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    // WB gains for camera R/G/B (w = exposure multiplier, linear).
    wb_and_exposure: vec4<f32>,
    out_w: u32,
    out_h: u32,
    // Chroma warp: 1 = sample the grid, 0 = skip it.
    warp_enable: u32,
    _pad0: u32,
    // Grid dimensions in cells (xy) — zw unused.
    warp_dims: vec4<f32>,
    // Chroma range the grid spans: (cb_min, cb_max, cr_min, cr_max).
    warp_range: vec4<f32>,
    // x = gray threshold below which correction fades out.
    warp_gray: vec4<f32>,
    // x enabled, y strength, z neighbour radius.
    recovery: vec4<f32>,
    negative_base: vec4<f32>,
    negative_controls: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;
// Chroma warp grid: one texel per node, rg = warped (cb', cr'). Cb varies
// along x. A 1x1 texture when disabled.
@group(0) @binding(3) var warp_tex: texture_2d<f32>;

// Luminance weights for linear Rec.2020 — the Y row of REC2020_TO_XYZ.
// Must match sp_core::color::REC2020_LUMA; sp-gpu's shader tests compare
// the two.
const REC2020_LUMA = vec3<f32>(0.262700, 0.677998, 0.059302);

// Luma-invariant chromaticity warp (sp_core::profile::ChromaWarp).
//
// Splitting linear RGB into luma Y and its normalized-RGB chromaticity
// n = RGB / (R+G+B) makes the lookup depend on chromaticity alone: scaling
// RGB scales the sum equally, so a colour renders the same in shadow and
// highlight. The warped chromaticity is rescaled back to the pixel's own Y,
// so the stage cannot shift exposure.
//
// Normalized RGB is bounded — every physical colour lands in the unit
// square however dark or saturated — so the grid never clamps a real colour
// to its edge, and the rescale divides by a luma that cannot reach zero.
fn chroma_warp(rgb: vec3<f32>) -> vec3<f32> {
    if params.warp_enable != 1u {
        return rgb;
    }
    let y = dot(REC2020_LUMA, rgb);
    let sum = rgb.r + rgb.g + rgb.b;
    if y <= 1e-6 || sum <= 1e-6 {
        return rgb;
    }
    let chroma = vec2<f32>(rgb.r / sum, rgb.b / sum);

    let dims = params.warp_dims.xy;
    let lo = vec2<f32>(params.warp_range.x, params.warp_range.z);
    let hi = vec2<f32>(params.warp_range.y, params.warp_range.w);
    let t = clamp((chroma - lo) / max(hi - lo, vec2<f32>(1e-6)), vec2<f32>(0.0), vec2<f32>(1.0))
        * (dims - vec2<f32>(1.0));
    let i0 = floor(t);
    let i1 = min(i0 + vec2<f32>(1.0), dims - vec2<f32>(1.0));
    let f = t - i0;
    let c00 = textureLoad(warp_tex, vec2<i32>(i32(i0.x), i32(i0.y)), 0).rg;
    let c10 = textureLoad(warp_tex, vec2<i32>(i32(i1.x), i32(i0.y)), 0).rg;
    let c01 = textureLoad(warp_tex, vec2<i32>(i32(i0.x), i32(i1.y)), 0).rg;
    let c11 = textureLoad(warp_tex, vec2<i32>(i32(i1.x), i32(i1.y)), 0).rg;
    let looked_up = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);

    // Near neutral, chromaticity is mostly noise; correcting it there
    // produces coloured mottle, so fade the effect in with distance from
    // the neutral point.
    let neutral = vec2<f32>(1.0 / 3.0, 1.0 / 3.0);
    let fade = smoothstep(0.0, max(params.warp_gray.x, 1e-5), length(chroma - neutral));
    let warped = mix(chroma, looked_up, fade);

    // Rebuild on the simplex, then rescale to the original luma.
    let n = vec3<f32>(warped.x, 1.0 - warped.x - warped.y, warped.y);
    let n_luma = dot(REC2020_LUMA, n);
    if n_luma <= 1e-4 {
        return rgb;
    }
    return n * (y / n_luma);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let cam = textureLoad(src_tex, p, 0).rgb;

    // Ratio-based clipped-channel reconstruction. Fully clipped pixels are
    // neutral; partially clipped pixels borrow local channel ratios from
    // non-clipped neighbors, preserving scene headroom instead of clamping
    // post-WB channels at 1.0.
    let clipped = cam >= vec3<f32>(0.995);
    let clip_count = u32(clipped.r) + u32(clipped.g) + u32(clipped.b);
    var reconstructed = cam;
    if params.negative_base.w > 0.5 {
        let transmission=max(cam / max(params.negative_base.rgb,vec3<f32>(1e-4)),vec3<f32>(1e-6));
        let density=-log(transmission)/log(10.0);
        reconstructed=pow(max((density-params.negative_controls.y)/params.negative_controls.x,vec3<f32>(0.0)),vec3<f32>(1.0/params.negative_controls.z));
    }
    // Two or more clipped sensor channels no longer carry trustworthy
    // chroma. Mark them for complete neutralization after WB and the camera
    // matrix, where equal Rec.2020 components are guaranteed to be white.
    // Neutralizing as equal camera components is wrong whenever WB gains are
    // unequal and was the source of pink rims around blown highlights.
    let recovery_amount = select(0.0, params.recovery.y / 100.0, params.recovery.x > 0.5 && params.negative_base.w < 0.5);
    var neutral_weight = select(0.0, recovery_amount, clip_count >= 2u);
    if clip_count == 3u && recovery_amount > 0.0 {
        reconstructed = vec3<f32>(1.0) / max(params.wb_and_exposure.rgb, vec3<f32>(1e-5));
        reconstructed = mix(cam, reconstructed, recovery_amount);
    } else if clip_count > 0u && recovery_amount > 0.0 {
        var ratios = vec3<f32>(0.0);
        var valid = 0.0;
        let radius = clamp(i32(round(params.recovery.z)), 1, 32);
        // Sparse exponentially growing rings propagate trustworthy ratios
        // across broad blown regions without an O(radius²) cost per pixel.
        var distance = 1;
        loop {
            for (var oy = -1; oy <= 1; oy++) {
                for (var ox = -1; ox <= 1; ox++) {
                    if ox == 0 && oy == 0 { continue; }
                let q = vec2<i32>(
                        clamp(p.x + ox * distance, 0, i32(params.out_w) - 1),
                        clamp(p.y + oy * distance, 0, i32(params.out_h) - 1),
                );
                let n = textureLoad(src_tex, q, 0).rgb;
                let peak = max(max(n.r, n.g), n.b);
                if peak < 0.98 && peak > 0.02 {
                        let weight = 1.0 / f32(distance);
                        ratios += n / peak * weight;
                        valid += weight;
                    }
                }
            }
            if distance >= radius { break; }
            distance = min(distance * 2, radius);
        }
        if valid > 0.0 {
            ratios /= valid;
            var scale = 0.0;
            if !clipped.r { scale = max(scale, cam.r / max(ratios.r, 1e-4)); }
            if !clipped.g { scale = max(scale, cam.g / max(ratios.g, 1e-4)); }
            if !clipped.b { scale = max(scale, cam.b / max(ratios.b, 1e-4)); }
            let estimated = ratios * scale;
            reconstructed = mix(cam, select(cam, estimated, clipped), recovery_amount);
            let near_clip = smoothstep(0.92, 1.0, max(max(cam.r, cam.g), cam.b));
            // A single reconstructed channel receives a gentle fade toward
            // neutral at the clip boundary. Multi-channel clipping stays
            // fully neutral, independent of whether a usable neighbor exists.
            neutral_weight = max(
                neutral_weight,
                near_clip * f32(clip_count) / 3.0,
            );
        }
    }
    let balanced = reconstructed * params.wb_and_exposure.rgb;
    var rec2020 = vec3<f32>(
        dot(params.m0.xyz, balanced),
        dot(params.m1.xyz, balanced),
        dot(params.m2.xyz, balanced),
    );
    if neutral_weight > 0.0 {
        // Equal components in the linear Rec.2020 working space are D65
        // white. Preserve the reconstructed peak so HDR headroom and the
        // surrounding highlight brightness do not collapse to SDR white.
        let peak = max(max(rec2020.r, rec2020.g), rec2020.b);
        rec2020 = mix(rec2020, vec3<f32>(peak), neutral_weight);
    }
    // The camera rendering: warp applied with the matrix, before tone.
    let rendered = chroma_warp(rec2020);
    let exposed = rendered * params.wb_and_exposure.w;

    // Negative values can appear from the matrix near saturated colors;
    // clamp only the floor, keep highlight headroom.
    textureStore(out_tex, p, vec4<f32>(max(exposed, vec3<f32>(0.0)), 1.0));
}
