//! Print the lens and sensor identity ShopPhoto derives from a folder of
//! RAW files.
//!
//! Lens shading correction only auto-matches if the decoder actually
//! surfaces a lens name, and that is easy to get silently wrong — rawler
//! drops the EXIF `LensModel` string whenever its own lens database has no
//! entry, which is the normal case for Hasselblad XCD glass. This prints
//! what reaches `ImageMeta`, so that failure is visible rather than showing
//! up as "lens corrections just never do anything".
//!
//! ```text
//! cargo run --release -p sp-raw --example lens_probe -- ~/Pictures/raws
//! ```

fn main() {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: lens_probe <directory-of-raws>");
        std::process::exit(2);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("cannot read {dir}");
        std::process::exit(1);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !sp_raw::is_supported(&path) {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        match sp_raw::decode(&path) {
            Ok(out) => {
                let m = &out.meta;
                let lens = m.lens_model.clone().unwrap_or_else(|| "<none>".into());
                let sensor = sp_core::hasselblad::sensor_for_model(&m.camera_model)
                    .map_or("<unknown>", |s| s.label);
                println!(
                    "{name}\n  body={} sensor={sensor} iso={:?}\n  lens={lens}\n  \
                     key={} zoom={:?} focal={:?} aperture={:?}",
                    m.camera_model,
                    m.iso,
                    sp_core::lens::lens_key(&lens),
                    sp_core::lens::zoom_position_mm(&lens),
                    m.focal_length_mm,
                    m.aperture_f,
                );
            }
            Err(e) => println!("{name}\n  DECODE FAILED: {e}"),
        }
    }
}
