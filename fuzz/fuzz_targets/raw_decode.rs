#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    let Ok(mut file) = tempfile::Builder::new().suffix(".dng").tempfile() else {
        return;
    };
    if file.write_all(data).is_ok() {
        let _ = sp_raw::decode(file.path());
    }
});
