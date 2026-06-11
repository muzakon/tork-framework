//! A reader for Server-Sent Events responses.

use std::time::Duration;

use http_body_util::BodyExt;
use serde::de::DeserializeOwned;

use super::client::StreamingBody;
use crate::error::{Error, Result};

/// A single event parsed from a Server-Sent Events stream.
pub struct TestSseEvent {
    event: Option<String>,
    data: String,
    id: Option<String>,
}

impl TestSseEvent {
    /// The event name (the `event:` field), if any.
    pub fn event(&self) -> Option<&str> {
        self.event.as_deref()
    }

    /// The event payload (the joined `data:` lines).
    pub fn data(&self) -> &str {
        &self.data
    }

    /// The event id (the `id:` field), if any.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Parses the event data as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_str(&self.data)
            .map_err(|error| Error::internal(format!("event data is not valid JSON: {error}")))
    }
}

/// A reader over a `text/event-stream` response body.
///
/// Returned by [`sse`](super::TestRequestBuilder::sse). Pull events with
/// [`next_event`](TestSseStream::next_event). Heartbeat comments are skipped, so
/// every returned event carries a name, data, or id.
pub struct TestSseStream {
    body: StreamingBody,
    buffer: String,
    done: bool,
}

impl TestSseStream {
    pub(crate) fn new(body: StreamingBody) -> Self {
        Self {
            body,
            buffer: String::new(),
            done: false,
        }
    }

    /// Returns the next event, or `None` once the stream ends.
    pub async fn next_event(&mut self) -> Result<Option<TestSseEvent>> {
        loop {
            // Emit any complete, non-heartbeat event already in the buffer.
            if let Some(block) = self.take_block() {
                if let Some(event) = parse_event(&block) {
                    return Ok(Some(event));
                }
                continue;
            }
            if self.done {
                return Ok(None);
            }
            // Otherwise read the next body frame into the buffer.
            match self.body.frame().await {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.buffer.push_str(&String::from_utf8_lossy(&data));
                    }
                }
                Some(Err(error)) => {
                    return Err(Error::internal(format!("event stream error: {error}")));
                }
                None => {
                    self.done = true;
                    // A trailing block without its blank-line terminator is still
                    // worth parsing once the stream ends.
                    if !self.buffer.trim().is_empty() {
                        let block = std::mem::take(&mut self.buffer);
                        if let Some(event) = parse_event(&block) {
                            return Ok(Some(event));
                        }
                    }
                    return Ok(None);
                }
            }
        }
    }

    /// Like [`next_event`](TestSseStream::next_event) but fails if no event
    /// arrives within `timeout`.
    pub async fn next_event_timeout(&mut self, timeout: Duration) -> Result<Option<TestSseEvent>> {
        tokio::time::timeout(timeout, self.next_event())
            .await
            .map_err(|_| Error::internal("timed out waiting for an event").with_code("SSE_TIMEOUT"))?
    }

    /// Removes and returns the next complete event block (terminated by a blank
    /// line) from the buffer, if one is present.
    fn take_block(&mut self) -> Option<String> {
        let index = self.buffer.find("\n\n")?;
        let block: String = self.buffer.drain(..index + 2).collect();
        Some(block)
    }
}

/// Parses one event block into an event, or `None` for a heartbeat/comment-only
/// block (no name, data, or id).
fn parse_event(block: &str) -> Option<TestSseEvent> {
    let mut event = None;
    let mut id = None;
    let mut data_lines: Vec<&str> = Vec::new();
    let mut has_field = false;

    for line in block.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue; // blank line or comment (heartbeat)
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => {
                event = Some(value.to_owned());
                has_field = true;
            }
            "id" => {
                id = Some(value.to_owned());
                has_field = true;
            }
            "data" => {
                data_lines.push(value);
                has_field = true;
            }
            "retry" => has_field = true,
            _ => {}
        }
    }

    if !has_field {
        return None;
    }
    Some(TestSseEvent {
        event,
        data: data_lines.join("\n"),
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::BoxError;
    use bytes::Bytes;
    use futures_util::stream;
    use http_body::Frame;
    use http_body_util::StreamBody;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Payload {
        value: i64,
    }

    fn body_from_chunks(
        chunks: Vec<std::result::Result<Frame<Bytes>, BoxError>>,
    ) -> StreamingBody {
        Box::pin(StreamBody::new(stream::iter(chunks)))
    }

    #[test]
    fn parse_event_collects_name_id_and_multiline_data() {
        let event = parse_event("event: tick\nid: 7\ndata: first\ndata: second\n\n").unwrap();

        assert_eq!(event.event(), Some("tick"));
        assert_eq!(event.id(), Some("7"));
        assert_eq!(event.data(), "first\nsecond");
    }

    #[test]
    fn parse_event_skips_heartbeat_only_blocks() {
        assert!(parse_event(": keep-alive\n\n").is_none());
    }

    #[test]
    fn event_json_reports_invalid_payload() {
        let event = parse_event("data: not-json\n\n").unwrap();

        let error = event.json::<Payload>().unwrap_err();
        assert!(error.message().starts_with("event data is not valid JSON:"));
    }

    #[tokio::test]
    async fn next_event_parses_trailing_block_at_end_of_stream() {
        let body = body_from_chunks(vec![Ok(Frame::data(Bytes::from_static(
            b"event: tick\ndata: {\"value\":1}",
        )))]);
        let mut stream = TestSseStream::new(body);

        let event = stream.next_event().await.unwrap().unwrap();
        assert_eq!(event.event(), Some("tick"));
        assert_eq!(event.json::<Payload>().unwrap(), Payload { value: 1 });
        assert!(stream.next_event().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn next_event_reports_stream_errors() {
        let error: BoxError = Box::new(std::io::Error::other("boom"));
        let body = body_from_chunks(vec![Err(error)]);
        let mut stream = TestSseStream::new(body);

        let error = match stream.next_event().await {
            Ok(_) => panic!("expected stream error"),
            Err(error) => error,
        };
        assert!(error.message().contains("event stream error: boom"));
    }

    #[tokio::test]
    async fn next_event_timeout_reports_deadline() {
        let body: StreamingBody = Box::pin(StreamBody::new(stream::pending::<
            std::result::Result<Frame<Bytes>, BoxError>,
        >()));
        let mut stream = TestSseStream::new(body);

        let error = match stream.next_event_timeout(Duration::from_millis(5)).await {
            Ok(_) => panic!("expected timeout"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "SSE_TIMEOUT");
    }
}
