#![no_main]
// L-645 / D-4: see src/fuzz_api.rs.
libfuzzer_sys::fuzz_target!(|data: &[u8]| searchdump_rs::fuzz_api::secret_redaction(data));
