//! Rollback netplay in the browser (feature `net`).
//!
//! One page tick is one [`RollbackSession::advance`]: add the local pad, then
//! run the returned requests in order against the live [`Game`] and a ring of
//! [`GameState`]s allocated once when the session is created:
//!
//! * `SaveState` copies the game into the ring (no allocation) and, when the
//!   session asks, reports the checksum of that saved state;
//! * `LoadState` restores a ring entry;
//! * `Advance` steps both pads. A `replay` frame skips the audio path
//!   entirely (its APU register writes are cleared unheard), so its samples
//!   never reach the queue `take_audio_f32` drains into the AudioWorklet.
//!
//! The page presents once after the tick (JS calls `render_frame` after
//! `net_step` returns), never between requests. `WaitRecommendation` turns the
//! next few ticks into network-only polls.

use z2_core::game::Game;
use z2_core::state::GameState;
use z2_net::{
    hash_state, RollbackError, RollbackEvent, RollbackRequest, RollbackSession, RollbackState,
};

use crate::netsim::SimTransport;
use crate::{json_escape, WebEmu};

/// Frames between entries of the confirmed-hash log (`net_confirmed_hashes`).
pub const HASH_LOG_EVERY: u32 = 15;
/// Entries kept in the confirmed-hash log.
pub const HASH_LOG_LEN: usize = 128;

/// Checksum of a saved state that rollback peers compare: FNV-1a 64 over the
/// versioned [`GameState::to_bytes`] image. The desktop frontend uses the same
/// definition, so a browser and a desktop peer can play one rollback session.
/// Only computed on checksum frames (every `check_interval` frames).
#[must_use]
pub fn rollback_checksum(state: &GameState) -> u64 {
    hash_state(&[&state.to_bytes()])
}

/// A live rollback session and the caller-owned state it needs.
pub(crate) struct RollbackSlot {
    pub(crate) session: RollbackSession<SimTransport>,
    /// Saved states, `max_prediction + 2` of them, keyed by `frame % len`.
    ring: Vec<GameState>,
    ring_frames: Vec<Option<u32>>,
    /// Ticks still to spend polling only (from `WaitRecommendation`).
    pub(crate) skip: u8,
    pub(crate) max_prediction: u8,
    pub(crate) interrupted: bool,
    /// `(frame, net_state_hash)` of every `HASH_LOG_EVERY`-th saved state.
    hash_log: Vec<(u32, u64)>,
    /// Last newly simulated (not replayed) frame and this peer's pad on it.
    pub(crate) last_local: Option<(u32, u8)>,
    /// Ticks spent polling only because of `WaitRecommendation`.
    pub(crate) skipped_ticks: u64,
}

impl RollbackSlot {
    pub(crate) fn new(session: RollbackSession<SimTransport>, max_prediction: u8) -> Self {
        let n = usize::from(max_prediction) + 2;
        Self {
            session,
            ring: (0..n).map(|_| GameState::new()).collect(),
            ring_frames: vec![None; n],
            skip: 0,
            max_prediction,
            interrupted: false,
            hash_log: vec![(u32::MAX, 0); HASH_LOG_LEN],
            last_local: None,
            skipped_ticks: 0,
        }
    }

    /// Hashes of saved states that no rollback can change any more, oldest
    /// first, as JSON `[[frame,"hex"],...]`.
    pub(crate) fn confirmed_hashes_json(&self) -> String {
        let fin = self.session.final_frame();
        let mut v: Vec<(u32, u64)> = self
            .hash_log
            .iter()
            .copied()
            .filter(|&(f, _)| f != u32::MAX && f <= fin)
            .collect();
        v.sort_unstable();
        let items: Vec<String> = v
            .iter()
            .map(|(f, h)| format!("[{f},\"{h:016x}\"]"))
            .collect();
        format!("[{}]", items.join(","))
    }
}

/// What a page tick did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TickOutcome {
    /// `Advance` requests executed (replays included): non-zero means the
    /// picture may have changed.
    pub(crate) simulated: u32,
}

impl WebEmu {
    /// One rollback tick (see the module docs). The caller has already
    /// advanced `slot.now_ms`.
    pub(crate) fn rollback_tick(&mut self, local_pad: u8) -> Result<TickOutcome, String> {
        let mut out = TickOutcome::default();
        let requests = {
            let Some(slot) = self.net.as_mut() else {
                return Ok(out);
            };
            let now = slot.now_ms;
            let started = slot.started;
            let Some(rb) = slot.rollback_mut() else {
                return Ok(out);
            };
            rb.session.transport_mut().set_now(now);
            if !started || rb.session.state() != RollbackState::Running || rb.skip > 0 {
                if started && rb.skip > 0 {
                    rb.skip -= 1;
                    rb.skipped_ticks += 1;
                }
                rb.session.poll(now);
                None
            } else {
                rb.session.add_local_input(local_pad);
                match rb.session.advance(now) {
                    Ok(r) => Some(r),
                    Err(RollbackError::Closed(_)) => None,
                    Err(e) => return Err(e.to_string()),
                }
            }
        };
        let mut audible: Option<u8> = None;
        if let Some(requests) = requests {
            let Self { game, net, .. } = self;
            let (Some(game), Some(slot)) = (game.as_mut(), net.as_mut()) else {
                return Ok(out);
            };
            let host = slot.role == z2_net::Role::Host;
            let Some(rb) = slot.rollback_mut() else {
                return Ok(out);
            };
            for r in requests {
                match r {
                    RollbackRequest::SaveState { frame } => {
                        let i = frame as usize % rb.ring.len();
                        game.save_state_into(&mut rb.ring[i]);
                        rb.ring_frames[i] = Some(frame);
                        if rb.session.needs_checksum(frame) {
                            let c = rollback_checksum(&rb.ring[i]);
                            rb.session.report_checksum(frame, c);
                        }
                        if frame % HASH_LOG_EVERY == 0 {
                            let j = (frame / HASH_LOG_EVERY) as usize % HASH_LOG_LEN;
                            rb.hash_log[j] = (frame, state_hash(game));
                        }
                    }
                    RollbackRequest::LoadState { frame } => {
                        let i = frame as usize % rb.ring.len();
                        if rb.ring_frames[i] != Some(frame) {
                            return Err(format!("rollback: no saved state for frame {frame}"));
                        }
                        game.load_state(&rb.ring[i]);
                    }
                    RollbackRequest::Advance {
                        frame,
                        pad1,
                        pad2,
                        replay,
                        ..
                    } => {
                        game.step2(pad1, pad2);
                        out.simulated += 1;
                        if replay {
                            // Already heard once: drop its APU writes.
                            game.apu.log.clear();
                        } else {
                            audible = Some(pad1);
                            rb.last_local = Some((frame, if host { pad1 } else { pad2 }));
                        }
                    }
                }
            }
        }
        if let Some(p1) = audible {
            // Only the newly simulated frame is heard; it is always the last
            // request, so the APU write log holds exactly that frame's writes.
            self.last_input = p1;
            self.audio_after_step();
        }
        self.drain_rollback_events()?;
        Ok(out)
    }

    pub(crate) fn drain_rollback_events(&mut self) -> Result<(), String> {
        let events = match self.net.as_mut().and_then(|s| s.rollback_mut()) {
            Some(rb) => rb.session.events(),
            None => return Ok(()),
        };
        for ev in events {
            match ev {
                RollbackEvent::Running {
                    input_delay,
                    coop_flags,
                    wram,
                    ..
                } => {
                    self.restart_for_session(coop_flags, &wram)?;
                    if let Some(slot) = self.net.as_mut() {
                        slot.delay = input_delay;
                        slot.started = true;
                        if let Some(rb) = slot.rollback_mut() {
                            rb.ring_frames.iter_mut().for_each(|f| *f = None);
                        }
                    }
                }
                RollbackEvent::WaitRecommendation { skip_frames } => {
                    if let Some(rb) = self.net.as_mut().and_then(|s| s.rollback_mut()) {
                        rb.skip = rb.skip.max(skip_frames);
                    }
                }
                RollbackEvent::NetworkInterrupted { .. } | RollbackEvent::NetworkResumed => {
                    let on = matches!(ev, RollbackEvent::NetworkInterrupted { .. });
                    if let Some(rb) = self.net.as_mut().and_then(|s| s.rollback_mut()) {
                        rb.interrupted = on;
                    }
                }
                RollbackEvent::DesyncDetected { frame, .. } => {
                    if let Some(slot) = self.net.as_mut() {
                        slot.last_error = Some(format!("desync at frame {frame}"));
                    }
                }
                RollbackEvent::Disconnected { reason } => {
                    if let Some(slot) = self.net.as_mut() {
                        if slot.last_error.is_none() {
                            slot.last_error = Some(reason.to_string());
                        }
                    }
                    self.audio_fifo.clear();
                }
                RollbackEvent::Synchronizing { .. } => {}
            }
        }
        Ok(())
    }
}

/// The lockstep desync hash (`ram`, `wram`, `oam`, co-op hash), also used for
/// the rollback confirmed-hash log. Must stay identical to the desktop's.
#[must_use]
pub fn state_hash(g: &Game) -> u64 {
    hash_state(&[g.ram(), g.wram(), g.oam(), &g.coop_hash().to_le_bytes()])
}

/// Rollback-specific fields of the status JSON (leading comma included).
pub(crate) fn rollback_status_fields(rb: &RollbackSlot) -> String {
    let st = rb.session.stats();
    let t = rb.session.transport();
    let (llf, llp) = match rb.last_local {
        Some((f, p)) => (i64::from(f), i64::from(p)),
        None => (-1, -1),
    };
    format!(
        ",\"rttMs\":{},\"rollbacks\":{},\"rollbackFrames\":{},\"maxRollbackDepth\":{},\
         \"predictionDepth\":{},\"maxPredictionDepth\":{},\"maxPrediction\":{},\
         \"confirmedFrame\":{},\"finalFrame\":{},\"stalledTicks\":{},\"skippedTicks\":{},\
         \"waitRecommendations\":{},\"interrupted\":{},\"lastLocalFrame\":{llf},\
         \"lastLocalPad\":{llp},\"simulate\":{},\"simDropped\":{},\"simDelayed\":{}",
        st.rtt_ms.map_or("null".to_string(), |r| r.to_string()),
        st.rollbacks,
        st.rollback_frames,
        st.max_rollback_depth,
        st.prediction_depth,
        st.max_prediction_depth,
        rb.max_prediction,
        st.confirmed_frame,
        rb.session.final_frame(),
        st.stalled_advances,
        rb.skipped_ticks,
        st.wait_recommendations,
        rb.interrupted,
        t.sim().json(),
        t.dropped,
        t.delayed,
    )
}

/// Rollback state name for the status JSON (the lockstep names, so the page
/// logic that waits for "running" works for both modes).
pub(crate) fn rollback_state_name(rb: &RollbackSlot) -> &'static str {
    match rb.session.state() {
        RollbackState::Idle => "idle",
        RollbackState::Synchronizing => "handshake",
        RollbackState::Running if rb.interrupted => "stalled",
        RollbackState::Running => "running",
        RollbackState::Closed => match rb.session.close_reason() {
            Some(z2_net::CloseReason::Desync) => "desynced",
            _ => "closed",
        },
    }
}

/// Close reason as a JSON value.
pub(crate) fn rollback_close_json(rb: &RollbackSlot) -> String {
    match rb.session.close_reason() {
        Some(r) => format!("\"{}\"", json_escape(&r.to_string())),
        None => "null".to_string(),
    }
}
