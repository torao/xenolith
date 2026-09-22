//! Validates arbitrary bytes against whatever DTD they declare.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
  xenolith_fuzz::validate_document(data);
});
