//! Netplay glue: drive a `z2-net` lockstep [`Session`] or rollback
//! [`RollbackSession`] over any [`Transport`] and step a `Game` with its pads.
//!
//! Rollback (the default online mode) is driven by [`rollback_tick`]: saves go
//! into a preallocated [`GameStateRing`], replayed frames are stepped silently,
//! and the frontend presents once per tick. Lockstep keeps the tick contract
//! below.
//!
//! The protocol core (`z2-net` with default features) has no dependencies, so
//! this module is **always compiled**: the session driver, the window-title
//! suffix and the save-state policy are unit-testable (and the loopback
//! integration test drives two real `Game`s through it) without pulling
//! WebRTC. Only [`connect_matchbox`] — the actual internet transport — sits
//! behind the `netplay` cargo feature.
//!
//! # Tick contract
//!
//! Per frontend tick, in this order:
//!
//! 1. [`NetLink::update`] — receive, run timers, send.
//! 2. [`NetLink::take_events`] — apply `Started` (rebuild the game), mute on
//!    `Stalled`, unmute on `Resumed`, surface `Closed`.
//! 3. [`step_session`] — for each frame due, latch the local pad, poll the
//!    confirmed pair and `Game::step2` it, reporting a state hash when asked.
//! 4. [`NetLink::update`] again, to flush the pads just latched.
//!
//! The local pad is sampled **once per tick** by the caller and reused for
//! every frame stepped in that tick (the same semantics the single-player
//! loop already has); only the bytes travel, so both peers stay identical.

use crate::app::{drain_audio_frame, emu_from_rom_body_with, Emu, Features};
use crate::audio::SharedAudio;
use z2_core::game::Game;
use z2_core::state::GameState;
use z2_net::{
    hash_state, ChecksumSink, CloseReason, ConnectStage, MissingState, Role, RollbackConfig,
    RollbackError, RollbackEvent, RollbackGame, RollbackRequest, RollbackSession, RollbackState,
    RollbackStats, Session, SessionConfig, SessionEvent, SessionState, SessionStats, Transport,
    COOP_TWO_LINKS, MAX_ROLLBACK_DELAY,
};

/// Milliseconds to keep pumping the transport after [`NetLink::close`] so the
/// goodbye actually leaves the machine before the process exits.
pub const GOODBYE_FLUSH_MS: u64 = 200;

/// A live lockstep session plus its transport.
///
/// Generic over the transport so tests can drive it over
/// `z2_net::LoopbackTransport` while the app uses
/// `z2_net::MatchboxTransport`.
pub struct NetLink<T: Transport> {
    /// Protocol state machine.
    pub session: Session,
    /// Packet pipe.
    pub transport: T,
    /// Our role: host = player 1 and camera owner, guest = player 2.
    pub role: Role,
    /// Room name (for the status line; never the full URL, which may carry
    /// nothing secret today but is not the user's business in a title bar).
    pub room: String,
    /// Set once `SessionEvent::Started` has been applied by the frontend.
    pub started: bool,
    /// Audio is muted because the peer is stalled.
    pub muted: bool,
    /// Effective input delay (the host's value once `Started` arrives).
    pub delay: u8,
    /// Delay this peer asked for — differs from [`Self::delay`] on a guest
    /// whose request the host overrode (reported once, so the user knows why
    /// their setting did nothing).
    pub requested_delay: u8,
    /// Last transport/session error text, for the title and stderr.
    pub last_error: Option<String>,
    /// Last connect stage written to stderr (so each is logged once).
    pub reported_stage: Option<ConnectStage>,
}

impl<T: Transport> NetLink<T> {
    /// Wrap a fresh session around `transport`.
    pub fn new(cfg: SessionConfig, transport: T, room: &str) -> Result<Self, String> {
        let role = cfg.role;
        let requested_delay = cfg.delay;
        let session = Session::new(cfg).map_err(|e| e.to_string())?;
        Ok(Self {
            session,
            transport,
            role,
            room: room.to_string(),
            started: false,
            muted: false,
            delay: requested_delay,
            requested_delay,
            last_error: None,
            reported_stage: None,
        })
    }

    /// Connection progress of the transport (status line).
    #[must_use]
    pub fn stage(&self) -> ConnectStage {
        self.transport.stage()
    }

    /// Log each connect stage once, as it is reached.
    pub fn report_stage(&mut self) {
        report_stage_once(&mut self.reported_stage, self.transport.stage());
    }

    /// Step 1/4 of the tick contract: receive, timers, send.
    pub fn update(&mut self, now_ms: u64) {
        self.session.update(&mut self.transport, now_ms);
    }

    /// Step 2: drain protocol events for the frontend to act on.
    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        self.session.take_events()
    }

    /// Coarse state for the status line.
    #[must_use]
    pub fn state(&self) -> SessionState {
        self.session.state()
    }

    /// Counters for the status line.
    #[must_use]
    pub fn stats(&self) -> SessionStats {
        self.session.stats()
    }

    /// Why the session ended, if it has.
    #[must_use]
    pub fn close_reason(&self) -> Option<&CloseReason> {
        self.session.close_reason()
    }

    /// True while the session can still step frames.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.session.is_live()
    }

    /// Whether the host owns the camera here (host = player 1).
    #[must_use]
    pub fn is_host(&self) -> bool {
        self.role == Role::Host
    }

    /// Announce the quit and flush it: `close()` then keep pumping for
    /// [`GOODBYE_FLUSH_MS`] so the Bye leaves before the process exits.
    /// Without this the peer only learns of the quit through the signalling
    /// server, or (if that is gone) an ICE timeout tens of seconds later.
    pub fn close_and_flush(&mut self, now_ms: u64) {
        self.session.close();
        let mut t = 0;
        while t <= GOODBYE_FLUSH_MS {
            self.session.update(&mut self.transport, now_ms + t);
            std::thread::sleep(std::time::Duration::from_millis(10));
            t += 10;
        }
    }
}

/// What [`step_session`] did this tick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepOutcome {
    /// Lockstep frames stepped (0 = waiting for the peer).
    pub stepped: u32,
    /// State hashes reported.
    pub hashed: u32,
}

/// Step at most `max_frames` confirmed frames.
///
/// `local_pad` is this tick's local pad byte, latched once per stepped frame
/// (see the module docs). `step` is called as `step(pad1, pad2, want_hash)`:
/// it must advance the game one frame and, when `want_hash` is true, return
/// `Some(state_hash)` for that frame. Returns early the moment the peer's pad
/// for the next frame is missing — that is the lockstep wait, not an error.
///
/// Taking one callback rather than a `&mut Game` keeps this function free of
/// any game type, so the same code path serves the windowed loop (which also
/// has to drain audio per frame) and the loopback tests.
pub fn step_session<T: Transport>(
    link: &mut NetLink<T>,
    max_frames: u32,
    local_pad: u8,
    mut step: impl FnMut(u8, u8, bool) -> Option<u64>,
) -> StepOutcome {
    let mut out = StepOutcome::default();
    if !link.started || !link.is_live() {
        return out;
    }
    for _ in 0..max_frames {
        link.session.latch_local(local_pad);
        let Some((frame, pad1, pad2)) = link.session.poll() else {
            break;
        };
        let want = link.session.needs_hash(frame);
        let hash = step(pad1, pad2, want);
        out.stepped += 1;
        if let Some(h) = hash {
            link.session.report_hash(frame, h);
            out.hashed += 1;
        }
    }
    out
}

/// The desync hash both peers compare: RAM + battery WRAM + OAM + the co-op
/// state that influences future frames.
///
/// Including `coop_hash` catches a P2 divergence that has not yet reached RAM
/// (the second Link's parked block lives outside the 2 KiB mirror while it is
/// swapped out).
#[must_use]
pub fn state_hash(game: &Game) -> u64 {
    hash_state(&[
        game.ram(),
        game.wram(),
        game.oam(),
        &game.coop_hash().to_le_bytes(),
    ])
}

/// Why a save-state action is refused, or `None` when it is allowed.
///
/// Rewinding one peer desyncs both — there is no rollback in this protocol —
/// so F5/F7 are blocked for the whole lifetime of a session, not just while
/// it is running.
#[must_use]
pub fn savestate_blocked(net_active: bool) -> Option<&'static str> {
    if net_active {
        Some("save states are disabled during netplay (rewinding one peer desyncs both)")
    } else {
        None
    }
}

/// Write `stage` to stderr the first time it is reached (shared by the
/// lockstep and rollback links).
pub fn report_stage_once(reported: &mut Option<ConnectStage>, stage: ConnectStage) {
    if *reported != Some(stage) {
        *reported = Some(stage);
        if stage != ConnectStage::Ended {
            eprintln!("netplay: {}", stage.describe());
        }
    }
}

/// Status-line text for a session that is not running yet: the connect
/// stage, numbered out of four (shared by both modes' title suffixes).
#[must_use]
pub fn connect_stage_label(stage: ConnectStage, room: &str) -> String {
    match stage {
        ConnectStage::Signalling => "1/4 connecting to signal server".to_string(),
        ConnectStage::WaitingForPeer => format!("2/4 waiting for the other player in {room}"),
        ConnectStage::EstablishingLink => "3/4 establishing peer link".to_string(),
        ConnectStage::Connected | ConnectStage::Ended => "4/4 connected, starting".to_string(),
    }
}

/// Window-title suffix for a session. Pure, like `app::audio_suffix`.
#[must_use]
pub fn netplay_suffix(
    role: Role,
    stage: ConnectStage,
    state: SessionState,
    reason: Option<&CloseReason>,
    stats: &SessionStats,
    delay: u8,
    room: &str,
) -> String {
    let who = match role {
        Role::Host => "host",
        Role::Guest => "guest",
    };
    match state {
        SessionState::Idle => format!(" [NET {who}: {}]", connect_stage_label(stage, room)),
        SessionState::Handshake => format!(
            " [NET {who}: {}]",
            connect_stage_label(ConnectStage::Connected, room)
        ),
        SessionState::Running => {
            let ahead = i64::from(stats.remote_known) - i64::from(stats.frame);
            let rtt = match stats.rtt_ms {
                Some(ms) => format!(" rtt={ms}ms"),
                None => String::new(),
            };
            format!(
                " [NET {who}: running f={} d={delay} ahead={ahead}{rtt}]",
                stats.frame
            )
        }
        SessionState::Stalled => format!(
            " [NET {who}: STALLED {:.1} s]",
            stats.stall_ms as f64 / 1000.0
        ),
        SessionState::Desynced => format!(" [NET {who}: DESYNC at f={}]", stats.frame),
        SessionState::Closed => match reason {
            Some(r) => format!(" [NET {who}: closed ({r})]"),
            None => format!(" [NET {who}: closed]"),
        },
    }
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

/// Checksum of a saved game state for rollback desync detection: FNV-1a over
/// the whole versioned [`GameState::to_bytes`] image (RAM, WRAM, OAM, palette,
/// framebuffer, CPU, mapper, PPU and co-op state).
///
/// Taken only on the frames the session asks for (every `check_interval`
/// frames), so the one image allocation it makes is not per frame.
#[must_use]
pub fn rollback_checksum(state: &GameState) -> u64 {
    hash_state(&[&state.to_bytes()])
}

/// The real game as a [`RollbackGame`], for `z2_net::apply_requests` and
/// `SyncTestSession` runs. Frames advanced through this impl never reach the
/// synth: their APU register writes are discarded.
///
/// `save` allocates a [`GameState`]; the windowed loop uses
/// [`GameStateRing`] and [`apply_rollback_requests`] instead, which do not.
impl RollbackGame for Emu {
    type State = GameState;

    fn save(&self) -> GameState {
        self.game.save_state()
    }

    fn load(&mut self, state: &GameState) {
        self.game.load_state(state);
    }

    fn advance(&mut self, pad1: u8, pad2: u8) {
        self.game.step2(pad1, pad2);
        self.game.apu.log.clear();
    }

    fn checksum(&self) -> u64 {
        rollback_checksum(&self.game.save_state())
    }
}

/// Preallocated ring of [`GameState`]s keyed by frame. Saving copies into an
/// existing slot, so a running session never allocates for save states.
pub struct GameStateRing {
    slots: Vec<(Option<u32>, GameState)>,
}

impl GameStateRing {
    /// Ring with `capacity` slots (at least 1). A rollback session needs
    /// `max_prediction + 2`; a sync test needs `check_distance + 2`.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity.max(1))
                .map(|_| (None, GameState::new()))
                .collect(),
        }
    }

    /// Number of slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Save `game` as the state at `frame` and return the stored copy.
    pub fn save_from(&mut self, frame: u32, game: &Game) -> &GameState {
        let i = frame as usize % self.slots.len();
        let slot = &mut self.slots[i];
        game.save_state_into(&mut slot.1);
        slot.0 = Some(frame);
        &slot.1
    }

    /// The state saved for `frame`, if its slot still holds it.
    #[must_use]
    pub fn get(&self, frame: u32) -> Option<&GameState> {
        match &self.slots[frame as usize % self.slots.len()] {
            (Some(f), s) if *f == frame => Some(s),
            _ => None,
        }
    }
}

/// What [`apply_rollback_requests`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RollbackApplied {
    /// New frames simulated (presented, audio kept).
    pub advanced: u32,
    /// Frames re-simulated after a rollback (audio dropped).
    pub replayed: u32,
    /// Frame restored by this tick's `LoadState`, if a rollback ran.
    pub loaded_from: Option<u32>,
}

/// Execute one tick's requests in order: save into `ring` (reporting the
/// saved state's checksum when `sink` asks), load, and step with
/// `Game::step2`.
///
/// Audio: a normal frame's APU register writes are drained into the synth and
/// its PCM pushed to `audio`; a `replay` frame's writes are cleared unheard
/// (that frame was already played once). `pcm` is a reused scratch buffer.
pub fn apply_rollback_requests<K: ChecksumSink + ?Sized>(
    requests: &[RollbackRequest],
    emu: &mut Emu,
    ring: &mut GameStateRing,
    sink: &mut K,
    pcm: &mut Vec<i16>,
    audio: Option<&SharedAudio>,
) -> Result<RollbackApplied, MissingState> {
    let mut out = RollbackApplied::default();
    for r in requests {
        match *r {
            RollbackRequest::SaveState { frame } => {
                let saved = ring.save_from(frame, &emu.game);
                if sink.needs_checksum(frame) {
                    sink.report_checksum(frame, rollback_checksum(saved));
                }
            }
            RollbackRequest::LoadState { frame } => {
                let state = ring.get(frame).ok_or(MissingState(frame))?;
                emu.game.load_state(state);
                out.loaded_from = Some(frame);
            }
            RollbackRequest::Advance {
                pad1, pad2, replay, ..
            } => {
                emu.game.step2(pad1, pad2);
                if replay {
                    emu.game.apu.log.clear();
                    out.replayed += 1;
                } else {
                    drain_audio_frame(emu, pcm, audio);
                    out.advanced += 1;
                }
            }
        }
    }
    Ok(out)
}

/// A live rollback session, its preallocated state ring and the per-tick
/// bookkeeping the frontend needs.
pub struct RollbackLink<T: Transport> {
    /// Protocol state machine (owns the transport).
    pub session: RollbackSession<T>,
    /// Saved states, `max_prediction + 2` slots.
    pub ring: GameStateRing,
    /// Room name for the status line.
    pub room: String,
    /// Set once `RollbackEvent::Running` has rebuilt the game.
    pub started: bool,
    /// Delay this peer asked for (the host's value wins).
    pub requested_delay: u8,
    /// Ticks still to skip for a `WaitRecommendation`.
    pub skip_ticks: u32,
    /// The peer has been silent past the interrupt threshold.
    pub interrupted: bool,
    /// Last error text, for stderr.
    pub last_error: Option<String>,
    /// Last connect stage written to stderr (so each is logged once).
    pub reported_stage: Option<ConnectStage>,
    pcm: Vec<i16>,
}

impl<T: Transport> RollbackLink<T> {
    /// Wrap a fresh rollback session around `transport`.
    pub fn new(cfg: RollbackConfig, transport: T, room: &str) -> Result<Self, String> {
        let requested_delay = cfg.input_delay;
        let ring = GameStateRing::new(usize::from(cfg.max_prediction) + 2);
        let session = RollbackSession::new(cfg, transport).map_err(|e| e.to_string())?;
        Ok(Self {
            session,
            ring,
            room: room.to_string(),
            started: false,
            requested_delay,
            skip_ticks: 0,
            interrupted: false,
            last_error: None,
            reported_stage: None,
            pcm: Vec::new(),
        })
    }

    /// Connection progress of the transport (status line).
    #[must_use]
    pub fn stage(&self) -> ConnectStage {
        self.session.transport().stage()
    }

    /// Log each connect stage once, as it is reached.
    pub fn report_stage(&mut self) {
        report_stage_once(&mut self.reported_stage, self.session.transport().stage());
    }

    /// Whether this peer is the host (player 1, camera owner).
    #[must_use]
    pub fn is_host(&self) -> bool {
        self.session.role() == Role::Host
    }

    /// Drain protocol events for the frontend.
    pub fn take_events(&mut self) -> Vec<RollbackEvent> {
        self.session.events()
    }

    /// Announce the quit and keep pumping for [`GOODBYE_FLUSH_MS`].
    pub fn close_and_flush(&mut self, now_ms: u64) {
        self.session.close();
        let mut t = 0;
        while t <= GOODBYE_FLUSH_MS {
            self.session.poll(now_ms + t);
            std::thread::sleep(std::time::Duration::from_millis(10));
            t += 10;
        }
    }
}

/// One rollback frontend tick:
///
/// * while a `WaitRecommendation` is being honoured, only poll the network;
/// * otherwise add the local pad, `advance`, and run every request through
///   [`apply_rollback_requests`] (saves into the preallocated ring, replayed
///   frames silent).
///
/// The caller drains [`RollbackLink::take_events`] right after this, before
/// the next tick (a `Running` event must rebuild the game before frame 0 is
/// stepped), and presents once after the tick.
pub fn rollback_tick<T: Transport>(
    link: &mut RollbackLink<T>,
    emu: &mut Emu,
    local_pad: u8,
    now_ms: u64,
    audio: Option<&SharedAudio>,
) -> RollbackApplied {
    if link.skip_ticks > 0 && link.session.is_running() {
        link.skip_ticks -= 1;
        link.session.poll(now_ms);
        return RollbackApplied::default();
    }
    // Until the frontend has applied `Running` (rebuilt the game), only pump
    // the network: the session may already be running if an earlier `poll`
    // received the start, and stepping frame 0 on the old game would desync.
    if link.session.state() == RollbackState::Closed || !link.started {
        link.session.poll(now_ms);
        return RollbackApplied::default();
    }
    link.session.add_local_input(local_pad);
    let requests = match link.session.advance(now_ms) {
        Ok(r) => r,
        // The reason also arrives as `RollbackEvent::Disconnected`.
        Err(RollbackError::Closed(_)) => return RollbackApplied::default(),
        Err(e) => {
            link.last_error = Some(e.to_string());
            return RollbackApplied::default();
        }
    };
    let RollbackLink {
        session, ring, pcm, ..
    } = link;
    match apply_rollback_requests(&requests, emu, ring, session, pcm, audio) {
        Ok(applied) => applied,
        Err(e) => {
            link.last_error = Some(e.to_string());
            link.session.close();
            RollbackApplied::default()
        }
    }
}

/// Effective rollback input delay: `--net-delay` wins over the config value,
/// and either must be within `0..=MAX_ROLLBACK_DELAY`.
pub fn rollback_delay(cli: Option<u8>, config: u8) -> Result<u8, String> {
    let (d, from) = match cli {
        Some(d) => (d, "--net-delay"),
        None => (config, "netplay.input_delay in the config"),
    };
    if d > MAX_ROLLBACK_DELAY {
        return Err(format!(
            "netplay: {from} is {d}, but rollback accepts 0-{MAX_ROLLBACK_DELAY} \
             (lower it, or use --net-mode lockstep)"
        ));
    }
    Ok(d)
}

/// Build the game a rollback session starts from on `Running`: power-on with
/// the session's co-op flags (and this peer's wide-gameplay margin, which the
/// handshake already matched through the trap-set identity), then the host's
/// WRAM when one was sent. Same construction as the lockstep `Started` path.
pub fn session_emu(
    body: &[u8],
    audio_rate: u32,
    record: bool,
    wide_gameplay: Option<u8>,
    coop_flags: u32,
    wram: &[u8],
) -> Result<Emu, String> {
    let feats = Features {
        coop: coop_flags & COOP_TWO_LINKS != 0,
        wide_gameplay,
        record,
        margin_sprites: false,
    };
    let mut emu = emu_from_rom_body_with(body, audio_rate, feats)?;
    if wram.len() == emu.game.wram.len() {
        emu.game.wram.copy_from_slice(wram);
        emu.game.coop_reset_area();
    }
    Ok(emu)
}

/// Window-title suffix for a rollback session: mode, round-trip time,
/// rollbacks and prediction depth (current/worst).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn rollback_suffix(
    role: Role,
    stage: ConnectStage,
    state: RollbackState,
    reason: Option<&CloseReason>,
    stats: &RollbackStats,
    delay: u8,
    interrupted: bool,
    room: &str,
) -> String {
    let who = match role {
        Role::Host => "host",
        Role::Guest => "guest",
    };
    match state {
        RollbackState::Idle => {
            format!(
                " [NET rollback {who}: {}]",
                connect_stage_label(stage, room)
            )
        }
        RollbackState::Synchronizing => format!(
            " [NET rollback {who}: {}]",
            connect_stage_label(ConnectStage::Connected, room)
        ),
        RollbackState::Running => {
            let rtt = match stats.rtt_ms {
                Some(ms) => format!("{ms}ms"),
                None => "-".to_string(),
            };
            let net = if interrupted { " INTERRUPTED" } else { "" };
            format!(
                " [NET rollback {who}: f={} d={delay} rtt={rtt} rollbacks={} pred={}/{}{net}]",
                stats.frame, stats.rollbacks, stats.prediction_depth, stats.max_prediction_depth
            )
        }
        RollbackState::Closed => match reason {
            Some(r) => format!(" [NET rollback {who}: closed ({r})]"),
            None => format!(" [NET rollback {who}: closed]"),
        },
    }
}

/// Open the WebRTC transport for `room_url` (feature `netplay`).
///
/// Never blocks and never fails eagerly: an unreachable server or a full room
/// surfaces later as a session close, which the title reports.
#[cfg(feature = "netplay")]
pub fn connect_matchbox(
    room_url: &str,
    ice: Option<z2_net::IceConfig>,
) -> z2_net::MatchboxTransport {
    z2_net::MatchboxTransport::connect(room_url, ice)
}

/// The ICE servers for a session: `--ice` if given, else the config's
/// `ice_url` (same text form, see `z2_net::ice`), else the default STUN pair.
///
/// `ice_username` / `ice_credential` from the config fill in TURN credentials
/// the text itself does not carry. Credentials are never logged.
pub fn ice_setting(
    flag: Option<&str>,
    cfg: &crate::config::NetplayConfig,
) -> Result<z2_net::IceConfig, z2_net::IceParseError> {
    let Some(spec) = flag.or(cfg.ice_url.as_deref()) else {
        return Ok(z2_net::IceConfig::default());
    };
    let mut spec = spec.to_string();
    if !spec.to_ascii_lowercase().contains("turn") {
        return z2_net::IceConfig::parse(&spec);
    }
    if !spec.contains("username=") {
        if let Some(u) = &cfg.ice_username {
            spec.push_str(&format!(" username={u}"));
        }
    }
    if !spec.contains("credential=") {
        if let Some(c) = &cfg.ice_credential {
            spec.push_str(&format!(" credential={c}"));
        }
    }
    z2_net::IceConfig::parse(&spec)
}

/// True when this build can open an online session.
#[must_use]
pub const fn supported() -> bool {
    cfg!(feature = "netplay")
}

/// Error text for `--coop-host/--coop-join` in a build without the transport.
pub const NO_TRANSPORT: &str =
    "this build has no netplay transport: rebuild without --no-default-features \
     (feature `netplay`) to use --coop-host/--coop-join";

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(frame: u32, remote: u32, stall: u64) -> SessionStats {
        SessionStats {
            frame,
            remote_known: remote,
            stall_ms: stall,
            ..SessionStats::default()
        }
    }

    #[test]
    fn savestates_are_refused_only_during_a_session() {
        assert!(savestate_blocked(false).is_none(), "solo play saves freely");
        let msg = savestate_blocked(true).expect("netplay blocks save states");
        assert!(msg.contains("desync"), "says why: {msg}");
    }

    #[test]
    fn suffix_reports_every_session_state() {
        let s = stats(1234, 1234, 0);
        let running = netplay_suffix(
            Role::Host,
            ConnectStage::Connected,
            SessionState::Running,
            None,
            &s,
            2,
            "z2-abc",
        );
        assert!(running.contains("NET host"), "{running}");
        assert!(running.contains("f=1234"), "{running}");
        assert!(running.contains("d=2"), "{running}");

        let waiting = netplay_suffix(
            Role::Guest,
            ConnectStage::WaitingForPeer,
            SessionState::Idle,
            None,
            &s,
            2,
            "z2-abc",
        );
        assert!(
            waiting.contains("2/4 waiting for the other player"),
            "{waiting}"
        );
        assert!(waiting.contains("z2-abc"), "{waiting}");
        for (stage, want) in [
            (ConnectStage::Signalling, "1/4 connecting to signal server"),
            (ConnectStage::EstablishingLink, "3/4 establishing peer link"),
            (ConnectStage::Connected, "4/4 connected"),
        ] {
            let t = netplay_suffix(Role::Host, stage, SessionState::Idle, None, &s, 2, "r");
            assert!(t.contains(want), "{t}");
        }

        let stalled = netplay_suffix(
            Role::Guest,
            ConnectStage::Connected,
            SessionState::Stalled,
            None,
            &stats(10, 10, 1200),
            2,
            "r",
        );
        assert!(stalled.contains("STALLED 1.2 s"), "{stalled}");

        let desync = netplay_suffix(
            Role::Host,
            ConnectStage::Connected,
            SessionState::Desynced,
            None,
            &s,
            2,
            "r",
        );
        assert!(desync.contains("DESYNC at f=1234"), "{desync}");

        let closed = netplay_suffix(
            Role::Host,
            ConnectStage::Ended,
            SessionState::Closed,
            Some(&CloseReason::PeerQuit),
            &s,
            2,
            "r",
        );
        assert!(closed.contains("closed (peer quit)"), "{closed}");
    }

    #[test]
    fn suffix_shows_how_far_ahead_the_peer_is() {
        let s = stats(100, 104, 0);
        let t = netplay_suffix(
            Role::Host,
            ConnectStage::Connected,
            SessionState::Running,
            None,
            &s,
            3,
            "r",
        );
        assert!(t.contains("ahead=4"), "{t}");
    }

    #[test]
    fn rollback_suffix_shows_mode_rtt_rollbacks_and_prediction() {
        let s = RollbackStats {
            frame: 900,
            rtt_ms: Some(48),
            rollbacks: 17,
            prediction_depth: 3,
            max_prediction_depth: 7,
            ..RollbackStats::default()
        };
        let t = rollback_suffix(
            Role::Guest,
            ConnectStage::Connected,
            RollbackState::Running,
            None,
            &s,
            2,
            false,
            "r",
        );
        for needle in [
            "rollback",
            "guest",
            "f=900",
            "d=2",
            "rtt=48ms",
            "rollbacks=17",
            "pred=3/7",
        ] {
            assert!(t.contains(needle), "{needle} missing from {t}");
        }
        assert!(!t.contains("INTERRUPTED"), "{t}");
        let t = rollback_suffix(
            Role::Host,
            ConnectStage::Connected,
            RollbackState::Running,
            None,
            &s,
            1,
            true,
            "r",
        );
        assert!(t.contains("INTERRUPTED"), "{t}");
        let t = rollback_suffix(
            Role::Host,
            ConnectStage::WaitingForPeer,
            RollbackState::Idle,
            None,
            &s,
            1,
            false,
            "z2-abc",
        );
        assert!(
            t.contains("2/4 waiting for the other player in z2-abc"),
            "{t}"
        );
        let t = rollback_suffix(
            Role::Host,
            ConnectStage::Ended,
            RollbackState::Closed,
            Some(&CloseReason::Desync),
            &s,
            1,
            false,
            "r",
        );
        assert!(t.contains("closed ("), "{t}");
    }

    #[test]
    fn rollback_delay_prefers_the_flag_and_enforces_the_range() {
        assert_eq!(rollback_delay(None, 2), Ok(2));
        assert_eq!(rollback_delay(Some(0), 2), Ok(0));
        assert_eq!(rollback_delay(Some(3), 8), Ok(3));
        let e = rollback_delay(None, 5).expect_err("config above 3");
        assert!(e.contains("config") && e.contains("lockstep"), "{e}");
        let e = rollback_delay(Some(4), 2).expect_err("flag above 3");
        assert!(e.contains("--net-delay"), "{e}");
    }

    #[test]
    fn state_ring_keys_slots_by_frame() {
        let game = Game::new();
        let mut ring = GameStateRing::new(3);
        assert_eq!(ring.capacity(), 3);
        assert!(ring.get(0).is_none());
        ring.save_from(4, &game);
        assert!(ring.get(4).is_some());
        assert!(ring.get(1).is_none(), "same slot, different frame");
        ring.save_from(7, &game);
        assert!(ring.get(4).is_none(), "overwritten by frame 7");
        assert!(ring.get(7).is_some());
    }

    #[test]
    fn ice_setting_prefers_the_flag_then_the_config_then_the_default() {
        let mut cfg = crate::config::NetplayConfig::default();
        assert_eq!(
            ice_setting(None, &cfg).unwrap(),
            z2_net::IceConfig::default()
        );
        cfg.ice_url = Some("turn:t.example:3478".into());
        cfg.ice_username = Some("u".into());
        cfg.ice_credential = Some("p".into());
        let from_cfg = ice_setting(None, &cfg).unwrap();
        assert_eq!(from_cfg.urls, vec!["turn:t.example:3478"]);
        assert_eq!(from_cfg.credential.as_deref(), Some("p"));
        assert!(ice_setting(Some("none"), &cfg).unwrap().is_none());
        assert!(ice_setting(Some("http://bad"), &cfg).is_err());
    }

    #[test]
    fn transport_support_matches_the_feature() {
        assert_eq!(supported(), cfg!(feature = "netplay"));
        assert!(NO_TRANSPORT.contains("netplay"));
    }
}
