// Tone + color pass: tone LUT, HSL bands, color grading and monochrome.
//
// Input:  rgba16float, scene-linear Rec.2020 (from wb_color).
// Output: rgba16float, scene-linear Rec.2020.
//
// The 1024-entry tone LUT (storage buffer, built on the CPU — see
// sp-pipeline/src/tone.rs, domains must stay in sync) applies exposure/
// contrast/highlights/shadows/whites/blacks + the user curve per channel.
// HSL runs in Oklab LCh: 8 raised-cosine hue bands (red→magenta) apply
// hue shift, saturation and luminance deltas; then vibrance (saturation
// weighted toward muted colors) and global saturation. Oklab is defined
// over linear BT.709, so we convert 2020→709 and back around it (wide-
// gamut colors clip slightly in the LCh detour; revisit in Phase 3B).

struct Params {
    // 8 bands × (hue_shift, saturation, luminance, _pad), each -100..100.
    bands: array<vec4<f32>, 8>,
    // shadows, midtones, highlights: (hue degrees, saturation, _, _).
    grading: array<vec4<f32>, 3>,
    // 8 monochrome mix values, packed in two vectors.
    monochrome_mix: array<vec4<f32>, 2>,
    // x = vibrance, y = saturation, z = grading balance, w = monochrome.
    global_sat: vec4<f32>,
    out_w: u32,
    out_h: u32,
    // 1 = every color control is neutral: skip the Oklab detour.
    color_neutral: u32,
    // 1 = all R/G/B point curves are neutral.
    channel_neutral: u32,
    photographic: PhotographicColor,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> tone_lut: array<f32>;
@group(0) @binding(4) var<storage, read> channel_lut: array<f32>;

const LUT_SIZE: f32 = 1024.0;
// Shaper cap; must equal MAX_INPUT in tone.rs. s(64) = 64/65.
const SHAPER_MAX_T: f32 = 0.98461538;
const CHANNEL_LUT_SIZE: u32 = 256u;

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

// Oklab (Björn Ottosson, public domain reference constants).
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

fn lut_apply(x: f32) -> f32 {
    // Negative scene-linear values can appear after matrix operations. Clamp
    // before the rational shaper: x <= -1 otherwise wraps toward the LUT's
    // white end rather than remaining black.
    let safe_x = max(x, 0.0);
    let t = clamp((safe_x / (safe_x + 1.0)) / SHAPER_MAX_T, 0.0, 1.0) * (LUT_SIZE - 1.0);
    let i = u32(min(t, LUT_SIZE - 2.0));
    let frac = t - f32(i);
    return mix(tone_lut[i], tone_lut[i + 1u], frac);
}

fn srgb_oetf(x: f32) -> f32 {
    if x <= 0.0031308 { return 12.92 * max(x, 0.0); }
    return 1.055 * pow(max(x, 0.0), 1.0 / 2.4) - 0.055;
}

fn srgb_eotf(x: f32) -> f32 {
    if x <= 0.04045 { return max(x, 0.0) / 12.92; }
    return pow((max(x, 0.0) + 0.055) / 1.055, 2.4);
}

fn channel_apply(x: f32, channel: u32) -> f32 {
    let base = clamp(x, 0.0, 1.0);
    let position = srgb_oetf(base) * f32(CHANNEL_LUT_SIZE - 1u);
    let index = u32(min(floor(position), f32(CHANNEL_LUT_SIZE - 2u)));
    let offset = channel * CHANNEL_LUT_SIZE;
    let encoded = mix(channel_lut[offset + index], channel_lut[offset + index + 1u], position - f32(index));
    return srgb_eotf(encoded) + (x - base);
}

// 8 band centers in degrees (Oklab hue): red, orange, yellow, green, aqua,
// blue, purple, magenta.
const BAND_CENTERS = array<f32, 8>(25.0, 55.0, 100.0, 145.0, 200.0, 260.0, 300.0, 345.0);
const BAND_WIDTH: f32 = 60.0; // half-width of the raised-cosine falloff

fn band_weight(hue_deg: f32, center: f32) -> f32 {
    var d = abs(hue_deg - center);
    d = min(d, 360.0 - d);
    if d >= BAND_WIDTH {
        return 0.0;
    }
    return 0.5 + 0.5 * cos(d / BAND_WIDTH * 3.14159265);
}

fn monochrome_value(index: u32) -> f32 {
    if index < 4u {
        return params.monochrome_mix[0u][index];
    }
    return params.monochrome_mix[1u][index - 4u];
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    var rgb = textureLoad(src_tex, p, 0).rgb;

    // Tone LUT per channel.
    rgb = vec3<f32>(lut_apply(rgb.r), lut_apply(rgb.g), lut_apply(rgb.b));
    if params.channel_neutral == 0u {
        rgb = vec3<f32>(channel_apply(rgb.r, 0u), channel_apply(rgb.g, 1u), channel_apply(rgb.b, 2u));
    }

    if params.color_neutral == 0u {
        // → Oklab LCh.
        let srgb_lin = max(REC2020_TO_REC709 * rgb, vec3<f32>(0.0));
        let lms = OK_M1 * srgb_lin;
        let lms_c = pow(max(lms, vec3<f32>(0.0)), vec3<f32>(1.0 / 3.0));
        let lab = OK_M2 * lms_c;
        let l = lab.x;
        var c = length(lab.yz);
        var h = atan2(lab.z, lab.y) * 57.29577951; // rad → deg
        if h < 0.0 {
            h += 360.0;
        }

        var l_scale = 1.0;
        var c_scale = 1.0;
        var h_shift = 0.0;
        var mono_delta = 0.0;
        var mono_weight = 0.0;
        for (var b = 0u; b < 8u; b++) {
            let w = band_weight(h, BAND_CENTERS[b]);
            if w > 0.0 {
                let band = params.bands[b];
                h_shift += w * band.x * 0.3;          // ±100 → ±30°
                c_scale *= 1.0 + w * band.y * 0.01;   // ±100 → ±100%
                l_scale *= 1.0 + w * band.z * 0.004;  // ±100 → ±40%
                mono_delta += w * monochrome_value(b);
                mono_weight += w;
            }
        }
        // Vibrance: boost muted colors more (chroma-weighted), gentle on skin
        // via reduced effect in the orange band.
        let vib = params.global_sat.x;
        if vib != 0.0 {
            let muted = 1.0 - clamp(c / 0.25, 0.0, 1.0);
            let skin_protect = 1.0 - 0.6 * band_weight(h, 55.0);
            c_scale *= 1.0 + vib * 0.01 * muted * skin_protect;
        }
        c_scale *= 1.0 + params.global_sat.y * 0.01;

        h = (h + h_shift) * 0.01745329; // deg → rad
        c = max(c * c_scale, 0.0);
        var new_l = clamp(l * l_scale, 0.0, 1.2);
        var chroma = vec2<f32>(c * cos(h), c * sin(h));

        // Lift the Oklab chroma toward each grading wheel. Balance shifts
        // the tonal split without altering luminance.
        let balance = params.global_sat.z * 0.0015;
        let shadow_w = 1.0 - smoothstep(0.20 + balance, 0.55 + balance, l);
        let highlight_w = smoothstep(0.45 + balance, 0.80 + balance, l);
        let midtone_w = max(1.0 - shadow_w - highlight_w, 0.0);
        let tonal_weights = array<f32, 3>(shadow_w, midtone_w, highlight_w);
        for (var wheel = 0u; wheel < 3u; wheel++) {
            let grade = params.grading[wheel];
            if grade.y > 0.0 {
                let angle = grade.x * 0.01745329;
                let amount = grade.y * 0.0008 * tonal_weights[wheel];
                chroma += vec2<f32>(cos(angle), sin(angle)) * amount;
            }
        }

        if params.global_sat.w > 0.5 {
            let mix_adjust = mono_delta / max(mono_weight, 1e-5);
            new_l = clamp(new_l * (1.0 + mix_adjust * 0.004), 0.0, 1.2);
            chroma = vec2<f32>(0.0);
        }
        let lab2 = vec3<f32>(new_l, chroma);

        let lms_c2 = OK_M2_INV * lab2;
        let lms2 = lms_c2 * lms_c2 * lms_c2;
        let srgb2 = OK_M1_INV * lms2;
        rgb = max(REC709_TO_REC2020 * srgb2, vec3<f32>(0.0));
    }

    rgb=photographic_color(rgb,params.photographic);
    textureStore(out_tex, p, vec4<f32>(rgb, 1.0));
}
