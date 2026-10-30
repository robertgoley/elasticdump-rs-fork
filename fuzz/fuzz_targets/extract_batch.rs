#![no_main]
// TST-API-084: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| elasticdump_rs::fuzz_api::extract_batch(data));
