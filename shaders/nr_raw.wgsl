// Pre-demosaic noise reduction (Phase 4 remainder).
//
// Input:  r32float prepared mosaic (black-subtracted, normalized, still one
//         channel per photosite).
// Output: r32float, same layout.
//
// Runs between raw_prepare and Malvar on the full-resolution path only —
// the superpixel/proxy path averages 2×2 quads, which already suppresses
// per-photosite noise, so `plan_passes` never schedules this there.
//
// Why a separate pass from nr_main.wgsl: single-photosite shot noise is
// invisible to a post-demosaic filter, because Malvar has already smeared
// each noisy site across its 5×5 neighborhood as a *correlated* error.
// Removing it here means the demosaic interpolates clean data, which
// prevents the colored zipper artifacts that the post-demosaic chroma
// bilateral can only blur, not undo.
//
// Filter: bilateral over the 5×5 SAME-COLOR neighborhood. On a Bayer CFA
// that is a stride of 2 photosites in each axis, which holds for every
// pattern (RGGB/BGGR/GRBG/GBRG) and both green planes, so no pattern id is
// needed. The range term keeps edges: a site only mixes with same-color
// neighbors of similar level.
//
// BUILD.md §10.3: the 16×16 tile plus its 4-site apron is staged in
// workgroup shared memory (24×24 f32 = 2.3 KB), so the 25 taps per site
// cost no global loads. Values stay f32 — this is pre-demosaic sensor
// data, where f16's 11-bit mantissa would quantize coarser than the
// 16-bit raw itself.

struct Params {
    // x = luminance 0..100, y = luminance_detail 0..100,
    // z = chroma 0..100, w = chroma_detail 0..100. Same layout as
    // nr_main.wgsl (`NrGpuParams` on the Rust side).
    nr: vec4<f32>,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<uniform> params: Params;

const WG_DIM: i32 = 16;
const APRON: i32 = 4;         // two same-color steps
const TILE_DIM: i32 = 24;     // WG_DIM + 2 * APRON
const TILE_LEN: u32 = 576u;   // TILE_DIM * TILE_DIM

var<workgroup> tile: array<f32, TILE_LEN>;

// Reflect `i` into 0..n keeping its parity: a tap outside the image lands
// on a photosite of the same CFA color instead of the repeated edge one.
fn mirror(i: i32, n: i32) -> i32 {
    if n < 2 {
        return 0;
    }
    let period = 2 * (n - 1);
    var m = ((i % period) + period) % period;
    if m >= n {
        m = period - m;
    }
    return m;
}

fn load_site(x: i32, y: i32) -> f32 {
    let cx = mirror(x, i32(params.out_w));
    let cy = mirror(y, i32(params.out_h));
    return textureLoad(src_tex, vec2<i32>(cx, cy), 0).r;
}

fn tap(local: vec2<i32>, sx: i32, sy: i32) -> u32 {
    return u32((local.y + APRON + sy) * TILE_DIM + (local.x + APRON + sx));
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) lindex: u32,
) {
    // Every thread fills its share before the barrier, including threads
    // whose own site is outside the image (uniform control flow).
    let base = vec2<i32>(i32(wid.x) * WG_DIM - APRON, i32(wid.y) * WG_DIM - APRON);
    for (var i = lindex; i < TILE_LEN; i += 256u) {
        let t = vec2<i32>(i32(i) % TILE_DIM, i32(i) / TILE_DIM);
        tile[i] = load_site(base.x + t.x, base.y + t.y);
    }
    workgroupBarrier();

    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let local = vec2<i32>(i32(lid.x), i32(lid.y));
    let center = tile[tap(local, 0, 0)];

    // Pre-demosaic filtering is deliberately gentler than the post-demosaic
    // pass: it operates on data that has had no interpolation smoothing yet,
    // so over-filtering here costs real resolution. Luminance drives it;
    // luminance_detail pulls it back.
    let amount = clamp(params.nr.x / 100.0, 0.0, 1.0)
        * (1.0 - 0.5 * clamp(params.nr.y / 100.0, 0.0, 1.0))
        * 0.6;
    if amount <= 0.0 {
        textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(center, 0.0, 0.0, 1.0));
        return;
    }

    // Range sigma tracks the expected noise floor: shot noise grows with
    // the square root of the signal, so scale with sqrt(level) and keep a
    // read-noise floor for the shadows. Sized to sit slightly above the
    // noise standard deviation it is meant to average away — below it the
    // range term rejects the very samples that would cancel the noise.
    let sigma_r = (0.008 + 0.045 * amount) * (0.35 + sqrt(max(center, 0.0)));
    let inv2sr = 1.0 / (2.0 * sigma_r * sigma_r);
    // Spatial sigma in same-color steps.
    let inv2ss = 1.0 / (2.0 * 1.2 * 1.2);

    var acc = 0.0;
    var wsum = 0.0;
    for (var sy = -2; sy <= 2; sy++) {
        for (var sx = -2; sx <= 2; sx++) {
            // Same-color neighbors sit two photosites away.
            let v = tile[tap(local, sx * 2, sy * 2)];
            let d = v - center;
            let ds = f32(sx * sx + sy * sy);
            let w = exp(-ds * inv2ss - d * d * inv2sr);
            acc += v * w;
            wsum += w;
        }
    }
    let filtered = acc / max(wsum, 1e-6);
    let out = mix(center, filtered, amount);
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(out, 0.0, 0.0, 1.0));
}
