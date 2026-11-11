#![no_main]
// TST-SUP-020: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| searchdump_rs::fuzz_api::zstd_in(data));
