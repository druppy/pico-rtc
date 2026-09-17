use serde::{Deserialize, Serialize};

/// A signaling message sent client → server via POST /api/room/:id/signal
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SignalMessage {
    #[serde(rename = "offer")]
    Offer { sdp: String },
    #[serde(rename = "answer")]
    Answer { sdp: String },
    #[serde(rename = "ice-candidate")]
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    Renegotiate,
}

/// A chat message (shared between server storage and client display)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub room: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub text: String,
    pub timestamp_ms: u64,
}

/// A chat message sent client → server via POST /api/room/:id/chat
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendRequest {
    pub text: String,
    pub sender_name: Option<String>,
}

/// Events pushed from server → client via SSE
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum SseEvent {
    #[serde(rename = "peer-joined")]
    PeerJoined {
        peer_id: String,
        peer_name: Option<String>,
    },
    #[serde(rename = "peer-left")]
    PeerLeft { peer_id: String },
    #[serde(rename = "offer")]
    Offer { from: String, sdp: String },
    #[serde(rename = "answer")]
    Answer { from: String, sdp: String },
    #[serde(rename = "ice-candidate")]
    IceCandidate {
        from: String,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    #[serde(rename = "chat-message")]
    ChatMessage {
        from: String,
        sender_name: Option<String>,
        text: String,
        timestamp_ms: u64,
    },
    #[serde(rename = "room-full")]
    RoomFull,
    #[serde(rename = "error")]
    Error { message: String },
}

/// Response from GET /api/turn-credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCredentials {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
    pub ttl_secs: u64,
}

/// Response from POST /api/room/:id/join
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum JoinResponse {
    #[serde(rename = "need-password")]
    NeedPassword {},
    #[serde(rename = "password-required")]
    PasswordRequired {},
    #[serde(rename = "ok")]
    Ok {
        self_id: String,
        peers: Vec<String>,
        /// Recent chat history (last 50 messages)
        chat: Vec<ChatMessage>,
    },
    #[serde(rename = "full")]
    Full {},
}

/// Request body for POST /api/room/:id/join
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    /// Existing password (for joining a room that already has one)
    pub password: Option<String>,
    /// First-time password claim (for creating a room)
    pub claim_password: Option<String>,
    /// Persistent session id (UUID from sessionStorage)
    pub session_id: String,
    /// Optional human-readable display name
    pub display_name: Option<String>,
    /// Optional stable user id (persisted in cookie for future auth)
    pub user_id: Option<String>,
}
