//! Server-sent event framing, shared by every streaming dialect.

/// One decoded server-sent event.
pub(crate) struct SseFrame {
    /// The `event:` name, when the frame carried one.
    pub(crate) event: Option<String>,
    /// The `data:` payload, with multiple `data:` lines joined by newlines.
    pub(crate) data: String,
}

/// The most one frame may buffer before the stream is abandoned.
///
/// A frame is a JSON object, and the largest any provider sends is a terminal
/// object carrying the whole interaction, so this is orders of magnitude above
/// anything legitimate. Without a ceiling, an endpoint that never emits a
/// separator -- broken, or hostile, and Freyja is built to be pointed at
/// gateways it has never met -- grows this buffer until the process dies. The
/// read timeout bounds silence, not volume, so it does not catch this.
pub(super) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The longest separator, less one: how far back a resumed scan must reach to
/// catch a separator straddling the previous chunk boundary.
const SEPARATOR_OVERLAP: usize = 3;

/// Accumulates response bytes and yields complete frames.
///
/// Bytes are held as `Vec<u8>` rather than `String` because a chunk boundary
/// can land in the middle of a multi-byte codepoint. UTF-8 is only interpreted
/// once a whole frame is in hand.
#[derive(Default)]
pub(super) struct SseBuffer {
    bytes: Vec<u8>,
    /// How much of `bytes` has already been searched and found separator-free.
    ///
    /// Without it every arriving chunk rescanned the whole buffer from zero,
    /// which is quadratic in the size of a frame: small text deltas hide it,
    /// a large reasoning blob does not.
    scanned: usize,
}

impl SseBuffer {
    /// Appends raw bytes from the response body.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
    }

    /// Whether the buffer has grown past what any real frame needs.
    pub(super) fn overflowed(&self) -> bool {
        self.bytes.len() > MAX_FRAME_BYTES
    }

    /// Splits off the next complete frame, or `None` when more bytes are needed.
    pub(super) fn next_frame(&mut self) -> Option<SseFrame> {
        let from = self.scanned.saturating_sub(SEPARATOR_OVERLAP);
        let Some((end, next)) = separator(&self.bytes[from..]) else {
            self.scanned = self.bytes.len();
            return None;
        };
        let (end, next) = (from + end, from + next);

        let raw: Vec<u8> = self.bytes.drain(..next).collect();
        self.scanned = 0;
        let text = String::from_utf8_lossy(&raw[..end]);

        let mut event = None;
        let mut data = String::new();
        for line in text.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                event = Some(rest.trim().to_string());
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }

        Some(SseFrame { event, data })
    }
}

/// Finds the blank line ending a frame.
///
/// Returns the offset where the frame's text ends and the offset where the next
/// frame begins, which differ by the length of the separator.
fn separator(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, index + 4));
        }
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, index + 2));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frames_across_pushes() {
        let mut buffer = SseBuffer::default();

        buffer.push(b"event: delta\ndata: ca");
        assert!(buffer.next_frame().is_none(), "frame is not complete yet");

        buffer.push("fé\n\ndata: second\n\n".as_bytes());

        let first = buffer.next_frame().expect("first frame");
        assert_eq!(first.event.as_deref(), Some("delta"));
        assert_eq!(first.data, "café");

        let second = buffer.next_frame().expect("second frame");
        assert_eq!(second.event, None);
        assert_eq!(second.data, "second");

        assert!(buffer.next_frame().is_none());
    }

    #[test]
    fn handles_codepoint_split_across_chunks() {
        let mut buffer = SseBuffer::default();
        let text = "data: café\n\n".as_bytes();

        // 'é' is two bytes; split the buffer between them.
        let split = text.len() - 4;
        buffer.push(&text[..split]);
        assert!(buffer.next_frame().is_none());

        buffer.push(&text[split..]);
        assert_eq!(buffer.next_frame().expect("frame").data, "café");
    }

    #[test]
    fn joins_multiple_data_lines_and_skips_comments() {
        let mut buffer = SseBuffer::default();
        buffer.push(b": keepalive\ndata: one\ndata: two\n\n");

        assert_eq!(buffer.next_frame().expect("frame").data, "one\ntwo");
    }

    /// The case the resumed scan could lose. A separator that straddles the
    /// boundary lies partly in bytes already reported separator-free, so a scan
    /// resuming at `scanned` would step over it.
    #[test]
    fn finds_a_separator_straddling_a_chunk_boundary() {
        for (first, second) in [
            (&b"data: hi\n"[..], &b"\ndata: two\n\n"[..]),
            (&b"data: hi\r\n\r"[..], &b"\ndata: two\r\n\r\n"[..]),
            (&b"data: hi\r\n"[..], &b"\r\ndata: two\r\n\r\n"[..]),
            (&b"data: hi\r"[..], &b"\n\r\ndata: two\r\n\r\n"[..]),
        ] {
            let mut buffer = SseBuffer::default();
            buffer.push(first);
            assert!(buffer.next_frame().is_none());

            buffer.push(second);
            assert_eq!(buffer.next_frame().expect("first frame").data, "hi");
            assert_eq!(buffer.next_frame().expect("second frame").data, "two");
            assert!(buffer.next_frame().is_none());
        }
    }

    #[test]
    fn reports_a_frame_that_never_ends() {
        let mut buffer = SseBuffer::default();
        assert!(!buffer.overflowed());

        buffer.push(&vec![b'x'; MAX_FRAME_BYTES]);
        assert!(!buffer.overflowed(), "the ceiling itself is still legal");

        buffer.push(b"x");
        assert!(buffer.overflowed());
    }

    #[test]
    fn accepts_crlf_separators() {
        let mut buffer = SseBuffer::default();
        buffer.push(b"event: ping\r\ndata: hi\r\n\r\n");

        let frame = buffer.next_frame().expect("frame");
        assert_eq!(frame.event.as_deref(), Some("ping"));
        assert_eq!(frame.data, "hi");
    }
}
