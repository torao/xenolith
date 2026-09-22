use std::pin::Pin;
use std::task::{Context, Poll};

use super::*;

/// A source that yields to the executor before every byte, as a socket would. Runtime-agnostic:
/// it drives on any executor, so the tests need no tokio.
struct Trickle {
  bytes: Vec<u8>,
  at: usize,
  ready: bool,
}

impl Trickle {
  fn new(text: &str) -> Self {
    Self { bytes: text.as_bytes().to_vec(), at: 0, ready: false }
  }
}

impl AsyncRead for Trickle {
  fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
    let this = self.get_mut();
    if this.at == this.bytes.len() {
      return Poll::Ready(Ok(0));
    }
    this.ready = !this.ready;
    if !this.ready {
      // Pending once per byte, so the parser really has to survive being suspended.
      cx.waker().wake_by_ref();
      return Poll::Pending;
    }
    buf[0] = this.bytes[this.at];
    this.at += 1;
    Poll::Ready(Ok(1))
  }
}

/// An owned `futures_io::AsyncRead` over a byte buffer, for entity content built at run time.
struct Bytes {
  data: Vec<u8>,
  at: usize,
}

impl AsyncRead for Bytes {
  fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
    let this = self.get_mut();
    let n = (this.data.len() - this.at).min(buf.len());
    buf[..n].copy_from_slice(&this.data[this.at..this.at + n]);
    this.at += n;
    Poll::Ready(Ok(n))
  }
}

async fn kinds<R: AsyncRead + Unpin>(mut reader: AsyncReader<R>) -> Result<Vec<TokenKind>> {
  let mut kinds = Vec::new();
  while let Some(kind) = reader.advance().await? {
    kinds.push(kind);
  }
  Ok(kinds)
}

/// The character data of the reader's current event, for the tests that collect a run by hand.
fn text_of<R: AsyncRead + Unpin, Res: AsyncUriResolver>(reader: &AsyncReader<R, Res>) -> &str {
  reader.parser().token_ref().and_then(|e| e.text()).expect("the current event is character data")
}

#[test]
fn reads_a_document() {
  pollster::block_on(async {
    let kinds = kinds(AsyncReader::new(&b"<a>x</a>"[..])).await.unwrap();
    assert_eq!(kinds, [TokenKind::StartElement, TokenKind::Text, TokenKind::EndElement]);
  });
}

#[test]
fn a_source_that_pends_between_bytes_parses_the_same() {
  pollster::block_on(async {
    let xml = "<?xml version='1.0'?><a x='1'>text<b/><!--c--></a>";
    let expected = kinds(AsyncReader::new(xml.as_bytes())).await.unwrap();
    assert_eq!(kinds(AsyncReader::new(Trickle::new(xml))).await.unwrap(), expected);
  });
}

#[test]
fn matches_the_blocking_reader_event_for_event() {
  pollster::block_on(async {
    // The two drivers share a parser; this is the assertion that keeps it that way.
    let xml = "<?xml version='1.0'?><a xmlns:p='urn:p' x='1'><p:b/>text<![CDATA[<]]></a>";
    let mut blocking = crate::io::StreamSource::new(xml.as_bytes());
    let mut expected = Vec::new();
    while let Some(kind) = blocking.advance().unwrap() {
      expected.push((kind, blocking.parser().local_name().to_owned()));
    }

    let mut reader = AsyncReader::new(Trickle::new(xml));
    let mut found = Vec::new();
    while let Some(kind) = reader.advance().await.unwrap() {
      found.push((kind, reader.parser().local_name().to_owned()));
    }
    assert_eq!(found, expected);
  });
}

#[test]
fn errors_carry_their_position() {
  pollster::block_on(async {
    let mut reader = AsyncReader::with_system_id(&b"<a>\n&nosuch;</a>"[..], "file:///doc.xml");
    let error = loop {
      match reader.advance().await {
        Ok(Some(_)) => {}
        Ok(None) => panic!("expected a failure"),
        Err(e) => break e,
      }
    };
    assert_eq!(error.location().system_id.as_deref(), Some("file:///doc.xml"));
    assert_eq!(error.location().line, 2);
  });
}

#[test]
fn a_document_larger_than_the_buffer_is_read_in_full() {
  pollster::block_on(async {
    let xml = format!("<a>{}</a>", "x".repeat(READ_BUFFER_SIZE * 3));
    let mut reader = AsyncReader::new(xml.as_bytes());
    let mut text = String::new();
    while let Some(kind) = reader.advance().await.unwrap() {
      if kind == TokenKind::Text {
        text.push_str(text_of(&reader));
      }
    }
    assert_eq!(text.len(), READ_BUFFER_SIZE * 3);
  });
}

/// A resolver over a runtime-agnostic reader, needing no tokio.
struct AsyncFixture(&'static [u8]);

impl AsyncUriResolver for AsyncFixture {
  async fn resolve(&mut self, _request: &crate::io::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
    // A real resolver would await a socket or a file here.
    Ok(Some(AsyncEntityReader::from_async_read(self.0)))
  }
}

#[test]
fn an_external_entity_is_resolved_asynchronously() {
  pollster::block_on(async {
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(AsyncFixture(b"<b>in</b>"));
    let mut names = Vec::new();
    while let Some(kind) = reader.advance().await.unwrap() {
      if matches!(kind, TokenKind::StartElement | TokenKind::EndElement) {
        names.push(reader.parser().local_name().to_owned());
      }
    }
    // <doc>, <b>, </b>, </doc>: four names.
    assert_eq!(names.len(), 4);
  });
}

#[test]
fn without_a_resolver_an_external_entity_is_refused() {
  pollster::block_on(async {
    let xml = "<!DOCTYPE doc [<!ENTITY e SYSTEM 'e.ent'>]><doc>&e;</doc>";
    let mut reader = AsyncReader::new(xml.as_bytes());
    let error = loop {
      match reader.advance().await {
        Ok(Some(_)) => {}
        Ok(None) => panic!("expected a failure"),
        Err(e) => break e,
      }
    };
    assert!(error.message().contains("no resolver is configured"));
  });
}

/// A resolver that owns its bytes, for content generated at run time.
struct OwnedAsyncEntity(&'static str, Vec<u8>);

impl AsyncUriResolver for OwnedAsyncEntity {
  async fn resolve(&mut self, request: &crate::io::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
    if request.name() == Some(self.0) {
      Ok(Some(AsyncEntityReader::from_async_read(Bytes { data: self.1.clone(), at: 0 })))
    } else {
      Ok(None)
    }
  }
}

#[test]
fn a_large_external_general_entity_streams_across_chunks() {
  pollster::block_on(async {
    // The entity is larger than one read buffer, so it is pulled through several `fill` chunks
    // rather than materialized whole; the reassembled text proves every byte arrived.
    let body = "y".repeat(READ_BUFFER_SIZE * 2 + 100);
    let entity = format!("<b>{body}</b>");
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(OwnedAsyncEntity("e", entity.into_bytes()));
    let mut text = String::new();
    while let Some(kind) = reader.advance().await.unwrap() {
      if kind == TokenKind::Text {
        text.push_str(text_of(&reader));
      }
    }
    assert_eq!(text.len(), READ_BUFFER_SIZE * 2 + 100);
  });
}

/// The `tokio` feature's adapter: a reader implementing tokio's own `AsyncRead` is bridged in.
#[cfg(feature = "tokio")]
#[test]
fn from_tokio_adapts_a_tokio_reader() {
  struct TokioFixture(&'static [u8]);

  impl AsyncUriResolver for TokioFixture {
    async fn resolve(&mut self, _request: &crate::io::resolve::EntityRequest) -> Result<Option<AsyncEntityReader>> {
      Ok(Some(AsyncEntityReader::from_tokio(std::io::Cursor::new(self.0.to_vec()))))
    }
  }

  pollster::block_on(async {
    let xml = "<!DOCTYPE d [<!ENTITY e SYSTEM 'e.ent'>]><d>&e;</d>";
    let mut reader = AsyncReader::new(xml.as_bytes()).with_resolver(TokioFixture(b"<b/>"));
    let mut names = Vec::new();
    while let Some(kind) = reader.advance().await.unwrap() {
      if matches!(kind, TokenKind::StartElement | TokenKind::EndElement) {
        names.push(reader.parser().local_name().to_owned());
      }
    }
    assert_eq!(names.len(), 4); // start and end of <d> and <b>, the entity's content parsed in place
  });
}
