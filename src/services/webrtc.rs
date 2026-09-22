//! The WebRTC mesh: one `RTCPeerConnection` per other participant.
//!
//! The transport (SSE down, `POST /signal` up) lives in [`crate::services::signaling`];
//! this module turns the room's events into peer connections. Hand it every
//! [`SseEvent`] with [`Mesh::on_event`] and it takes the media ones for itself.
//!
//! # Who offers to whom
//!
//! The peer whose id sorts *first* creates the offer. Both ends compute the same
//! answer from the same two ids, so exactly one offer is made per pair and no glare
//! arises in any join order: the designated answerer never offers, and an offer that
//! reaches a peer mid-negotiation is dropped rather than applied.
//!
//! Signals carry a `to` because the transport is a room-wide SSE stream but media is
//! not: the alternative is for every peer to receive every offer, answer and
//! candidate, with no way to tell a candidate meant for a third party from one meant
//! for it — both come from the same sender, whose one connection per peer it then
//! pollutes with candidates that can never validate.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use js_sys::Reflect;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    HtmlMediaElement, HtmlVideoElement, MediaStream, MediaStreamTrack, RtcConfiguration,
    RtcIceCandidateInit, RtcIceConnectionState, RtcIceServer, RtcPeerConnection,
    RtcPeerConnectionIceEvent, RtcPeerConnectionState, RtcSdpType, RtcSessionDescriptionInit,
    RtcSignalingState, RtcTrackEvent, console,
};

use crate::services::audio_level::LevelMeter;
use crate::services::signaling::send_signal;
use crate::types::{SignalMessage, SseEvent};

/// Used when the room's TURN endpoint cannot be reached, so that a dev server
/// without coturn still gathers server-reflexive candidates. Peers on the same
/// machine connect over host candidates either way.
const PUBLIC_STUN: &str = "stun:stun.l.google.com:19302";

/// Element id of the local tile (`<VideoTile id="local">` in `RoomPage`).
const LOCAL_TILE_ID: &str = "local";

/// ICE servers for every peer connection: whatever the server hands out (the
/// configured coturn), falling back to public STUN when that request fails.
pub async fn ice_configuration() -> RtcConfiguration {
    let servers = js_sys::Array::new();

    match fetch_turn_config().await {
        Ok(creds) if !creds.urls.is_empty() => {
            let urls = js_sys::Array::new();
            for url in &creds.urls {
                urls.push(&JsValue::from_str(url));
            }
            let server = RtcIceServer::new();
            server.set_urls(&JsValue::from(urls));
            if !creds.username.is_empty() {
                server.set_username(&creds.username);
                server.set_credential(&creds.credential);
            }
            servers.push(&JsValue::from(server));
        }
        Ok(_) => console::warn_1(&"the server offered no ICE servers".into()),
        Err(e) => console::warn_1(&format!("TURN credentials unavailable ({e})").into()),
    }

    if servers.length() == 0 {
        let server = RtcIceServer::new();
        server.set_urls_str(PUBLIC_STUN);
        servers.push(&JsValue::from(server));
    }

    let cfg = RtcConfiguration::new();
    cfg.set_ice_servers(&JsValue::from(servers));
    cfg
}

/// Initialize local media (camera + mic).
///
/// `Err` means the user denied access (or there is no such device); the caller
/// joins the room receive-only instead.
pub async fn get_local_media() -> Result<MediaStream, String> {
    let window = web_sys::window().ok_or("no window")?;
    let media_devices = window
        .navigator()
        .media_devices()
        .map_err(|e| format!("{e:?}"))?;

    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    constraints.set_video(&JsValue::TRUE);

    let promise = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| format!("getUserMedia: {e:?}"))?;

    let stream = JsFuture::from(promise)
        .await
        .map_err(|e| format!("getUserMedia rejected: {e:?}"))?;

    stream
        .dyn_into::<MediaStream>()
        .map_err(|e| format!("{e:?}"))
}

/// Fetch TURN credentials from the server.
pub async fn fetch_turn_config() -> Result<crate::types::TurnCredentials, String> {
    let resp = gloo_net::http::Request::get("/api/turn-credentials")
        .send()
        .await
        .map_err(|e| format!("TURN fetch failed: {e}"))?;

    if resp.ok() {
        resp.json::<crate::types::TurnCredentials>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

/// One peer connection, plus everything that has to be torn down with it.
struct PeerHandle {
    pc: RtcPeerConnection,
    /// Candidates that arrived before there was a remote description to attach
    /// them to. Shared with the task that applies that description, so it can
    /// drain what the event handler queued in the meantime.
    pending_ice: Rc<RefCell<VecDeque<RtcIceCandidateInit>>>,
    /// A remote description has been applied, so candidates can be added at once.
    remote_set: bool,
    /// We have already offered here. One initial offer per pair; renegotiation (the
    /// screen-share toggle) is not wired up yet.
    negotiated: bool,
    /// The stream seen in `ontrack`, and whether it has reached the DOM yet.
    remote: Rc<RefCell<Option<MediaStream>>>,
    attached: bool,
    /// Handlers the PC calls back into. Detached before they are
    /// dropped, or JS could reach a freed Rust closure.
    _on_ice: Closure<dyn FnMut(RtcPeerConnectionIceEvent)>,
    _on_track: Closure<dyn FnMut(RtcTrackEvent)>,
    _on_state: Closure<dyn FnMut(JsValue)>,
}

/// The mesh for one room visit.
///
/// It holds no Leptos state on purpose: the UI learns about peers from the presence
/// list, and media reaches the DOM directly, a `MediaStream` being neither `Send`
/// nor `Sync` and so unwelcome in a signal. Built once the join succeeds, dropped in
/// the component's `on_cleanup`.
pub struct Mesh {
    room: String,
    self_id: String,
    cfg: RtcConfiguration,
    local: Option<MediaStream>,
    local_attached: bool,
    peers: HashMap<String, PeerHandle>,
    /// Shared with every `ontrack` handler, which is where a peer's stream first
    /// becomes visible. `None` only where `AudioContext` is unavailable, in which
    /// case the stage simply has no owner.
    meter: Option<Rc<RefCell<LevelMeter>>>,
}

impl Mesh {
    /// `local` is the camera/mic stream, or `None` when the user declined.
    /// Fetches the ICE configuration, so the caller must still be able to buffer
    /// room events while that is in flight.
    pub async fn new(room: &str, self_id: &str, local: Option<MediaStream>) -> Self {
        Self {
            room: room.to_string(),
            self_id: self_id.to_string(),
            cfg: ice_configuration().await,
            local,
            local_attached: false,
            peers: HashMap::new(),
            meter: LevelMeter::new().map(|meter| Rc::new(RefCell::new(meter))),
        }
    }

    /// Applies one room event. Whatever the mesh ignores is presence or chat.
    pub fn on_event(&mut self, ev: &SseEvent) {
        match ev {
            // The snapshot doubles as the repair path: a peer we never managed to
            // reach while its own stream was opening is still listed here.
            SseEvent::Resync { peers, .. } => {
                for peer in peers {
                    self.ensure_peer(&peer.peer_id);
                    self.maybe_offer(&peer.peer_id);
                }
            }
            SseEvent::PeerJoined { peer_id, .. } => {
                self.ensure_peer(peer_id);
                self.maybe_offer(peer_id);
            }
            SseEvent::PeerLeft { peer_id } => self.close_peer(peer_id),
            SseEvent::Offer { from, sdp } => self.apply_offer(from, sdp),
            SseEvent::Answer { from, sdp } => self.apply_answer(from, sdp),
            SseEvent::IceCandidate {
                from,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => self.on_ice(from, candidate, sdp_mid.as_deref(), *sdp_mline_index),
            _ => {}
        }
        // Tiles are created by the presence list, which can lag the media by a
        // render pass, so every event is a chance to retry what did not attach.
        self.attach_pending();
    }

    /// Tears every connection down and releases the camera.
    pub fn close_all(&mut self) {
        for id in self.peers.keys().cloned().collect::<Vec<_>>() {
            self.close_peer(&id);
        }
        if let Some(stream) = self.local.take() {
            stop_tracks(&stream);
        }
    }

    /// This side owns the offer for a pair when its own id sorts first.
    fn we_initiate(&self, peer: &str) -> bool {
        self.self_id.as_str() < peer
    }

    fn ensure_peer(&mut self, peer: &str) {
        if peer == self.self_id || self.peers.contains_key(peer) {
            return;
        }
        match self.create_peer(peer) {
            Ok(handle) => {
                self.peers.insert(peer.to_string(), handle);
            }
            Err(e) => console::warn_1(&format!("no peer connection with {peer}: {e}").into()),
        }
    }

    fn create_peer(&self, peer: &str) -> Result<PeerHandle, String> {
        let pc =
            RtcPeerConnection::new_with_configuration(&self.cfg).map_err(|e| format!("{e:?}"))?;

        match &self.local {
            Some(stream) => {
                for track in stream.get_tracks().iter() {
                    if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                        pc.add_track_0(&track, stream);
                    }
                }
            }
            None => {
                // Receive-only. A transceiver with no track yields a `recvonly`
                // m-line, so the pair still negotiates one-way media; offering no
                // media at all would negotiate nothing.
                pc.add_transceiver_with_str("audio");
                pc.add_transceiver_with_str("video");
            }
        }

        let pending_ice = Rc::new(RefCell::new(VecDeque::new()));
        let remote: Rc<RefCell<Option<MediaStream>>> = Rc::new(RefCell::new(None));

        let on_ice = {
            let room = self.room.clone();
            let sid = self.self_id.clone();
            // Every candidate this connection gathers belongs to this one peer.
            let to = peer.to_string();
            Closure::wrap(Box::new(move |ev: RtcPeerConnectionIceEvent| {
                // A null candidate marks the end of gathering.
                let Some(candidate) = ev.candidate() else {
                    return;
                };
                let signal = SignalMessage::IceCandidate {
                    to: to.clone(),
                    candidate: candidate.candidate(),
                    sdp_mid: candidate.sdp_mid(),
                    sdp_mline_index: candidate.sdp_m_line_index(),
                };
                let (room, sid) = (room.clone(), sid.clone());
                spawn_local(async move {
                    if let Err(e) = send_signal(&room, &sid, &signal).await {
                        console::warn_1(&format!("ice candidate not relayed: {e}").into());
                    }
                });
            }) as Box<dyn FnMut(RtcPeerConnectionIceEvent)>)
        };

        let on_track = {
            let remote = Rc::clone(&remote);
            let meter = self.meter.clone();
            let tile = peer.to_string();
            Closure::wrap(Box::new(move |ev: RtcTrackEvent| {
                // The sender's tracks travel together in its first stream.
                let Ok(stream) = ev.streams().get(0).dyn_into::<MediaStream>() else {
                    return;
                };
                *remote.borrow_mut() = Some(stream.clone());
                // Start measuring this peer as soon as there is audio to measure,
                // whether or not the tile is in the DOM yet.
                if let Some(meter) = &meter {
                    meter.borrow_mut().watch(&tile, &stream);
                }
                if let Some(video) = video_element(&tile) {
                    attach_stream(&video, Some(&stream));
                }
            }) as Box<dyn FnMut(RtcTrackEvent)>)
        };

        pc.set_onicecandidate(Some(on_ice.as_ref().unchecked_ref()));
        pc.set_ontrack(Some(on_track.as_ref().unchecked_ref()));

        // A pair that never comes up is otherwise silent: the tile simply stays
        // empty. Only the states that mean "this is not going to work" are worth
        // hearing about; checking/connected chatter would drown out the rest.
        let on_state = {
            let short: String = peer.chars().take(8).collect();
            let probe = pc.clone();
            let closure = Closure::wrap(Box::new(move |_: JsValue| {
                let state = probe.ice_connection_state();
                if matches!(
                    state,
                    RtcIceConnectionState::Failed | RtcIceConnectionState::Disconnected
                ) {
                    console::warn_1(
                        &format!(
                            "ice with {short} is {state:?} (signaling {:?})",
                            probe.signaling_state(),
                        )
                        .into(),
                    );
                }
            }) as Box<dyn FnMut(JsValue)>);
            pc.set_oniceconnectionstatechange(Some(closure.as_ref().unchecked_ref()));
            closure
        };

        Ok(PeerHandle {
            pc,
            pending_ice,
            remote_set: false,
            negotiated: false,
            remote,
            attached: false,
            _on_ice: on_ice,
            _on_track: on_track,
            _on_state: on_state,
        })
    }

    /// Creates the one offer this pair gets, if we are the side that owns it.
    fn maybe_offer(&mut self, peer: &str) {
        if !self.we_initiate(peer) {
            return;
        }
        let Some(handle) = self.peers.get_mut(peer) else {
            return;
        };
        // Only ever the first offer: after an SSE reconnect the `resync` lists
        // peers we are already talking to, and offering again would renegotiate
        // them. If the offer never arrives (a lost POST), the pair stays dark and
        // the log says so — there is no retransmit yet.
        if handle.negotiated
            || handle.pc.signaling_state() != RtcSignalingState::Stable
            || handle.pc.connection_state() != RtcPeerConnectionState::New
        {
            return;
        }
        handle.negotiated = true;

        let pc = handle.pc.clone();
        let room = self.room.clone();
        let sid = self.self_id.clone();
        let peer = peer.to_string();
        spawn_local(async move {
            let offer = match JsFuture::from(pc.create_offer()).await {
                Ok(offer) => offer,
                Err(e) => return warn(&format!("create_offer for {peer}"), &e),
            };
            let Some(sdp) = js_text(&offer, "sdp") else {
                return warn(&format!("create_offer for {peer} gave no sdp"), &offer);
            };
            let desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
            desc.set_sdp(&sdp);
            let what = format!("set_local_description for {peer} (offer)");
            if !set_description(pc.set_local_description(&desc), &what).await {
                return;
            }
            if let Err(e) = send_signal(
                &room,
                &sid,
                &SignalMessage::Offer {
                    to: peer.clone(),
                    sdp,
                },
            )
            .await
            {
                console::warn_1(&format!("offer for {peer} not relayed: {e}").into());
            }
        });
    }

    fn apply_offer(&mut self, from: &str, sdp: &str) {
        self.ensure_peer(from);
        let Some(handle) = self.peers.get(from) else {
            return;
        };
        // Belt and braces: signals are addressed now, so an offer should only reach
        // its answerer — but one sent before this peer's id was known to the sender
        // (or a replayed one) must not land on a connection already negotiating.
        if self.we_initiate(from) && handle.negotiated {
            console::warn_1(&format!("ignoring offer from {from}: ours to initiate").into());
            return;
        }
        // Anything but `stable` means we are already mid-negotiation with this peer.
        if handle.remote_set || handle.pc.signaling_state() != RtcSignalingState::Stable {
            console::warn_1(&format!("ignoring offer from {from}: not stable").into());
            return;
        }
        let pc = handle.pc.clone();
        let queue = Rc::clone(&handle.pending_ice);

        let Some(handle) = self.peers.get_mut(from) else {
            return;
        };
        handle.remote_set = true;

        let room = self.room.clone();
        let sid = self.self_id.clone();
        let peer = from.to_string();
        let desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        desc.set_sdp(sdp);

        spawn_local(async move {
            let what = format!("set_remote_description for {peer} (offer)");
            if !set_description(pc.set_remote_description(&desc), &what).await {
                return;
            }
            drain_ice(pc.clone(), Rc::clone(&queue), peer.clone()).await;

            let answer = match JsFuture::from(pc.create_answer()).await {
                Ok(answer) => answer,
                Err(e) => return warn(&format!("create_answer for {peer}"), &e),
            };
            let Some(sdp) = js_text(&answer, "sdp") else {
                return warn(&format!("create_answer for {peer} gave no sdp"), &answer);
            };
            let local = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
            local.set_sdp(&sdp);
            let what = format!("set_local_description for {peer} (answer)");
            if !set_description(pc.set_local_description(&local), &what).await {
                return;
            }
            if let Err(e) = send_signal(
                &room,
                &sid,
                &SignalMessage::Answer {
                    to: peer.clone(),
                    sdp,
                },
            )
            .await
            {
                console::warn_1(&format!("answer for {peer} not relayed: {e}").into());
            }
            drain_ice(pc, queue, peer).await;
        });
    }

    fn apply_answer(&mut self, from: &str, sdp: &str) {
        let Some(handle) = self.peers.get_mut(from) else {
            console::warn_1(&format!("answer from {from}, a peer we do not have").into());
            return;
        };
        // Only meaningful against the offer we sent. Anything else is a signal meant
        // for another peer, or one that arrived after we gave up on this pair.
        if handle.pc.signaling_state() != RtcSignalingState::HaveLocalOffer {
            console::warn_1(&format!("ignoring answer from {from}: no offer outstanding").into());
            return;
        }
        handle.remote_set = true;

        let pc = handle.pc.clone();
        let queue = Rc::clone(&handle.pending_ice);
        let peer = from.to_string();
        let desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        desc.set_sdp(sdp);

        spawn_local(async move {
            let what = format!("set_remote_description for {peer} (answer)");
            set_description(pc.set_remote_description(&desc), &what).await;
            drain_ice(pc, queue, peer).await;
        });
    }

    fn on_ice(
        &mut self,
        from: &str,
        candidate: &str,
        sdp_mid: Option<&str>,
        sdp_mline_index: Option<u16>,
    ) {
        // A candidate can overtake the signal that would have created this peer's
        // connection, so make the connection on demand and queue the candidate until
        // there is a remote description to attach it to.
        self.ensure_peer(from);
        self.maybe_offer(from);
        let Some(handle) = self.peers.get_mut(from) else {
            return;
        };
        let init = RtcIceCandidateInit::new(candidate);
        init.set_sdp_mid(sdp_mid);
        init.set_sdp_m_line_index(sdp_mline_index);
        handle.pending_ice.borrow_mut().push_back(init);
        if handle.remote_set {
            let pc = handle.pc.clone();
            let queue = Rc::clone(&handle.pending_ice);
            let peer = from.to_string();
            spawn_local(drain_ice(pc, queue, peer));
        }
    }

    fn close_peer(&mut self, peer: &str) {
        let Some(handle) = self.peers.remove(peer) else {
            return;
        };
        // Stop listening for their volume, and clear any claim they had on the
        // stage, before the connection that fed it goes away.
        if let Some(meter) = &self.meter {
            meter.borrow_mut().forget(peer);
        }
        // Detach the handlers, then close the PC; the closures are freed when this
        // scope ends, by which point JS can no longer reach them.
        handle.pc.set_onicecandidate(None);
        handle.pc.set_ontrack(None);
        handle.pc.set_oniceconnectionstatechange(None);
        handle.pc.close();
        handle.pending_ice.borrow_mut().clear();
        *handle.remote.borrow_mut() = None;
        if let Some(video) = video_element(peer) {
            attach_stream(&video, None);
        }
    }

    /// The peer to show large right now, or `None` when nobody is speaking.
    ///
    /// Polled rather than pushed: levels are only interesting as they cross each
    /// other, and the mesh deliberately holds no reactive state.
    pub fn dominant_speaker(&mut self) -> Option<String> {
        self.meter
            .as_ref()
            .and_then(|meter| meter.borrow_mut().sample())
    }

    /// Hands every stream we hold to the `<video>` element whose id names its
    /// participant. Elements not mounted yet are picked up on the next event, since
    /// nothing in here forces a render. Also called once by `RoomPage` right after
    /// the mesh is built, to put the local preview on screen.
    pub fn attach_pending(&mut self) {
        let attached = self.local_attached || {
            let stream = self.local.as_ref();
            attach_to(stream, LOCAL_TILE_ID)
        };
        self.local_attached = attached;

        for (peer, handle) in &mut self.peers {
            if handle.attached {
                continue;
            }
            let stream = handle.remote.borrow().clone();
            handle.attached = attach_to(stream.as_ref(), peer);
        }
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// Sets `srcObject` if the tile exists yet. Returns whether the stream is in the
/// DOM, so callers can stop trying once it is.
fn attach_to(stream: Option<&MediaStream>, id: &str) -> bool {
    let Some(stream) = stream else {
        return false;
    };
    match video_element(id) {
        Some(video) => {
            attach_stream(&video, Some(stream));
            true
        }
        None => false,
    }
}

/// Awaits a `setLocalDescription`/`setRemoteDescription` promise, logging which
/// peer and which direction failed.
async fn set_description(promise: js_sys::Promise, what: &str) -> bool {
    match JsFuture::from(promise).await {
        Ok(_) => true,
        Err(e) => {
            warn(what, &e);
            false
        }
    }
}

/// Adds queued candidates, oldest first, until the queue runs dry. Two drains can
/// be in flight at once (one started by an arriving candidate, one by a description
/// landing); popping as we go still adds each candidate exactly once.
async fn drain_ice(
    pc: RtcPeerConnection,
    queue: Rc<RefCell<VecDeque<RtcIceCandidateInit>>>,
    peer: String,
) {
    loop {
        let Some(next) = queue.borrow_mut().pop_front() else {
            return;
        };
        let add = pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&next));
        if let Err(e) = JsFuture::from(add).await {
            warn(&format!("add_ice_candidate for {peer}"), &e);
        }
    }
}

/// Reads a string property off a JS object. The SDP is taken this way rather than by
/// casting: `createOffer`/`createAnswer` may resolve with a plain `{ type, sdp }`
/// dictionary, which need not be an `RTCSessionDescription`.
fn js_text(obj: &JsValue, key: &str) -> Option<String> {
    Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
}

/// Logs a JS-side failure together with the step that produced it.
fn warn(what: &str, detail: &JsValue) {
    console::warn_1(&format!("{what}: {detail:?}").into());
}

fn stop_tracks(stream: &MediaStream) {
    for track in stream.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
            track.stop();
        }
    }
}

/// The tile for a participant: `<video id="local">`, or `<video id="{peer_id}">`.
fn video_element(id: &str) -> Option<HtmlVideoElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<HtmlVideoElement>()
        .ok()
}

/// Sets `srcObject`. The setter is only bound on `HtmlMediaElement`, which every
/// video element is an instance of; `HtmlVideoElement` has no binding for it, and
/// this web-sys version offers no `Deref` between the two.
fn attach_stream(video: &HtmlVideoElement, stream: Option<&MediaStream>) {
    if let Some(media) = video.dyn_ref::<HtmlMediaElement>() {
        media.set_src_object(stream);
    }
}
