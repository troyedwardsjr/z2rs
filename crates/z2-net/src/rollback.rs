//! Rollback session: apply local input at once, predict the remote pad, and
//! re-simulate from a saved state when the real remote pad differs.
//!
//! The session never touches game state. It hands the caller an ordered list
//! of [`RollbackRequest`]s per [`RollbackSession::advance`]; the caller owns
//! the saved states (see [`StateRing`]) and reports checksums of the states it
//! saves. Desyncs are detected on *confirmed* frames only.
//!
//! ```text
//! Idle --link Connected / first packet--> Synchronizing (Hello sent)
//! Synchronizing --valid peer Hello (host: sends Start) / Start (guest)--> Running
//! Running --checksum mismatch / peer Bye(Desync)--> Closed(Desync)
//! any live state --mismatch, timeout, bye, peer left, room full, transport error--> Closed
//! ```
//!
//! # Frame model
//!
//! * "State at frame `f`" is the game after `f` simulated frames. `SaveState
//!   { frame: f }` asks for that state, `Advance { frame: f, .. }` steps it to
//!   `f + 1`.
//! * Input delay `D` (host's value wins, `0..=MAX_ROLLBACK_DELAY`): the pad
//!   added while frame `f` is next belongs to frame `f + D`. Frames `0..D` use
//!   pad 0 on both sides.
//! * A remote pad not yet received is predicted as the last pad received in
//!   order (0 before any). Frame `f` is *confirmed* when every remote pad
//!   `<= f` is known.
//! * At most `max_prediction` frames run ahead of the confirmed remote input;
//!   beyond that `advance` issues no new `Advance` (the session stalls).

use std::collections::{BTreeMap, VecDeque};

use crate::input::{InputLog, Insert};
use crate::session::{CloseReason, HelloCheck};
use crate::transport::{Channel, LinkState, Transport};
use crate::wire::{ByeReason, DecodeError, Hello, Message, Role, Start};
use crate::{
    MismatchKind, DEFAULT_HANDSHAKE_TIMEOUT_MS, DEFAULT_STALL_TIMEOUT_MS, MAX_BATCH,
    PING_INTERVAL_MS, RELIABLE_RESEND_MS, WRAM_LEN,
};

/// Protocol version a rollback session sends in [`Hello`]. Distinct from
/// [`crate::PROTO_VERSION`] so lockstep and rollback peers refuse each other
/// with [`MismatchKind::NetMode`].
pub const ROLLBACK_PROTO_VERSION: u16 = 0x0101;
/// Default rollback input delay in frames.
pub const DEFAULT_ROLLBACK_DELAY: u8 = 2;
/// Largest rollback input delay.
pub const MAX_ROLLBACK_DELAY: u8 = 3;
/// Default prediction window in frames.
pub const DEFAULT_MAX_PREDICTION: u8 = 8;
/// Largest prediction window.
pub const MAX_PREDICTION_WINDOW: u8 = 30;
/// Default confirmed-frame checksum interval.
pub const DEFAULT_CHECK_INTERVAL: u16 = 60;
/// Spacing of time-sync [`Message::Status`] packets.
pub const STATUS_INTERVAL_MS: u64 = 100;
/// Silence from the peer this long emits [`RollbackEvent::NetworkInterrupted`].
pub const INTERRUPT_NOTIFY_MS: u64 = 500;
/// Lead samples averaged per side for time sync.
const SYNC_WINDOW: usize = 16;
/// Minimum samples on each side before a recommendation.
const SYNC_MIN_SAMPLES: usize = 8;
/// A half-skew of at least this many frames produces a recommendation.
const MIN_WAIT_RECOMMENDATION: i32 = 2;
/// Largest recommended wait.
const MAX_WAIT_RECOMMENDATION: i32 = 8;
/// Frames between recommendations.
const RECOMMENDATION_COOLDOWN: u32 = 60;
/// Input frames kept behind the current frame (must exceed the prediction window).
const HISTORY: u32 = 64;

/// One step for the caller, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackRequest {
    /// Store the current game state as the state at `frame`.
    SaveState {
        /// Frame the state belongs to.
        frame: u32,
    },
    /// Restore the state saved for `frame`.
    LoadState {
        /// Frame to restore.
        frame: u32,
    },
    /// Simulate `frame` with these pads.
    Advance {
        /// Frame being simulated (the state moves from `frame` to `frame + 1`).
        frame: u32,
        /// Player 1 (host) pad.
        pad1: u8,
        /// Player 2 (guest) pad.
        pad2: u8,
        /// Every remote pad up to and including `frame` is real, so this
        /// frame's result is final. A frame first simulated on a prediction
        /// that proves right is not re-issued; use
        /// [`RollbackSession::final_frame`] to learn when it settles.
        confirmed: bool,
        /// Re-simulation after a rollback: the frame was shown before, so skip
        /// presenting video and audio for it.
        replay: bool,
    },
}

/// Something the frontend should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackEvent {
    /// Handshake progress in percent (link up 33, peer accepted 66).
    Synchronizing {
        /// 0..=99.
        progress: u8,
    },
    /// Session running from frame 0: rebuild a fresh game, apply `coop_flags`,
    /// copy `wram` if non-empty.
    Running {
        /// Effective input delay.
        input_delay: u8,
        /// Effective co-op flags.
        coop_flags: u32,
        /// Effective checksum interval.
        check_interval: u16,
        /// Host's WRAM snapshot (empty = keep power-on WRAM).
        wram: Vec<u8>,
    },
    /// This side runs ahead of the peer: skip `advance` for `skip_frames`
    /// ticks (keep calling [`RollbackSession::poll`]).
    WaitRecommendation {
        /// Ticks to wait.
        skip_frames: u8,
    },
    /// Nothing heard from the peer for `silent_ms`.
    NetworkInterrupted {
        /// Silence so far.
        silent_ms: u64,
    },
    /// Traffic from the peer resumed after an interruption.
    NetworkResumed,
    /// Confirmed-frame checksums differed (followed by `Disconnected(Desync)`).
    DesyncDetected {
        /// Frame whose state was checksummed.
        frame: u32,
        /// Local checksum.
        local: u64,
        /// Remote checksum.
        remote: u64,
    },
    /// The session ended.
    Disconnected {
        /// Why.
        reason: CloseReason,
    },
}

/// Coarse state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RollbackState {
    /// Waiting for the link.
    Idle,
    /// Handshake in progress.
    Synchronizing,
    /// Frames advance.
    Running,
    /// Ended; see [`RollbackSession::close_reason`].
    Closed,
}

/// Error from [`RollbackSession::advance`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackError {
    /// The session ended.
    Closed(CloseReason),
    /// [`RollbackSession::add_local_input`] was not called for this frame.
    MissingLocalInput {
        /// Frame the local pad is missing for.
        frame: u32,
    },
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackError::Closed(r) => write!(f, "session closed: {r}"),
            RollbackError::MissingLocalInput { frame } => {
                write!(f, "no local input added for frame {frame}")
            }
        }
    }
}

impl std::error::Error for RollbackError {}

/// Invalid [`RollbackConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackConfigError {
    /// Delay above [`MAX_ROLLBACK_DELAY`].
    DelayOutOfRange(u8),
    /// Prediction window above [`MAX_PREDICTION_WINDOW`].
    PredictionOutOfRange(u8),
    /// Co-op flags are zero.
    NoCoopFlags,
    /// WRAM snapshot length other than [`WRAM_LEN`].
    BadWramLen(usize),
}

impl std::fmt::Display for RollbackConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackConfigError::DelayOutOfRange(d) => {
                write!(f, "input delay {d} outside 0..={MAX_ROLLBACK_DELAY}")
            }
            RollbackConfigError::PredictionOutOfRange(p) => {
                write!(
                    f,
                    "prediction window {p} outside 0..={MAX_PREDICTION_WINDOW}"
                )
            }
            RollbackConfigError::NoCoopFlags => f.write_str("co-op flags must be non-zero"),
            RollbackConfigError::BadWramLen(n) => {
                write!(f, "WRAM snapshot must be {WRAM_LEN} bytes, got {n}")
            }
        }
    }
}

impl std::error::Error for RollbackConfigError {}

/// Rollback session parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackConfig {
    /// Host (P1) or guest (P2).
    pub role: Role,
    /// ROM body CRC32; must match the peer.
    pub rom_crc32: u32,
    /// Trap-table identity; must match the peer.
    pub trapset_id: u64,
    /// Host: session co-op flags (non-zero). Guest: supported flags.
    pub coop_flags: u32,
    /// Input delay `0..=MAX_ROLLBACK_DELAY` (host's wins).
    pub input_delay: u8,
    /// Frames the session may run ahead of confirmed remote input
    /// (`0..=MAX_PREDICTION_WINDOW`; 0 never predicts). Local policy, not negotiated.
    pub max_prediction: u8,
    /// Confirmed-frame checksum interval; 0 disables desync detection (host's wins).
    pub check_interval: u16,
    /// Silence from the peer this long closes the session.
    pub disconnect_timeout_ms: u64,
    /// Link up but not running for this long closes the session.
    pub handshake_timeout_ms: u64,
    /// Host: WRAM snapshot for the guest. Guest: ignored.
    pub wram: Option<Vec<u8>>,
}

impl RollbackConfig {
    fn with_role(role: Role, rom_crc32: u32, trapset_id: u64, coop_flags: u32) -> Self {
        Self {
            role,
            rom_crc32,
            trapset_id,
            coop_flags,
            input_delay: DEFAULT_ROLLBACK_DELAY,
            max_prediction: DEFAULT_MAX_PREDICTION,
            check_interval: DEFAULT_CHECK_INTERVAL,
            disconnect_timeout_ms: DEFAULT_STALL_TIMEOUT_MS,
            handshake_timeout_ms: DEFAULT_HANDSHAKE_TIMEOUT_MS,
            wram: None,
        }
    }

    /// Host defaults: delay 2, prediction window 8, checksum every 60 frames.
    pub fn host(rom_crc32: u32, trapset_id: u64, coop_flags: u32) -> Self {
        Self::with_role(Role::Host, rom_crc32, trapset_id, coop_flags)
    }

    /// Guest defaults; `supported_coop_flags` must cover the host's flags.
    pub fn guest(rom_crc32: u32, trapset_id: u64, supported_coop_flags: u32) -> Self {
        Self::with_role(Role::Guest, rom_crc32, trapset_id, supported_coop_flags)
    }
}

/// Counters for status displays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RollbackStats {
    /// Next frame to simulate.
    pub frame: u32,
    /// Every remote pad `< confirmed_frame` is known.
    pub confirmed_frame: u32,
    /// Frames currently simulated on predicted remote input.
    pub prediction_depth: u32,
    /// Largest prediction depth seen.
    pub max_prediction_depth: u32,
    /// Latest round-trip time.
    pub rtt_ms: Option<u32>,
    /// Rollbacks performed.
    pub rollbacks: u64,
    /// Frames re-simulated across all rollbacks.
    pub rollback_frames: u64,
    /// Deepest single rollback.
    pub max_rollback_depth: u32,
    /// `advance` calls that stalled at the prediction window.
    pub stalled_advances: u64,
    /// Latest local lead estimate in frames (positive = this side ahead).
    pub local_lead: i32,
    /// Latest lead the peer reported for itself.
    pub remote_lead: i32,
    /// Wait recommendations issued.
    pub wait_recommendations: u32,
    /// Confirmed checksums that matched the peer's.
    pub checksums_ok: u32,
    /// Packets received.
    pub packets_in: u64,
    /// Packets sent.
    pub packets_out: u64,
    /// Reliable-channel input copies sent.
    pub reliable_resends: u64,
}

/// Where requests' checksums go; implemented by both session kinds so
/// [`apply_requests`] can drive either.
pub trait ChecksumSink {
    /// Whether the state saved for `frame` should be checksummed.
    fn needs_checksum(&self, frame: u32) -> bool;
    /// Checksum of the state saved for `frame`.
    fn report_checksum(&mut self, frame: u32, checksum: u64);
}

/// A game the helpers can drive: cheap save/load, two-pad stepping, checksum.
pub trait RollbackGame {
    /// Saved state.
    type State;
    /// Snapshot the current state.
    fn save(&self) -> Self::State;
    /// Restore a snapshot.
    fn load(&mut self, state: &Self::State);
    /// Simulate one frame.
    fn advance(&mut self, pad1: u8, pad2: u8);
    /// Deterministic checksum of the current state.
    fn checksum(&self) -> u64;
}

/// Ring of saved states keyed by frame. Holding `max_prediction + 2` entries
/// is always enough for a [`RollbackSession`]; a [`SyncTestSession`] needs
/// `check_distance + 2`.
#[derive(Debug, Clone)]
pub struct StateRing<S> {
    slots: Vec<Option<(u32, S)>>,
}

impl<S> StateRing<S> {
    /// Ring with `capacity` slots (at least 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| None).collect(),
        }
    }

    /// Store `state` for `frame`, replacing whatever shared its slot.
    pub fn save(&mut self, frame: u32, state: S) {
        let i = frame as usize % self.slots.len();
        self.slots[i] = Some((frame, state));
    }

    /// The state saved for `frame`, if still held.
    pub fn get(&self, frame: u32) -> Option<&S> {
        match &self.slots[frame as usize % self.slots.len()] {
            Some((f, s)) if *f == frame => Some(s),
            _ => None,
        }
    }
}

/// A `LoadState` named a frame the ring no longer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingState(pub u32);

impl std::fmt::Display for MissingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no saved state for frame {}", self.0)
    }
}

impl std::error::Error for MissingState {}

/// Execute `requests` in order against `game`, saving into `states` and
/// reporting checksums to `sink`. Frontends that must silence replayed
/// frames handle requests themselves; this is the reference loop.
pub fn apply_requests<G: RollbackGame, K: ChecksumSink + ?Sized>(
    game: &mut G,
    states: &mut StateRing<G::State>,
    requests: &[RollbackRequest],
    sink: &mut K,
) -> Result<(), MissingState> {
    for r in requests {
        match *r {
            RollbackRequest::SaveState { frame } => {
                if sink.needs_checksum(frame) {
                    sink.report_checksum(frame, game.checksum());
                }
                states.save(frame, game.save());
            }
            RollbackRequest::LoadState { frame } => {
                game.load(states.get(frame).ok_or(MissingState(frame))?);
            }
            RollbackRequest::Advance { pad1, pad2, .. } => game.advance(pad1, pad2),
        }
    }
    Ok(())
}

/// Two-player rollback session over one [`Transport`].
#[derive(Debug)]
pub struct RollbackSession<T: Transport> {
    cfg: RollbackConfig,
    transport: T,
    state: RollbackState,
    close_reason: Option<CloseReason>,
    delay: u8,
    coop_flags: u32,
    check_interval: u16,
    /// Next frame to simulate.
    frame: u32,
    local: InputLog,
    remote: InputLog,
    /// Remote pads used as predictions, by frame, for frames already simulated.
    predicted: BTreeMap<u32, u8>,
    /// Earliest simulated frame whose prediction proved wrong.
    first_incorrect: Option<u32>,
    peer_ack: u32,
    ack_sent: u32,
    ack_dirty: bool,
    peer_hello: Option<Hello>,
    now_ms: u64,
    handshake_since: u64,
    last_recv_ms: u64,
    interrupted: bool,
    ack_progress_ms: u64,
    last_reliable_input_ms: Option<u64>,
    last_ping_ms: Option<u64>,
    last_status_ms: Option<u64>,
    /// Checksums of states `< hashable_below` are final.
    hashable_below: u32,
    /// `Advance` results for frames `< settled_below` are final.
    settled_below: u32,
    next_check_frame: u32,
    local_checksums: BTreeMap<u32, u64>,
    sent_checksums: BTreeMap<u32, u64>,
    remote_checksums: BTreeMap<u32, u64>,
    /// Smoothed RTT (integer EMA, 1/4 weight per sample) for lead estimates.
    rtt_smooth_ms: Option<u32>,
    local_leads: VecDeque<i32>,
    remote_leads: VecDeque<i32>,
    next_recommendation_frame: u32,
    outbox: Vec<(Channel, Vec<u8>)>,
    events: Vec<RollbackEvent>,
    stats: RollbackStats,
}

impl<T: Transport> RollbackSession<T> {
    /// Validate `cfg` and create an idle session that owns `transport`.
    pub fn new(cfg: RollbackConfig, transport: T) -> Result<Self, RollbackConfigError> {
        if cfg.input_delay > MAX_ROLLBACK_DELAY {
            return Err(RollbackConfigError::DelayOutOfRange(cfg.input_delay));
        }
        if cfg.max_prediction > MAX_PREDICTION_WINDOW {
            return Err(RollbackConfigError::PredictionOutOfRange(
                cfg.max_prediction,
            ));
        }
        if cfg.coop_flags == 0 {
            return Err(RollbackConfigError::NoCoopFlags);
        }
        if let Some(w) = &cfg.wram {
            if w.len() != WRAM_LEN {
                return Err(RollbackConfigError::BadWramLen(w.len()));
            }
        }
        Ok(Self {
            transport,
            state: RollbackState::Idle,
            close_reason: None,
            delay: cfg.input_delay,
            coop_flags: cfg.coop_flags,
            check_interval: cfg.check_interval,
            frame: 0,
            local: InputLog::default(),
            remote: InputLog::default(),
            predicted: BTreeMap::new(),
            first_incorrect: None,
            peer_ack: 0,
            ack_sent: 0,
            ack_dirty: false,
            peer_hello: None,
            now_ms: 0,
            handshake_since: 0,
            last_recv_ms: 0,
            interrupted: false,
            ack_progress_ms: 0,
            last_reliable_input_ms: None,
            last_ping_ms: None,
            last_status_ms: None,
            hashable_below: 0,
            settled_below: 0,
            next_check_frame: 0,
            local_checksums: BTreeMap::new(),
            sent_checksums: BTreeMap::new(),
            remote_checksums: BTreeMap::new(),
            rtt_smooth_ms: None,
            local_leads: VecDeque::new(),
            remote_leads: VecDeque::new(),
            next_recommendation_frame: 0,
            outbox: Vec::new(),
            events: Vec::new(),
            stats: RollbackStats::default(),
            cfg,
        })
    }

    /// Current state.
    pub fn state(&self) -> RollbackState {
        self.state
    }

    /// Why the session ended.
    pub fn close_reason(&self) -> Option<&CloseReason> {
        self.close_reason.as_ref()
    }

    /// Running.
    pub fn is_running(&self) -> bool {
        self.state == RollbackState::Running
    }

    /// Own role (host = player 1).
    pub fn role(&self) -> Role {
        self.cfg.role
    }

    /// Effective input delay.
    pub fn input_delay(&self) -> u8 {
        self.delay
    }

    /// Effective co-op flags.
    pub fn coop_flags(&self) -> u32 {
        self.coop_flags
    }

    /// Effective checksum interval.
    pub fn check_interval(&self) -> u16 {
        self.check_interval
    }

    /// Next frame to simulate.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Every remote pad `< confirmed_frame()` is known.
    pub fn confirmed_frame(&self) -> u32 {
        self.remote.contiguous()
    }

    /// Once the requests of the latest [`RollbackSession::advance`] have run,
    /// the result of the last `Advance` of every frame `< final_frame()` is
    /// final: both pads were real and no rollback can reach it.
    pub fn final_frame(&self) -> u32 {
        self.settled_below
    }

    /// The transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport, mutably.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Give the transport back.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Counters.
    pub fn stats(&self) -> RollbackStats {
        let mut s = self.stats;
        s.frame = self.frame;
        s.confirmed_frame = self.remote.contiguous();
        s.prediction_depth = self.frame.saturating_sub(self.remote.contiguous());
        s
    }

    /// Drain pending events.
    pub fn events(&mut self) -> Vec<RollbackEvent> {
        std::mem::take(&mut self.events)
    }

    /// Record this tick's local pad for frame `frame() + input_delay()`.
    /// Returns false when that frame already has a pad (e.g. while stalled)
    /// or the session is not running.
    pub fn add_local_input(&mut self, pad: u8) -> bool {
        if !self.is_running() {
            return false;
        }
        let target = self.frame + u32::from(self.delay);
        if self.local.contiguous() != target {
            return false;
        }
        if self.local.contiguous() == self.peer_ack {
            self.ack_progress_ms = self.now_ms;
        }
        self.local.insert(target, pad) == Insert::New
    }

    /// Pump the network without advancing (use while honouring a
    /// [`RollbackEvent::WaitRecommendation`] or before the session runs).
    pub fn poll(&mut self, now_ms: u64) {
        self.pump(now_ms);
        self.queue_inputs();
        self.flush();
    }

    /// Pump the network, then return the requests for this tick: any
    /// rollback (`LoadState`, then replayed `SaveState`/`Advance` pairs) and,
    /// unless the prediction window is full, `SaveState` + `Advance` for the
    /// next frame. Call [`RollbackSession::add_local_input`] first. Before the
    /// session runs, and on the tick it starts, this returns an empty list.
    pub fn advance(&mut self, now_ms: u64) -> Result<Vec<RollbackRequest>, RollbackError> {
        let was_running = self.is_running();
        self.pump(now_ms);
        let reqs = match self.state {
            RollbackState::Closed => {
                self.flush();
                let reason = self.close_reason.clone().unwrap_or(CloseReason::LocalQuit);
                return Err(RollbackError::Closed(reason));
            }
            RollbackState::Idle | RollbackState::Synchronizing => Vec::new(),
            // Started during this pump: frame 0 steps from the next tick on.
            RollbackState::Running if !was_running => Vec::new(),
            RollbackState::Running => {
                let target = self.frame + u32::from(self.delay);
                if self.local.get(target).is_none() {
                    self.flush();
                    return Err(RollbackError::MissingLocalInput { frame: target });
                }
                self.step_frames()
            }
        };
        self.queue_inputs();
        self.flush();
        Ok(reqs)
    }

    /// Leave: send Bye(Quit) now and close. Keep calling [`RollbackSession::poll`]
    /// briefly if the transport needs time to deliver it.
    pub fn close(&mut self) {
        self.fail(CloseReason::LocalQuit, Some(ByeReason::Quit));
        self.flush();
    }

    // ---- internals -------------------------------------------------------

    fn step_frames(&mut self) -> Vec<RollbackRequest> {
        let mut reqs = Vec::new();
        let cur = self.frame;
        if let Some(r) = self.first_incorrect.take() {
            reqs.push(RollbackRequest::LoadState { frame: r });
            // Predictions from r on are re-made during the replay.
            let _ = self.predicted.split_off(&r);
            for f in r..cur {
                if f > r {
                    reqs.push(RollbackRequest::SaveState { frame: f });
                }
                reqs.push(self.simulate(f, true));
            }
            let depth = cur - r;
            self.stats.rollbacks += 1;
            self.stats.rollback_frames += u64::from(depth);
            self.stats.max_rollback_depth = self.stats.max_rollback_depth.max(depth);
        }
        let contiguous = self.remote.contiguous();
        if cur < contiguous + u32::from(self.cfg.max_prediction) {
            reqs.push(RollbackRequest::SaveState { frame: cur });
            reqs.push(self.simulate(cur, false));
            self.frame += 1;
        } else {
            self.stats.stalled_advances += 1;
        }
        let depth = self.frame.saturating_sub(contiguous);
        self.stats.max_prediction_depth = self.stats.max_prediction_depth.max(depth);
        // Every state < frame is saved and corrected for all remote pads known now.
        self.hashable_below = self.frame.min(contiguous + 1);
        self.settled_below = self.frame.min(contiguous);
        self.prune();
        reqs
    }

    fn simulate(&mut self, f: u32, replay: bool) -> RollbackRequest {
        let local = self.local.get(f).unwrap_or(0);
        let contiguous = self.remote.contiguous();
        let remote = match self.remote.get(f) {
            Some(p) => p,
            None => {
                let p = contiguous
                    .checked_sub(1)
                    .and_then(|last| self.remote.get(last))
                    .unwrap_or(0);
                self.predicted.insert(f, p);
                p
            }
        };
        let (pad1, pad2) = match self.cfg.role {
            Role::Host => (local, remote),
            Role::Guest => (remote, local),
        };
        RollbackRequest::Advance {
            frame: f,
            pad1,
            pad2,
            confirmed: f < contiguous,
            replay,
        }
    }

    fn prune(&mut self) {
        let floor = self.frame.saturating_sub(HISTORY);
        self.remote
            .prune_below(floor.min(self.remote.contiguous().saturating_sub(1)));
        self.local.prune_below(floor.min(self.peer_ack));
        let keep_from = self
            .next_check_frame
            .saturating_sub(16 * u32::from(self.check_interval.max(1)));
        self.sent_checksums = self.sent_checksums.split_off(&keep_from);
        self.remote_checksums = self.remote_checksums.split_off(&keep_from);
        self.local_checksums = self.local_checksums.split_off(&keep_from);
    }

    fn pump(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        let packets = self.transport.recv();
        let link = self.transport.state();
        if link == LinkState::Connected {
            self.on_link_up();
        }
        for (channel, bytes) in packets {
            self.stats.packets_in += 1;
            self.on_packet(channel, &bytes);
        }
        match link {
            LinkState::PeerLeft => self.fail(CloseReason::PeerLeft, None),
            LinkState::RoomFull => self.fail(CloseReason::RoomFull, None),
            LinkState::Closed(m) => self.fail(CloseReason::Transport(m), None),
            LinkState::Connecting | LinkState::Connected => {}
        }
        self.run_timers();
        self.send_checksums();
    }

    fn queue(&mut self, channel: Channel, msg: &Message) {
        self.outbox.push((channel, msg.encode()));
    }

    fn fail(&mut self, reason: CloseReason, bye: Option<ByeReason>) {
        if self.state == RollbackState::Closed {
            return;
        }
        if let Some(b) = bye {
            self.queue(Channel::Reliable, &Message::Bye(b));
        }
        self.state = RollbackState::Closed;
        self.close_reason = Some(reason.clone());
        self.events.push(RollbackEvent::Disconnected { reason });
    }

    fn on_link_up(&mut self) {
        if self.state != RollbackState::Idle {
            return;
        }
        self.state = RollbackState::Synchronizing;
        self.handshake_since = self.now_ms;
        self.last_recv_ms = self.now_ms;
        self.events
            .push(RollbackEvent::Synchronizing { progress: 33 });
        let hello = Hello {
            proto: ROLLBACK_PROTO_VERSION,
            role: self.cfg.role,
            rom_crc32: self.cfg.rom_crc32,
            trapset_id: self.cfg.trapset_id,
            coop_flags: self.cfg.coop_flags,
            delay: self.cfg.input_delay,
            hash_interval: self.cfg.check_interval,
        };
        self.queue(Channel::Reliable, &Message::Hello(hello));
    }

    fn on_packet(&mut self, _channel: Channel, bytes: &[u8]) {
        if self.state == RollbackState::Closed {
            return;
        }
        self.on_link_up();
        self.last_recv_ms = self.now_ms;
        if self.interrupted {
            self.interrupted = false;
            self.events.push(RollbackEvent::NetworkResumed);
        }
        let msg = match Message::decode(bytes) {
            Ok(m) => m,
            Err(DecodeError::BadVersion(_)) => {
                self.fail(
                    CloseReason::Mismatch(MismatchKind::WireVersion),
                    Some(ByeReason::VersionMismatch),
                );
                return;
            }
            Err(e) => {
                self.fail(
                    CloseReason::Protocol(format!("malformed packet: {e}")),
                    Some(ByeReason::Protocol),
                );
                return;
            }
        };
        match msg {
            Message::Hello(h) => self.on_hello(h),
            Message::Start(s) => self.on_start(s),
            Message::Input { ack, first, pads } => self.on_input(ack, first, &pads),
            Message::Hash { frame, hash } => {
                if self.is_running() {
                    self.remote_checksums.insert(frame, hash);
                    self.compare_checksums(frame);
                }
            }
            Message::Status { frame, lead } => self.on_status(frame, lead),
            Message::Ping { token } => self.queue(Channel::Unreliable, &Message::Pong { token }),
            Message::Pong { token } => {
                let rtt = (self.now_ms as u32).wrapping_sub(token).min(10_000);
                self.stats.rtt_ms = Some(rtt);
                self.rtt_smooth_ms = Some(self.rtt_smooth_ms.map_or(rtt, |s| (s * 3 + rtt) / 4));
            }
            Message::Bye(r) => {
                let reason = match r {
                    ByeReason::Quit => CloseReason::PeerQuit,
                    ByeReason::Desync => CloseReason::Desync,
                    ByeReason::VersionMismatch => CloseReason::Mismatch(MismatchKind::Proto),
                    ByeReason::RomMismatch => CloseReason::Mismatch(MismatchKind::Rom),
                    ByeReason::TrapSetMismatch => CloseReason::Mismatch(MismatchKind::TrapSet),
                    ByeReason::CoopFlagsMismatch => CloseReason::Mismatch(MismatchKind::CoopFlags),
                    ByeReason::RoleConflict => CloseReason::RoleConflict,
                    ByeReason::Timeout => CloseReason::Timeout,
                    ByeReason::Protocol => {
                        CloseReason::Protocol("peer reported a protocol error".into())
                    }
                };
                self.fail(reason, None);
            }
        }
    }

    fn on_hello(&mut self, h: Hello) {
        if self.state != RollbackState::Synchronizing || self.peer_hello.is_some() {
            return;
        }
        let local = HelloCheck {
            proto: ROLLBACK_PROTO_VERSION,
            role: self.cfg.role,
            rom_crc32: self.cfg.rom_crc32,
            trapset_id: self.cfg.trapset_id,
            coop_flags: self.cfg.coop_flags,
            max_delay: MAX_ROLLBACK_DELAY,
        };
        if let Err((reason, bye)) = local.check(&h) {
            self.fail(reason, Some(bye));
            return;
        }
        self.peer_hello = Some(h);
        self.events
            .push(RollbackEvent::Synchronizing { progress: 66 });
        if self.cfg.role == Role::Host {
            let start = Start {
                frame: 0,
                delay: self.delay,
                coop_flags: self.coop_flags,
                hash_interval: self.check_interval,
                wram: self.cfg.wram.clone().unwrap_or_default(),
            };
            self.queue(Channel::Reliable, &Message::Start(start.clone()));
            self.begin(start.wram);
        }
    }

    fn on_start(&mut self, s: Start) {
        if self.cfg.role == Role::Host {
            self.fail(
                CloseReason::Protocol("guest sent Start".into()),
                Some(ByeReason::Protocol),
            );
            return;
        }
        if self.state != RollbackState::Synchronizing {
            return;
        }
        let Some(hello) = &self.peer_hello else {
            self.fail(
                CloseReason::Protocol("Start before Hello".into()),
                Some(ByeReason::Protocol),
            );
            return;
        };
        let bad = if s.frame != 0 {
            Some(format!("start frame {} unsupported", s.frame))
        } else if s.delay > MAX_ROLLBACK_DELAY {
            Some(format!("start delay {} out of range", s.delay))
        } else if s.coop_flags != hello.coop_flags || s.hash_interval != hello.hash_interval {
            Some("Start disagrees with host Hello".into())
        } else {
            None
        };
        if let Some(m) = bad {
            self.fail(CloseReason::Protocol(m), Some(ByeReason::Protocol));
            return;
        }
        self.delay = s.delay;
        self.coop_flags = s.coop_flags;
        self.check_interval = s.hash_interval;
        self.begin(s.wram);
    }

    fn begin(&mut self, wram: Vec<u8>) {
        for f in 0..u32::from(self.delay) {
            self.local.insert(f, 0);
            self.remote.insert(f, 0);
        }
        self.peer_ack = u32::from(self.delay);
        self.ack_sent = u32::from(self.delay);
        self.frame = 0;
        self.ack_progress_ms = self.now_ms;
        self.last_recv_ms = self.now_ms;
        self.next_recommendation_frame = RECOMMENDATION_COOLDOWN;
        self.state = RollbackState::Running;
        self.events.push(RollbackEvent::Running {
            input_delay: self.delay,
            coop_flags: self.coop_flags,
            check_interval: self.check_interval,
            wram,
        });
    }

    fn on_input(&mut self, ack: u32, first: u32, pads: &[u8]) {
        if !self.is_running() {
            return;
        }
        let ack = ack.min(self.local.contiguous());
        if ack > self.peer_ack {
            self.peer_ack = ack;
            self.ack_progress_ms = self.now_ms;
        }
        if !pads.is_empty() {
            self.ack_dirty = true;
        }
        for (i, &pad) in pads.iter().enumerate() {
            let frame = first + i as u32;
            match self.remote.insert(frame, pad) {
                Insert::New => {
                    if let Some(p) = self.predicted.remove(&frame) {
                        if p != pad {
                            let r = self.first_incorrect.map_or(frame, |r| r.min(frame));
                            self.first_incorrect = Some(r);
                        }
                    }
                }
                Insert::Conflict => {
                    self.fail(
                        CloseReason::Protocol(format!("remote input for frame {frame} rewritten")),
                        Some(ByeReason::Protocol),
                    );
                    return;
                }
                Insert::Same | Insert::Stale | Insert::TooFar => {}
            }
        }
    }

    /// Frames of travel for half the round trip (60 frames per second).
    fn half_rtt_frames(&self) -> i32 {
        self.rtt_smooth_ms.map_or(0, |r| (r * 3 / 100) as i32)
    }

    fn on_status(&mut self, remote_frame: u32, remote_lead: i8) {
        if !self.is_running() {
            return;
        }
        let estimate = i64::from(remote_frame) + i64::from(self.half_rtt_frames());
        let lead = (i64::from(self.frame) - estimate).clamp(-127, 127) as i32;
        self.stats.local_lead = lead;
        self.stats.remote_lead = i32::from(remote_lead);
        push_window(&mut self.local_leads, lead);
        push_window(&mut self.remote_leads, i32::from(remote_lead));
        if self.local_leads.len() < SYNC_MIN_SAMPLES
            || self.remote_leads.len() < SYNC_MIN_SAMPLES
            || self.frame < self.next_recommendation_frame
        {
            return;
        }
        let avg = |w: &VecDeque<i32>| w.iter().sum::<i32>() / w.len() as i32;
        let skew = (avg(&self.local_leads) - avg(&self.remote_leads)) / 2;
        if skew >= MIN_WAIT_RECOMMENDATION {
            let skip = skew.min(MAX_WAIT_RECOMMENDATION) as u8;
            self.events
                .push(RollbackEvent::WaitRecommendation { skip_frames: skip });
            self.stats.wait_recommendations += 1;
            self.next_recommendation_frame = self.frame + RECOMMENDATION_COOLDOWN;
            self.local_leads.clear();
            self.remote_leads.clear();
        }
    }

    /// Whether the checksum of the state saved for `frame` is wanted.
    pub fn needs_checksum(&self, frame: u32) -> bool {
        self.check_interval != 0
            && frame.is_multiple_of(u32::from(self.check_interval))
            && frame >= self.next_check_frame
    }

    /// Report the checksum of the state saved for `frame` (latest save wins).
    pub fn report_checksum(&mut self, frame: u32, checksum: u64) {
        if self.is_running() && self.needs_checksum(frame) {
            self.local_checksums.insert(frame, checksum);
        }
    }

    fn send_checksums(&mut self) {
        if !self.is_running() || self.check_interval == 0 {
            return;
        }
        let step = u32::from(self.check_interval);
        while self.next_check_frame < self.hashable_below {
            let f = self.next_check_frame;
            self.next_check_frame += step;
            if let Some(c) = self.local_checksums.remove(&f) {
                self.queue(Channel::Reliable, &Message::Hash { frame: f, hash: c });
                self.sent_checksums.insert(f, c);
                self.compare_checksums(f);
                if !self.is_running() {
                    return;
                }
            }
        }
    }

    fn compare_checksums(&mut self, frame: u32) {
        let (Some(&local), Some(&remote)) = (
            self.sent_checksums.get(&frame),
            self.remote_checksums.get(&frame),
        ) else {
            return;
        };
        self.sent_checksums.remove(&frame);
        self.remote_checksums.remove(&frame);
        if local == remote {
            self.stats.checksums_ok += 1;
        } else {
            self.events.push(RollbackEvent::DesyncDetected {
                frame,
                local,
                remote,
            });
            self.fail(CloseReason::Desync, Some(ByeReason::Desync));
        }
    }

    fn run_timers(&mut self) {
        let now = self.now_ms;
        match self.state {
            RollbackState::Synchronizing
                if now.saturating_sub(self.handshake_since) >= self.cfg.handshake_timeout_ms =>
            {
                self.fail(CloseReason::Timeout, Some(ByeReason::Timeout));
            }
            RollbackState::Running => {
                let silent = now.saturating_sub(self.last_recv_ms);
                if silent >= self.cfg.disconnect_timeout_ms {
                    self.fail(CloseReason::Timeout, Some(ByeReason::Timeout));
                    return;
                }
                if silent >= INTERRUPT_NOTIFY_MS && !self.interrupted {
                    self.interrupted = true;
                    self.events
                        .push(RollbackEvent::NetworkInterrupted { silent_ms: silent });
                }
                if self
                    .last_ping_ms
                    .is_none_or(|t| now.saturating_sub(t) >= PING_INTERVAL_MS)
                {
                    self.last_ping_ms = Some(now);
                    self.queue(Channel::Unreliable, &Message::Ping { token: now as u32 });
                }
                if self
                    .last_status_ms
                    .is_none_or(|t| now.saturating_sub(t) >= STATUS_INTERVAL_MS)
                {
                    self.last_status_ms = Some(now);
                    let lead = self.stats.local_lead.clamp(-127, 127) as i8;
                    let msg = Message::Status {
                        frame: self.frame,
                        lead,
                    };
                    self.queue(Channel::Unreliable, &msg);
                }
            }
            _ => {}
        }
    }

    fn queue_inputs(&mut self) {
        if !self.is_running() {
            return;
        }
        let now = self.now_ms;
        let known = self.remote.contiguous();
        let contiguous = self.local.contiguous();
        if contiguous > self.peer_ack {
            let first = self.peer_ack;
            let end = contiguous.min(first + MAX_BATCH as u32);
            let pads: Vec<u8> = (first..end).filter_map(|f| self.local.get(f)).collect();
            debug_assert_eq!(pads.len() as u32, end - first, "local log pruned too far");
            let msg = Message::Input {
                ack: known,
                first,
                pads,
            };
            self.queue(Channel::Unreliable, &msg);
            let overdue = now.saturating_sub(self.ack_progress_ms) >= RELIABLE_RESEND_MS;
            let spaced = self
                .last_reliable_input_ms
                .is_none_or(|t| now.saturating_sub(t) >= RELIABLE_RESEND_MS);
            if overdue && spaced {
                self.queue(Channel::Reliable, &msg);
                self.last_reliable_input_ms = Some(now);
                self.stats.reliable_resends += 1;
            }
            self.ack_sent = known;
            self.ack_dirty = false;
        } else if known > self.ack_sent || self.ack_dirty {
            let msg = Message::Input {
                ack: known,
                first: self.peer_ack,
                pads: Vec::new(),
            };
            self.queue(Channel::Unreliable, &msg);
            self.ack_sent = known;
            self.ack_dirty = false;
        }
    }

    fn flush(&mut self) {
        for (channel, bytes) in std::mem::take(&mut self.outbox) {
            match self.transport.send(channel, &bytes) {
                Ok(()) => self.stats.packets_out += 1,
                Err(e) => {
                    if channel == Channel::Reliable {
                        self.fail(CloseReason::Transport(e.to_string()), None);
                    }
                }
            }
        }
        self.outbox.clear();
    }
}

impl<T: Transport> ChecksumSink for RollbackSession<T> {
    fn needs_checksum(&self, frame: u32) -> bool {
        RollbackSession::needs_checksum(self, frame)
    }
    fn report_checksum(&mut self, frame: u32, checksum: u64) {
        RollbackSession::report_checksum(self, frame, checksum);
    }
}

fn push_window(w: &mut VecDeque<i32>, v: i32) {
    if w.len() == SYNC_WINDOW {
        w.pop_front();
    }
    w.push_back(v);
}

/// Local determinism check: every frame rolls back `check_distance` frames,
/// re-simulates, and compares each re-saved state's checksum with the first
/// one reported for that frame. No transport; both pads come from the caller.
#[derive(Debug)]
pub struct SyncTestSession {
    check_distance: u32,
    frame: u32,
    inputs: VecDeque<(u8, u8)>,
    inputs_base: u32,
    first_checksums: BTreeMap<u32, u64>,
    events: Vec<RollbackEvent>,
    mismatches: u32,
}

impl SyncTestSession {
    /// Roll back `check_distance` frames (at least 1) every frame.
    pub fn new(check_distance: u8) -> Self {
        Self {
            check_distance: u32::from(check_distance.max(1)),
            frame: 0,
            inputs: VecDeque::new(),
            inputs_base: 0,
            first_checksums: BTreeMap::new(),
            events: Vec::new(),
            mismatches: 0,
        }
    }

    /// Next frame to simulate.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Checksum mismatches found so far.
    pub fn mismatches(&self) -> u32 {
        self.mismatches
    }

    /// Drain events (only [`RollbackEvent::DesyncDetected`]).
    pub fn events(&mut self) -> Vec<RollbackEvent> {
        std::mem::take(&mut self.events)
    }

    /// Requests for one frame with pads `(pad1, pad2)`: a rollback of
    /// `check_distance` frames once enough history exists, then the new frame.
    pub fn advance(&mut self, pad1: u8, pad2: u8) -> Vec<RollbackRequest> {
        let cur = self.frame;
        self.inputs.push_back((pad1, pad2));
        let mut reqs = Vec::new();
        if cur >= self.check_distance {
            let r = cur - self.check_distance;
            reqs.push(RollbackRequest::LoadState { frame: r });
            for f in r..cur {
                if f > r {
                    reqs.push(RollbackRequest::SaveState { frame: f });
                }
                reqs.push(self.request(f, true));
            }
        }
        reqs.push(RollbackRequest::SaveState { frame: cur });
        reqs.push(self.request(cur, false));
        self.frame += 1;
        let floor = self.frame.saturating_sub(self.check_distance + 1);
        while self.inputs_base < floor {
            self.inputs.pop_front();
            self.inputs_base += 1;
        }
        self.first_checksums = self.first_checksums.split_off(&floor);
        reqs
    }

    fn request(&self, f: u32, replay: bool) -> RollbackRequest {
        let (pad1, pad2) = self.inputs[(f - self.inputs_base) as usize];
        RollbackRequest::Advance {
            frame: f,
            pad1,
            pad2,
            confirmed: true,
            replay,
        }
    }
}

impl ChecksumSink for SyncTestSession {
    fn needs_checksum(&self, _frame: u32) -> bool {
        true
    }

    fn report_checksum(&mut self, frame: u32, checksum: u64) {
        match self.first_checksums.get(&frame) {
            None => {
                self.first_checksums.insert(frame, checksum);
            }
            Some(&first) if first != checksum => {
                self.mismatches += 1;
                self.events.push(RollbackEvent::DesyncDetected {
                    frame,
                    local: first,
                    remote: checksum,
                });
            }
            Some(_) => {}
        }
    }
}
