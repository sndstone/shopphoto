//! macOS EDR colorspace shim.
//!
//! wgpu 29 sets `CAMetalLayer.wantsExtendedDynamicRangeContent` for
//! `Rgba16Float` surfaces but NEVER assigns a colorspace. With a nil
//! colorspace, Core Animation clamps component values at 1.0 — extended
//! range silently does nothing (the "HDR shows no extra brightness"
//! symptom). This module finds wgpu's CAMetalLayer (attached by
//! raw-window-metal as a sublayer of the window's content view) and sets
//! `kCGColorSpaceExtendedLinearSRGB`, which is the scRGB contract the
//! render pipeline already targets. It also reads the screen's current EDR
//! headroom for the UI badge.
//!
//! This is the documented §10.6 exception to the "no platform code" rule:
//! wgpu 29 provably lacks the capability (wgpu ≥ 30 exposes it as
//! `SurfaceColorSpace::ExtendedSrgbLinear`); delete this module when
//! eframe upgrades. Tracking note lives in docs/phase-6.md.

use objc2::encode::{Encoding, RefEncode};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::ffi::c_void;

/// Opaque CoreGraphics colorspace with the exact Objective-C type encoding
/// (`^{CGColorSpace=}`) that `-[CAMetalLayer setColorspace:]` declares —
/// objc2 verifies encodings at runtime in debug builds.
#[repr(C)]
struct CGColorSpace {
    _opaque: [u8; 0],
}
unsafe impl RefEncode for CGColorSpace {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColorSpace", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceExtendedLinearSRGB: *const c_void; // CFStringRef
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut CGColorSpace;
    fn CGColorSpaceRelease(space: *mut CGColorSpace);
}

/// Walk one level of sublayers looking for a CAMetalLayer.
unsafe fn find_metal_layer(view: *mut AnyObject) -> Option<*mut AnyObject> {
    if view.is_null() {
        return None;
    }
    let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
    if layer.is_null() {
        return None;
    }
    let metal_class = class!(CAMetalLayer);
    let is_metal: Bool = unsafe { msg_send![layer, isKindOfClass: metal_class] };
    if is_metal.as_bool() {
        return Some(layer);
    }
    let sublayers: *mut AnyObject = unsafe { msg_send![layer, sublayers] };
    if sublayers.is_null() {
        return None;
    }
    let count: usize = unsafe { msg_send![sublayers, count] };
    for i in 0..count {
        let sub: *mut AnyObject = unsafe { msg_send![sublayers, objectAtIndex: i] };
        let is_metal: Bool = unsafe { msg_send![sub, isKindOfClass: metal_class] };
        if is_metal.as_bool() {
            return Some(sub);
        }
    }
    None
}

/// Assign the extended-linear-sRGB colorspace to the app window's metal
/// layer. Returns `true` once applied; call again next frame until it
/// succeeds (the window/layer may not exist during the first frames).
pub fn apply_extended_colorspace() -> bool {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut AnyObject = msg_send![app, windows];
        if windows.is_null() {
            return false;
        }
        let count: usize = msg_send![windows, count];
        for i in 0..count {
            let window: *mut AnyObject = msg_send![windows, objectAtIndex: i];
            let view: *mut AnyObject = msg_send![window, contentView];
            let Some(layer) = find_metal_layer(view) else {
                continue;
            };
            let space = CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearSRGB);
            if space.is_null() {
                return false;
            }
            let _: () = msg_send![layer, setColorspace: space];
            let _: () = msg_send![layer, setWantsExtendedDynamicRangeContent: Bool::YES];
            CGColorSpaceRelease(space); // the layer retains its own reference
            tracing::info!("CAMetalLayer colorspace set to extended linear sRGB (EDR live)");
            return true;
        }
    }
    false
}

/// The screen's POTENTIAL extra dynamic range — the panel's capability
/// regardless of the moment's brightness (≈16 on XDR MacBook Pros, 1.0 on
/// SDR-only panels). The right signal for "is this an HDR display".
pub fn potential_edr_headroom() -> f32 {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let window: *mut AnyObject = msg_send![app, mainWindow];
        let screen: *mut AnyObject = if window.is_null() {
            msg_send![class!(NSScreen), mainScreen]
        } else {
            msg_send![window, screen]
        };
        if screen.is_null() {
            return 1.0;
        }
        let max: f64 = msg_send![
            screen,
            maximumPotentialExtendedDynamicRangeColorComponentValue
        ];
        if max.is_finite() && max >= 1.0 {
            max as f32
        } else {
            1.0
        }
    }
}

/// The screen's CURRENT extra dynamic range: 1.0 = no headroom (SDR-only
/// panel, brightness maxed, or no EDR content on screen yet — macOS ramps
/// this up once extended-range pixels are actually presented). Poll it.
pub fn current_edr_headroom() -> f32 {
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let window: *mut AnyObject = msg_send![app, mainWindow];
        let screen: *mut AnyObject = if window.is_null() {
            msg_send![class!(NSScreen), mainScreen]
        } else {
            msg_send![window, screen]
        };
        if screen.is_null() {
            return 1.0;
        }
        let max: f64 = msg_send![screen, maximumExtendedDynamicRangeColorComponentValue];
        if max.is_finite() && max >= 1.0 {
            max as f32
        } else {
            1.0
        }
    }
}
