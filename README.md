# WebRTC Room

Peer-to-peer video chat with room-based access. Rust fullstack (Axum + Leptos), SSE signaling, no WebSockets.

## Quick Start (Dev)

### Prerequisites

- Rust 1.85+ (edition 2024)
- [Trunk](https://trunkrs.dev/) — `cargo install trunk`
- [wasm32 target](https://rustwasm.github.io) — `rustup target add wasm32-unknown-unknown`
- [coturn](https://github.com/coturn/coturn) — `apt install coturn` (or Docker)

### Run Server

```bash
# Terminal 1: Axum server (SSE + signaling + TURN credentials)
cargo run --bin server --features ssr

# Terminal 2: Trunk dev server (Leptos client with live reload + proxy)
trunk serve
```

Open http://localhost:8080, navigate to a room like `/room/my-test`, set a password, share the link.

### TURN (Optional for Dev)

For local testing on the same machine or same NAT, STUN alone is sufficient.
For testing across NATs:

```bash
# Edit turnserver.conf, then:
turnserver -c turnserver.conf
```

Or Docker:

```bash
docker run -d --network=host \
  -v $(pwd)/turnserver.conf:/etc/coturn/turnserver.conf \
  coturn/coturn
```

## Production Deployment

### Architecture

```
┌────────────┐     ┌──────────────────────┐     ┌──────────┐
│  Browser   │────►│  Axum (TLS via Caddy)│     │  coturn  │
│            │◄────│  - Leptos static     │     │  (TURN)  │
│  WebRTC ◄──┼─────┼──────────────────────┼─────┼────────► │
└────────────┘     └──────────────────────┘     └──────────┘
                   P2P media (or relay via TURN)
```

### Build

```bash
# Client bundle
cd webrtc-app
trunk build --release

# Server binary
cargo build --release --bin server --features ssr
```

### Deploy Checklist

1. **TLS is mandatory** — WebRTC requires a secure context (`https://`)
2. Place Caddy or Nginx in front of the Axum binary for TLS termination
3. Run coturn on the same host (or a nearby one) to minimize relay latency
4. Set environment variables:
   - `TURN_HOST` — public hostname of TURN server
   - `TURN_SECRET` — shared secret for ephemeral HMAC credentials
   - `TURN_PORT` — usually `3478`
5. Open UDP ports `3478` (TURN/STUN) and `49152-65535` (relay range) on firewall

### Docker Compose (Full Stack)

```yaml
services:
  app:
    build: .
    ports:
      - "8080:8080"
    environment:
      - TURN_HOST=turn.example.com
      - TURN_SECRET=change-me-to-random-hex
      - TURN_PORT=3478
    restart: unless-stopped

  coturn:
    image: coturn/coturn
    network_mode: host
    volumes:
      - ./turnserver.conf:/etc/coturn/turnserver.conf
    restart: unless-stopped

  caddy:
    image: caddy:2
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile
      - caddy_data:/data
    restart: unless-stopped

volumes:
  caddy_data:
```

### Caddyfile

```
yourdomain.com {
    reverse_proxy localhost:8080
}
```

### coturn Config (turnserver.conf)

```conf
listening-port=3478
realm=turn.yourdomain.com

# Auth — use use-auth-secret for ephemeral HMAC
use-auth-secret
static-auth-secret=change-me-to-random-hex

# Relay range
min-port=49152
max-port=65535

# IPs
external-ip=YOUR_PUBLIC_IP

# Logging
log-file=/var/log/turnserver.log
verbose
```

## Project Structure

```
webrtc-app/
├── .rules                  # Technical constraints & decisions
├── README.md               # This file
├── Cargo.toml              # Workspace root
├── Trunk.toml              # Trunk (client bundler) config
├── rust-toolchain.toml
├── turnserver.conf         # coturn config template
├── Caddyfile               # Reverse proxy template
├── Dockerfile
├── docker-compose.yml
├── src/
│   ├── main.rs             # Entry (server mode)
│   ├── app.rs              # Leptos <App>
│   ├── lib.rs              # Shared types
│   ├── pages/
│   │   ├── mod.rs
│   │   ├── home.rs         # Landing: enter room name
│   │   └── room.rs         # Video call UI
│   ├── server/
│   │   ├── mod.rs          # Axum router assembly
│   │   ├── rooms.rs        # Room state, participant tracking
│   │   ├── signaling.rs    # SSE stream + POST signal endpoints
│   │   ├── turn.rs         # Ephemeral TURN credential generation
│   │   └── auth.rs         # Room password check (trait for future auth)
│   ├── services/
│   │   ├── mod.rs
│   │   ├── signaling.rs    # Client: SSE consumer + signal sender
│   │   └── webrtc.rs       # Client: RTCPeerConnection wrapper
│   └── components/
│       ├── mod.rs
│       ├── video_tile.rs   # Single <video> element
│       └── controls.rs     # Mute/screen-share buttons
├── style/
│   └── main.css            # Minimal custom styles on top of PicoCSS
└── index.html              # Trunk entry point
```

## API Reference

All endpoints under `/api/`.

### `POST /room/:id/join`
Join (or claim) a room.

```json
// Request
{
  "password": "optional-existing-pw",
  "claim_password": "set-on-first-visit",
  "session_id": "uuid-from-cookie",
  "display_name": "Alice",
  "user_id": "cookie-stable-id"
}

// Response
{"status": "ok", "self_id": "...", "peers": ["..."] , "chat": [...]}
{"status": "need-password"}        // room is new, you must set one
{"status": "password-required"}   // wrong password
{"status": "full"}                // room at capacity
```

### `POST /room/:id/signal?session_id=...`
Relay WebRTC signaling (offer/answer/ICE) to other peers.

```json
{"type": "offer", "sdp": "..."}
{"type": "answer", "sdp": "..."}
{"type": "ice-candidate", "candidate": "...", "sdp_mid": "...", "sdp_mline_index": 0}
```

### `GET /room/:id/events?session_id=...`
SSE stream. Event data (JSON, no `event:` field — parse `data:`):

```json
{"event": "peer-joined", "peer_id": "...", "peer_name": "Alice"}
{"event": "peer-left", "peer_id": "..."}
{"event": "offer", "from": "...", "sdp": "..."}
{"event": "answer", "from": "...", "sdp": "..."}
{"event": "ice-candidate", "from": "...", "candidate": "...", "sdp_mid": "...", "sdp_mline_index": 0}
{"event": "chat-message", "from": "...", "sender_name": "...", "text": "...", "timestamp_ms": 123}
```

### `POST /room/:id/chat?session_id=...`
Send a chat message (persisted, broadcast via SSE).

```json
{"text": "hello", "sender_name": "Alice"}
```

### `GET /room/:id/chat/history`
Last 100 chat messages for the room.

### `GET /turn-credentials`
Ephemeral TURN/STUN credentials (HMAC-SHA1 per TURN REST API spec, 1hr TTL).

```json
{"urls": ["turn:host:3478", ...], "username": "<expiry>:<id>", "credential": "<hmac>", "ttl_secs": 3600}
```

### Identity Cookies (browser-side)

| Cookie | Purpose | Lifetime |
|--------|---------|----------|
| `wr_uid` | Stable UUID v4 per browser (future auth `sub`) | 1 year |
| `wr_name` | Display name | 1 year |

## Future Roadmap

- [ ] SFU (mediasoup or custom) for >4 participants
- [ ] Data channels (collaborative drawing, reactions)
- [ ] JWT/OIDC auth replacing room passwords
- [ ] Recording (server-side or client-side MediaRecorder)
- [ ] Chat sidebar (over existing DataChannel)
- [ ] Screen share via simulcast layers
- [ ] Mobile PWA (web manifest + service worker)

## License

MIT
