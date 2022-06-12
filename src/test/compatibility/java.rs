use crate::test;
use std::{fs, process::Command};

const CLASS_NAME: &str = "CompatibilityTest";

/// Executes the specified Java code and returns standard output results.
///
pub fn run(test_case: &str, code: &str) -> String {
  let file_name = format!("{}.java", CLASS_NAME);
  let code = format!(
    r#"import java.io.*; import org.xml.sax.*; import javax.xml.parsers.*; import org.w3c.dom.*;public class {} {{ public static void main(String[] args) throws Exception {{ {} }}}}"#,
    CLASS_NAME, code
  );

  let dir = test::temp_dir(test_case);
  let file = dir.as_ref().join(&file_name);
  fs::write(file, code).unwrap();

  capture_output(
    Command::new("javac")
      .arg("-J-Dfile.encoding=UTF-8")
      .arg("-J-Duser.language=en")
      .arg("-J-Duser.country=US")
      .arg(file_name)
      .current_dir(&dir),
  );

  capture_output(
    Command::new("java")
      .arg("-Dfile.encoding=UTF-8")
      .arg("-Duser.language=en")
      .arg("-Duser.country=US")
      .arg(CLASS_NAME)
      .current_dir(&dir),
  )
}

fn capture_output(cmd: &mut Command) -> String {
  let output = match cmd.output() {
    Ok(output) => output,
    Err(err) => panic!("[{}] {:?}", cmd.get_program().to_string_lossy().to_string(), err),
  };
  if !output.status.success() {
    let msg = String::from_utf8_lossy(&output.stderr).to_string();
    panic!("[{}] {}", output.status.code().unwrap_or(-1), msg);
  }
  String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn test_run() {
  let output = run("test_run", r#"System.out.print("hello, world");"#);
  assert_eq!("hello, world", output);
}

#[test]
fn test_document() {
  let output = run(
    "test_document",
    r#"
    String xml = "<some-xml/>";
    DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
    DocumentBuilder builder = factory.newDocumentBuilder();
    InputSource is = new InputSource(new StringReader(xml));
    Document doc = builder.parse(is);
    System.out.print(doc.getParentNode());
  "#,
  );
  assert_eq!("null", output);
}
