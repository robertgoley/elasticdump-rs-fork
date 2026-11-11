#![no_main]
// TST-INJ-015: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| searchdump_rs::fuzz_api::meta_names(data));
