#![no_main]
// TST-INJ-015: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| elasticdump_rs::fuzz_api::url_segments(data));
