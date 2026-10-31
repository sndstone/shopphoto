//! Stable develop commands shared by desktop controls and headless automation.
//! Commands validate completely before producing an edit; the caller owns undo/persistence.
use crate::{
    edit::{LocalAdjustment, LocalMask},
    EditState, SpError,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct ControlSpec {
    pub id: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

macro_rules! controls {
    ($($id:literal, $min:expr, $max:expr, $default:expr;)+) => {
        pub const CONTROLS: &[ControlSpec] = &[$(ControlSpec { id: $id, min: $min, max: $max, default: $default },)+];
    };
}
controls! {
    "light.exposure", -5.0, 5.0, 0.0;
    "light.brightness", -100.0, 100.0, 0.0;
    "light.contrast", -100.0, 100.0, 0.0;
    "light.highlights", -100.0, 100.0, 0.0;
    "light.shadows", -100.0, 100.0, 0.0;
    "light.whites", -100.0, 100.0, 0.0;
    "light.blacks", -100.0, 100.0, 0.0;
    "color.vibrance", -100.0, 100.0, 0.0;
    "color.saturation", -100.0, 100.0, 0.0;
    "effects.clarity", -100.0, 100.0, 0.0;
    "effects.texture", -100.0, 100.0, 0.0;
    "effects.dehaze", -100.0, 100.0, 0.0;
    "wb.temperature", 2000.0, 50000.0, 5500.0;
    "wb.tint", -150.0, 150.0, 0.0;
    "detail.sharpening", 0.0, 200.0, 0.0;
}

fn invalid(message: impl Into<String>) -> SpError {
    SpError::CorruptData {
        path: "command".into(),
        reason: message.into(),
    }
}

/// Read a registered scalar control; None means an unknown control.
pub fn control_value(edit: &EditState, id: &str) -> Option<f32> {
    Some(match id {
        "light.exposure" => edit.tone.exposure_ev,
        "light.brightness" => edit.tone.brightness,
        "light.contrast" => edit.tone.contrast,
        "light.highlights" => edit.tone.highlights,
        "light.shadows" => edit.tone.shadows,
        "light.whites" => edit.tone.whites,
        "light.blacks" => edit.tone.blacks,
        "color.vibrance" => edit.color.vibrance,
        "color.saturation" => edit.color.saturation,
        "effects.clarity" => edit.local.clarity,
        "effects.texture" => edit.local.texture,
        "effects.dehaze" => edit.local.dehaze,
        "wb.temperature" => edit.wb.temp_kelvin.unwrap_or(5500.0),
        "wb.tint" => edit.wb.tint,
        "detail.sharpening" => edit.sharpen.amount,
        _ => return None,
    })
}

fn set(edit: &mut EditState, id: &str, value: f32) {
    match id {
        "light.exposure" => edit.tone.exposure_ev = value,
        "light.brightness" => edit.tone.brightness = value,
        "light.contrast" => edit.tone.contrast = value,
        "light.highlights" => edit.tone.highlights = value,
        "light.shadows" => edit.tone.shadows = value,
        "light.whites" => edit.tone.whites = value,
        "light.blacks" => edit.tone.blacks = value,
        "color.vibrance" => edit.color.vibrance = value,
        "color.saturation" => edit.color.saturation = value,
        "effects.clarity" => edit.local.clarity = value,
        "effects.texture" => edit.local.texture = value,
        "effects.dehaze" => edit.local.dehaze = value,
        "wb.temperature" => edit.wb.temp_kelvin = Some(value),
        "wb.tint" => edit.wb.tint = value,
        "detail.sharpening" => edit.sharpen.amount = value,
        _ => {} // validated against CONTROLS before mutation
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "params")]
pub enum EditCommand {
    #[serde(rename = "develop.set")]
    Set(SetValues),
    #[serde(rename = "develop.reset")]
    Reset(Empty),
    #[serde(rename = "mask.radial")]
    Radial(RadialMask),
    #[serde(rename = "mask.linear")]
    Linear(LinearMask),
    #[serde(rename = "mask.remove")]
    Remove(RemoveMask),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetValues {
    pub values: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadialMask {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    pub feather: f32,
    pub exposure_ev: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearMask {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub exposure_ev: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveMask {
    pub index: usize,
}

fn normalized(values: &[f32]) -> bool {
    values
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
fn exposure(value: f32) -> bool {
    value.is_finite() && (-5.0..=5.0).contains(&value)
}

impl EditCommand {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Set(_) => "develop.set",
            Self::Reset(_) => "develop.reset",
            Self::Radial(_) => "mask.radial",
            Self::Linear(_) => "mask.linear",
            Self::Remove(_) => "mask.remove",
        }
    }

    /// Produces a new edit only after all parameters pass validation.
    pub fn apply(&self, edit: &EditState) -> Result<EditState, SpError> {
        let mut next = edit.clone();
        let mask = match self {
            Self::Set(params) => {
                if params.values.is_empty() || params.values.len() > CONTROLS.len() {
                    return Err(invalid("Supply at least one registered control"));
                }
                for (id, value) in &params.values {
                    let spec = CONTROLS
                        .iter()
                        .find(|spec| spec.id == id)
                        .ok_or_else(|| invalid(format!("Unknown control {id}")))?;
                    if !value.is_finite() || !(spec.min..=spec.max).contains(value) {
                        return Err(invalid(format!(
                            "{id} must be between {} and {}",
                            spec.min, spec.max
                        )));
                    }
                }
                for (id, value) in &params.values {
                    set(&mut next, id, *value);
                }
                None
            }
            Self::Reset(_) => return Ok(EditState::default()),
            Self::Radial(p) => {
                if !normalized(&p.center)
                    || !normalized(&p.radius)
                    || p.radius.contains(&0.0)
                    || !normalized(&[p.feather])
                    || !exposure(p.exposure_ev)
                {
                    return Err(invalid("Invalid normalized radial mask or exposure"));
                }
                Some((
                    LocalMask::Radial {
                        center: p.center,
                        radius: p.radius,
                        feather: p.feather,
                    },
                    p.exposure_ev,
                ))
            }
            Self::Linear(p) => {
                if !normalized(&p.start)
                    || !normalized(&p.end)
                    || p.start == p.end
                    || !exposure(p.exposure_ev)
                {
                    return Err(invalid("Invalid normalized linear mask or exposure"));
                }
                Some((
                    LocalMask::LinearGradient {
                        start: p.start,
                        end: p.end,
                    },
                    p.exposure_ev,
                ))
            }
            Self::Remove(p) => {
                if p.index >= next.local_adjustments.len() {
                    return Err(invalid("Mask index is out of range"));
                }
                next.local_adjustments.remove(p.index);
                None
            }
        };
        if let Some((mask, exposure_ev)) = mask {
            if next.local_adjustments.len() >= 64 {
                return Err(invalid("Maximum of 64 local adjustments reached"));
            }
            let mut adjustment = LocalAdjustment {
                mask,
                ..Default::default()
            };
            adjustment.tone.exposure_ev = exposure_ev;
            next.local_adjustments.push(adjustment);
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_invalid_updates_never_partially_apply() {
        let original = EditState::default();
        let command = EditCommand::Set(SetValues {
            values: [
                ("light.exposure".into(), 1.0),
                ("light.shadows".into(), 200.0),
            ]
            .into(),
        });
        assert!(command.apply(&original).is_err());
        assert_eq!(original, EditState::default());
        let command = EditCommand::Set(SetValues {
            values: [("light.exposure".into(), f32::NAN)].into(),
        });
        assert!(command.apply(&original).is_err());
    }

    #[test]
    fn registry_controls_all_round_trip_and_invalid_mask_is_refused() {
        for spec in CONTROLS {
            let command = EditCommand::Set(SetValues {
                values: [(spec.id.into(), spec.max)].into(),
            });
            let edit = command.apply(&EditState::default()).unwrap();
            assert_eq!(control_value(&edit, spec.id), Some(spec.max));
        }
        let mask = EditCommand::Linear(LinearMask {
            start: [0.5; 2],
            end: [0.5; 2],
            exposure_ev: 1.0,
        });
        assert!(mask.apply(&EditState::default()).is_err());
    }
}
