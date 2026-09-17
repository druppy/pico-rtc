use std::collections::VecDeque;
use std::sync::Mutex;

pub use crate::types::ChatMessage;

/// Storage abstraction — swap for SQLite/Postgres in future.
/// Currently in-memory (per-room ring buffer).
pub trait ChatStore: Send + Sync {
    fn append(&self, msg: &ChatMessage);
    /// Return the last `limit` messages for a room (oldest first).
    fn history(&self, room: &str, limit: usize) -> Vec<ChatMessage>;
}

/// In-memory implementation: ring buffer per room, capped at 200 messages.
pub struct InMemoryChat {
    rooms: dashmap::DashMap<String, Mutex<VecDeque<ChatMessage>>>,
}

const MAX_MESSAGES_PER_ROOM: usize = 200;

impl InMemoryChat {
    pub fn new() -> Self {
        Self {
            rooms: dashmap::DashMap::new(),
        }
    }
}

impl ChatStore for InMemoryChat {
    fn append(&self, msg: &ChatMessage) {
        let entry = self.rooms.entry(msg.room.clone()).or_default();
        let mut buf = entry.lock().unwrap();
        if buf.len() >= MAX_MESSAGES_PER_ROOM {
            buf.pop_front();
        }
        buf.push_back(msg.clone());
    }

    fn history(&self, room: &str, limit: usize) -> Vec<ChatMessage> {
        self.rooms
            .get(room)
            .map(|entry| {
                let buf = entry.lock().unwrap();
                let start = buf.len().saturating_sub(limit);
                buf.iter().skip(start).cloned().collect()
            })
            .unwrap_or_default()
    }
}
