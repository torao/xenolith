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
  assert_eq!(cdata("a]]>b"), "<![CDATA[a]]>]]&gt;<![CDATA[b]]>");

  // A section is opened only for data to put in it, so `]]>` in a row, or at either end, leaves none holding nothing.
  assert_eq!(cdata("]]>]]>]]>"), "]]&gt;]]&gt;]]&gt;");
  assert_eq!(cdata("]]>a"), "]]&gt;<![CDATA[a]]>");
  assert_eq!(cdata("a]]>"), "<![CDATA[a]]>]]&gt;");
  assert_eq!(cdata("a]]>]]>b"), "<![CDATA[a]]>]]&gt;]]&gt;<![CDATA[b]]>");

  // Data with nothing in it is the one empty section written, since that is what the event says was there.
  assert_eq!(cdata(""), "<![CDATA[]]>");
}
