//! Test-only network conditions for the page's transport (`z2.ext.net.simulate`).
//!
//! Two windows on one machine see almost no latency, so a rollback session
//! between them never mispredicts. [`SimTransport`] wraps the real transport
//! and holds outgoing packets back for `latency ± jitter` milliseconds of the
//! session clock, and drops a share of **unreliable** packets. Reliable
//! packets are never dropped (the real channel retransmits them) and keep
//! their order. With every knob at zero it is a plain pass-through, which is
//! the default: normal play never turns it on.

use std::collections::VecDeque;

use z2_net::{Channel, ConnectStage, LinkState, Transport, TransportError};

/// Simulated link conditions, applied to packets this page sends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetSim {
    /// Fixed one-way delay added to every outgoing packet (ms).
    pub latency_ms: u32,
    /// Extra random delay `0..=jitter_ms` per packet (ms).
    pub jitter_ms: u32,
    /// Share of unreliable packets dropped, in percent (0..=100).
    pub loss_pct: u8,
}

impl NetSim {
    /// Largest accepted latency or jitter (ms).
    pub const MAX_MS: u32 = 2000;

    /// Clamp every knob into its accepted range.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            latency_ms: self.latency_ms.min(Self::MAX_MS),
            jitter_ms: self.jitter_ms.min(Self::MAX_MS),
            loss_pct: self.loss_pct.min(100),
        }
    }

    /// All knobs at zero: packets pass straight through.
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.latency_ms == 0 && self.jitter_ms == 0 && self.loss_pct == 0
    }

    /// Status JSON fragment (`{"latencyMs":..,"jitterMs":..,"lossPct":..}`).
    #[must_use]
    pub fn json(&self) -> String {
        format!(
            "{{\"latencyMs\":{},\"jitterMs\":{},\"lossPct\":{}}}",
            self.latency_ms, self.jitter_ms, self.loss_pct
        )
    }
}

/// A transport that delays and drops outgoing packets per [`NetSim`].
pub struct SimTransport {
    inner: Box<dyn Transport>,
    sim: NetSim,
    now_ms: u64,
    rng: u64,
    /// Packets waiting for their release time, oldest first per channel.
    held: VecDeque<(u64, Channel, Vec<u8>)>,
    /// Latest release time handed to a reliable packet (keeps them ordered).
    last_reliable_release: u64,
    /// Unreliable packets dropped so far.
    pub dropped: u64,
    /// Packets that were held back so far.
    pub delayed: u64,
}

impl SimTransport {
    /// Wrap `inner`; `seed` varies the jitter/loss sequence between peers.
    pub fn new(inner: Box<dyn Transport>, sim: NetSim, seed: u64) -> Self {
        Self {
            inner,
            sim: sim.clamped(),
            now_ms: 0,
            rng: seed | 1,
            held: VecDeque::new(),
            last_reliable_release: 0,
            dropped: 0,
            delayed: 0,
        }
    }

    /// Change the conditions for packets sent from now on.
    pub fn set_sim(&mut self, sim: NetSim) {
        self.sim = sim.clamped();
    }

    /// Current conditions.
    pub fn sim(&self) -> NetSim {
        self.sim
    }

    /// Advance the clock that release times are measured on, and hand every
    /// packet that is due to the real transport.
    pub fn set_now(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        self.release();
    }

    fn next_rand(&mut self) -> u64 {
        // xorshift64*: deterministic, no dependency, plenty for test noise.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn release(&mut self) {
        // Held packets are not sorted overall (jitter reorders unreliable
        // ones), so scan them all; the queue is a few dozen entries at most.
        let now = self.now_ms;
        let mut i = 0;
        while i < self.held.len() {
            if self.held[i].0 <= now {
                if let Some((_, ch, bytes)) = self.held.remove(i) {
                    // A late send failing is what a lossy link looks like; the
                    // link state still reports a real closure.
                    let _ = self.inner.send(ch, &bytes);
                }
            } else {
                i += 1;
            }
        }
    }
}

impl Transport for SimTransport {
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError> {
        if self.sim.is_off() && self.held.is_empty() {
            return self.inner.send(channel, bytes);
        }
        if channel == Channel::Unreliable
            && self.sim.loss_pct > 0
            && self.next_rand() % 100 < u64::from(self.sim.loss_pct)
        {
            self.dropped += 1;
            return Ok(());
        }
        let jitter = if self.sim.jitter_ms > 0 {
            self.next_rand() % (u64::from(self.sim.jitter_ms) + 1)
        } else {
            0
        };
        let mut at = self.now_ms + u64::from(self.sim.latency_ms) + jitter;
        if channel == Channel::Reliable {
            at = at.max(self.last_reliable_release);
            self.last_reliable_release = at;
        }
        self.delayed += 1;
        self.held.push_back((at, channel, bytes.to_vec()));
        self.release();
        Ok(())
    }

    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)> {
        self.release();
        self.inner.recv()
    }

    fn state(&self) -> LinkState {
        self.inner.state()
    }

    fn stage(&self) -> ConnectStage {
        self.inner.stage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_net::{loopback_pair, LoopbackConfig};

    fn pair() -> (SimTransport, Box<dyn Transport>) {
        let (_link, a, b) = loopback_pair(LoopbackConfig::default(), 1);
        (
            SimTransport::new(Box::new(a), NetSim::default(), 7),
            Box::new(b),
        )
    }

    #[test]
    fn off_passes_through() {
        let (mut a, mut b) = pair();
        a.send(Channel::Unreliable, &[1, 2]).unwrap();
        a.recv();
        let got = b.recv();
        assert_eq!(got, vec![(Channel::Unreliable, vec![1, 2])]);
        assert_eq!(a.delayed, 0);
    }

    #[test]
    fn latency_holds_until_due_and_keeps_reliable_order() {
        let (mut a, mut b) = pair();
        a.set_sim(NetSim {
            latency_ms: 50,
            jitter_ms: 30,
            loss_pct: 0,
        });
        for i in 0..20u8 {
            a.send(Channel::Reliable, &[i]).unwrap();
        }
        a.set_now(49);
        assert!(b.recv().is_empty());
        a.set_now(80);
        let got: Vec<u8> = b.recv().into_iter().map(|(_, v)| v[0]).collect();
        assert_eq!(got, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn loss_drops_only_unreliable() {
        let (mut a, mut b) = pair();
        a.set_sim(NetSim {
            latency_ms: 0,
            jitter_ms: 0,
            loss_pct: 50,
        });
        for i in 0..200u8 {
            a.send(Channel::Unreliable, &[i]).unwrap();
            a.send(Channel::Reliable, &[i]).unwrap();
        }
        a.set_now(1);
        let got = b.recv();
        let rel = got.iter().filter(|(c, _)| *c == Channel::Reliable).count();
        let unrel = got.len() - rel;
        assert_eq!(rel, 200);
        assert!(unrel > 50 && unrel < 150, "unreliable delivered: {unrel}");
        assert_eq!(a.dropped as usize, 200 - unrel);
    }
}
