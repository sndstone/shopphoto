//! # sp-core
//!
//! Foundation crate for ShopPhoto. Contains the types shared by every other
//! crate: raw image buffers, camera metadata, edit parameters, camera color
//! profiles, and the workspace-wide error type.
//!
//! Rules (see BUILD.md §3.1 and §4):
//! - This crate depends on nothing inside the workspace and only on
//!   `serde`/`thiserror` externally. Keep it that way.
//! - Color values crossing crate boundaries always carry their color space in
//!   the type ([`color::LinearRec2020`]) — never bare `[f32; 3]`.
//! - Every serialized struct carries a `version` field and its `Default` impl
//!   is the neutral (no-op) edit.

pub mod analysis;
pub mod astro;
pub mod color;
pub mod commands;
pub mod correction;
pub mod cube;
pub mod edit;
pub mod error;
pub mod geometry;
pub mod hasselblad;
pub mod lens;
pub mod mask;
pub mod multiframe;
pub mod panorama;
pub mod pixel_shift;
pub mod preset;
pub mod profile;
pub mod raw;
pub mod restoration;
pub mod retouch;
pub mod sequence;
pub mod source;
pub mod wb;

pub use edit::EditState;
pub use error::SpError;
pub use lens::LensProfile;
pub use raw::{CfaPattern, ImageMeta, RawImage};
