fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/raster/imageio.m")
            .flag("-fobjc-arc")
            .compile("sp_raw_imageio");
        for framework in ["ImageIO", "CoreGraphics", "CoreFoundation", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        println!("cargo:rerun-if-changed=src/raster/imageio.m");
    }
}
