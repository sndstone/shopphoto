// Superpixel demosaic, fused with raw prepare.
//
// Input:  r16uint full-sensor CFA texture (raw counts).
// Output: rgba16float, quarter resolution of the active area, LINEAR CAMERA
//         RGB normalized to 0..1 by black/white levels. No WB, no matrix —
//         color happens in wb_color.wgsl.
//
// Each output pixel consumes one 2x2 Bayer quad of the active area: the two
// greens are averaged, red and blue taken directly. This is both the fast
// first-frame path and the Super2x2 supersampling mode (BUILD.md Phase 5):
// zero interpolation, zero demosaic artifacts, ~1 stop noise benefit.

struct Params {
    // Active-area origin in full-sensor coordinates.
    origin_x: u32,
    origin_y: u32,
    // Output size in pixels (active_w / 2, active_h / 2).
    out_w: u32,
    out_h: u32,
    // Per-channel black/white levels in raw counts, RGBE order [R, G, B, G2].
    black: vec4<f32>,
    white: vec4<f32>,
    // CFA pattern id relative to FULL-SENSOR origin: 0=RGGB 1=BGGR 2=GRBG 3=GBRG.
    cfa: u32,
    // Struct layout shared with raw_prepare.wgsl; the superpixel path
    // applies a fixed mild hot clamp regardless (it runs once at open).
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
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;
// Lens shading gains: one texel per grid cell, RGBA = the four CFA quad
// slots in (y&1)*2 + (x&1) order. A 1x1 all-ones texture when disabled.
@group(0) @binding(3) var lens_tex: texture_2d<f32>;

// Color of the photosite at absolute sensor coordinate (x, y).
// 0 = red, 1 = green, 2 = blue.
fn cfa_color(x: u32, y: u32) -> u32 {
    let qx = x & 1u;
    let qy = y & 1u;
    let idx = qy * 2u + qx;
    switch params.cfa {
        case 0u: { // RGGB
            return select(select(0u, 1u, idx == 1u || idx == 2u), 2u, idx == 3u);
        }
        case 1u: { // BGGR
            return select(select(2u, 1u, idx == 1u || idx == 2u), 0u, idx == 3u);
        }
        case 2u: { // GRBG
            switch idx {
                case 0u: { return 1u; }
                case 1u: { return 0u; }
                case 2u: { return 2u; }
                default: { return 1u; }
            }
        }
        default: { // GBRG
            switch idx {
                case 0u: { return 1u; }
                case 1u: { return 2u; }
                case 2u: { return 0u; }
                default: { return 1u; }
            }
        }
    }
}

// Normalize a raw count to 0..1 using the level entry for its color
// (levels are RGBE; greens use slot 1).
fn normalize_count(v: u32, color: u32) -> f32 {
    let b = params.black[min(color, 2u)];
    let w = params.white[min(color, 2u)];
    return clamp((f32(v) - b) / max(w - b, 1.0), 0.0, 2.0);
}

// Multiplicative shading gain for the photosite at absolute sensor
// coordinate (x, y). Must stay identical to the copy in raw_prepare.wgsl —
// sp-gpu's shader tests compare the two.
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
    // Top-left photosite of this output pixel's quad, absolute coords.
    let sx = params.origin_x + gid.x * 2u;
    let sy = params.origin_y + gid.y * 2u;

    var r = 0.0;
    var g = 0.0;
    var b = 0.0;
    var g_count = 0.0;
    for (var dy = 0u; dy < 2u; dy++) {
        for (var dx = 0u; dx < 2u; dx++) {
            let x = sx + dx;
            let y = sy + dy;
            let v = textureLoad(cfa_tex, vec2<i32>(i32(x), i32(y)), 0).r;
            let c = cfa_color(x, y);
            var n = normalize_count(v, c);
            // Mild always-on hot clamp vs same-color axis neighbors
            // (hot photosites otherwise survive the 2×2 average as
            // quarter-strength colored dots in the proxy).
            let nmax = max(
                max(
                    normalize_count(
                        textureLoad(cfa_tex, vec2<i32>(i32(x) - 2, i32(y)), 0).r,
                        c,
                    ),
                    normalize_count(
                        textureLoad(cfa_tex, vec2<i32>(i32(x) + 2, i32(y)), 0).r,
                        c,
                    ),
                ),
                max(
                    normalize_count(
                        textureLoad(cfa_tex, vec2<i32>(i32(x), i32(y) - 2), 0).r,
                        c,
                    ),
                    normalize_count(
                        textureLoad(cfa_tex, vec2<i32>(i32(x), i32(y) + 2), 0).r,
                        c,
                    ),
                ),
            );
            if n > nmax * 2.0 + 0.02 {
                n = nmax;
            }
            n = clamp(n * lens_gain(x, y), 0.0, 4.0);
            if c == 0u {
                r = n;
            } else if c == 2u {
                b = n;
            } else {
                g += n;
                g_count += 1.0;
            }
        }
    }
    g /= max(g_count, 1.0);

    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(r, g, b, 1.0));
}
