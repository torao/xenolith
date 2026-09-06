//! Data models for the independent structures of XML's core.
//!
//! These are the parsed shapes of XML constructs, kept as plain data with their queries and building methods. The
//! behavior that produces or interprets them, reading text into a model, assembling one, checking a document against
//! one, lives in higher crates. Keeping the models low lets the shared event vocabulary and every crate above core
//! name them without depending on that behavior.

pub mod dtd;
