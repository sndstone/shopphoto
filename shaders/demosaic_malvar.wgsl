// Malvar–He–Cutler 5×5 linear demosaic (full quality).
//
// Input:  r32float prepared CFA (from raw_prepare.wgsl), one channel/pixel,
//         already cropped to the active area (coordinates are local).
// Output: rgba16float, same resolution, linear camera RGB.
//
// Reference: Malvar, He, Cutler, "High-quality linear interpolation for
// demosaicing of Bayer-patterned color images", ICASSP 2004. All kernels
// are the paper's, normalized by 1/8:
//   G at R/B:            (4*C + 2*cross - axial2) / 8
//   R/B at G, same row:  (5*C + 4*h1 - diag - h2 + 0.5*v2) / 8
//   R/B at G, same col:  (5*C + 4*v1 - diag - v2 + 0.5*h2) / 8
//   R at B / B at R:     (6*C + 2*diag - 1.5*axial2) / 8
//
// Edge taps mirror about the border without repeating the edge photosite
// (-1 → 1, w → w-2), so a tap outside the image lands on a photosite of the
// same CFA color. Clamping repeated the edge photosite, which hands the
// kernels the wrong color at the outermost two pixels and paints a magenta
// line along the frame once the active area is the real image edge.
// Wired for the Full-resolution path and Super3x3/4x4 (Phase 5).

struct Params {
    out_w: u32,
    out_h: u32,
    // CFA pattern id AFTER crop (local origin): 0=RGGB 1=BGGR 2=GRBG 3=GBRG.
    cfa: u32,
    _pad0: u32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn cfa_color(x: i32, y: i32) -> u32 {
    let idx = u32(y & 1) * 2u + u32(x & 1);
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

// Reflect `i` into 0..n keeping its parity, so the Bayer phase survives.
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

fn tap(x: i32, y: i32) -> f32 {
    let cx = mirror(x, i32(params.out_w));
    let cy = mirror(y, i32(params.out_h));
    return textureLoad(src_tex, vec2<i32>(cx, cy), 0).r;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    let c = cfa_color(x, y);
    let center = tap(x, y);

    let h1 = tap(x - 1, y) + tap(x + 1, y);
    let v1 = tap(x, y - 1) + tap(x, y + 1);
    let cross = h1 + v1;
    let diag = tap(x - 1, y - 1) + tap(x + 1, y - 1) + tap(x - 1, y + 1) + tap(x + 1, y + 1);
    let h2 = tap(x - 2, y) + tap(x + 2, y);
    let v2 = tap(x, y - 2) + tap(x, y + 2);
    let axial2 = h2 + v2;

    var r = 0.0;
    var g = 0.0;
    var b = 0.0;

    if c == 1u {
        g = center;
        let row_est = (5.0 * center + 4.0 * h1 - diag - h2 + 0.5 * v2) / 8.0;
        let col_est = (5.0 * center + 4.0 * v1 - diag - v2 + 0.5 * h2) / 8.0;
        if cfa_color(x - 1, y) == 0u {
            // Red lives on this row.
            r = row_est;
            b = col_est;
        } else {
            r = col_est;
            b = row_est;
        }
    } else {
        g = (4.0 * center + 2.0 * cross - axial2) / 8.0;
        let opposite = (6.0 * center + 2.0 * diag - 1.5 * axial2) / 8.0;
        if c == 0u {
            r = center;
            b = opposite;
        } else {
            b = center;
            r = opposite;
        }
    }

    textureStore(
        out_tex,
        vec2<i32>(x, y),
        vec4<f32>(max(vec3<f32>(r, g, b), vec3<f32>(0.0)), 1.0),
    );
}
