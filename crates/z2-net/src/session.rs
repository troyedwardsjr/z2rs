//! Lockstep session state machine.
//!
//! ```text
//! Idle --link Connected / first packet--> Handshake (Hello sent)
//! Handshake --valid peer Hello (host: sends Start) / Start (guest)--> Running
//! Running --remote pad missing >= STALL_NOTIFY_MS--> Stalled --frame stepped--> Running
//! Running|Stalled --hash mismatch / peer Bye(Desync)--> Desynced
//! any live state --mismatch, timeout, bye, peer left, room full, transport error--> Closed
//! ```

use std::collections::BTreeMap;

use crate::input::{InputLog, Insert};
use crate::transport::{Channel, LinkState, Transport};
use crate::wire::{ByeReason, DecodeError, Hello, Message, Role, Start};
use crate::{
    DEFAULT_DELAY, DEFAULT_HANDSHAKE_TIMEOUT_MS, DEFAULT_HASH_INTERVAL, DEFAULT_STALL_TIMEOUT_MS,
    MAX_BATCH, MAX_DELAY, PING_INTERVAL_MS, PROTO_VERSION, RELIABLE_RESEND_MS,
    ROLLBACK_PROTO_VERSION, STALL_NOTIFY_MS, WRAM_LEN,
};

/// Remote frames kept behind the next frame (for rewrite detection).
const HISTORY: u32 = 64;

/// What did not match in the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MismatchKind {
    /// Envelope version byte.
    WireVersion,
    /// [`PROTO_VERSION`].
    Proto,
    /// ROM CRC32.
    Rom,
    /// Trap-set identity.
    TrapSet,
    /// Host co-op flags zero or not supported by the guest.
    CoopFlags,
    /// One peer runs lockstep ([`Session`]) and the other rollback
    /// ([`crate::RollbackSession`]).
    NetMode,
}

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// [`Session::close`] was called.
    LocalQuit,
    /// The peer sent Bye(Quit).
    PeerQuit,
    /// The transport reported the peer gone.
    PeerLeft,
    /// The signalling server refused us (room already has two players).
    RoomFull,
    /// Handshake mismatch (either side detected it).
    Mismatch(MismatchKind),
    /// Both peers claimed the same role.
    RoleConflict,
    /// Handshake or stall timeout.
    Timeout,
    /// State hashes differed.
    Desync,
    /// Malformed or contradictory traffic.
    Protocol(String),
    /// Transport failure.
    Transport(String),
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloseReason::LocalQuit => f.write_str("quit"),
            CloseReason::PeerQuit => f.write_str("peer quit"),
            CloseReason::PeerLeft => f.write_str("peer left"),
            CloseReason::RoomFull => f.write_str("room full"),
            CloseReason::Mismatch(k) => write!(f, "mismatch: {k:?}"),
            CloseReason::RoleConflict => f.write_str("role conflict (both host or both guest)"),
            CloseReason::Timeout => f.write_str("timeout"),
            CloseReason::Desync => f.write_str("desync"),
            CloseReason::Protocol(m) => write!(f, "protocol error: {m}"),
            CloseReason::Transport(m) => write!(f, "transport: {m}"),
        }
    }
}

/// Coarse session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionState {
    /// Link not up yet (waiting for a peer).
    Idle,
    /// Link up, Hello exchanged / waiting for Start.
    Handshake,
    /// Stepping.
    Running,
    /// Waiting for the remote pad for at least [`STALL_NOTIFY_MS`].
    Stalled,
    /// Hash mismatch; nothing steps any more. See [`Session::close_reason`].
    Desynced,
    /// Ended. See [`Session::close_reason`].
    Closed,
}

/// Invalid [`SessionConfig`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// Delay above [`MAX_DELAY`].
    DelayOutOfRange(u8),
    /// Co-op flags are zero: the guest would have no gameplay.
    NoCoopFlags,
    /// WRAM snapshot length other than [`WRAM_LEN`].
    BadWramLen(usize),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::DelayOutOfRange(d) => write!(f, "input delay {d} outside 0..={MAX_DELAY}"),
            ConfigError::NoCoopFlags => f.write_str("co-op flags must be non-zero"),
            ConfigError::BadWramLen(n) => {
                write!(f, "WRAM snapshot must be {WRAM_LEN} bytes, got {n}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Session parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// Host (P1) or guest (P2).
    pub role: Role,
    /// ROM body CRC32; must match the peer.
    pub rom_crc32: u32,
    /// Trap-table identity; must match the peer.
    pub trapset_id: u64,
    /// Host: session co-op flags (non-zero). Guest: supported flags (non-zero).
    pub coop_flags: u32,
    /// Requested input delay `0..=MAX_DELAY` (host's wins).
    pub delay: u8,
    /// Hash interval in frames; 0 disables hashing (host's wins).
    pub hash_interval: u16,
    /// Waiting this long for the remote pad closes the session.
    pub stall_timeout_ms: u64,
    /// Link up but not started for this long closes the session.
    pub handshake_timeout_ms: u64,
    /// Host: WRAM snapshot sent to the guest (None = both keep power-on WRAM). Guest: ignored.
    pub wram: Option<Vec<u8>>,
}

impl SessionConfig {
    fn with_role(role: Role, rom_crc32: u32, trapset_id: u64, coop_flags: u32) -> Self {
        Self {
            role,
            rom_crc32,
            trapset_id,
            coop_flags,
            delay: DEFAULT_DELAY,
            hash_interval: DEFAULT_HASH_INTERVAL,
            stall_timeout_ms: DEFAULT_STALL_TIMEOUT_MS,
            handshake_timeout_ms: DEFAULT_HANDSHAKE_TIMEOUT_MS,
            wram: None,
        }
    }

    /// Host defaults: delay 2, hash every 60 frames, 60 s stall / 30 s handshake timeouts.
    pub fn host(rom_crc32: u32, trapset_id: u64, coop_flags: u32) -> Self {
        Self::with_role(Role::Host, rom_crc32, trapset_id, coop_flags)
    }

    /// Guest defaults; `supported_coop_flags` must cover the host's flags.
    pub fn guest(rom_crc32: u32, trapset_id: u64, supported_coop_flags: u32) -> Self {
        Self::with_role(Role::Guest, rom_crc32, trapset_id, supported_coop_flags)
    }
}

/// Something the frontend should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The peer's Hello passed all checks.
    PeerHello {
        /// Peer role.
        role: Role,
        /// Peer's requested delay.
        delay: u8,
        /// Peer's co-op flags (host: session flags, guest: supported flags).
        coop_flags: u32,
    },
    /// Session running from frame 0: rebuild a fresh game, apply `coop_flags`, copy `wram` if non-empty.
    Started {
        /// Effective input delay.
        delay: u8,
        /// Effective co-op flags.
        coop_flags: u32,
        /// Effective hash interval.
        hash_interval: u16,
        /// Host's WRAM snapshot (empty = keep power-on WRAM), identical on both peers.
        wram: Vec<u8>,
    },
    /// Waiting for the remote pad of `frame`.
    Stalled {
        /// Next frame to step.
        frame: u32,
    },
    /// Stepping again after a stall.
    Resumed {
        /// Frame that just became steppable.
        frame: u32,
        /// How long the wait lasted.
        stalled_ms: u64,
    },
    /// This peer detected a hash mismatch (followed by `Closed(Desync)`).
    Desync {
        /// Frame the hashes were taken after.
        frame: u32,
        /// Local hash.
        local: u64,
        /// Remote hash.
        remote: u64,
    },
    /// The session ended (state Closed, or Desynced for `CloseReason::Desync`).
    Closed(CloseReason),
}

/// Counters for status displays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Next frame to step.
    pub frame: u32,
    /// Remote frames known contiguously from 0.
    pub remote_known: u32,
    /// Local frames the peer acknowledged.
    pub peer_ack: u32,
    /// Latest round-trip time.
    pub rtt_ms: Option<u32>,
    /// Current wait for the remote pad.
    pub stall_ms: u64,
    /// Packets received.
    pub packets_in: u64,
    /// Packets sent.
    pub packets_out: u64,
    /// Reliable-channel input copies sent.
    pub reliable_resends: u64,
    /// Hash comparisons that matched.
    pub hashes_ok: u32,
}

/// The local side of a handshake, checked against the peer's [`Hello`].
/// Shared by the lockstep and rollback sessions.
pub(crate) struct HelloCheck {
    pub(crate) proto: u16,
    pub(crate) role: Role,
    pub(crate) rom_crc32: u32,
    pub(crate) trapset_id: u64,
    pub(crate) coop_flags: u32,
    pub(crate) max_delay: u8,
}

impl HelloCheck {
    /// Ok when the peer may play with us; otherwise the local close reason
    /// and the Bye to send.
    pub(crate) fn check(&self, h: &Hello) -> Result<(), (CloseReason, ByeReason)> {
        let is_rb = |p: u16| p == ROLLBACK_PROTO_VERSION;
        if h.proto != self.proto {
            // A rollback peer meeting a lockstep peer gets a precise reason; the
            // Bye stays VersionMismatch so older peers can decode it.
            let kind = if is_rb(h.proto) != is_rb(self.proto)
                && (h.proto == PROTO_VERSION || is_rb(h.proto))
            {
                MismatchKind::NetMode
            } else {
                MismatchKind::Proto
            };
            return Err((CloseReason::Mismatch(kind), ByeReason::VersionMismatch));
        }
        if h.rom_crc32 != self.rom_crc32 {
            return Err((
                CloseReason::Mismatch(MismatchKind::Rom),
                ByeReason::RomMismatch,
            ));
        }
        if h.trapset_id != self.trapset_id {
            return Err((
                CloseReason::Mismatch(MismatchKind::TrapSet),
                ByeReason::TrapSetMismatch,
            ));
        }
        if h.role == self.role {
            return Err((CloseReason::RoleConflict, ByeReason::RoleConflict));
        }
        let (host_flags, guest_flags) = match self.role {
            Role::Host => (self.coop_flags, h.coop_flags),
            Role::Guest => (h.coop_flags, self.coop_flags),
        };
        if host_flags == 0 || host_flags & !guest_flags != 0 {
            return Err((
                CloseReason::Mismatch(MismatchKind::CoopFlags),
                ByeReason::CoopFlagsMismatch,
            ));
        }
        if self.role == Role::Guest && h.delay > self.max_delay {
            return Err((
                CloseReason::Protocol(format!("host delay {} out of range", h.delay)),
                ByeReason::Protocol,
            ));
        }
        Ok(())
    }
}

/// One peer's lockstep session. See the crate docs for the driving loop.
#[derive(Debug)]
pub struct Session {
    cfg: SessionConfig,
    state: SessionState,
    close_reason: Option<CloseReason>,
    delay: u8,
    coop_flags: u32,
    hash_interval: u16,
    next_frame: u32,
    local: InputLog,
    remote: InputLog,
    /// Peer holds all our frames `< peer_ack`.
    peer_ack: u32,
    /// Last `remote.contiguous()` we told the peer.
    ack_sent: u32,
    ack_dirty: bool,
    peer_hello: Option<Hello>,
    now_ms: u64,
    handshake_since: u64,
    waiting_since: Option<u64>,
    ack_progress_ms: u64,
    last_reliable_input_ms: Option<u64>,
    last_ping_ms: Option<u64>,
    force_reliable: bool,
    local_hashes: BTreeMap<u32, u64>,
    remote_hashes: BTreeMap<u32, u64>,
    outbox: Vec<(Channel, Vec<u8>)>,
    events: Vec<SessionEvent>,
    stats: SessionStats,
}

impl Session {
    /// Validate `cfg` and create an idle session.
    pub fn new(cfg: SessionConfig) -> Result<Session, ConfigError> {
        if cfg.delay > MAX_DELAY {
            return Err(ConfigError::DelayOutOfRange(cfg.delay));
        }
        if cfg.coop_flags == 0 {
            return Err(ConfigError::NoCoopFlags);
        }
        if let Some(w) = &cfg.wram {
            if w.len() != WRAM_LEN {
                return Err(ConfigError::BadWramLen(w.len()));
            }
        }
        Ok(Session {
            state: SessionState::Idle,
            close_reason: None,
            delay: cfg.delay,
            coop_flags: cfg.coop_flags,
            hash_interval: cfg.hash_interval,
            next_frame: 0,
            local: InputLog::default(),
            remote: InputLog::default(),
            peer_ack: 0,
            ack_sent: 0,
            ack_dirty: false,
            peer_hello: None,
            now_ms: 0,
            handshake_since: 0,
            waiting_since: None,
            ack_progress_ms: 0,
            last_reliable_input_ms: None,
            last_ping_ms: None,
            force_reliable: false,
            local_hashes: BTreeMap::new(),
            remote_hashes: BTreeMap::new(),
            outbox: Vec::new(),
            events: Vec::new(),
            stats: SessionStats::default(),
            cfg,
        })
    }

    /// Current state.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Why the session ended (Some in Closed and Desynced).
    pub fn close_reason(&self) -> Option<&CloseReason> {
        self.close_reason.as_ref()
    }

    /// Running or Stalled.
    pub fn is_live(&self) -> bool {
        matches!(self.state, SessionState::Running | SessionState::Stalled)
    }

    /// Own role.
    pub fn role(&self) -> Role {
        self.cfg.role
    }

    /// Effective input delay (host's value once started).
    pub fn delay(&self) -> u8 {
        self.delay
    }

    /// Effective co-op flags (host's value once started).
    pub fn coop_flags(&self) -> u32 {
        self.coop_flags
    }

    /// Effective hash interval.
    pub fn hash_interval(&self) -> u16 {
        self.hash_interval
    }

    /// Next frame [`Session::poll`] will return.
    pub fn frame(&self) -> u32 {
        self.next_frame
    }

    /// Counters.
    pub fn stats(&self) -> SessionStats {
        let mut s = self.stats;
        s.frame = self.next_frame;
        s.remote_known = self.remote.contiguous();
        s.peer_ack = self.peer_ack;
        s
    }

    /// Drain pending events.
    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.events)
    }

    /// Pump the transport: apply link state, handle received packets, run
    /// timers (`now_ms` is the caller's monotonic clock), then send queued
    /// packets. Call at least once per frontend tick; calling it again after
    /// stepping flushes freshly latched input sooner.
    pub fn update<T: Transport + ?Sized>(&mut self, t: &mut T, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        let packets = t.recv();
        let link = t.state();
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
        self.queue_inputs();
        self.flush(t);
    }

    /// Latch `pad` for frame `frame() + delay()` unless already latched.
    /// Returns true when the pad was taken. Call once before each `poll`.
    pub fn latch_local(&mut self, pad: u8) -> bool {
        if !self.is_live() {
            return false;
        }
        let target = self.next_frame + u32::from(self.delay);
        if self.local.contiguous() != target {
            return false;
        }
        if self.local.contiguous() == self.peer_ack {
            // First unacknowledged frame: restart the ack clock.
            self.ack_progress_ms = self.now_ms;
        }
        self.local.insert(target, pad) == Insert::New
    }

    /// The next frame to step as `(frame, pad1, pad2)` when both pads are
    /// known; advances the session. Frames come in order, exactly once.
    pub fn poll(&mut self) -> Option<(u32, u8, u8)> {
        if !self.is_live() {
            return None;
        }
        let f = self.next_frame;
        let local = self.local.get(f)?;
        let Some(remote) = self.remote.get(f) else {
            self.waiting_since.get_or_insert(self.now_ms);
            return None;
        };
        self.next_frame += 1;
        if let Some(since) = self.waiting_since.take() {
            if self.state == SessionState::Stalled {
                self.state = SessionState::Running;
                self.events.push(SessionEvent::Resumed {
                    frame: f,
                    stalled_ms: self.now_ms.saturating_sub(since),
                });
            }
        }
        self.stats.stall_ms = 0;
        self.remote
            .prune_below(self.next_frame.saturating_sub(HISTORY));
        self.local.prune_below(self.peer_ack.min(self.next_frame));
        let keep_from = self
            .next_frame
            .saturating_sub(16 * u32::from(self.hash_interval.max(1)));
        self.local_hashes = self.local_hashes.split_off(&keep_from);
        self.remote_hashes = self.remote_hashes.split_off(&keep_from);
        Some(match self.cfg.role {
            Role::Host => (f, local, remote),
            Role::Guest => (f, remote, local),
        })
    }

    /// Consecutive frames steppable right now (without latching more input).
    pub fn frames_ready(&self) -> u32 {
        if !self.is_live() {
            return 0;
        }
        let mut n = 0;
        while self.local.get(self.next_frame + n).is_some()
            && self.remote.get(self.next_frame + n).is_some()
        {
            n += 1;
        }
        n
    }

    /// Whether the frontend should hash its state after stepping `frame`.
    pub fn needs_hash(&self, frame: u32) -> bool {
        self.hash_interval != 0 && (frame + 1).is_multiple_of(u32::from(self.hash_interval))
    }

    /// Report the state hash taken after stepping `frame` (a frame already polled).
    pub fn report_hash(&mut self, frame: u32, hash: u64) {
        if !self.is_live() || frame >= self.next_frame {
            return;
        }
        self.local_hashes.insert(frame, hash);
        self.queue(Channel::Reliable, &Message::Hash { frame, hash });
        self.compare_hashes(frame);
    }

    /// Leave: queue Bye(Quit) and close. Call `update` once more to send it.
    pub fn close(&mut self) {
        self.fail(CloseReason::LocalQuit, Some(ByeReason::Quit));
    }

    // ---- internals -------------------------------------------------------

    fn queue(&mut self, channel: Channel, msg: &Message) {
        self.outbox.push((channel, msg.encode()));
    }

    fn fail(&mut self, reason: CloseReason, bye: Option<ByeReason>) {
        if matches!(self.state, SessionState::Closed | SessionState::Desynced) {
            return;
        }
        if let Some(b) = bye {
            self.queue(Channel::Reliable, &Message::Bye(b));
        }
        self.state = if reason == CloseReason::Desync {
            SessionState::Desynced
        } else {
            SessionState::Closed
        };
        self.close_reason = Some(reason.clone());
        self.events.push(SessionEvent::Closed(reason));
    }

    fn on_link_up(&mut self) {
        if self.state != SessionState::Idle {
            return;
        }
        self.state = SessionState::Handshake;
        self.handshake_since = self.now_ms;
        let hello = Hello {
            proto: PROTO_VERSION,
            role: self.cfg.role,
            rom_crc32: self.cfg.rom_crc32,
            trapset_id: self.cfg.trapset_id,
            coop_flags: self.cfg.coop_flags,
            delay: self.cfg.delay,
            hash_interval: self.cfg.hash_interval,
        };
        self.queue(Channel::Reliable, &Message::Hello(hello));
    }

    fn on_packet(&mut self, _channel: Channel, bytes: &[u8]) {
        if matches!(self.state, SessionState::Closed | SessionState::Desynced) {
            return;
        }
        self.on_link_up();
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
                if self.is_live() {
                    self.remote_hashes.insert(frame, hash);
                    self.compare_hashes(frame);
                }
            }
            Message::Status { .. } => {}
            Message::Ping { token } => self.queue(Channel::Unreliable, &Message::Pong { token }),
            Message::Pong { token } => {
                self.stats.rtt_ms = Some((self.now_ms as u32).wrapping_sub(token));
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
        if self.state != SessionState::Handshake || self.peer_hello.is_some() {
            return;
        }
        let local = HelloCheck {
            proto: PROTO_VERSION,
            role: self.cfg.role,
            rom_crc32: self.cfg.rom_crc32,
            trapset_id: self.cfg.trapset_id,
            coop_flags: self.cfg.coop_flags,
            max_delay: MAX_DELAY,
        };
        match local.check(&h) {
            Ok(()) => self.accept_hello(h),
            Err((reason, bye)) => self.fail(reason, Some(bye)),
        }
    }

    fn accept_hello(&mut self, h: Hello) {
        self.events.push(SessionEvent::PeerHello {
            role: h.role,
            delay: h.delay,
            coop_flags: h.coop_flags,
        });
        self.peer_hello = Some(h);
        if self.cfg.role == Role::Host {
            let start = Start {
                frame: 0,
                delay: self.delay,
                coop_flags: self.coop_flags,
                hash_interval: self.hash_interval,
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
        if self.state != SessionState::Handshake {
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
        } else if s.delay > MAX_DELAY {
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
        self.hash_interval = s.hash_interval;
        self.begin(s.wram);
    }

    fn begin(&mut self, wram: Vec<u8>) {
        for f in 0..u32::from(self.delay) {
            self.local.insert(f, 0);
            self.remote.insert(f, 0);
        }
        self.peer_ack = u32::from(self.delay);
        self.ack_sent = u32::from(self.delay);
        self.next_frame = 0;
        self.ack_progress_ms = self.now_ms;
        self.state = SessionState::Running;
        self.events.push(SessionEvent::Started {
            delay: self.delay,
            coop_flags: self.coop_flags,
            hash_interval: self.hash_interval,
            wram,
        });
    }

    fn on_input(&mut self, ack: u32, first: u32, pads: &[u8]) {
        if !self.is_live() {
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
            if self.remote.insert(frame, pad) == Insert::Conflict {
                self.fail(
                    CloseReason::Protocol(format!("remote input for frame {frame} rewritten")),
                    Some(ByeReason::Protocol),
                );
                return;
            }
        }
    }

    fn compare_hashes(&mut self, frame: u32) {
        let (Some(&local), Some(&remote)) = (
            self.local_hashes.get(&frame),
            self.remote_hashes.get(&frame),
        ) else {
            return;
        };
        self.local_hashes.remove(&frame);
        self.remote_hashes.remove(&frame);
        if local == remote {
            self.stats.hashes_ok += 1;
        } else {
            self.events.push(SessionEvent::Desync {
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
            SessionState::Handshake
                if now.saturating_sub(self.handshake_since) >= self.cfg.handshake_timeout_ms =>
            {
                self.fail(CloseReason::Timeout, Some(ByeReason::Timeout));
            }
            SessionState::Running | SessionState::Stalled => {
                if let Some(since) = self.waiting_since {
                    let waited = now.saturating_sub(since);
                    self.stats.stall_ms = waited;
                    if waited >= self.cfg.stall_timeout_ms {
                        self.fail(CloseReason::Timeout, Some(ByeReason::Timeout));
                        return;
                    }
                    if waited >= STALL_NOTIFY_MS && self.state == SessionState::Running {
                        self.state = SessionState::Stalled;
                        self.force_reliable = true;
                        self.events.push(SessionEvent::Stalled {
                            frame: self.next_frame,
                        });
                    }
                }
                if self
                    .last_ping_ms
                    .is_none_or(|t| now.saturating_sub(t) >= PING_INTERVAL_MS)
                {
                    self.last_ping_ms = Some(now);
                    self.queue(Channel::Unreliable, &Message::Ping { token: now as u32 });
                }
            }
            _ => {}
        }
    }

    fn queue_inputs(&mut self) {
        if !self.is_live() {
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
            if (self.force_reliable || overdue) && spaced {
                self.queue(Channel::Reliable, &msg);
                self.last_reliable_input_ms = Some(now);
                self.stats.reliable_resends += 1;
                self.force_reliable = false;
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

    fn flush<T: Transport + ?Sized>(&mut self, t: &mut T) {
        for (channel, bytes) in std::mem::take(&mut self.outbox) {
            match t.send(channel, &bytes) {
                Ok(()) => self.stats.packets_out += 1,
                Err(e) => {
                    if channel == Channel::Reliable {
                        self.fail(CloseReason::Transport(e.to_string()), None);
                    }
                }
            }
        }
        // A failure above may have queued nothing sendable; drop leftovers.
        self.outbox.clear();
    }
}
