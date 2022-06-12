/// An abstracted character stream that can read characters via buffer.
///

pub trait CharReader {
  fn read(&mut self, buffer: &[char]) -> usize;
}
