//! Builds a DOM from arbitrary bytes, writes it out, and reads it back.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
  xenolith_fuzz::build_and_serialize(data);
});
