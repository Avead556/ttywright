use serde_json::Value;

pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body.as_bytes());
    frame
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("bad LSP header")]
    BadHeader,
    #[error("bad LSP message: {0}")]
    BadJson(String),
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn next_message(&mut self) -> Option<Result<Value, FrameError>> {
        const END: &[u8] = b"\r\n\r\n";
        let header_end = self.buffer.windows(END.len()).position(|w| w == END)?;
        let body_start = header_end + END.len();
        let Some(len) = content_length(&self.buffer[..header_end]) else {
            self.buffer.drain(..body_start);
            return Some(Err(FrameError::BadHeader));
        };
        let body_end = body_start.checked_add(len)?;
        if self.buffer.len() < body_end {
            return None;
        }
        let parsed = serde_json::from_slice(&self.buffer[body_start..body_end])
            .map_err(|e| FrameError::BadJson(e.to_string()));
        self.buffer.drain(..body_end);
        Some(parsed)
    }
}

fn content_length(header: &[u8]) -> Option<usize> {
    std::str::from_utf8(header)
        .ok()?
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_message_survives_being_cut_at_any_byte() {
        let message = json!({"text": "привет"});
        let frame = encode(&message);
        for cut in 0..frame.len() {
            let mut decoder = FrameDecoder::default();
            decoder.push(&frame[..cut]);
            assert_eq!(decoder.next_message(), None, "cut at {cut}");
            decoder.push(&frame[cut..]);
            assert_eq!(decoder.next_message(), Some(Ok(message.clone())));
            assert_eq!(decoder.next_message(), None);
        }
    }

    #[test]
    fn length_counts_bytes_not_chars() {
        let frame = encode(&json!("ы"));
        assert!(frame.starts_with(b"Content-Length: 4\r\n"));
    }

    #[test]
    fn several_messages_in_one_read_come_out_in_order() {
        let mut bytes = encode(&json!(1));
        bytes.extend(encode(&json!(2)));
        let mut decoder = FrameDecoder::default();
        decoder.push(&bytes);
        assert_eq!(decoder.next_message(), Some(Ok(json!(1))));
        assert_eq!(decoder.next_message(), Some(Ok(json!(2))));
    }

    #[test]
    fn other_headers_and_any_case_are_accepted() {
        let mut decoder = FrameDecoder::default();
        decoder.push(b"Content-Type: x\r\ncontent-length: 1\r\n\r\n7");
        assert_eq!(decoder.next_message(), Some(Ok(json!(7))));
    }

    #[test]
    fn bad_messages_are_reported_and_skipped() {
        let mut decoder = FrameDecoder::default();
        decoder.push(b"Nonsense: 1\r\n\r\n");
        decoder.push(b"Content-Length: 1\r\n\r\n{");
        decoder.push(&encode(&json!(5)));
        assert_eq!(decoder.next_message(), Some(Err(FrameError::BadHeader)));
        assert!(matches!(
            decoder.next_message(),
            Some(Err(FrameError::BadJson(_)))
        ));
        assert_eq!(decoder.next_message(), Some(Ok(json!(5))));
    }
}
