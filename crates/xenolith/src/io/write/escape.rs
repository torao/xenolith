//! Write text, attribute values, and CDATA sections in a way that ensures they are restored to their original state
//! when read back.
//!
//! Applying escaping enables round-tripping, where the original values are reproduced upon reading back the written
//! content. What needs to be escaped depends on the specific context.
//!
//! Within text, characters such as `<` and `&` signify the start of markup, so they must be written as references (XML
//! 1.0 §2.4). Carriage return (CR) characters must also be written as references. This is because, during line-ending
//! normalization, literal carriage returns are converted into line feeds (LF) (§2.11), whereas writing them as
//! references preserves them without such conversion.
//!
//! In attribute values, tabs, LF, and CR are treated as delimiters, in addition to the quotation marks used to enclose
//! the value. During attribute-value normalization, all of these are converted into spaces (§3.3.3). In contrast,
//! within standard text content, tabs and line breaks are preserved exactly as they are, without conversion.
//!
//! No escaping is performed within CDATA sections. The content is written exactly as is; however, if the content
//! includes the string `]]>`, which would unintentionally terminate the section, it must be split across two separate
//! sections (§2.7). The `]]>` itself goes between them as text, where its `>` is written as a reference because §2.4
//! forbids `]]>` in character data. A section is opened only for data to put in it, so content that begins or ends
//! with `]]>`, or holds several in a row, does not produce sections holding nothing.

#[cfg(test)]
mod test;

/// Appends `text` as character data, escaping characters that would otherwise be interpreted as the start of markup
/// (XML 1.0 §2.4).
pub(crate) fn push_text(out: &mut String, text: &str) {
  for c in text.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      // §2.4 requires `>` as a reference only where it would close a `]]>`, and for compatibility at that. Writing
      // every one that way is simpler and correct everywhere.
      '>' => out.push_str("&gt;"),
      // A literal carriage return would be folded to a line feed on the next parse; keep it as a reference so the
      // round trip is exact.
      '\r' => out.push_str("&#13;"),
      _ => out.push(c),
    }
  }
}

/// Appends `value` as a double-quoted attribute value, applying escaping in accordance with requirements for
/// delimiters and attribute-value normalization (XML 1.0 §3.3.3).
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

/// Appends `data` within a CDATA section, splitting the content as necessary to prevent the section from being
/// prematurely terminated by an embedded `]]>` sequence (XML 1.0 §2.7).
pub(crate) fn push_cdata(out: &mut String, data: &str) {
  if data.is_empty() {
    push_section(out, data);
    return;
  }
  let mut rest = data;
  while let Some(at) = rest.find("]]>") {
    // Only what there is goes in a section. Data that begins or ends with `]]>`, or holds several in a row, would
    // otherwise be written around sections holding nothing.
    if at > 0 {
      push_section(out, &rest[..at]);
    }
    // The `]]>` itself goes between the sections as text. Its `>` must be a reference: §2.4 forbids `]]>` in
    // character data.
    out.push_str("]]&gt;");
    rest = &rest[at + 3..];
  }
  if !rest.is_empty() {
    push_section(out, rest);
  }
}

/// Appends `<![CDATA[`, `text`, and `]]>`. `text` is written as it stands, so it must hold no `]]>`.
fn push_section(out: &mut String, text: &str) {
  out.push_str("<![CDATA[");
  out.push_str(text);
  out.push_str("]]>");
}
