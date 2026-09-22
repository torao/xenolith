use super::*;

use crate::dom::build::DomBuilder;
use crate::event::validate::ValidatorSet;
use crate::io::write::XmlWriter;

/// The text a written document produces through [`XmlWriter`].
fn written(write: impl FnOnce(&mut WriterSource<'_>) -> Result<()>) -> String {
  let mut writer = XmlWriter::new(Vec::new());
  {
    let mut doc = WriterSource::new().with_handler(&mut writer);
    write(&mut doc).expect("the document is written");
  }
  String::from_utf8(writer.into_inner()).expect("UTF-8")
}

#[test]
fn writes_elements_attributes_and_text() {
  let out = written(|doc| {
    doc.write_start_element("greeting")?;
    doc.write_attribute("xml:lang", "en")?;
    doc.write_characters("Hello & welcome")?;
    doc.write_end_element()?;
    doc.end_document()
  });
  assert_eq!(out, "<greeting xml:lang=\"en\">Hello &amp; welcome</greeting>");
}

#[test]
fn an_element_with_nothing_inside_collapses() {
  let out = written(|doc| {
    doc.write_start_element("a")?;
    doc.write_start_element("b")?;
    doc.write_end_element()?;
    doc.write_end_element()?;
    doc.end_document()
  });
  assert_eq!(out, "<a><b/></a>");
}

#[test]
fn writes_the_other_kinds_of_node() {
  let out = written(|doc| {
    // Nothing here reads declarations, so an empty DTD carries the declaration on its own.
    doc.write_doctype("a", None, Some("a.dtd"), &Dtd::default(), &NamePool::new())?;
    doc.write_start_element("a")?;
    doc.write_comment(" note ")?;
    doc.write_processing_instruction("pi", "data")?;
    doc.write_cdata("1 < 2")?;
    doc.write_end_element()?;
    doc.end_document()
  });
  assert_eq!(out, "<!DOCTYPE a SYSTEM \"a.dtd\"><a><!-- note --><?pi data?><![CDATA[1 < 2]]></a>");
}

#[test]
fn the_same_calls_build_a_tree() {
  let mut builder = DomBuilder::new();
  {
    let mut doc = WriterSource::new().with_handler(&mut builder);
    doc.write_start_element_ns(Some("urn:e"), "e:note").unwrap();
    doc.write_attribute("xmlns:e", "urn:e").unwrap();
    doc.write_start_element("child").unwrap();
    doc.write_characters("hi").unwrap();
    doc.write_end_element().unwrap();
    doc.write_end_element().unwrap();
    doc.end_document().unwrap();
  }
  let doc = builder.into_document();
  let root = doc.document_element().expect("a root element");
  assert_eq!(doc.node_name(root), "e:note");
  assert_eq!(doc.namespace_uri(root), Some("urn:e"));
  assert_eq!(doc.attribute(root, "xmlns:e"), Some("urn:e"));
  assert_eq!(doc.text_content(root), "hi");
}

#[test]
fn a_validator_in_front_refuses_what_it_forbids() {
  // The schema allows only `a`, so the `b` that follows is refused before the writer sees it.
  let dtd = "<!ELEMENT a EMPTY>";
  let (dtd, pool) = crate::dtd::DtdReader::new(dtd.as_bytes()).read().expect("the DTD is read");
  let schema = crate::dtd::DtdSchema::new(dtd, pool);
  let mut validation = ValidatorSet::new().with_schema(&schema).with_error_limit(1);
  let mut writer = XmlWriter::new(Vec::new());
  let error = {
    let mut lane = crate::event::Dispatch::new().with_handler(&mut validation).with_handler(&mut writer);
    let mut doc = WriterSource::new().with_handler(&mut lane);
    doc.write_start_element("a").expect("the root is allowed");
    doc.write_start_element("b").expect("held until its attributes are in");
    let refused = doc.write_characters("x").expect_err("the element is not declared");
    // The run ended as failed, and a write that follows says so rather than being quietly accepted.
    let after = doc.write_characters("y").expect_err("the run has already failed");
    assert!(after.to_string().contains("a handler refused an event"), "{after}");
    refused
  };
  assert!(error.message().contains('b'), "{error}");
  // `<a` is still open, since the writer closes a start tag only when something follows it, and nothing did: the
  // refused element never reached the writer.
  assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<a");
}

#[test]
fn a_handler_that_finishes_early_ends_the_run() {
  /// Takes the first element and finishes.
  #[derive(Default)]
  struct First {
    names: Vec<String>,
    outcome: Option<&'static str>,
  }
  impl EventHandler for First {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      if let EventRef::StartElement(event) = event {
        self.names.push(event.local.to_owned());
      }
      Ok(())
    }

    fn should_continue(&self) -> bool {
      self.names.is_empty()
    }

    fn finish(&mut self, outcome: Outcome<'_>) {
      self.outcome = Some(match outcome {
        Outcome::Completed => "completed",
        Outcome::Stopped => "stopped",
        Outcome::Failed(_) => "failed",
        Outcome::Abandoned => "abandoned",
      });
    }
  }

  let mut first = First::default();
  {
    let mut doc = WriterSource::new().with_handler(&mut first);
    doc.write_start_element("a").unwrap();
    doc.write_start_element("b").unwrap();
    doc.write_end_element().unwrap();
    doc.write_end_element().unwrap();
    doc.end_document().unwrap();
  }
  assert_eq!(first.names, ["a"], "nothing after the handler finished");
  assert_eq!(first.outcome, Some("stopped"));
}

#[test]
fn a_handler_that_finishes_at_the_end_of_the_document_completed_the_run() {
  /// Finishes once it has seen the whole document, `EndDocument` included.
  #[derive(Default)]
  struct Whole {
    ended: bool,
    outcome: Option<&'static str>,
  }

  impl EventHandler for Whole {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      self.ended |= matches!(event, EventRef::EndDocument);
      Ok(())
    }

    fn should_continue(&self) -> bool {
      !self.ended
    }

    fn finish(&mut self, outcome: Outcome<'_>) {
      self.outcome = Some(match outcome {
        Outcome::Completed => "completed",
        Outcome::Stopped => "stopped",
        Outcome::Failed(_) => "failed",
        Outcome::Abandoned => "abandoned",
      });
    }
  }

  let mut whole = Whole::default();
  {
    let mut doc = WriterSource::new().with_handler(&mut whole);
    doc.write_start_element("a").unwrap();
    doc.write_end_element().unwrap();
    doc.end_document().unwrap();
  }
  // Finishing at `EndDocument` is accepting it, not cutting the run short, so the outcome is `Completed` and not
  // `Stopped`. This is what the other sources report there as well.
  assert_eq!(whole.outcome, Some("completed"));
}

#[test]
fn dropping_without_ending_the_document_abandons_the_run() {
  #[derive(Default)]
  struct Outcomes(Vec<&'static str>);
  impl EventHandler for Outcomes {
    fn handle(&mut self, _event: &EventRef<'_>) -> Result<()> {
      Ok(())
    }

    fn finish(&mut self, outcome: Outcome<'_>) {
      self.0.push(match outcome {
        Outcome::Completed => "completed",
        Outcome::Stopped => "stopped",
        Outcome::Failed(_) => "failed",
        Outcome::Abandoned => "abandoned",
      });
    }
  }

  let mut outcomes = Outcomes::default();
  {
    let mut doc = WriterSource::new().with_handler(&mut outcomes);
    doc.write_start_element("a").unwrap();
    doc.write_end_element().unwrap();
  }
  assert_eq!(outcomes.0, ["abandoned"], "the run was given up part way");
}

#[test]
fn the_document_is_started_and_ended_once() {
  #[derive(Default)]
  struct Kinds(Vec<&'static str>);
  impl EventHandler for Kinds {
    fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
      self.0.push(match event {
        EventRef::StartDocument => "start-document",
        EventRef::EndDocument => "end-document",
        EventRef::StartElement(_) => "start",
        EventRef::EndElement(_) => "end",
        _ => "other",
      });
      Ok(())
    }
  }

  let mut kinds = Kinds::default();
  {
    let mut doc = WriterSource::new().with_handler(&mut kinds);
    doc.start_document().unwrap();
    // The document has begun, so a second call reports nothing; it is not a mistake, since the first write begins the
    // run anyway.
    doc.start_document().unwrap();
    doc.write_start_element("a").unwrap();
    doc.write_end_element().unwrap();
    doc.end_document().unwrap();
    // The run is over, and writing after it is the caller's mistake.
    let error = doc.end_document().expect_err("the document has already ended");
    assert!(error.to_string().contains("end_document was already called"), "{error}");
    let error = doc.write_start_element("b").expect_err("the document has already ended");
    assert!(error.to_string().contains("the document has ended"), "{error}");
  }
  assert_eq!(kinds.0, ["start-document", "start", "end", "end-document"]);
}

#[test]
fn the_dtd_given_reaches_the_doctype_event() {
  let (dtd, pool) =
    crate::dtd::DtdReader::new("<!ATTLIST a k ID #IMPLIED>".as_bytes()).read().expect("the DTD is read");
  let mut builder = DomBuilder::new();
  {
    let mut doc = WriterSource::new().with_handler(&mut builder);
    doc.write_doctype("a", None, None, &dtd, &pool).unwrap();
    doc.write_start_element("a").unwrap();
    doc.write_attribute("k", "x").unwrap();
    doc.write_end_element().unwrap();
    doc.end_document().unwrap();
  }
  let doc = builder.into_document();
  // The builder marked `k` as an ID because the DTD the source carried declares it one.
  assert!(doc.get_element_by_id("x").is_some(), "the DTD reached the builder");
}

#[test]
fn depth_counts_the_open_elements() {
  let mut doc = WriterSource::new();
  assert_eq!(doc.depth(), 0);
  doc.write_start_element("a").unwrap();
  assert_eq!(doc.depth(), 1, "a start tag still open counts");
  doc.write_start_element("b").unwrap();
  assert_eq!(doc.depth(), 2);
  doc.write_end_element().unwrap();
  assert_eq!(doc.depth(), 1);
  doc.end_document().unwrap();
}
