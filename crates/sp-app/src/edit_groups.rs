//! Inspector groups over `EditState`: which controls belong to which card,
//! how many of them differ from their defaults, how to reset or bypass one
//! card, and a short human label for an edit step ("Exposure +0.60").
//! Pure functions, no egui, so the history and card badges are testable.

use sp_core::EditState;

/// One develop-inspector card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Basic,
    Curve,
    Color,
    Local,
    Detail,
    Lens,
    Effects,
    Resolution,
    Geometry,
    Hdr,
}

impl Group {
    pub const ALL: [Group; 10] = [
        Group::Basic,
        Group::Curve,
        Group::Color,
        Group::Local,
        Group::Detail,
        Group::Lens,
        Group::Effects,
        Group::Resolution,
        Group::Geometry,
        Group::Hdr,
    ];

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Group::Basic => "Basic",
            Group::Curve => "Tone Curve",
            Group::Color => "Color",
            Group::Local => "Local & retouch",
            Group::Detail => "Detail",
            Group::Lens => "Lens Corrections",
            Group::Effects => "Effects",
            Group::Resolution => "Resolution",
            Group::Geometry => "Geometry",
            Group::Hdr => "HDR",
        }
    }

    /// Bit used by the bypass mask in `AppState`.
    #[must_use]
    pub fn bit(self) -> u16 {
        1 << (self as u16)
    }

    /// Groups whose effect can be judged by temporarily switching them off.
    /// Geometry and resolution change the output frame, so bypassing them
    /// would move the image rather than compare it.
    #[must_use]
    pub fn can_bypass(self) -> bool {
        !matches!(self, Group::Resolution | Group::Geometry)
    }
}

/// How a changed value is written in labels.
#[derive(Debug, Clone, Copy)]
enum Fmt {
    Signed0,
    Signed2,
    Ev,
    Plain0,
    Plain1,
    Deg,
    Toggle,
}

fn signed(value: f32, decimals: usize) -> String {
    let rounded = format!("{:.*}", decimals, value.abs());
    if rounded.trim_start_matches(['0', '.']).is_empty() {
        return format!("{:.*}", decimals, 0.0);
    }
    format!("{}{rounded}", if value < 0.0 { "−" } else { "+" })
}

fn format_value(fmt: Fmt, value: f32) -> String {
    match fmt {
        Fmt::Signed0 => signed(value, 0),
        Fmt::Signed2 => signed(value, 2),
        Fmt::Ev => format!("{} EV", signed(value, 2)),
        Fmt::Plain0 => format!("{value:.0}"),
        Fmt::Plain1 => format!("{value:.1}"),
        Fmt::Deg => format!("{value:.0}°"),
        Fmt::Toggle => crate::i18n::text(if value > 0.5 { "on" } else { "off" }).to_owned(),
    }
}

type Getter = fn(&EditState) -> f32;

const fn f(
    group: Group,
    label: &'static str,
    fmt: Fmt,
    get: Getter,
) -> (Group, &'static str, Fmt, Getter) {
    (group, label, fmt, get)
}

fn b(value: bool) -> f32 {
    f32::from(u8::from(value))
}

/// Every scalar control, in inspector order.
const SCALARS: &[(Group, &str, Fmt, Getter)] = &[
    f(Group::Basic, "Film negative", Fmt::Toggle, |e| {
        b(e.negative.enabled)
    }),
    f(Group::Basic, "Film base red", Fmt::Plain1, |e| {
        e.negative.base.0[0]
    }),
    f(Group::Basic, "Film base green", Fmt::Plain1, |e| {
        e.negative.base.0[1]
    }),
    f(Group::Basic, "Film base blue", Fmt::Plain1, |e| {
        e.negative.base.0[2]
    }),
    f(Group::Basic, "Film density range", Fmt::Plain1, |e| {
        e.negative.density_range
    }),
    f(Group::Basic, "Film black density", Fmt::Plain1, |e| {
        e.negative.black_density
    }),
    f(Group::Basic, "Film gamma", Fmt::Plain1, |e| {
        e.negative.gamma
    }),
    f(Group::Basic, "Tint", Fmt::Signed0, |e| e.wb.tint),
    f(Group::Basic, "Exposure", Fmt::Ev, |e| e.tone.exposure_ev),
    f(Group::Basic, "Brightness", Fmt::Signed0, |e| {
        e.tone.brightness
    }),
    f(Group::Basic, "Contrast", Fmt::Signed0, |e| e.tone.contrast),
    f(Group::Basic, "Highlights", Fmt::Signed0, |e| {
        e.tone.highlights
    }),
    f(Group::Basic, "Shadows", Fmt::Signed0, |e| e.tone.shadows),
    f(Group::Basic, "Whites", Fmt::Signed0, |e| e.tone.whites),
    f(Group::Basic, "Blacks", Fmt::Signed0, |e| e.tone.blacks),
    f(Group::Basic, "Look strength", Fmt::Plain0, |e| {
        e.look_strength.unwrap_or(100.0)
    }),
    f(Group::Basic, "Highlight recovery", Fmt::Toggle, |e| {
        b(e.highlight_recovery.enabled)
    }),
    f(Group::Basic, "Recovery", Fmt::Plain0, |e| {
        e.highlight_recovery.strength
    }),
    f(Group::Basic, "Propagation", Fmt::Plain0, |e| {
        f32::from(e.highlight_recovery.radius)
    }),
    f(Group::Curve, "Curve highlights", Fmt::Signed0, |e| {
        e.curve.parametric.highlights
    }),
    f(Group::Curve, "Curve lights", Fmt::Signed0, |e| {
        e.curve.parametric.lights
    }),
    f(Group::Curve, "Curve darks", Fmt::Signed0, |e| {
        e.curve.parametric.darks
    }),
    f(Group::Curve, "Curve shadows", Fmt::Signed0, |e| {
        e.curve.parametric.shadows
    }),
    f(Group::Color, "Vibrance", Fmt::Signed0, |e| e.color.vibrance),
    f(Group::Color, "Saturation", Fmt::Signed0, |e| {
        e.color.saturation
    }),
    f(Group::Color, "Shadows hue", Fmt::Deg, |e| {
        e.color.grading.shadows.hue_degrees
    }),
    f(Group::Color, "Shadows grade", Fmt::Plain0, |e| {
        e.color.grading.shadows.saturation
    }),
    f(Group::Color, "Midtones hue", Fmt::Deg, |e| {
        e.color.grading.midtones.hue_degrees
    }),
    f(Group::Color, "Midtones grade", Fmt::Plain0, |e| {
        e.color.grading.midtones.saturation
    }),
    f(Group::Color, "Highlights hue", Fmt::Deg, |e| {
        e.color.grading.highlights.hue_degrees
    }),
    f(Group::Color, "Highlights grade", Fmt::Plain0, |e| {
        e.color.grading.highlights.saturation
    }),
    f(Group::Color, "Grading balance", Fmt::Signed0, |e| {
        e.color.grading.balance
    }),
    f(Group::Color, "Monochrome", Fmt::Toggle, |e| {
        b(e.color.monochrome)
    }),
    f(Group::Detail, "High-detail demosaic", Fmt::Toggle, |e| {
        b(e.high_detail_demosaic)
    }),
    f(Group::Detail, "Sharpness", Fmt::Plain0, |e| {
        e.sharpen.sharpness
    }),
    f(Group::Detail, "Sharpen amount", Fmt::Plain0, |e| {
        e.sharpen.amount
    }),
    f(Group::Detail, "Sharpen radius", Fmt::Plain1, |e| {
        e.sharpen.radius_px
    }),
    f(Group::Detail, "Sharpen detail", Fmt::Plain0, |e| {
        e.sharpen.detail
    }),
    f(Group::Detail, "Masking", Fmt::Plain0, |e| {
        e.sharpen.threshold
    }),
    f(Group::Detail, "Halo guard", Fmt::Plain0, |e| {
        e.sharpen.halo_suppression
    }),
    f(Group::Detail, "Restore blur", Fmt::Plain0, |e| {
        e.sharpen.restore_blur
    }),
    f(Group::Detail, "Blur radius", Fmt::Plain1, |e| {
        e.sharpen.restore_radius_px
    }),
    f(Group::Detail, "Noise guard", Fmt::Plain0, |e| {
        e.sharpen.restore_regularization
    }),
    f(Group::Detail, "Luminance NR", Fmt::Plain0, |e| {
        e.nr.luminance
    }),
    f(Group::Detail, "Luminance detail", Fmt::Plain0, |e| {
        e.nr.luminance_detail
    }),
    f(Group::Detail, "Chroma NR", Fmt::Plain0, |e| e.nr.chroma),
    f(Group::Detail, "Chroma detail", Fmt::Plain0, |e| {
        e.nr.chroma_detail
    }),
    f(Group::Detail, "Hot-pixel removal", Fmt::Toggle, |e| {
        b(e.nr.hot_pixels)
    }),
    f(Group::Lens, "Lens corrections", Fmt::Toggle, |e| {
        b(e.lens.enable)
    }),
    f(Group::Lens, "Lens vignetting", Fmt::Plain0, |e| {
        e.lens.vignetting
    }),
    f(Group::Lens, "Remove fringes", Fmt::Toggle, |e| {
        b(e.lens.defringe)
    }),
    f(Group::Lens, "Purple fringe", Fmt::Plain0, |e| {
        e.lens.defringe_purple
    }),
    f(Group::Lens, "Purple hue", Fmt::Deg, |e| {
        e.lens.defringe_purple_hue
    }),
    f(Group::Lens, "Purple range", Fmt::Plain0, |e| {
        e.lens.defringe_purple_range
    }),
    f(Group::Lens, "Green fringe", Fmt::Plain0, |e| {
        e.lens.defringe_green
    }),
    f(Group::Lens, "Green hue", Fmt::Deg, |e| {
        e.lens.defringe_green_hue
    }),
    f(Group::Lens, "Green range", Fmt::Plain0, |e| {
        e.lens.defringe_green_range
    }),
    f(Group::Lens, "Wide bloom", Fmt::Toggle, |e| {
        b(e.lens.defringe_wide_bloom)
    }),
    f(Group::Lens, "Fringe width", Fmt::Plain0, |e| {
        e.lens.defringe_radius_px
    }),
    f(Group::Lens, "Edge protection", Fmt::Plain0, |e| {
        e.lens.defringe_threshold
    }),
    f(Group::Lens, "Distortion", Fmt::Signed2, |e| {
        e.lens.distortion[0]
    }),
    f(Group::Lens, "Distortion k2", Fmt::Signed2, |e| {
        e.lens.distortion[1]
    }),
    f(Group::Lens, "Distortion k3", Fmt::Signed2, |e| {
        e.lens.distortion[2]
    }),
    f(Group::Lens, "Red CA", Fmt::Signed2, |e| {
        e.lens.lateral_ca[0] * 1000.0
    }),
    f(Group::Lens, "Blue CA", Fmt::Signed2, |e| {
        e.lens.lateral_ca[1] * 1000.0
    }),
    f(Group::Lens, "Crop to valid", Fmt::Toggle, |e| {
        b(e.lens.crop_to_valid)
    }),
    f(Group::Effects, "Texture", Fmt::Signed0, |e| e.local.texture),
    f(Group::Effects, "Clarity", Fmt::Signed0, |e| e.local.clarity),
    f(Group::Effects, "Dehaze", Fmt::Signed0, |e| e.local.dehaze),
    f(Group::Effects, "Vignette", Fmt::Signed0, |e| {
        e.vignette.amount
    }),
    f(Group::Effects, "Vignette midpoint", Fmt::Plain0, |e| {
        e.vignette.midpoint
    }),
    f(Group::Effects, "Vignette roundness", Fmt::Plain0, |e| {
        e.vignette.roundness
    }),
    f(Group::Effects, "Vignette feather", Fmt::Plain0, |e| {
        e.vignette.feather
    }),
    f(Group::Effects, "Grain", Fmt::Plain0, |e| e.grain.amount),
    f(Group::Effects, "Grain size", Fmt::Plain1, |e| e.grain.size),
    f(Group::Effects, "Grain roughness", Fmt::Plain0, |e| {
        e.grain.roughness
    }),
    f(Group::Geometry, "Straighten", Fmt::Deg, |e| {
        e.geometry.rotate_deg
    }),
    f(Group::Geometry, "Flip horizontal", Fmt::Toggle, |e| {
        b(e.geometry.flip_horizontal)
    }),
    f(Group::Geometry, "Flip vertical", Fmt::Toggle, |e| {
        b(e.geometry.flip_vertical)
    }),
    f(Group::Geometry, "Rotation", Fmt::Deg, |e| {
        f32::from(e.geometry.quarter_turns) * 90.0
    }),
    f(Group::Geometry, "Vertical perspective", Fmt::Signed0, |e| {
        e.geometry.perspective_vertical
    }),
    f(
        Group::Geometry,
        "Horizontal perspective",
        Fmt::Signed0,
        |e| e.geometry.perspective_horizontal,
    ),
    f(Group::Geometry, "Scale", Fmt::Plain0, |e| e.geometry.scale),
    f(Group::Hdr, "HDR", Fmt::Toggle, |e| b(e.hdr.enabled)),
    f(Group::Hdr, "Peak headroom", Fmt::Plain1, |e| {
        e.hdr.headroom_stops
    }),
    f(Group::Hdr, "Reference white", Fmt::Signed2, |e| {
        e.hdr.scene_white_ev
    }),
    f(Group::Hdr, "Shoulder", Fmt::Plain0, |e| {
        e.hdr.highlight_rolloff
    }),
];

const BAND_NAMES: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// One control whose value differs between two edits.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub group: Group,
    pub label: String,
    /// New value as text, when the control has a single value.
    pub value: Option<String>,
}

/// Every control that differs between `before` and `after`.
#[must_use]
pub fn changes(before: &EditState, after: &EditState) -> Vec<Change> {
    let mut out = Vec::new();
    let mut push = |group, label: String, value: Option<String>| {
        out.push(Change {
            group,
            label,
            value,
        })
    };
    if before.sample_points != after.sample_points {
        push(Group::Local, "Color sample points".into(), None);
    }
    if before.profile != after.profile {
        let name = match &after.profile {
            sp_core::edit::ProfileChoice::Standard => "ShopPhoto Standard".to_owned(),
            sp_core::edit::ProfileChoice::Named(name) => name.clone(),
        };
        push(Group::Basic, "Profile".into(), Some(name));
    }
    if before.look != after.look {
        push(
            Group::Basic,
            "Look".into(),
            Some(after.look.clone().unwrap_or_else(|| "Standard".into())),
        );
    }
    if before.wb.temp_kelvin != after.wb.temp_kelvin {
        push(
            Group::Basic,
            "Temp".into(),
            Some(
                after
                    .wb
                    .temp_kelvin
                    .map_or_else(|| "As shot".into(), |k| format!("{k:.0} K")),
            ),
        );
    }
    if before.color.photographic.mixer != after.color.photographic.mixer {
        push(Group::Color, "Channel mixer".into(), None);
    }
    if before.color.photographic.selective != after.color.photographic.selective
        || before.color.photographic.relative != after.color.photographic.relative
    {
        push(Group::Color, "Selective color".into(), None);
    }
    if before.color.photographic.skin != after.color.photographic.skin {
        push(Group::Color, "Skin color uniformity".into(), None);
    }
    for (group, label, fmt, get) in SCALARS {
        let (a, z) = (get(before), get(after));
        if (a - z).abs() > 1e-4 {
            push(*group, (*label).to_owned(), Some(format_value(*fmt, z)));
        }
    }
    for (name, (a, z)) in [
        ("Curve", (&before.curve.luma, &after.curve.luma)),
        ("Red curve", (&before.curve.red, &after.curve.red)),
        ("Green curve", (&before.curve.green, &after.curve.green)),
        ("Blue curve", (&before.curve.blue, &after.curve.blue)),
    ] {
        if a != z {
            push(Group::Curve, name.into(), None);
        }
    }
    for (index, band) in BAND_NAMES.iter().enumerate() {
        let (a, z) = (before.color.hsl[index], after.color.hsl[index]);
        for (part, x, y) in [
            ("hue", a.hue_shift, z.hue_shift),
            ("saturation", a.saturation, z.saturation),
            ("luminance", a.luminance, z.luminance),
        ] {
            if (x - y).abs() > 1e-4 {
                push(Group::Color, format!("{band} {part}"), Some(signed(y, 0)));
            }
        }
        let (x, y) = (
            before.color.monochrome_mix[index],
            after.color.monochrome_mix[index],
        );
        if (x - y).abs() > 1e-4 {
            push(
                Group::Color,
                crate::i18n::format("B&W {0}", &[band.to_string()]),
                Some(signed(y, 0)),
            );
        }
    }
    if before.local_adjustments != after.local_adjustments {
        let label = match after
            .local_adjustments
            .len()
            .cmp(&before.local_adjustments.len())
        {
            std::cmp::Ordering::Greater => "Add local adjustment",
            std::cmp::Ordering::Less => "Remove local adjustment",
            std::cmp::Ordering::Equal => "Local adjustment",
        };
        push(Group::Local, label.into(), None);
    }
    if before.spots != after.spots {
        push(
            Group::Local,
            "Heal & clone".into(),
            Some(after.spots.len().to_string()),
        );
    }
    if before.nr.camera_denoise_mode != after.nr.camera_denoise_mode {
        push(
            Group::Detail,
            "AI denoise".into(),
            Some(format!("{:?}", after.nr.camera_denoise_mode)),
        );
    }
    if before.lens.profile != after.lens.profile
        || before.lens.lens_model_override != after.lens.lens_model_override
    {
        push(Group::Lens, "Lens profile".into(), None);
    }
    if before.resolution != after.resolution {
        push(
            Group::Resolution,
            "Resolution".into(),
            Some(format!("{:?}", after.resolution.mode)),
        );
    }
    if before.geometry.crop != after.geometry.crop {
        push(Group::Geometry, "Crop".into(), None);
    }
    if before.hdr.target != after.hdr.target {
        push(
            Group::Hdr,
            "HDR target".into(),
            Some(format!("{:?}", after.hdr.target)),
        );
    }
    out
}

/// Number of controls in `group` that differ from their defaults.
#[must_use]
pub fn modified_count(edit: &EditState, group: Group) -> usize {
    changes(&EditState::default(), edit)
        .iter()
        .filter(|change| change.group == group)
        .count()
}

/// Short history label for the step from `before` to `after`.
#[must_use]
pub fn describe(before: &EditState, after: &EditState) -> String {
    let changes = changes(before, after);
    match changes.as_slice() {
        [] => crate::i18n::text("Edit").to_owned(),
        [one] => match &one.value {
            Some(value) => format!("{} {value}", crate::i18n::text(&one.label)),
            None => crate::i18n::text(&one.label).to_owned(),
        },
        [first, rest @ ..] if rest.iter().all(|c| c.group == first.group) => {
            let __l10n_args = (&(first.group.title()), &(changes.len()));
            crate::i18n::format(
                "{0} · {1} changes",
                &[
                    crate::i18n::text(__l10n_args.0).to_owned(),
                    __l10n_args.1.to_string(),
                ],
            )
        }
        _ => {
            let __l10n_args = (&(changes.len()),);
            crate::i18n::format("{0} changes", &[__l10n_args.0.to_string()])
        }
    }
}

/// Copy `group`'s controls from `source` into `edit`.
pub fn copy_group(edit: &mut EditState, source: &EditState, group: Group) {
    match group {
        Group::Basic => {
            edit.profile = source.profile.clone();
            edit.look = source.look.clone();
            edit.look_strength = source.look_strength;
            edit.wb = source.wb;
            edit.negative = source.negative;
            edit.tone = source.tone;
            edit.highlight_recovery = source.highlight_recovery;
        }
        Group::Curve => edit.curve = source.curve.clone(),
        Group::Color => edit.color = source.color,
        Group::Local => {
            edit.local_adjustments = source.local_adjustments.clone();
            edit.spots = source.spots.clone();
            edit.sample_points = source.sample_points.clone();
        }
        Group::Detail => {
            edit.sharpen = source.sharpen;
            // The AI denoise mode swaps the decoded source; keep it so a
            // reset or bypass does not start a neural re-run.
            let mode = edit.nr.camera_denoise_mode;
            edit.nr = source.nr;
            edit.nr.camera_denoise_mode = mode;
            edit.high_detail_demosaic = source.high_detail_demosaic;
        }
        Group::Lens => edit.lens = source.lens.clone(),
        Group::Effects => {
            edit.local = source.local;
            edit.vignette = source.vignette;
            edit.grain = source.grain;
        }
        Group::Resolution => edit.resolution = source.resolution,
        Group::Geometry => edit.geometry = source.geometry,
        Group::Hdr => edit.hdr = source.hdr,
    }
}

/// Reset `group`'s controls to their defaults.
pub fn reset_group(edit: &mut EditState, group: Group) {
    copy_group(edit, &EditState::default(), group);
}

/// The edit to render with every group in `bypass_mask` switched off.
#[must_use]
pub fn with_bypass(edit: &EditState, bypass_mask: u16) -> EditState {
    let mut out = edit.clone();
    for group in Group::ALL {
        if bypass_mask & group.bit() != 0 && group.can_bypass() {
            reset_group(&mut out, group);
        }
    }
    out
}

fn lerp_value(a: &serde_json::Value, b: &serde_json::Value, t: f64) -> serde_json::Value {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
                Value::from((x as f64 + (y - x) as f64 * t).round() as i64)
            } else {
                let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
                serde_json::Number::from_f64(x + (y - x) * t)
                    .map_or_else(|| b.clone(), Value::Number)
            }
        }
        (Value::Object(x), Value::Object(y)) => Value::Object(
            y.iter()
                .map(|(key, yv)| {
                    let value = x
                        .get(key)
                        .map_or_else(|| yv.clone(), |xv| lerp_value(xv, yv, t));
                    (key.clone(), value)
                })
                .collect(),
        ),
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => Value::Array(
            x.iter()
                .zip(y)
                .map(|(xv, yv)| lerp_value(xv, yv, t))
                .collect(),
        ),
        _ => {
            if t < 0.5 {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// `amount` (0–1) of the way from `base` to `full`: numbers interpolate,
/// switches, names and point lists flip at the halfway mark. Used by the
/// preset Amount slider, so it covers every field a preset can touch.
#[must_use]
pub fn blend(base: &EditState, full: &EditState, amount: f32) -> EditState {
    let t = f64::from(amount.clamp(0.0, 1.0));
    let (Ok(a), Ok(b)) = (serde_json::to_value(base), serde_json::to_value(full)) else {
        return full.clone();
    };
    serde_json::from_value::<EditState>(lerp_value(&a, &b, t))
        .map(EditState::clamped)
        .unwrap_or_else(|_| full.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creative_preset_amount_interpolates_hdr_without_changing_output_intent() {
        use sp_core::edit::HdrTarget;
        use sp_core::preset::DevelopPreset;

        for preset in [DevelopPreset::GoldenHourRadiance, DevelopPreset::NeonNight] {
            for enabled in [false, true] {
                for target in [HdrTarget::Sdr, HdrTarget::Hdr] {
                    let mut base = EditState::default();
                    base.hdr.enabled = enabled;
                    base.hdr.target = target;
                    base.hdr.headroom_stops = 1.25;
                    base.hdr.highlight_rolloff = 30.0;
                    base.hdr.scene_white_ev = 0.25;
                    let mut full = base.clone();
                    preset.apply(&mut full);

                    assert_eq!(blend(&base, &full, 0.0), base);
                    assert_eq!(blend(&base, &full, 1.0), full);
                    let half = blend(&base, &full, 0.5);
                    assert_eq!(half.hdr.enabled, enabled);
                    assert_eq!(half.hdr.target, target);
                    assert_eq!(half.hdr.scene_white_ev, base.hdr.scene_white_ev);
                    if enabled {
                        assert_ne!(full.hdr.highlight_rolloff, base.hdr.highlight_rolloff);
                        assert!(
                            (half.hdr.highlight_rolloff
                                - (base.hdr.highlight_rolloff + full.hdr.highlight_rolloff) / 2.0)
                                .abs()
                                < 1e-4
                        );
                        assert!(
                            (half.hdr.headroom_stops
                                - (base.hdr.headroom_stops + full.hdr.headroom_stops) / 2.0)
                                .abs()
                                < 1e-4
                        );
                    } else {
                        assert_eq!(half.hdr, base.hdr);
                        assert_eq!(full.hdr, base.hdr);
                    }
                }
            }
        }
    }

    #[test]
    fn blend_interpolates_numbers_and_flips_switches_at_half() {
        let base = EditState::default();
        let mut full = base.clone();
        full.tone.highlights = -20.0;
        full.look = Some("Muted".into());
        full.color.monochrome = true;
        let quarter = blend(&base, &full, 0.25);
        assert!((quarter.tone.highlights + 5.0).abs() < 1e-4);
        assert_eq!(quarter.look, None);
        assert!(!quarter.color.monochrome);
        let most = blend(&base, &full, 0.75);
        assert_eq!(most.look.as_deref(), Some("Muted"));
        assert_eq!(blend(&base, &full, 1.0), full.clone().clamped());
    }

    #[test]
    fn describes_a_single_change_with_its_value() {
        let before = EditState::default();
        let mut after = before.clone();
        after.tone.exposure_ev = 0.6;
        assert_eq!(describe(&before, &after), "Exposure +0.60 EV");
        after.tone.highlights = -14.0;
        assert_eq!(describe(&before, &after), "Basic · 2 changes");
        after.local.clarity = 12.0;
        assert_eq!(describe(&before, &after), "3 changes");
    }

    #[test]
    fn negative_values_use_a_real_minus_sign() {
        let before = EditState::default();
        let mut after = before.clone();
        after.tone.highlights = -14.0;
        assert_eq!(describe(&before, &after), "Highlights −14");
    }

    #[test]
    fn counts_and_resets_only_the_requested_group() {
        let mut edit = EditState::default();
        edit.tone.exposure_ev = 1.0;
        edit.tone.contrast = 20.0;
        edit.color.vibrance = 10.0;
        assert_eq!(modified_count(&edit, Group::Basic), 2);
        assert_eq!(modified_count(&edit, Group::Color), 1);
        assert_eq!(modified_count(&edit, Group::Effects), 0);
        reset_group(&mut edit, Group::Basic);
        assert_eq!(modified_count(&edit, Group::Basic), 0);
        assert_eq!(edit.color.vibrance, 10.0);
    }

    #[test]
    fn bypass_leaves_the_stored_edit_untouched() {
        let mut edit = EditState::default();
        edit.color.saturation = 40.0;
        let rendered = with_bypass(&edit, Group::Color.bit());
        assert_eq!(rendered.color.saturation, 0.0);
        assert_eq!(edit.color.saturation, 40.0);
        // Geometry is never bypassed: it would move the frame.
        edit.geometry.rotate_deg = 3.0;
        assert_eq!(
            with_bypass(&edit, Group::Geometry.bit())
                .geometry
                .rotate_deg,
            3.0
        );
    }

    #[test]
    fn default_edit_has_no_changes() {
        assert!(changes(&EditState::default(), &EditState::default()).is_empty());
    }
}
