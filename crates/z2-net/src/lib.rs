//! z2-net: deterministic netplay for two-player co-op, as input-delay
//! lockstep ([`Session`]) or rollback ([`RollbackSession`]).
//!
//! The crate is transport-agnostic and has no dependencies in its default
//! configuration (host + wasm32). A [`Session`] runs the handshake, carries
//! both players' pads, detects stalls and desyncs, and hands the frontend the
//! frames that are safe to step: in order, never skipping, never duplicating.
//! The optional `matchbox` feature adds [`MatchboxTransport`] (WebRTC).
//!
//! # Lockstep model
//!
//! * Host = player 1 (`pad1`), guest = player 2 (`pad2`), on both peers.
//! * Input delay `D` (host's value wins, `0..=MAX_DELAY`, default 2): the local
//!   pad latched while frame `f` is the next frame to step belongs to frame
//!   `f + D`. Frames `0..D` use pad 0 on both sides.
//! * Frame `f` is steppable only when both pads for `f` are known.
//! * Inputs travel in batches of every not-yet-acknowledged frame (oldest
//!   first, up to [`MAX_BATCH`]) on the unreliable channel on every update, with
//!   a rate-limited copy on the reliable channel when acknowledgements stall.
//! * Every `hash_interval` frames (default 60) each peer reports an FNV-1a 64
//!   hash of caller-chosen state bytes ([`hash_state`]); a mismatch ends the
//!   session as [`CloseReason::Desync`].
//! * Time only enters as the caller's `now_ms` and only drives stall/timeout
//!   timers and RTT, never inputs. No floats, no wall clock, no hash-map order.
//! * The session always carries non-zero co-op flags (host-authoritative,
//!   checked against the guest's supported set in the handshake), so a guest
//!   always has gameplay.
//!
//! # Driving a session (per frontend tick)
//!
//! ```
//! use z2_net::{loopback_pair, LoopbackConfig, Session, SessionConfig, COOP_TWO_LINKS};
//! let (link, mut ta, mut tb) = loopback_pair(LoopbackConfig::default(), 1);
//! let mut host = Session::new(SessionConfig::host(0xBA32_2865, 7, COOP_TWO_LINKS)).unwrap();
//! let mut guest = Session::new(SessionConfig::guest(0xBA32_2865, 7, COOP_TWO_LINKS)).unwrap();
//! let mut stepped = 0;
//! for tick in 0..200u64 {
//!     link.advance(1);
//!     let now_ms = tick * 16;
//!     for (s, t) in [(&mut host, &mut ta), (&mut guest, &mut tb)] {
//!         s.update(t, now_ms); // receive, timers, send
//!         for _ in 0..2 {
//!             // frame budget for this tick
//!             s.latch_local(0); // this tick's local pad
//!             let Some((frame, pad1, pad2)) = s.poll() else { break };
//!             // game.step2(pad1, pad2);
//!             if s.needs_hash(frame) {
//!                 s.report_hash(frame, z2_net::hash_state(&[&[pad1, pad2]]));
//!             }
//!             stepped += 1;
//!         }
//!         s.update(t, now_ms); // flush freshly latched input
//!         for _event in s.take_events() {}
//!     }
//! }
//! assert!(stepped > 0);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod hash;
pub mod ice;
mod input;
pub mod loopback;
#[cfg(feature = "matchbox")]
pub mod matchbox;
pub mod rollback;
pub mod session;
pub mod transport;
pub mod wire;

pub use hash::{hash_state, Fnv64};
pub use ice::{IceConfig, IceParseError, DEFAULT_ICE_SPEC, DEFAULT_ICE_URLS};
pub use loopback::{loopback_pair, LoopbackConfig, LoopbackLink, LoopbackTransport, Side};
#[cfg(feature = "matchbox")]
pub use matchbox::{MatchboxTransport, LINK_TIMEOUT_MS, SIGNALLING_TIMEOUT_MS};
pub use rollback::{
    apply_requests, ChecksumSink, MissingState, RollbackConfig, RollbackConfigError, RollbackError,
    RollbackEvent, RollbackGame, RollbackRequest, RollbackSession, RollbackState, RollbackStats,
    StateRing, SyncTestSession, DEFAULT_CHECK_INTERVAL, DEFAULT_MAX_PREDICTION,
    DEFAULT_ROLLBACK_DELAY, MAX_PREDICTION_WINDOW, MAX_ROLLBACK_DELAY, ROLLBACK_PROTO_VERSION,
};
pub use session::{
    CloseReason, ConfigError, MismatchKind, Session, SessionConfig, SessionEvent, SessionState,
    SessionStats,
};
pub use transport::{Channel, ConnectStage, LinkState, Transport, TransportError};
pub use wire::{ByeReason, DecodeError, Hello, Message, Role, Start};

/// Envelope version: first byte of every packet. Bumped on any layout change.
pub const WIRE_VERSION: u8 = 1;
/// Protocol (semantics) version carried in [`Hello`].
pub const PROTO_VERSION: u16 = 1;
/// Default input delay in frames.
pub const DEFAULT_DELAY: u8 = 2;
/// Largest accepted input delay in frames.
pub const MAX_DELAY: u8 = 8;
/// Default desync-check interval in frames.
pub const DEFAULT_HASH_INTERVAL: u16 = 60;
/// Most pads carried by one Input packet (oldest unacknowledged first).
pub const MAX_BATCH: usize = 32;
/// Size of the battery-backed WRAM snapshot the host may send in [`Start`].
pub const WRAM_LEN: usize = 0x2000;
/// Largest valid packet: a Start carrying a full WRAM snapshot (8207 bytes).
pub const MAX_PACKET_LEN: usize = wire::START_HEADER_LEN + WRAM_LEN;
/// Waiting this long for the remote pad turns Running into Stalled.
pub const STALL_NOTIFY_MS: u64 = 250;
/// Default: waiting this long for the remote pad closes the session.
pub const DEFAULT_STALL_TIMEOUT_MS: u64 = 60_000;
/// Default: a handshake (link up, session not started) lasting this long closes the session.
pub const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 30_000;
/// Minimum spacing of reliable-channel input copies while acknowledgements stall.
pub const RELIABLE_RESEND_MS: u64 = 200;
/// Ping spacing for RTT measurement.
pub const PING_INTERVAL_MS: u64 = 500;
/// Default signalling port of `z2-signal`.
pub const DEFAULT_SIGNAL_PORT: u16 = 3536;
/// Default signalling base URL for local play.
pub const DEFAULT_SIGNAL_URL: &str = "ws://127.0.0.1:3536";
/// Prefix of room paths built by [`room_url`].
pub const ROOM_PREFIX: &str = "z2-";
/// Co-op flag bit: second Link / pad-2 gameplay (`z2_core::coop` owns the meaning).
pub const COOP_TWO_LINKS: u32 = 1 << 0;
/// Co-op flag bit: unlimited sprites (co-op only).
pub const COOP_SPRITE_UNLIMITED: u32 = 1 << 1;

/// Room names: 1..=32 characters of `[A-Za-z0-9_-]`.
pub fn room_name_valid(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Error from [`room_url`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomUrlError {
    /// Signal URL is not `ws://host[:port]` or `wss://host[:port]`.
    BadSignalUrl,
    /// Room name fails [`room_name_valid`].
    BadRoom,
}

impl std::fmt::Display for RoomUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoomUrlError::BadSignalUrl => {
                f.write_str("signal URL must be ws://host[:port] or wss://host[:port]")
            }
            RoomUrlError::BadRoom => {
                f.write_str("room name must be 1-32 characters of A-Z a-z 0-9 _ -")
            }
        }
    }
}

impl std::error::Error for RoomUrlError {}

/// `ws://host:3536` + room `abc` -> `ws://host:3536/z2-abc`. A trailing `/` on
/// the base is trimmed. The URL path is the room; `z2-signal` admits two peers
/// per room.
pub fn room_url(signal_base: &str, room: &str) -> Result<String, RoomUrlError> {
    let base = signal_base.trim().trim_end_matches('/');
    let rest = base
        .strip_prefix("ws://")
        .or_else(|| base.strip_prefix("wss://"))
        .ok_or(RoomUrlError::BadSignalUrl)?;
    if rest.is_empty() || rest.contains(['/', '?', '#', ' ']) {
        return Err(RoomUrlError::BadSignalUrl);
    }
    if !room_name_valid(room) {
        return Err(RoomUrlError::BadRoom);
    }
    Ok(format!("{base}/{ROOM_PREFIX}{room}"))
}
