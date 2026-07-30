// Evaluates XPath 1.0 expressions with the JDK's own engine, for xylograph to compare itself
// against. See `tests/differential.rs`, which drives this.
//
// One expression per line on standard input; one answer per line on standard output, as either
// "ok<TAB>value" or "error<TAB>message". The value is escaped so that one answer is one line
// whatever it contains — the Rust side escapes the same way before comparing.
//
// Run with the JDK's single-file source mode, so nothing has to be built first:
//
//     java XPathReference.java document.xml < expressions.txt
//
// The engine is javax.xml.xpath on whatever the JDK ships (Xalan's, on every JDK to date). That
// is the implementation this library sets out to agree with.

import java.io.BufferedReader;
import java.io.File;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.xpath.XPath;
import javax.xml.xpath.XPathConstants;
import javax.xml.xpath.XPathFactory;
import org.w3c.dom.Document;

public final class XPathReference {
  public static void main(String[] args) throws Exception {
    if (args.length < 1) {
      System.err.println("usage: java XPathReference.java <document.xml> < expressions");
      System.exit(2);
    }

    DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
    // Without this, a prefixed name is one long name and the namespace axis says nothing —
    // which is not the XPath data model, and would make every namespaced case disagree for a
    // reason that is about the parser rather than about XPath.
    factory.setNamespaceAware(true);
    Document document = factory.newDocumentBuilder().parse(new File(args[0]));

    XPath xpath = XPathFactory.newInstance().newXPath();
    BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
    PrintStream out = new PrintStream(System.out, true, StandardCharsets.UTF_8);

    String line;
    while ((line = in.readLine()) != null) {
      if (line.isEmpty()) {
        continue;
      }
      try {
        // Every expression arrives wrapped so that its value is a string; see differential.rs
        // for why the comparison goes through string() rather than through node serialization.
        String value = (String) xpath.compile(line).evaluate(document, XPathConstants.STRING);
        out.println("ok\t" + escape(value));
      } catch (Exception failure) {
        Throwable cause = failure.getCause() == null ? failure : failure.getCause();
        out.println("error\t" + escape(String.valueOf(cause.getMessage())));
      }
    }
  }

  /// Makes one answer one line: backslash, tab and the line breaks become escapes.
  private static String escape(String text) {
    StringBuilder written = new StringBuilder(text.length());
    for (int index = 0; index < text.length(); index++) {
      char character = text.charAt(index);
      switch (character) {
        case '\\': written.append("\\\\"); break;
        case '\t': written.append("\\t"); break;
        case '\n': written.append("\\n"); break;
        case '\r': written.append("\\r"); break;
        default: written.append(character);
      }
    }
    return written.toString();
  }
}
