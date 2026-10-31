// Raw prepare (standalone, full resolution).
//
// Input:  r16uint full-sensor CFA texture (raw counts).
// Output: r32float, active-area crop, normalized 0..1 by per-channel
//         black/white levels. Still mosaiced — one channel per pixel.
//
// Used by the full-resolution Malvar path; the superpixel path fuses this
// work into demosaic_superpixel.wgsl instead. Hot-pixel suppression is
// added here in Phase 4 (median-of-neighbors clamp, ISO-scaled threshold).

struct Params {
    origin_x: u32,
    origin_y: u32,
    out_w: u32,
    out_h: u32,
    black: vec4<f32>,
    white: vec4<f32>,
    // CFA pattern id relative to FULL-SENSOR origin: 0=RGGB 1=BGGR 2=GRBG 3=GBRG.
    cfa: u32,
    // Hot-pixel suppression (Phase 4): 1 = on; ratio vs same-color
    // neighbors above which a photosite is treated as hot.
    hot_enable: u32,
    hot_thresh: f32,
    // Lens shading correction: 1 = sample the gain grid, 0 = skip it.
    lens_enable: u32,
    // xy = grid dimensions in cells, zw = optical centre in full-sensor px.
    lens_grid: vec4<f32>,
    // x = grid sample spacing in sensor px, y = correction strength 0..1.
    lens_scale: vec4<f32>,
}

@group(0) @binding(0) var cfa_tex: texture_2d<u32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var<uniform> params: Params;
// Lens shading gains: one texel per grid cell, RGBA = the four CFA quad
// slots in (y&1)*2 + (x&1) order. A 1x1 all-ones texture when disabled.
@group(0) @binding(3) var lens_tex: texture_2d<f32>;

fn cfa_color(x: u32, y: u32) -> u32 {
    let idx = (y & 1u) * 2u + (x & 1u);
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

fn site_norm(x: i32, y: i32, b: f32, w: f32) -> f32 {
    let v = textureLoad(cfa_tex, vec2<i32>(x, y), 0).r;
    return clamp((f32(v) - b) / max(w - b, 1.0), 0.0, 2.0);
}

// Multiplicative shading gain for the photosite at absolute sensor
// coordinate (x, y). Must stay identical to the copy in
// demosaic_superpixel.wgsl — sp-gpu's shader tests compare the two.
//
// The grid is anchored so its centre cell sits on the optical axis, with
// samples every `spacing` sensor pixels. Each texel holds four gains, one
// per CFA quad slot, so a per-channel colour cast toward the corners is
// corrected as such rather than averaged into a luminance falloff.
fn lens_gain(x: u32, y: u32) -> f32 {
    if params.lens_enable != 1u {
        return 1.0;
    }
    let dims = params.lens_grid.xy;
    let center = params.lens_grid.zw;
    let spacing = max(params.lens_scale.x, 1.0);
    let g = (vec2<f32>(f32(x), f32(y)) - center) / spacing + (dims - 1.0) * 0.5;
    // Clamp rather than extrapolate: past the calibrated field the edge
    // gain is held.
    let gc = clamp(g, vec2<f32>(0.0), dims - vec2<f32>(1.0));
    let i0 = floor(gc);
    let i1 = min(i0 + vec2<f32>(1.0), dims - vec2<f32>(1.0));
    let f = gc - i0;
    let c00 = textureLoad(lens_tex, vec2<i32>(i32(i0.x), i32(i0.y)), 0);
    let c10 = textureLoad(lens_tex, vec2<i32>(i32(i1.x), i32(i0.y)), 0);
    let c01 = textureLoad(lens_tex, vec2<i32>(i32(i0.x), i32(i1.y)), 0);
    let c11 = textureLoad(lens_tex, vec2<i32>(i32(i1.x), i32(i1.y)), 0);
    let quad = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
    let slot = (y & 1u) * 2u + (x & 1u);
    let gain = clamp(quad[slot], 0.25, 8.0);
    // Strength fades toward the uncorrected frame, not toward zero gain.
    return mix(1.0, gain, params.lens_scale.y);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let x = params.origin_x + gid.x;
    let y = params.origin_y + gid.y;
    let c = cfa_color(x, y);
    let b = params.black[min(c, 2u)];
    let w = params.white[min(c, 2u)];
    var n = site_norm(i32(x), i32(y), b, w);

    if params.hot_enable == 1u {
        // Same-color axis neighbors are 2 photosites away on a Bayer CFA.
        let nmax = max(
            max(
                site_norm(i32(x) - 2, i32(y), b, w),
                site_norm(i32(x) + 2, i32(y), b, w),
            ),
            max(
                site_norm(i32(x), i32(y) - 2, b, w),
                site_norm(i32(x), i32(y) + 2, b, w),
            ),
        );
        if n > nmax * params.hot_thresh + 0.02 {
            n = nmax;
        }
    }

    // Shading correction last: hot-pixel detection compares neighbours that
    // all carry near-identical gain, so correcting first would only cost
    // four extra grid lookups without changing the outcome.
    n = clamp(n * lens_gain(x, y), 0.0, 4.0);
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(n, 0.0, 0.0, 1.0));
}
