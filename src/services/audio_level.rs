//! Loudness per peer, so the UI knows who is talking.
//!
//! The room has no server-side knowledge of speech — media never touches it — so
//! the level is measured here, from the same remote [`MediaStream`]s the
//! `<video>` elements play. Feeding an [`AnalyserNode`] from a stream does not
//! take ownership of it: the element keeps rendering and playing, this only taps
//! the audio.

use std::collections::HashMap;

use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    AnalyserNode, AudioContext, AudioContextState, MediaStream, MediaStreamAudioSourceNode, console,
};

/// Window analysed per sample. Small enough to catch the start of a word, large
/// enough that a plosive is not mistaken for speech.
const FFT_SIZE: u32 = 1024;

/// RMS (0.0–1.0) at which a peer counts as making noise at all. Below this the
/// stage has no owner, and the UI falls back to a stable choice of its own.
const SPEECH_FLOOR: f32 = 0.02;

/// Smoothing applied to each reading. The raw RMS of speech is spiky; without
/// this the stage would follow individual syllables.
const SMOOTHING: f32 = 0.3;

/// How much louder a challenger must be to take the stage from whoever holds it.
/// Without this hysteresis, two people of similar volume would hand the stage
/// back and forth on every sample.
const SWITCH_RATIO: f32 = 1.6;

/// Gives up on a context that will not start: a rejected `resume()` is not
/// retried forever, but one attempt is not enough either (the gesture that
/// unlocks autoplay can land a moment after the first sample).
const MAX_RESUME_TRIES: u8 = 5;

/// Tracks who is speaking. Call [`LevelMeter::watch`] for every peer's stream,
/// then [`LevelMeter::sample`] on a timer.
///
/// Levels live here rather than in a signal: they are polled at a fixed cadence
/// and only the winner matters, so pushing every reading through the reactive
/// graph would cause renders that nothing reads.
pub struct LevelMeter {
    ctx: AudioContext,
    /// Peer id → its source and analyser. The source is held only to keep it
    /// alive: a node with no other reference can be collected, which would
    /// silently disconnect the analyser feeding it.
    inputs: HashMap<String, (MediaStreamAudioSourceNode, AnalyserNode)>,
    /// Smoothed level per watched peer.
    levels: HashMap<String, f32>,
    /// Who last held the stage, so [`SWITCH_RATIO`] has something to compare to.
    holder: Option<String>,
    /// Reused scratch buffer: one sample per peer per tick, and allocating a
    /// kilobyte per peer to throw away is pure waste.
    buf: Vec<u8>,
    resume_tries: u8,
}

impl LevelMeter {
    /// `None` when the browser has no `AudioContext`, in which case the room
    /// works and the stage simply has no owner.
    pub fn new() -> Option<Self> {
        let ctx = AudioContext::new().ok()?;
        Some(Self {
            ctx,
            inputs: HashMap::new(),
            levels: HashMap::new(),
            holder: None,
            buf: vec![0; FFT_SIZE as usize],
            resume_tries: 0,
        })
    }

    /// Starts following a peer's stream. Idempotent, so it can be called every
    /// time a stream is (re)attached.
    pub fn watch(&mut self, peer: &str, stream: &MediaStream) {
        if self.inputs.contains_key(peer) {
            return;
        }
        let analyser = match self.ctx.create_analyser() {
            Ok(analyser) => analyser,
            Err(e) => return console::warn_1(&format!("no analyser for {peer}: {e:?}").into()),
        };
        analyser.set_fft_size(FFT_SIZE);
        // Deliberately not connected to `destination`: the `<video>` element
        // already plays this stream, and a second path would double it.
        let source = match self.ctx.create_media_stream_source(stream) {
            Ok(source) => source,
            Err(e) => {
                return console::warn_1(&format!("no level meter for {peer}: {e:?}").into());
            }
        };
        if let Err(e) = source.connect_with_audio_node(&analyser) {
            return console::warn_1(&format!("level meter for {peer}: {e:?}").into());
        }
        self.inputs.insert(peer.to_string(), (source, analyser));
        self.levels.insert(peer.to_string(), 0.0);
    }

    /// Stops following a peer, and drops its claim on the stage.
    pub fn forget(&mut self, peer: &str) {
        if let Some((source, analyser)) = self.inputs.remove(peer) {
            let _ = source.disconnect();
            let _ = analyser.disconnect();
        }
        self.levels.remove(peer);
        if self.holder.as_deref() == Some(peer) {
            self.holder = None;
        }
    }

    /// Reads every peer once and returns who should be shown large, or `None`
    /// when nobody is speaking.
    pub fn sample(&mut self) -> Option<String> {
        self.ensure_running();

        for peer in self.inputs.keys().cloned().collect::<Vec<_>>() {
            // Cloned out of the map so that reading it does not overlap the
            // mutable borrow of `buf` and `levels` below.
            let analyser = self.inputs[&peer].1.clone();
            let level = rms(&analyser, &mut self.buf);
            let previous = self.levels.get(&peer).copied().unwrap_or(0.0);
            self.levels
                .insert(peer, SMOOTHING * level + (1.0 - SMOOTHING) * previous);
        }

        let Some((best, best_level)) = self.loudest() else {
            self.holder = None;
            return None;
        };
        // The incumbent keeps the stage unless the challenger is clearly louder.
        if let Some(held) = self.holder.clone() {
            let holds = held != best
                && self
                    .levels
                    .get(&held)
                    .is_some_and(|held_level| best_level < held_level * SWITCH_RATIO);
            if holds {
                return Some(held);
            }
        }
        self.holder = Some(best.clone());
        Some(best)
    }

    /// The peer above [`SPEECH_FLOOR`] with the highest smoothed level.
    fn loudest(&self) -> Option<(String, f32)> {
        self.levels
            .iter()
            .filter(|(_, level)| **level > SPEECH_FLOOR)
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(peer, level)| (peer.clone(), *level))
    }

    /// Browsers park an `AudioContext` until the page has had a user gesture. A
    /// parked context reads as silence, which is indistinguishable from a quiet
    /// room, so ask for it to start — a bounded number of times.
    fn ensure_running(&mut self) {
        let parked = self.ctx.state() == AudioContextState::Suspended;
        if !parked || self.resume_tries >= MAX_RESUME_TRIES {
            return;
        }
        self.resume_tries += 1;
        let resume = match self.ctx.resume() {
            Ok(resume) => resume,
            Err(e) => {
                return console::warn_1(&format!("cannot resume audio context: {e:?}").into());
            }
        };
        spawn_local(async move {
            if let Err(e) = JsFuture::from(resume).await {
                console::warn_1(&format!("audio context still suspended: {e:?}").into());
            }
        });
    }
}

/// RMS of the last window, normalised to 0.0–1.0. The byte buffer is unsigned
/// around 128, so the mean is subtracted before squaring. `buf` is the caller's
/// scratch space, sized to the analyser's FFT window.
fn rms(analyser: &AnalyserNode, buf: &mut [u8]) -> f32 {
    analyser.get_byte_time_domain_data(buf);
    let sum: f64 = buf
        .iter()
        .map(|&sample| {
            let offset = f64::from(sample) - 128.0;
            offset * offset
        })
        .sum();
    let root = (sum / buf.len() as f64).sqrt();
    (root / 128.0) as f32
}
