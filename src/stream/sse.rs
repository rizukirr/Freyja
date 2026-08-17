//! Server-sent event framing, shared by every streaming dialect.

/// One decoded server-sent event.
pub(crate) struct SseFrame {
    /// The `event:` name, when the frame carried one.
    pub(crate) event: Option<String>,
    /// The `data:` payload, with multiple `data:` lines joined by newlines.
    pub(crate) data: String,
}

/// Accumulates response bytes and yields complete frames.
///
/// Bytes are held as `Vec<u8>` rather than `String` because a chunk boundary
/// can land in the middle of a multi-byte codepoint. UTF-8 is only interpreted
/// once a whole frame is in hand.
#[derive(Default)]
pub(super) struct SseBuffer {
    bytes: Vec<u8>,
}

impl SseBuffer {
    /// Appends raw bytes from the response body.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
    }

    /// Splits off the next complete frame, or `None` when more bytes are needed.
    pub(super) fn next_frame(&mut self) -> Option<SseFrame> {
        let (end, next) = separator(&self.bytes)?;
        let raw: Vec<u8> = self.bytes.drain(..next).collect();
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

    #[test]
    fn accepts_crlf_separators() {
        let mut buffer = SseBuffer::default();
        buffer.push(b"event: ping\r\ndata: hi\r\n\r\n");

        let frame = buffer.next_frame().expect("frame");
        assert_eq!(frame.event.as_deref(), Some("ping"));
        assert_eq!(frame.data, "hi");
    }
}
