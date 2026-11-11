#![no_main]
// TST-API-084: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| searchdump_rs::fuzz_api::renamer_new_index(data));
