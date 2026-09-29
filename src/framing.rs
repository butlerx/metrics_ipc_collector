//! Length-prefixed framing for metric events.
//!
//! Each frame is a big-endian `u32` payload length followed by a `MessagePack`
//! encoded [`MetricEvent`]. Binary payloads can contain any byte, so delimiter
//! based framing is not safe.

use crate::{error::MetricsError, events::MetricEvent};
#[cfg(any(not(feature = "tokio"), test))]
use std::io::Read;
use std::io::{self, ErrorKind, Write};
#[cfg(feature = "tokio")]
use tokio::io::{AsyncRead, AsyncReadExt};

/// Largest payload accepted from a peer. Guards against huge allocations when a
/// length prefix is corrupt.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

const HEADER_LEN: usize = 4;

/// Encodes an event into a complete frame, header included.
pub fn encode(event: &MetricEvent) -> Result<Vec<u8>, MetricsError> {
    let payload: Vec<u8> = event.try_into()?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|&len| len as usize <= MAX_FRAME_LEN)
        .ok_or(MetricsError::FrameTooLarge(payload.len()))?;

    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn payload_len(header: [u8; HEADER_LEN]) -> io::Result<usize> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME_LEN} byte limit"),
        ));
    }
    Ok(len)
}

/// Reads one frame's payload into `buffer`.
///
/// Returns `Ok(false)` when the peer closed the stream on a frame boundary.
#[cfg(any(not(feature = "tokio"), test))]
pub fn read_frame<R: Read>(reader: &mut R, buffer: &mut Vec<u8>) -> io::Result<bool> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    buffer.resize(payload_len(header)?, 0);
    reader.read_exact(buffer)?;
    Ok(true)
}

/// Async version of [`read_frame`].
#[cfg(feature = "tokio")]
pub async fn read_frame_async<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut Vec<u8>,
) -> io::Result<bool> {
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        match reader.read(&mut header[filled..]).await {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    buffer.resize(payload_len(header)?, 0);
    reader.read_exact(buffer).await?;
    Ok(true)
}

/// Writes all of `bytes`, retrying when the handle is non-blocking.
///
/// A partial write followed by an error would desynchronise the stream, so
/// `WouldBlock` is retried from the current offset instead of being returned.
///
/// This deliberately never calls `flush`: the writers do no userspace
/// buffering, and `interprocess` implements `flush` on Unix as `fsync`, which
/// can fail with `EINVAL` on pipes and sockets.
pub fn write_all_blocking<W: Write>(writer: &mut W, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MetricData, MetricOperation};
    use std::{collections::BTreeMap, io::Cursor};

    fn event(operation: MetricOperation) -> MetricEvent {
        MetricEvent::Metric(MetricData {
            name: "requests".into(),
            labels: BTreeMap::new(),
            operation,
        })
    }

    fn decode_all(bytes: Vec<u8>) -> Vec<MetricEvent> {
        let mut reader = Cursor::new(bytes);
        let mut buffer = Vec::new();
        let mut events = Vec::new();
        while read_frame(&mut reader, &mut buffer).expect("frame should read") {
            events.push(MetricEvent::try_from(buffer.as_slice()).expect("event should decode"));
        }
        events
    }

    #[test]
    fn payloads_containing_newlines_round_trip() {
        // 10 encodes as 0x0a, which broke the old newline-delimited framing.
        let mut stream = encode(&event(MetricOperation::SetCounter(10))).unwrap();
        stream.extend(
            encode(&event(MetricOperation::SetGauge(f64::from_bits(
                0x0a0a_0a0a_0a0a_0a0a,
            ))))
            .unwrap(),
        );
        stream.extend(encode(&event(MetricOperation::IncrementCounter(3))).unwrap());

        let events = decode_all(stream);
        assert_eq!(events.len(), 3);
        let MetricEvent::Metric(first) = &events[0] else {
            panic!("expected a metric event");
        };
        assert!(matches!(first.operation, MetricOperation::SetCounter(10)));
        let MetricEvent::Metric(last) = &events[2] else {
            panic!("expected a metric event");
        };
        assert!(matches!(
            last.operation,
            MetricOperation::IncrementCounter(3)
        ));
    }

    #[test]
    fn clean_eof_ends_stream() {
        let mut buffer = Vec::new();
        assert!(!read_frame(&mut Cursor::new(Vec::new()), &mut buffer).unwrap());
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut frame = encode(&event(MetricOperation::SetCounter(1))).unwrap();
        frame.truncate(frame.len() - 1);
        let mut buffer = Vec::new();
        let err = read_frame(&mut Cursor::new(frame), &mut buffer).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let header = u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes();
        let mut buffer = Vec::new();
        let err = read_frame(&mut Cursor::new(header.to_vec()), &mut buffer).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }
}
