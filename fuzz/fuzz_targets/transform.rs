//! Compiles arbitrary bytes as a stylesheet and runs it over a fixed document.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
  xenolith_fuzz::transform(data);
});
