// Scene-linear Rec.2020 rgba16float -> rgba16float. Masked local edits and heal/clone.

struct Params {
    out_w: u32,
    out_h: u32,
    origin_x: u32,
    origin_y: u32,
    full_w: u32,
    full_h: u32,
    local_count: u32,
    spot_count: u32,
    source_origin_x: u32,
    source_origin_y: u32,
    patch_source: u32,
    preview_mask: u32,
    storage_origin: vec2<u32>,
    _region_pad: vec2<u32>,
}

struct Record {
    // x kind, y enabled/clone, z point offset, w point count.
    header: vec4<f32>,
    a: vec4<f32>,
    b: vec4<f32>,
    // exposure, contrast, highlights, shadows.
    tone0: vec4<f32>,
    // whites, blacks, brightness, unused.
    tone1: vec4<f32>,
    // vibrance, saturation, monochrome, unused.
    color: vec4<f32>,
    mask: vec4<f32>,
    wb: vec4<f32>,
    spatial: vec4<f32>,
    extra: vec4<f32>,
    flags: vec4<f32>,
    corrective: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var out_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> records: array<Record>;
@group(0) @binding(4) var<storage, read> points: array<vec4<f32>>;
@group(0) @binding(5) var source_tex: texture_2d<f32>;
@group(0) @binding(6) var ring_tex: texture_2d<f32>;
@group(0) @binding(7) var target_scene_tex:texture_2d<f32>;

const BAND_CENTERS = array<f32, 8>(25.0, 55.0, 100.0, 145.0, 200.0, 260.0, 300.0, 345.0);
const LUMA_2020 = vec3<f32>(0.2627, 0.6780, 0.0593);
const REC2020_TO_REC709 = mat3x3<f32>(
    vec3<f32>(1.660491, -0.124550, -0.018151),
    vec3<f32>(-0.587641, 1.132900, -0.100579),
    vec3<f32>(-0.072850, -0.008349, 1.118730),
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

const REC709_TO_REC2020 = mat3x3<f32>(
    vec3<f32>(0.627404, 0.069097, 0.016391),
    vec3<f32>(0.329283, 0.919540, 0.088013),
    vec3<f32>(0.043313, 0.011362, 0.895595),
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

fn oklab_of(rgb2020: vec3<f32>) -> vec3<f32> {
    let linear709 = max(REC2020_TO_REC709 * rgb2020, vec3<f32>(0.0));
    let lms = max(OK_M1 * linear709, vec3<f32>(0.0));
    return OK_M2 * pow(lms, vec3<f32>(1.0 / 3.0));
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
    return length(p - (a + ab * t));
}

fn stroke_distance(uv: vec2<f32>, offset: u32, count: u32, fallback: vec2<f32>) -> f32 {
    let metric = vec2<f32>(f32(params.full_w), f32(params.full_h)) / f32(min(params.full_w, params.full_h));
    if count == 0u { return length((uv - fallback) * metric); }
    var distance = length((uv - points[offset].xy) * metric);
    for (var i = 1u; i < count; i++) {
        distance = min(distance, segment_distance(uv * metric, points[offset + i - 1u].xy * metric, points[offset + i].xy * metric));
    }
    return distance;
}

fn feathered_disk(distance: f32, radius: f32, feather: f32) -> f32 {
    let inner = radius * (1.0 - clamp(feather, 0.0, 1.0));
    return 1.0 - smoothstep(inner, max(radius, inner + 1e-6), distance);
}

fn primitive_mask(header: vec4<f32>, a: vec4<f32>, b: vec4<f32>, uv: vec2<f32>, rgb: vec3<f32>) -> f32 {
    let kind = u32(round(header.x));
    if kind == 8u {
        let metric = vec2<f32>(f32(params.full_w), f32(params.full_h)) / f32(min(params.full_w, params.full_h));
        var coverage = 0.0;
        let offset = u32(header.z);
        for (var i = 0u; i < u32(header.w); i++) {
            let dab = points[offset + i];
            let radius = a.x * select(1.0, max(dab.z, 0.05), a.w > 0.5);
            let delta = (uv - dab.xy) * metric;
            if any(abs(delta) > vec2<f32>(radius)) { continue; }
            let value = feathered_disk(length(delta), radius, a.y) * a.z;
            if dab.w > 0.5 { coverage *= 1.0 - value; } else { coverage += (1.0 - coverage) * value; }
        }
        return coverage;
    }
    if kind == 6u {
        let p = local_pixel(uv) - vec2<i32>(params.storage_origin);
        let index = u32(p.y) * params.out_w + u32(p.x);
        return points[u32(round(header.z)) + index / 4u][index % 4u];
    }
    if kind == 1u {
        if header.w < 0.5 { return 0.0; }
        let distance = stroke_distance(
            uv,
            u32(round(header.z)),
            u32(round(header.w)),
            a.zw,
        );
        return feathered_disk(distance, a.x, a.y);
    }
    if kind == 2u {
        let q = (uv - a.xy) / max(a.zw, vec2<f32>(1e-5));
        return feathered_disk(length(q), 1.0, b.x);
    }
    if kind == 3u {
        let start = a.xy;
        let end = a.zw;
        let axis = end - start;
        return clamp(dot(uv - start, axis) / max(dot(axis, axis), 1e-8), 0.0, 1.0);
    }
    if kind == 4u {
        let lo = a.x;
        let hi = a.y;
        let feather = max(a.z, 1e-5);
        let luma = dot(rgb, LUMA_2020);
        return smoothstep(lo - feather, lo, luma) * (1.0 - smoothstep(hi, hi + feather, luma));
    }
    if kind == 5u {
        let distance = length(oklab_of(rgb).yz - a.xy);
        let inner = max(a.z, 1e-5);
        return 1.0 - smoothstep(inner, inner + max(a.w, 1e-5), distance);
    }
    return 0.0;
}

fn local_mask(record: Record, uv: vec2<f32>, rgb: vec3<f32>) -> f32 {
    var stack: array<f32, 32>;
    var top = 0u;
    let offset = u32(record.mask.x);
    let count = u32(record.mask.y);
    if count == 0u || count > 1024u { return 0.0; }
    for (var i = 0u; i < count; i++) {
        let instruction = offset + i * 3u;
        let header = points[instruction];
        let a = points[instruction + 1u];
        let b = points[instruction + 2u];
        let kind = u32(header.x);
        if kind == 23u {
            if top == 0u { return 0.0; }
            var value = stack[top - 1u];
            if a.x > 0.5 { value = 1.0 - value; }
            stack[top - 1u] = clamp(value * a.y, 0.0, 1.0);
        } else if kind >= 20u {
            if top < 2u { return 0.0; }
            let right = stack[top - 1u];
            let left = stack[top - 2u];
            top -= 1u;
            if kind == 20u { stack[top - 1u] = left + right - left * right; }
            if kind == 21u { stack[top - 1u] = left * (1.0 - right); }
            if kind == 22u { stack[top - 1u] = left * right; }
        } else {
            if top >= 32u { return 0.0; }
            stack[top] = primitive_mask(header, a, b, uv, rgb);
            top += 1u;
        }
    }
    if top != 1u { return 0.0; }
    var coverage = stack[0];
    if record.mask.z > 0.5 { coverage = 1.0 - coverage; }
    return clamp(coverage * record.mask.w, 0.0, 1.0);
}

fn rgb_of(lab: vec3<f32>) -> vec3<f32> {
    let lms = OK_M2_INV * lab;
    return max(REC709_TO_REC2020 * (OK_M1_INV * (lms * lms * lms)), vec3<f32>(0.0));
}

fn srgb_oetf(x: f32) -> f32 {
    if x <= 0.0031308 { return 12.92 * max(x, 0.0); }
    return 1.055 * pow(max(x, 0.0), 1.0 / 2.4) - 0.055;
}

fn srgb_eotf(x: f32) -> f32 {
    if x <= 0.04045 { return max(x, 0.0) / 12.92; }
    return pow((max(x, 0.0) + 0.055) / 1.055, 2.4);
}

fn local_curve(x: f32, record: Record, channel: u32) -> f32 {
    var value = max(x, 0.0);
    if record.flags.x > 0.5 {
        let t = clamp((value / (value + 1.0)) / 0.98461538, 0.0, 1.0) * 1023.0;
        let i = u32(min(t, 1022.0));
        let offset = u32(record.extra.y);
        value = mix(points[offset + i].x, points[offset + i + 1u].x, t - f32(i));
    }
    if record.flags.y > 0.5 {
        let base = clamp(value, 0.0, 1.0);
        let t = srgb_oetf(base) * 255.0;
        let i = u32(min(t, 254.0));
        let offset = u32(record.extra.z);
        let encoded = mix(points[offset + i][channel], points[offset + i + 1u][channel], t - f32(i));
        value = srgb_eotf(encoded) + (value - base);
    }
    return value;
}

fn band_weight(hue: f32, center: f32) -> f32 {
    var distance = abs(hue - center);
    distance = min(distance, 360.0 - distance);
    if distance >= 60.0 { return 0.0; }
    return 0.5 + 0.5 * cos(distance / 60.0 * 3.14159265);
}

fn extended_color(rgb: vec3<f32>, record: Record) -> vec3<f32> {
    let lab = oklab_of(rgb);
    var hue = atan2(lab.z, lab.y) * 57.29577951;
    if hue < 0.0 { hue += 360.0; }
    var chroma = length(lab.yz);
    var l_scale = 1.0;
    var c_scale = 1.0;
    var h_shift = 0.0;
    var mono_delta = 0.0;
    var mono_weight = 0.0;
    let offset = u32(record.extra.w);
    for (var i = 0u; i < 8u; i++) {
        let weight = band_weight(hue, BAND_CENTERS[i]);
        let band = points[offset + i];
        h_shift += weight * band.x * 0.3;
        c_scale *= 1.0 + weight * band.y * 0.01;
        l_scale *= 1.0 + weight * band.z * 0.004;
        mono_delta += weight * points[offset + 11u + i / 4u][i % 4u];
        mono_weight += weight;
    }
    hue = (hue + h_shift) * 0.01745329;
    chroma = max(chroma * c_scale, 0.0);
    var new_l = max(lab.x * l_scale, 0.0);
    var ab = vec2<f32>(cos(hue), sin(hue)) * chroma;
    let balance = record.color.w * 0.0015;
    let shadow = 1.0 - smoothstep(0.20 + balance, 0.55 + balance, lab.x);
    let highlight = smoothstep(0.45 + balance, 0.80 + balance, lab.x);
    let weights = array<f32, 3>(shadow, max(1.0 - shadow - highlight, 0.0), highlight);
    for (var i = 0u; i < 3u; i++) {
        let wheel = points[offset + 8u + i];
        let angle = wheel.x * 0.01745329;
        ab += vec2<f32>(cos(angle), sin(angle)) * wheel.y * 0.0008 * weights[i];
    }
    if record.color.z > 0.5 {
        new_l *= max(1.0 + mono_delta / max(mono_weight, 1e-5) * 0.004, 0.0);
        ab = vec2<f32>(0.0);
    }
    return rgb_of(vec3<f32>(new_l, ab));
}

fn neighborhood_rgb(p: vec2<i32>, radius: i32) -> vec3<f32> {
    var value = textureLoad(src_tex, p, 0).rgb * 4.0;
    let limit = vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            if dx == 0 && dy == 0 { continue; }
            value += textureLoad(src_tex, clamp(p + vec2<i32>(dx, dy) * radius, vec2<i32>(0), limit), 0).rgb;
        }
    }
    return value / 12.0;
}

fn perceptual(rgb: vec3<f32>) -> f32 {
    return pow(max(dot(rgb, LUMA_2020), 0.0), 1.0 / 3.0);
}

// Coverage lookups stay inside the output window: alpha masks are packed
// for that window only, while colour may still be read from tile margins.
fn window_uv(q: vec2<i32>) -> vec2<f32> {
    let window = clamp(
        q - vec2<i32>(params.storage_origin),
        vec2<i32>(0),
        vec2<i32>(i32(params.out_w) - 1, i32(params.out_h) - 1),
    );
    let full = vec2<f32>(f32(params.full_w), f32(params.full_h));
    return (vec2<f32>(window) + vec2<f32>(f32(params.origin_x), f32(params.origin_y)) + 0.5) / full;
}

// Local mean of perceptual lightness for layer clarity. Taps across a strong
// edge, or outside the layer's own coverage, are down-weighted so a bright
// background cannot print a dark rim (or a dark one a glow) onto the selected
// subject. Two rings keep large radii from ghosting the subject outline.
fn guarded_mean(p: vec2<i32>, radius: i32, center: f32, record: Record) -> f32 {
    let limit = vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
    var sum = center * 2.0;
    var total = 2.0;
    for (var ring = 1; ring <= 2; ring++) {
        let step = max(radius * ring / 2, 1);
        let ring_weight = select(0.6, 1.0, ring == 1);
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                if dx == 0 && dy == 0 { continue; }
                let q = clamp(p + vec2<i32>(dx, dy) * step, vec2<i32>(0), limit);
                let rgb = textureLoad(src_tex, q, 0).rgb;
                let l = perceptual(rgb);
                let d = (l - center) / 0.18;
                let w = ring_weight * exp(-0.5 * d * d) * local_mask(record, window_uv(q), rgb);
                sum += l * w;
                total += w;
            }
        }
    }
    return sum / total;
}

fn spatial_adjusted(rgb: vec3<f32>, uv: vec2<f32>, record: Record) -> vec3<f32> {
    var value = rgb;
    let p = local_pixel(uv);
    let source = textureLoad(src_tex, p, 0).rgb;
    let luma = dot(source, LUMA_2020);
    if record.spatial.x != 0.0 || record.spatial.y != 0.0 || record.spatial.w != 0.0 {
        let fine = dot(neighborhood_rgb(p, 1), LUMA_2020);
        let texture = dot(neighborhood_rgb(p, 2), LUMA_2020);
        let edge = abs(luma - fine) / max(luma + fine, 0.02);
        let detail_gate = 1.0 - smoothstep(0.2, 0.6, edge);
        let midtone = max(1.0 - abs(srgb_oetf(clamp(luma, 0.0, 1.0)) * 2.0 - 1.0), 0.0);
        var delta = (luma - fine) * record.spatial.w * 0.015
            + (luma - texture) * record.spatial.y * 0.008 * detail_gate;
        if record.spatial.x != 0.0 {
            // Clarity works in perceptual lightness: linear differences next
            // to a bright background are large enough to blacken edges.
            let radius = max(4, min(32, i32(round(f32(params.full_w) / 40.0))));
            let l = perceptual(source);
            let broad = guarded_mean(p, radius, l, record);
            let lifted = max(l + (l - broad) * record.spatial.x * 0.016 * midtone, 0.0);
            delta += lifted * lifted * lifted - luma;
        }
        value *= clamp((luma + delta) / max(luma, 1e-6), 0.0, 4.0);
    }
    if record.extra.x > 0.0 {
        let limit = vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
        let lab = oklab_of(source);
        var sum = vec3<f32>(0.0);
        var total = 0.0;
        let sigma = 0.01 + record.extra.x * 0.0006;
        for (var dy = -2; dy <= 2; dy++) {
            for (var dx = -2; dx <= 2; dx++) {
                let neighbor = textureLoad(src_tex, clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), limit), 0).rgb;
                let difference = oklab_of(neighbor) - lab;
                let weight = exp(-dot(difference, difference) / (2.0 * sigma * sigma) - f32(dx * dx + dy * dy) / 4.0);
                sum += neighbor * weight; total += weight;
            }
        }
        // Apply only the residual so prior local edits remain intact.
        value += (sum / max(total, 1e-6) - source) * record.extra.x * 0.01;
    }
    if record.corrective.y > 0.0 {
        let limit=vec2<i32>(textureDimensions(src_tex)) - vec2<i32>(1);
        let step=max(1.0,record.corrective.z)/3.0;
        var chroma_sum=vec3<f32>(0.0);
        var weights=0.0;
        for (var dy=-3;dy<=3;dy++) { for(var dx=-3;dx<=3;dx++) {
            let neighbor=textureLoad(src_tex,clamp(p+vec2<i32>(round(vec2<f32>(f32(dx),f32(dy))*step)),vec2<i32>(0),limit),0).rgb;
            let y=dot(neighbor,LUMA_2020);
            let difference=(y-luma)/max(y+luma,0.02);
            let weight=exp(-difference*difference*32.0-f32(dx*dx+dy*dy)/9.0);
            chroma_sum+=(neighbor-vec3<f32>(y))*weight;weights+=weight;
        }}
        // Chroma-only residual retains fine luminance texture and previous edits.
        value+=(chroma_sum/max(weights,1e-6)-(source-vec3<f32>(luma)))*record.corrective.y*0.01;
    }
    if record.corrective.x > 0.0 {
        var linear709=max(REC2020_TO_REC709*value,vec3<f32>(0.0));
        let redness=(linear709.r-max(linear709.g,linear709.b))/max(linear709.r,1e-5);
        let selected=smoothstep(0.2,0.65,redness)*record.corrective.x*0.01;
        linear709.r=mix(linear709.r,(linear709.g+linear709.b)*0.5,selected);
        value=REC709_TO_REC2020*linear709;
    }
    if record.spatial.z != 0.0 {
        let strength = record.spatial.z * 0.0035;
        let luminance = max(dot(value, LUMA_2020), 0.0);
        let adjusted_luminance = max((luminance - 0.08 * strength) / max(1.0 - strength, 0.2), 0.0);
        value *= clamp(adjusted_luminance / max(luminance, 1e-6), 0.0, 8.0);
    }
    return max(value, vec3<f32>(0.0));
}

fn adjusted(rgb_in: vec3<f32>, record: Record, amount: f32, uv: vec2<f32>) -> vec3<f32> {
    var rgb = mix(rgb_in, spatial_adjusted(rgb_in, uv, record), amount);
    rgb *= mix(vec3<f32>(1.0), record.wb.xyz, amount);
    rgb = max(rgb * exp2(record.tone0.x * amount), vec3<f32>(0.0));
    var luma = max(dot(rgb, LUMA_2020), 0.0);
    let original_luma = max(luma, 1e-6);
    let contrast = 1.0 + record.tone0.y / 100.0 * amount;
    luma = max(0.0, 0.18 + (luma - 0.18) * contrast);
    luma += record.tone0.z / 100.0 * amount * smoothstep(0.45, 1.0, luma) * 0.35;
    luma += record.tone0.w / 100.0 * amount * (1.0 - smoothstep(0.0, 0.55, luma)) * 0.25;
    luma += record.tone1.x / 100.0 * amount * smoothstep(0.72, 1.0, luma) * 0.25;
    luma += record.tone1.y / 100.0 * amount * (1.0 - smoothstep(0.0, 0.28, luma)) * 0.18;
    let midtone = 1.0 - clamp(abs(luma - 0.5) * 2.0, 0.0, 1.0);
    luma += record.tone1.z / 100.0 * amount * midtone * 0.25;
    rgb *= max(luma, 0.0) / original_luma;

    if record.flags.x > 0.5 || record.flags.y > 0.5 {
        let curved = vec3<f32>(local_curve(rgb.r, record, 0u), local_curve(rgb.g, record, 1u), local_curve(rgb.b, record, 2u));
        rgb = mix(rgb, curved, amount);
    }
    if record.flags.z > 0.5 { rgb = mix(rgb, extended_color(rgb, record), amount); }
    let gray = vec3<f32>(max(dot(rgb, LUMA_2020), 0.0));
    let chroma = max(max(rgb.r, rgb.g), rgb.b) - min(min(rgb.r, rgb.g), rgb.b);
    let vibrance = record.color.x / 100.0 * amount * (1.0 - clamp(chroma, 0.0, 1.0));
    let saturation = record.color.y / 100.0 * amount;
    rgb = mix(gray, rgb, max(0.0, 1.0 + vibrance + saturation));
    if record.color.z > 0.5 && record.flags.z < 0.5 { rgb = mix(rgb, gray, amount); }
    if record.flags.w > 0.5 {
        let offset=u32(record.corrective.w);
        var settings:PhotographicColor;
        for(var row=0u;row<3u;row++) {settings.mixer[row]=points[offset+row];}
        for(var row=0u;row<9u;row++) {settings.selective[row]=points[offset+3u+row];}
        settings.skin=points[offset+12u];settings.control=points[offset+13u];
        rgb=mix(rgb,photographic_color(rgb,settings),amount);
    }
    return max(rgb, vec3<f32>(0.0));
}

fn local_pixel(source_uv: vec2<f32>) -> vec2<i32> {
    let full = vec2<f32>(f32(params.full_w), f32(params.full_h));
    let local = source_uv * full - vec2<f32>(f32(params.origin_x), f32(params.origin_y)) + vec2<f32>(params.storage_origin);
    return clamp(vec2<i32>(floor(local)),vec2<i32>(0),vec2<i32>(textureDimensions(src_tex))-vec2<i32>(1));
}

fn ring_mean(center: vec2<f32>, radius: f32) -> vec3<f32> {
    let step = vec2<f32>(radius) * f32(min(params.full_w, params.full_h)) / vec2<f32>(f32(params.full_w), f32(params.full_h));
    return (
        textureLoad(src_tex, local_pixel(center + vec2<f32>(step.x, 0.0)), 0).rgb +
        textureLoad(src_tex, local_pixel(center - vec2<f32>(step.x, 0.0)), 0).rgb +
        textureLoad(src_tex, local_pixel(center + vec2<f32>(0.0, step.y)), 0).rgb +
        textureLoad(src_tex, local_pixel(center - vec2<f32>(0.0, step.y)), 0).rgb
    ) * 0.25;
}

fn source_pixel(uv: vec2<f32>) -> vec2<i32> {
    let pixel = vec2<i32>(floor(uv * vec2<f32>(f32(params.full_w), f32(params.full_h))));
    let origin = vec2<i32>(i32(params.source_origin_x), i32(params.source_origin_y));
    return clamp(pixel - origin, vec2<i32>(0), vec2<i32>(textureDimensions(source_tex)) - vec2<i32>(1));
}

fn stored_ring_mean(offset: i32) -> vec3<f32> {
    return (textureLoad(ring_tex, vec2<i32>(offset, 0), 0).rgb
        + textureLoad(ring_tex, vec2<i32>(offset + 1, 0), 0).rgb
        + textureLoad(ring_tex, vec2<i32>(offset + 2, 0), 0).rgb
        + textureLoad(ring_tex, vec2<i32>(offset + 3, 0), 0).rgb) * 0.25;
}

fn healing_low_frequency(center:vec2<f32>,radius:f32,source:bool)->vec3<f32> {
    let full=vec2<f32>(f32(params.full_w),f32(params.full_h));
    let extent=i32(ceil(clamp(radius,1.0,8.0)));
    let sigma=max(radius/3.0,0.65);
    var mean=vec3<f32>(0.0);var total=0.0;
    for(var dy=-extent;dy<=extent;dy++) {for(var dx=-extent;dx<=extent;dx++) {
        let offset=vec2<f32>(f32(dx),f32(dy));
        let uv=center+offset/full;
        var sample=vec3<f32>(0.0);
        if source {sample=textureLoad(source_tex,source_pixel(uv),0).rgb;}
        else {sample=textureLoad(target_scene_tex,local_pixel(uv),0).rgb;}
        let weight=exp(-dot(offset,offset)/(2.0*sigma*sigma));
        mean+=sample*weight;total+=weight;
    }}
    return mean/total;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.out_w || gid.y >= params.out_h { return; }
    let p = vec2<i32>(gid.xy + params.storage_origin);
    let full = vec2<f32>(f32(params.full_w), f32(params.full_h));
    let uv = (vec2<f32>(f32(params.origin_x + gid.x), f32(params.origin_y + gid.y)) + 0.5) / full;
    var rgb = textureLoad(src_tex, p, 0).rgb;

    for (var i = 0u; i < params.spot_count; i++) {
        let record = records[params.local_count + i];
        if record.b.w < 0.5 { continue; }
        let distance = stroke_distance(
            uv,
            u32(round(record.header.z)),
            u32(round(record.header.w)),
            record.a.xy,
        );
        let mask = feathered_disk(distance, record.b.x, record.b.y) * record.b.z;
        if mask <= 0.0 { continue; }
        let source_uv = uv + record.a.zw - record.a.xy;
        let sampled=textureLoad(source_tex,source_pixel(source_uv),0).rgb;
        var replacement = sampled;
        if record.header.y < 0.5 {
            var target_mean = ring_mean(record.a.xy, record.b.x * 1.25);
            var source_mean = ring_mean(record.a.zw, record.b.x * 1.25);
            if params.patch_source != 0u {
                target_mean = stored_ring_mean(0);
                source_mean = stored_ring_mean(4);
            }
            replacement = max(replacement + target_mean - source_mean, vec3<f32>(0.0));
            if record.corrective.x > 0.0 {
                let radius=max(record.corrective.y,1.0);
                let texture_match=sampled-healing_low_frequency(source_uv,radius,true)+healing_low_frequency(uv,radius,false);
                replacement=mix(replacement,max(texture_match,vec3<f32>(0.0)),record.corrective.x*0.01);
            }
        }
        rgb = mix(rgb, replacement, mask);
    }

    for (var i = 0u; i < params.local_count; i++) {
        let record = records[i];
        if record.header.y < 0.5 { continue; }
        let mask = local_mask(record, uv, rgb);
        if params.preview_mask == i + 1u {
            textureStore(out_tex, p, vec4<f32>(mask));
            return;
        }
        if mask > 0.0 { rgb = adjusted(rgb, record, mask, uv); }
    }
    textureStore(out_tex, p, vec4<f32>(rgb, 1.0));
}
