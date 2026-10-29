//! SSE encoding, incremental decoding and retained-position resumption.
use crate::{Failure, FailureKind, bad_request};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Reserved recovery event shared by stream producers and generated clients.
pub const RELOAD_EVENT: &str = "reload";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventPosition {
    pub generation: String,
    pub cursor: String,
}
impl EventPosition {
    pub fn event_id(&self) -> String {
        // A JSON tuple is unambiguous for arbitrary product positions.
        serde_json::to_string(&(&self.generation, &self.cursor)).expect("strings serialize")
    }
    pub fn from_event_id(value: &str) -> Result<Self, Failure> {
        let (generation, cursor) = serde_json::from_str(value)
            .map_err(|e| bad_request(format!("invalid Last-Event-ID: {e}")))?;
        Ok(Self { generation, cursor })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event: String,
    pub data: String,
    pub retry: Option<u64>,
}
impl SseEvent {
    pub fn json<T: Serialize>(
        id: impl Into<String>,
        event: impl Into<String>,
        value: &T,
    ) -> Result<Self, Failure> {
        let result = Self {
            id: Some(id.into()),
            event: event.into(),
            data: serde_json::to_string(value)
                .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))?,
            retry: None,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), Failure> {
        if self.event.contains(['\r', '\n'])
            || self
                .id
                .as_ref()
                .is_some_and(|id| id.contains(['\r', '\n', '\0']))
        {
            return Err(bad_request("SSE event and id must each be one line"));
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<String, Failure> {
        self.validate()?;
        let mut out = String::new();
        if let Some(id) = &self.id {
            out.push_str("id: ");
            out.push_str(id);
            out.push('\n');
        }
        out.push_str("event: ");
        out.push_str(&self.event);
        out.push('\n');
        if let Some(retry) = self.retry {
            out.push_str(&format!("retry: {retry}\n"));
        }
        // Normalize line endings as the EventSource algorithm does.
        for line in self
            .data
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
        {
            out.push_str("data: ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        Ok(out)
    }
}

/// Stateful byte decoder. It retains split UTF-8 and split CRLF between reads.
pub struct SseDecoder {
    buffer: Vec<u8>,
    current: Option<SseEvent>,
    last_id: Option<String>,
    data: Vec<String>,
    first_line: bool,
}
impl Default for SseDecoder {
    fn default() -> Self {
        Self {
            buffer: vec![],
            current: None,
            last_id: None,
            data: vec![],
            first_line: true,
        }
    }
}
impl SseDecoder {
    pub fn new() -> Self {
        Self {
            first_line: true,
            ..Self::default()
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, Failure> {
        self.buffer.extend_from_slice(bytes);
        let mut result = vec![];
        while let Some(index) = self.buffer.iter().position(|b| matches!(b, b'\r' | b'\n')) {
            if self.buffer[index] == b'\r' && index + 1 == self.buffer.len() {
                break;
            }
            let end = index
                + if self.buffer[index] == b'\r' && self.buffer.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            let line = std::str::from_utf8(&self.buffer[..index])
                .map_err(|e| bad_request(format!("invalid SSE UTF-8: {e}")))?
                .to_string();
            self.buffer.drain(..end);
            let line = if self.first_line {
                self.first_line = false;
                line.trim_start_matches('\u{feff}')
            } else {
                &line
            };
            if line.is_empty() {
                if !self.data.is_empty() {
                    let mut event = self.current.take().unwrap_or_else(|| SseEvent {
                        id: self.last_id.clone(),
                        event: "message".into(),
                        data: String::new(),
                        retry: None,
                    });
                    event.data = self.data.join("\n");
                    self.data.clear();
                    if event.event.is_empty() {
                        event.event = "message".into();
                    }
                    result.push(event);
                } else {
                    self.current = None;
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            let current = self.current.get_or_insert_with(|| SseEvent {
                id: self.last_id.clone(),
                event: "message".into(),
                data: String::new(),
                retry: None,
            });
            match field {
                "data" => self.data.push(value.into()),
                "event" => current.event = value.into(),
                "id" if !value.contains('\0') => {
                    self.last_id = Some(value.into());
                    current.id = Some(value.into());
                }
                "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                    current.retry = value.parse().ok()
                }
                _ => {}
            }
        }
        Ok(result)
    }
}

/// An event supplied only after the product's commit. The product's commit
/// position is opaque to this crate and must uniquely identify each event.
#[derive(Clone, Debug)]
pub struct CommittedEvent<T> {
    pub position: EventPosition,
    pub event: String,
    pub value: T,
}
pub enum Resume<'a, T> {
    Events(Vec<&'a CommittedEvent<T>>),
    Reload { reason: String },
}
/// Retention is a product policy, independent of framing. No event is evicted
/// implicitly; the product explicitly advances its retained boundary.
pub struct EventLog<T> {
    generation: String,
    events: VecDeque<CommittedEvent<T>>,
    base: Option<EventPosition>,
}
impl<T> EventLog<T> {
    pub fn new(generation: impl Into<String>) -> Self {
        Self {
            generation: generation.into(),
            events: VecDeque::new(),
            base: None,
        }
    }
    pub fn append(&mut self, event: CommittedEvent<T>) -> Result<(), Failure> {
        if event.position.generation != self.generation {
            return Err(bad_request("event belongs to a different generation"));
        }
        if self.base.as_ref() == Some(&event.position)
            || self.events.iter().any(|e| e.position == event.position)
        {
            return Err(bad_request("duplicate event position"));
        }
        self.events.push_back(event);
        Ok(())
    }
    /// Drop committed events through this position, retaining it as the
    /// oldest resumable boundary. An absent position cannot advance retention.
    pub fn retain_after(&mut self, position: &EventPosition) -> Result<(), Failure> {
        let index = self
            .events
            .iter()
            .position(|e| e.position == *position)
            .ok_or_else(|| bad_request("retention position not found"))?;
        self.events.drain(..=index);
        self.base = Some(position.clone());
        Ok(())
    }
    pub fn resume(&self, last_event_id: Option<&str>) -> Resume<'_, T> {
        let Some(last) = last_event_id else {
            return Resume::Events(self.events.iter().collect());
        };
        let Ok(position) = EventPosition::from_event_id(last) else {
            return Resume::Reload {
                reason: "unrecognized position".into(),
            };
        };
        if position.generation != self.generation {
            return Resume::Reload {
                reason: "generation changed".into(),
            };
        }
        if self.base.as_ref() == Some(&position) {
            return Resume::Events(self.events.iter().collect());
        }
        match self.events.iter().position(|e| e.position == position) {
            Some(i) => Resume::Events(self.events.iter().skip(i + 1).collect()),
            None => Resume::Reload {
                reason: "position no longer retained".into(),
            },
        }
    }
}
impl<T: Serialize> Resume<'_, T> {
    pub fn encode(&self) -> Result<String, Failure> {
        match self {
            Self::Events(events) => events
                .iter()
                .map(|e| SseEvent::json(e.position.event_id(), &e.event, &e.value)?.encode())
                .collect(),
            Self::Reload { reason } => SseEvent {
                id: None,
                event: RELOAD_EVENT.into(),
                data: serde_json::json!({"reason":reason}).to_string(),
                retry: None,
            }
            .encode(),
        }
    }
}
pub const SSE_HEADERS: [(&str, &str); 3] = [
    ("content-type", "text/event-stream"),
    ("cache-control", "no-cache"),
    ("x-accel-buffering", "no"),
];
