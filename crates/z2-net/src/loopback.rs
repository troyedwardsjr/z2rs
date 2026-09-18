//! Deterministic in-memory transport pair for tests and frontend self-tests.
//!
//! Time is a shared tick counter advanced by the caller ([`LoopbackLink::advance`]).
//! Latency, jitter (which reorders unreliable packets) and unreliable loss are
//! driven by a seeded xorshift, so a given seed and call sequence always
//! produces the same delivery schedule. Reliable packets are never lost and
//! keep their order. Single-threaded (`Rc<RefCell>`): fine on host and wasm.

use std::cell::RefCell;
use std::rc::Rc;

use crate::transport::{Channel, LinkState, Transport, TransportError};

/// Which end of the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// First transport returned by [`loopback_pair`].
    A,
    /// Second transport returned by [`loopback_pair`].
    B,
}

impl Side {
    const fn idx(self) -> usize {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
    }
}

/// Link impairments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopbackConfig {
    /// Fixed delivery delay in ticks (both channels).
    pub latency_ticks: u32,
    /// Extra random delay `0..=jitter_ticks` per packet (reorders unreliable traffic).
    pub jitter_ticks: u32,
    /// Unreliable packets dropped per thousand.
    pub loss_per_mille: u16,
    /// Link reports Connecting until this many ticks have elapsed.
    pub connect_after_ticks: u32,
}

#[derive(Debug)]
struct Pkt {
    at: u64,
    seq: u64,
    channel: Channel,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct Shared {
    cfg: LoopbackConfig,
    now: u64,
    rng: u64,
    seq: u64,
    /// Indexed by destination side.
    queues: [Vec<Pkt>; 2],
    last_reliable_at: [u64; 2],
    gone: [bool; 2],
    blackout: bool,
    delivered: u64,
    dropped: u64,
}

impl Shared {
    fn next_rng(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn state_of(&self, me: usize) -> LinkState {
        if self.gone[me] {
            LinkState::Closed("local end disconnected".into())
        } else if self.gone[1 - me] {
            LinkState::PeerLeft
        } else if self.now < u64::from(self.cfg.connect_after_ticks) {
            LinkState::Connecting
        } else {
            LinkState::Connected
        }
    }
}

/// Control handle shared by both ends.
#[derive(Debug, Clone)]
pub struct LoopbackLink {
    shared: Rc<RefCell<Shared>>,
}

impl LoopbackLink {
    /// Advance the shared clock.
    pub fn advance(&self, ticks: u64) {
        self.shared.borrow_mut().now += ticks;
    }

    /// Current tick.
    pub fn now(&self) -> u64 {
        self.shared.borrow().now
    }

    /// While on: unreliable packets (sent or due) are dropped and reliable
    /// packets are held back until the blackout ends.
    pub fn set_blackout(&self, on: bool) {
        self.shared.borrow_mut().blackout = on;
    }

    /// Replace the impairment settings for packets sent from now on.
    pub fn set_config(&self, cfg: LoopbackConfig) {
        self.shared.borrow_mut().cfg = cfg;
    }

    /// Disconnect one end: it sees `Closed`, the other sees `PeerLeft`
    /// (and still receives packets already in flight).
    pub fn disconnect(&self, side: Side) {
        self.shared.borrow_mut().gone[side.idx()] = true;
    }

    /// Packets delivered so far.
    pub fn delivered(&self) -> u64 {
        self.shared.borrow().delivered
    }

    /// Unreliable packets dropped so far.
    pub fn dropped(&self) -> u64 {
        self.shared.borrow().dropped
    }
}

/// One end of the in-memory link.
#[derive(Debug)]
pub struct LoopbackTransport {
    shared: Rc<RefCell<Shared>>,
    side: Side,
}

/// Create a connected pair with impairments `cfg` and RNG `seed`.
pub fn loopback_pair(
    cfg: LoopbackConfig,
    seed: u64,
) -> (LoopbackLink, LoopbackTransport, LoopbackTransport) {
    let shared = Rc::new(RefCell::new(Shared {
        cfg,
        now: 0,
        rng: if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        },
        seq: 0,
        queues: [Vec::new(), Vec::new()],
        last_reliable_at: [0; 2],
        gone: [false; 2],
        blackout: false,
        delivered: 0,
        dropped: 0,
    }));
    (
        LoopbackLink {
            shared: Rc::clone(&shared),
        },
        LoopbackTransport {
            shared: Rc::clone(&shared),
            side: Side::A,
        },
        LoopbackTransport {
            shared,
            side: Side::B,
        },
    )
}

impl LoopbackTransport {
    /// Which end this is.
    pub fn side(&self) -> Side {
        self.side
    }
}

impl Transport for LoopbackTransport {
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError> {
        let mut s = self.shared.borrow_mut();
        let me = self.side.idx();
        match s.state_of(me) {
            LinkState::Connected => {}
            LinkState::Connecting => return Err(TransportError::NotConnected),
            _ => return Err(TransportError::Closed),
        }
        let dst = 1 - me;
        if channel == Channel::Unreliable {
            let loss = u64::from(s.cfg.loss_per_mille);
            if s.blackout || (loss > 0 && s.next_rng() % 1000 < loss) {
                s.dropped += 1;
                return Ok(());
            }
        }
        let jitter = u64::from(s.cfg.jitter_ticks);
        let extra = if jitter > 0 {
            s.next_rng() % (jitter + 1)
        } else {
            0
        };
        let mut at = s.now + u64::from(s.cfg.latency_ticks) + extra;
        if channel == Channel::Reliable {
            at = at.max(s.last_reliable_at[dst]);
            s.last_reliable_at[dst] = at;
        }
        s.seq += 1;
        let seq = s.seq;
        s.queues[dst].push(Pkt {
            at,
            seq,
            channel,
            bytes: bytes.to_vec(),
        });
        Ok(())
    }

    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)> {
        let mut s = self.shared.borrow_mut();
        let me = self.side.idx();
        if s.gone[me] {
            s.queues[me].clear();
            return Vec::new();
        }
        let now = s.now;
        let blackout = s.blackout;
        let queue = std::mem::take(&mut s.queues[me]);
        let mut due = Vec::new();
        let mut keep = Vec::new();
        let mut dropped = 0;
        for p in queue {
            if p.at > now {
                keep.push(p);
            } else if blackout {
                if p.channel == Channel::Reliable {
                    keep.push(p);
                } else {
                    dropped += 1;
                }
            } else {
                due.push(p);
            }
        }
        s.queues[me] = keep;
        s.dropped += dropped;
        due.sort_by_key(|p| (p.at, p.seq));
        s.delivered += due.len() as u64;
        due.into_iter().map(|p| (p.channel, p.bytes)).collect()
    }

    fn state(&self) -> LinkState {
        self.shared.borrow().state_of(self.side.idx())
    }
}
