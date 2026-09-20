use dashmap::DashMap;
use tokio::sync::broadcast;

use crate::types::SseEvent;

/// Per-participant event buffer. Covers the gap between `join` and the
/// participant's SSE stream (re)connecting; oldest events are dropped when full.
pub const SSE_BUFFER_SIZE: usize = 128;

/// State for a single room
pub struct RoomState {
    /// Password set by the first participant (claim)
    pub password: Option<String>,

    /// Active participants: session_id → broadcast sender of serialized SSE events.
    /// Each SSE connection subscribes to the sender. Note a new subscriber only
    /// receives events sent *after* it subscribed (the buffer retains events for
    /// lagging subscribers, not for late ones) — the SSE endpoint therefore sends
    /// a `Resync` state snapshot on every (re)connect to heal the gap.
    pub participants: DashMap<String, broadcast::Sender<String>>,

    /// Optional human-readable display names per session
    pub display_names: DashMap<String, String>,
}

impl RoomState {
    pub fn new(password: Option<String>) -> Self {
        Self {
            password,
            participants: DashMap::new(),
            display_names: DashMap::new(),
        }
    }

    /// Whether `session_id` is an active participant.
    pub fn is_participant(&self, session_id: &str) -> bool {
        self.participants.contains_key(session_id)
    }

    /// Broadcast an event to all participants, optionally excluding one (the sender).
    ///
    /// Note: no stale-participant cleanup happens here — broadcast channels don't
    /// report dropped receivers. Stale entries are bounded by the room's max
    /// participant count and are replaced when the same session re-joins.
    pub fn broadcast(&self, event: &SseEvent, exclude: Option<&str>) {
        let json = serde_json::to_string(event).unwrap_or_default();
        for entry in self.participants.iter() {
            if Some(entry.key().as_str()) == exclude {
                continue;
            }
            // send() only fails when the buffer is full (slow consumer); drop the event.
            let _ = entry.value().send(json.clone());
        }
    }

    /// Delivers an event to exactly one participant, used for media signals.
    ///
    /// Delivering to a peer that has since left is a no-op: the sender has nothing
    /// better to do with the signal, and the pair is re-established from the next
    /// `resync` if the peer comes back.
    pub fn send_to(&self, peer_id: &str, event: &SseEvent) {
        let Some(tx) = self.participants.get(peer_id) else {
            return;
        };
        let json = serde_json::to_string(event).unwrap_or_default();
        let _ = tx.value().send(json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> SseEvent {
        SseEvent::ChatMessage {
            from: "a".into(),
            sender_name: Some("Alice".into()),
            text: "hi".into(),
            timestamp_ms: 1,
        }
    }

    #[test]
    fn broadcast_reaches_participants_but_not_sender() {
        let room = RoomState::new(None);
        for id in ["a", "b", "c"] {
            let (tx, _) = broadcast::channel(SSE_BUFFER_SIZE);
            room.participants.insert(id.into(), tx);
        }

        // Subscribers are created first (like live SSE connections), then an event
        // is broadcast.
        let mut ra = room.participants.get("a").unwrap().value().subscribe();
        let mut rb = room.participants.get("b").unwrap().value().subscribe();
        let mut rc = room.participants.get("c").unwrap().value().subscribe();

        room.broadcast(&sample_event(), Some("a"));

        // b and c receive it; a (the excluded sender) does not.
        assert!(rb.try_recv().is_ok());
        assert!(rc.try_recv().is_ok());
        assert!(ra.try_recv().is_err());
    }

    #[test]
    fn send_to_reaches_only_the_addressee() {
        let room = RoomState::new(None);
        for id in ["a", "b", "c"] {
            let (tx, _) = broadcast::channel(SSE_BUFFER_SIZE);
            room.participants.insert(id.into(), tx);
        }
        let mut ra = room.participants.get("a").unwrap().value().subscribe();
        let mut rb = room.participants.get("b").unwrap().value().subscribe();
        let mut rc = room.participants.get("c").unwrap().value().subscribe();

        room.send_to("b", &sample_event());

        assert!(rb.try_recv().is_ok());
        assert!(ra.try_recv().is_err());
        assert!(rc.try_recv().is_err());

        // An address that has left the room is simply dropped.
        room.send_to("gone", &sample_event());
        assert!(ra.try_recv().is_err());
    }

    #[test]
    fn is_participant_checks_membership() {
        let room = RoomState::new(None);
        assert!(!room.is_participant("x"));
        let (tx, _) = broadcast::channel(SSE_BUFFER_SIZE);
        room.participants.insert("x".into(), tx);
        assert!(room.is_participant("x"));
        assert!(!room.is_participant("y"));
    }
}
