// Noise reduction (Phase 4): luminance NLM-lite + chroma bilateral, one pass.
//
// Input:  rgba16float, scene-linear Rec.2020 (after wb/profile, BEFORE tone —
//         noise statistics are still radiometric here, which keeps slider
//         behavior consistent across exposure/curve settings).
// Output: rgba16float, scene-linear Rec.2020.
//
// Luminance: non-local-means "lite" — 3×3 patch distances over a 5×5
// search window on log-luma, blended by `luminance`; `luminance_detail`
// blends the high-frequency residual back in.
// Chroma: edge-aware bilateral over the same window on Oklab a/b only
// (luma edges gate the kernel), driven by `chroma`/`chroma_detail`.
// Zero-strength axes are exact pass-throughs; the whole pass is culled by
// the CPU when both are zero.
//
// BUILD.md §10.3: the 16×16 tile plus its 3 px apron is staged in workgroup
// shared memory as pre-computed Oklab + cube-root luma. Each thread then
// reads its 25 (luma) + 49 (chroma) taps out of shared memory instead of
// re-loading and re-transforming global memory — the Oklab transform runs
// 484 times per workgroup instead of ~12 500.
//
// BUILD.md §10.2: `fp` is f16 wherever the adapter reports SHADER_F16 (all
// Apple GPUs), which halves the shared-memory footprint and runs the tap
// loops at double rate. Weight accumulators stay f32 so output precision
// does not depend on the adapter, and the reconstruction below mixes with
// exact f32 center values.
//#SP_PRECISION

struct Params {
    // x = luminance 0..100, y = luminance_detail 0..100,
    // z = chroma 0..100, w = chroma_detail 0..100.
    nr: vec4<f32>,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

const REC2020_TO_REC709 = mat3x3<f32>(
    vec3<f32>(1.660491, -0.124550, -0.018151),
    vec3<f32>(-0.587641, 1.132900, -0.100579),
    vec3<f32>(-0.072850, -0.008349, 1.118730),
);
const REC709_TO_REC2020 = mat3x3<f32>(
    vec3<f32>(0.627404, 0.069097, 0.016391),
    vec3<f32>(0.329283, 0.919540, 0.088013),
    vec3<f32>(0.043313, 0.011362, 0.895595),
);
const OK_M1 = mat3x3<f32>(
    vec3<f32>(0.4122214708, 0.2119034982, 0.0883024619),
    vec3<f32>(0.5363325363, 0.6806995451, 0.2817188376),
    vec3<f32>(0.0514459929, 0.1073969566, 0.6299787005),
);
const OK_M2 = mat3x3<f32>(
    vec3<f32>(0.2104542553, 1.9779984951, 0.0259040371),
    vec3<f32>(0.7936177850, -2.4285922050, 0.7827717662),
    vec3<f32>(-0.0040720468, 0.4505937099, -0.8086757660),
);
const OK_M2_INV = mat3x3<f32>(
    vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(0.3963377774, -0.1055613458, -0.0894841775),
    vec3<f32>(0.2158037573, -0.0638541728, -1.2914855480),
);
const OK_M1_INV = mat3x3<f32>(
    vec3<f32>(4.0767416621, -1.2684380046, -0.0041960863),
    vec3<f32>(-3.3077115913, 2.6097574011, -0.7034186147),
    vec3<f32>(0.2309699292, -0.3413193965, 1.7076147010),
);

// Workgroup tile geometry. The apron covers the widest reach in this
// shader: chroma's 7×7 bilateral, and luminance's 5×5 search extended by
// the 3×3 patch (2 + 1).
const WG_DIM: i32 = 16;
const APRON: i32 = 3;
const TILE_DIM: i32 = 22;     // WG_DIM + 2 * APRON
const TILE_LEN: u32 = 484u;   // TILE_DIM * TILE_DIM

// xyz = Oklab (L, a, b), w = cube-root luma.
var<workgroup> tile: array<fp4, TILE_LEN>;

fn load_rgb(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(params.out_w) - 1);
    let cy = clamp(y, 0, i32(params.out_h) - 1);
    return textureLoad(src_tex, vec2<i32>(cx, cy), 0).rgb;
}

fn oklab_of(rgb2020: vec3<f32>) -> vec3<f32> {
    let srgb_lin = max(REC2020_TO_REC709 * rgb2020, vec3<f32>(0.0));
    let lms = OK_M1 * srgb_lin;
    let lms_c = pow(max(lms, vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
    return OK_M2 * lms_c;
}

// Perceptual-ish luma proxy for patch distances (cheap cube-root luma).
fn luma_of(rgb2020: vec3<f32>) -> f32 {
    let y = max(dot(rgb2020, vec3<f32>(0.2627, 0.6780, 0.0593)), 0.0);
    return pow(y, 1.0 / 3.0);
}

// Shared-memory index of the tap at (sx, sy) relative to a thread's pixel.
fn tap(local: vec2<i32>, sx: i32, sy: i32) -> u32 {
    return u32((local.y + APRON + sy) * TILE_DIM + (local.x + APRON + sx));
}

// 3×3 patch squared distance in luma between a thread's own pixel and the
// candidate at (sx, sy). Shared memory only.
fn patch_dist(local: vec2<i32>, sx: i32, sy: i32) -> fp {
    var d = fp(0.0);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let la = tile[tap(local, dx, dy)].w;
            let lb = tile[tap(local, sx + dx, sy + dy)].w;
            let e = la - lb;
            d += e * e;
        }
    }
    return d / fp(9.0);
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) lindex: u32,
) {
    // Stage the tile + apron. EVERY thread participates, including threads
    // whose own pixel lies outside the image, so the barrier below stays in
    // uniform control flow.
    let base = vec2<i32>(i32(wid.x) * WG_DIM - APRON, i32(wid.y) * WG_DIM - APRON);
    for (var i = lindex; i < TILE_LEN; i += 256u) {
        let t = vec2<i32>(i32(i) % TILE_DIM, i32(i) / TILE_DIM);
        let rgb = load_rgb(base.x + t.x, base.y + t.y);
        tile[i] = fp4(fp3(oklab_of(rgb)), fp(luma_of(rgb)));
    }
    workgroupBarrier();

    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let local = vec2<i32>(i32(lid.x), i32(lid.y));
    // Exact f32 center values drive the final mix; the tile copy only feeds
    // the distance/weight math.
    let center_rgb = load_rgb(x, y);
    let center_lab = oklab_of(center_rgb);
    let center_luma = luma_of(center_rgb);

    let lum_amt = clamp(params.nr.x / 100.0, 0.0, 1.0);
    let lum_detail = clamp(params.nr.y / 100.0, 0.0, 1.0);
    let chr_amt = clamp(params.nr.z / 100.0, 0.0, 1.0);
    let chr_detail = clamp(params.nr.w / 100.0, 0.0, 1.0);

    // --- Luminance: NLM-lite on cube-root luma ---------------------------
    var l_out = center_luma;
    if lum_amt > 0.0 {
        // Filtering strength: h² grows with the slider.
        let h2 = fp(0.0004 + lum_amt * lum_amt * 0.006);
        var acc = 0.0;
        var wsum = 0.0;
        for (var sy = -2; sy <= 2; sy++) {
            for (var sx = -2; sx <= 2; sx++) {
                let d2 = patch_dist(local, sx, sy);
                let w = f32(exp(-d2 / h2));
                acc += f32(tile[tap(local, sx, sy)].w) * w;
                wsum += w;
            }
        }
        let smoothed = acc / max(wsum, 1e-6);
        // Detail blend-back keeps micro-texture (default 50 keeps ~35%).
        let filtered = smoothed + (center_luma - smoothed) * lum_detail * 0.7;
        l_out = mix(center_luma, filtered, lum_amt);
    }

    // --- Chroma: edge-aware bilateral on Oklab a/b -----------------------
    var ab_out = center_lab.yz;
    if chr_amt > 0.0 {
        // Edge sigma: tighter with high chroma_detail (protects color edges).
        let sigma_l = mix(0.10, 0.025, chr_detail);
        let inv2sl = fp(1.0 / (2.0 * sigma_l * sigma_l));
        // Spatial reach grows with strength.
        let sigma_s = 1.0 + chr_amt * 1.6;
        let inv2ss = fp(1.0 / (2.0 * sigma_s * sigma_s));
        // Compare against the tile's own center so both sides of the edge
        // term carry identical rounding.
        let center_l = tile[tap(local, 0, 0)].x;
        var acc = vec2<f32>(0.0);
        var wsum = 0.0;
        for (var sy = -3; sy <= 3; sy++) {
            for (var sx = -3; sx <= 3; sx++) {
                let lab = tile[tap(local, sx, sy)];
                let dl = lab.x - center_l;
                let ds = fp(f32(sx * sx + sy * sy));
                let w = f32(exp(-dl * dl * inv2sl - ds * inv2ss));
                acc += vec2<f32>(f32(lab.y), f32(lab.z)) * w;
                wsum += w;
            }
        }
        let smoothed = acc / max(wsum, 1e-6);
        ab_out = mix(center_lab.yz, smoothed, chr_amt);
    }

    // Reassemble: keep the ORIGINAL Oklab L axis except for the luma NLM
    // ratio applied in linear luma space (avoids double transforms).
    var out_rgb = center_rgb;
    if chr_amt > 0.0 {
        let lab2 = vec3<f32>(center_lab.x, ab_out);
        let lms_c = OK_M2_INV * lab2;
        let lms = lms_c * lms_c * lms_c;
        let srgb = OK_M1_INV * lms;
        out_rgb = max(REC709_TO_REC2020 * srgb, vec3<f32>(0.0));
    }
    if lum_amt > 0.0 {
        // Scale linear RGB by the luma ratio (cube-root domain → cube).
        let ratio_cbrt = l_out / max(center_luma, 1e-5);
        let ratio = ratio_cbrt * ratio_cbrt * ratio_cbrt;
        out_rgb *= clamp(ratio, 0.25, 4.0);
    }

    textureStore(out_tex, vec2<i32>(x, y), vec4<f32>(out_rgb, 1.0));
}
