fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/coreml.m")
            .file("src/enhancement.m")
            .flag("-fobjc-arc")
            .compile("sp_ai_coreml");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=CoreML");
        println!("cargo:rerun-if-changed=src/coreml.m");
        println!("cargo:rerun-if-changed=src/enhancement.m");
    }
}
