#![no_main]
// TST-SUP-022: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| searchdump_rs::fuzz_api::sonic_vs_serde(data));
