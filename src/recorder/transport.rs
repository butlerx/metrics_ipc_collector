//! Blocking transports the recorder core writes frames to.

use crate::framing;
use std::io::{self, Write};

/// Where a blocking recorder writes encoded frames.
pub trait Transport: Send {
    /// Called once before any other write, with the hello frame if the
    /// recorder has labels. Transports that reconnect send it again on every
    /// new connection.
    ///
    /// # Errors
    /// Returns the error from writing the hello frame.
    fn start(&mut self, hello: Option<Vec<u8>>) -> io::Result<()>;

    /// Writes one or more complete frames.
    ///
    /// # Errors
    /// On error, none of `frames` should be assumed delivered.
    fn send(&mut self, frames: &[u8]) -> io::Result<()>;

    /// How many times the transport has reconnected.
    fn reconnects(&self) -> u64 {
        0
    }
}

/// A transport over a single writer that never reconnects, such as a pipe.
pub struct PlainTransport<W>(pub W);

impl<W: Write + Send> Transport for PlainTransport<W> {
    fn start(&mut self, hello: Option<Vec<u8>>) -> io::Result<()> {
        hello.map_or(Ok(()), |hello| self.send(&hello))
    }

    fn send(&mut self, frames: &[u8]) -> io::Result<()> {
        framing::write_all_blocking(&mut self.0, frames)
    }
}
