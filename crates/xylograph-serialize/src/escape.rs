//! Escaping character data and attribute values.
//!
//! Serialization must turn the characters a value happens to contain back into markup that
//! parses to the same value. The two contexts differ: in text `<`, `&` and `>` are the markup
//! characters, while in a double-quoted attribute the quote and the whitespace that attribute
//! normalization would fold also have to be written as references.

/// Appends `text` as character data, escaping the markup-significant characters.
pub(crate) fn push_text(out: &mut String, text: &str) {
  for c in text.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      // `>` only has to be escaped to break up `]]>`, but escaping every one is simpler and
      // always correct.
      '>' => out.push_str("&gt;"),
      // A literal carriage return would be folded to a line feed on the next parse; keep it as
      // a reference so the round trip is exact.
      '\r' => out.push_str("&#13;"),
      _ => out.push(c),
    }
  }
}

/// Appends `value` as the contents of a double-quoted attribute, escaping what that context and
/// attribute-value normalization require.
pub(crate) fn push_attribute(out: &mut String, value: &str) {
  for c in value.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '"' => out.push_str("&quot;"),
      // Tab, line feed and carriage return survive only as references: written literally they
      // would be normalized to spaces when the attribute is read back.
      '\t' => out.push_str("&#9;"),
      '\n' => out.push_str("&#10;"),
      '\r' => out.push_str("&#13;"),
      _ => out.push(c),
    }
  }
}

/// Appends `data` inside a CDATA section, splitting it so an embedded `]]>` cannot close the
/// section early.
pub(crate) fn push_cdata(out: &mut String, data: &str) {
  out.push_str("<![CDATA[");
  let mut rest = data;
  while let Some(at) = rest.find("]]>") {
    out.push_str(&rest[..at]);
    // Close before the `>` and reopen, so the `]]>` is split across two sections.
    out.push_str("]]]]><![CDATA[>");
    rest = &rest[at + 3..];
  }
  out.push_str(rest);
  out.push_str("]]>");
}

#[cfg(test)]
mod tests {
  use super::*;

  fn text(s: &str) -> String {
    let mut out = String::new();
    push_text(&mut out, s);
    out
  }

  fn attribute(s: &str) -> String {
    let mut out = String::new();
    push_attribute(&mut out, s);
    out
  }

  fn cdata(s: &str) -> String {
    let mut out = String::new();
    push_cdata(&mut out, s);
    out
  }

  #[test]
  fn text_escapes_markup() {
    assert_eq!(text("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    assert_eq!(text("line\rend"), "line&#13;end");
    assert_eq!(text("plain"), "plain");
  }

  #[test]
  fn attribute_escapes_quote_and_whitespace() {
    assert_eq!(attribute("say \"hi\""), "say &quot;hi&quot;");
    assert_eq!(attribute("a\tb\nc"), "a&#9;b&#10;c");
    assert_eq!(attribute("x & y < z"), "x &amp; y &lt; z");
  }

  #[test]
  fn cdata_splits_the_close_delimiter() {
    assert_eq!(cdata("plain"), "<![CDATA[plain]]>");
    assert_eq!(cdata("a]]>b"), "<![CDATA[a]]]]><![CDATA[>b]]>");
  }
}
