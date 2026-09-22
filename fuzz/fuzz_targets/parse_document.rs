//! Reads arbitrary bytes with the pull parser, both as a slice and a byte at a time.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
  xenolith_fuzz::parse_document(data);
  // The same bytes through a reader that splits every token, which the slice never does.
  xenolith_fuzz::parse_document_in_pieces(data);
});
