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
