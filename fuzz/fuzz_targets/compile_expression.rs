//! Parses an XPath expression, prints it, parses it again, and evaluates it.
#![no_main]

use libfuzzer_sys::fuzz_target;

// An expression is text, so the fuzzer is asked for text rather than bytes: it then spends its
// time on expressions rather than on rediscovering UTF-8.
fuzz_target!(|text: &str| {
  xenolith_fuzz::compile_expression(text);
  xenolith_fuzz::evaluate_expression(text);
});
