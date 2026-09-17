use axum::response::sse::Event;
use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::types::SseEvent;

/// State for a single room
pub struct RoomState {
    /// Password set by the first participant (claim)
    pub password: Option<String>,

    /// Active participants: session_id → SSE sender
    pub participants: DashMap<String, mpsc::UnboundedSender<Result<Event, std::convert::Infallible>>>,

    /// Optional human-readable display names per session
    pub display_names: DashMap<String, String>,

    /// Receivers created during join, parked until the SSE GET endpoint picks them up.
    pub pending_receivers: Vec<mpsc::UnboundedReceiver<Result<Event, std::convert::Infallible>>>,
}

impl RoomState {
    pub fn new(password: Option<String>) -> Self {
        Self {
            password,
            participants: DashMap::new(),
            display_names: DashMap::new(),
            pending_receivers: Vec::new(),
        }
    }

    /// Broadcast an event to all participants, optionally excluding one (the sender).
    pub fn broadcast(&self, event: &SseEvent, exclude: Option<&str>) {
        let json = serde_json::to_string(event).unwrap_or_default();
        let sse_event = Event::default().data(json);

        let mut stale = Vec::new();

        for entry in self.participants.iter() {
            let (id, tx) = entry.pair();
            if Some(id.as_str()) == exclude {
                continue;
            }
            if tx.send(Ok(sse_event.clone())).is_err() {
                stale.push(id.clone());
            }
        }

        for id in stale {
            self.participants.remove(&id);
            self.display_names.remove(&id);
        }
    }
}
