use super::*;

#[test]
fn display_uses_the_spec_constant_name() {
  // The value a caller sees must read as the code other DOM implementations report, not as a Rust identifier.
  let error = DomException::new(ExceptionCode::HIERARCHY_REQUEST_ERR, "a text node cannot have children");
  assert_eq!(error.to_string(), "HIERARCHY_REQUEST_ERR: a text node cannot have children");
}
