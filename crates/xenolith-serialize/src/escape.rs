//! Writing text, attribute values, and CDATA sections so that they read back unchanged.
//!
//! Escaping makes the round trip work: read the output back, and the value you wrote comes out again. What to rewrite
//! depends on where it is written.
//!
//! In text, `<` and `&` start markup, so you write them as references (XML 1.0 §2.4). A carriage return is written as
//! one too. Line-end normalization folds a literal one into a line feed (§2.11), and a reference survives that.
//!
//! In an attribute value, the delimiting quote joins them, and so do tab, line feed, and carriage return.
//! Attribute-value normalization folds each of those into a space (§3.3.3). Text can hold a tab or a line feed as-is,
//! because nothing folds them there.
//!
//! A CDATA section escapes nothing. Its content is written as-is, and only the `]]>` that would close it early is
//! split across two sections (§2.7).
//!

/// Appends `text` as character data, escaping what would otherwise start markup (XML 1.0 §2.4).
pub(crate) fn push_text(out: &mut String, text: &str) {
  for c in text.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      // §2.4 requires `>` as a reference only where it would close a `]]>`, and for compatibility at that. Writing
      // every one that way is simpler and correct everywhere.
      '>' => out.push_str("&gt;"),
      // A literal carriage return would be folded to a line feed on the next parse; keep it as
      // a reference so the round trip is exact.
      '\r' => out.push_str("&#13;"),
      _ => out.push(c),
    }
  }
}

/// Appends `value` as the content of a double-quoted attribute, escaping what the delimiter and attribute-value
/// normalization require (XML 1.0 §3.3.3).
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

/// Appends `data` inside a CDATA section, splitting it so an embedded `]]>` cannot close the section early
/// (XML 1.0 §2.7).
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
