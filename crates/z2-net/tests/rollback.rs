//! Rollback session over the deterministic loopback transport (ROM-free).

use std::collections::BTreeMap;

use z2_net::{
    apply_requests, hash_state, loopback_pair, ChecksumSink, CloseReason, LoopbackConfig,
    LoopbackLink, LoopbackTransport, MismatchKind, RollbackConfig, RollbackConfigError,
    RollbackError, RollbackEvent, RollbackGame, RollbackRequest, RollbackSession, RollbackState,
    Session, SessionConfig, SessionEvent, Side, StateRing, SyncTestSession, COOP_SPRITE_UNLIMITED,
    COOP_TWO_LINKS, WRAM_LEN,
};

const CRC: u32 = 0xBA32_2865;
const TRAPS: u64 = 0x5EED_0001;
const TICK_MS: u64 = 16;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Deterministic stand-in game: an FNV accumulator over (frame, pads).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MockGame {
    frame: u32,
    acc: u64,
}

impl RollbackGame for MockGame {
    type State = MockGame;
    fn save(&self) -> MockGame {
        *self
    }
    fn load(&mut self, state: &MockGame) {
        *self = *state;
    }
    fn advance(&mut self, pad1: u8, pad2: u8) {
        self.acc = hash_state(&[
            &self.acc.to_le_bytes(),
            &self.frame.to_le_bytes(),
            &[pad1, pad2],
        ]);
        self.frame += 1;
    }
    fn checksum(&self) -> u64 {
        hash_state(&[&self.acc.to_le_bytes(), &self.frame.to_le_bytes()])
    }
}

/// Lockstep reference: the checksum after each frame given both true pad streams.
fn reference(p1: &BTreeMap<u32, u8>, p2: &BTreeMap<u32, u8>, frames: u32) -> Vec<u64> {
    let mut g = MockGame::default();
    (0..frames)
        .map(|f| {
            g.advance(
                p1.get(&f).copied().unwrap_or(0),
                p2.get(&f).copied().unwrap_or(0),
            );
            g.checksum()
        })
        .collect()
}

type PadFn = Box<dyn FnMut(u32, &mut Rng) -> u8>;

struct Peer {
    s: RollbackSession<LoopbackTransport>,
    game: MockGame,
    ring: StateRing<MockGame>,
    rng: Rng,
    pads: PadFn,
    /// Local pads by the frame they apply to.
    added: BTreeMap<u32, u8>,
    /// Checksum after the latest Advance of each frame not yet final.
    latest: BTreeMap<u32, u64>,
    /// Checksum after each final frame.
    confirmed: BTreeMap<u32, u64>,
    events: Vec<RollbackEvent>,
    loads: u64,
    replays: u64,
    skip: u32,
    honour_wait: bool,
    corrupt_checksum_at: Option<u32>,
}

impl Peer {
    fn new(cfg: RollbackConfig, t: LoopbackTransport, seed: u64) -> Self {
        let cap = usize::from(cfg.max_prediction) + 2;
        Self {
            s: RollbackSession::new(cfg, t).expect("valid config"),
            game: MockGame::default(),
            ring: StateRing::new(cap),
            rng: Rng(seed | 1),
            pads: Box::new(|_, r| r.next() as u8),
            added: BTreeMap::new(),
            latest: BTreeMap::new(),
            confirmed: BTreeMap::new(),
            events: Vec::new(),
            loads: 0,
            replays: 0,
            skip: 0,
            honour_wait: true,
            corrupt_checksum_at: None,
        }
    }

    fn is_host(&self) -> bool {
        self.s.role() == z2_net::Role::Host
    }

    /// One frontend tick with `budget` advance attempts.
    fn tick(&mut self, now_ms: u64, budget: u32) {
        if self.skip > 0 {
            self.skip -= 1;
            self.s.poll(now_ms);
            self.drain_events();
            return;
        }
        for _ in 0..budget {
            let g = self.s.frame();
            let pad = (self.pads)(g, &mut self.rng);
            let fresh = self.s.add_local_input(pad);
            if fresh {
                let target = g + u32::from(self.s.input_delay());
                assert!(self.added.insert(target, pad).is_none());
            }
            let reqs = match self.s.advance(now_ms) {
                Ok(r) => r,
                Err(RollbackError::Closed(_)) => break,
                Err(e) => panic!("{:?}: {e}", self.s.role()),
            };
            let delay = u32::from(self.s.input_delay());
            for r in &reqs {
                match *r {
                    RollbackRequest::SaveState { frame } => {
                        assert_eq!(self.game.frame, frame);
                        if self.s.needs_checksum(frame) {
                            let mut c = self.game.checksum();
                            if self.corrupt_checksum_at == Some(frame) {
                                c ^= 1;
                            }
                            self.s.report_checksum(frame, c);
                        }
                        self.ring.save(frame, self.game.save());
                    }
                    RollbackRequest::LoadState { frame } => {
                        self.loads += 1;
                        let st = *self.ring.get(frame).expect("state held for rollback");
                        self.game.load(&st);
                        assert_eq!(self.game.frame, frame);
                    }
                    RollbackRequest::Advance {
                        frame,
                        pad1,
                        pad2,
                        confirmed,
                        replay,
                    } => {
                        assert_eq!(self.game.frame, frame, "advance in order");
                        let local = if self.is_host() { pad1 } else { pad2 };
                        // The local pad takes effect exactly `delay` frames after it was added.
                        let expect = if frame < delay {
                            0
                        } else {
                            *self.added.get(&frame).expect("local pad added for frame")
                        };
                        assert_eq!(local, expect, "local pad at frame {frame}");
                        if !replay && delay == 0 && fresh {
                            assert_eq!(frame, g, "zero delay: applied the same tick");
                        }
                        if replay {
                            self.replays += 1;
                        }
                        self.game.advance(pad1, pad2);
                        let c = self.game.checksum();
                        if confirmed {
                            if let Some(&prev) = self.confirmed.get(&frame) {
                                assert_eq!(prev, c, "confirmed frame {frame} changed");
                            }
                        }
                        self.latest.insert(frame, c);
                    }
                }
            }
            // Frames below final_frame() can never change again.
            let fin = self.s.final_frame();
            let rest = self.latest.split_off(&fin);
            for (f, c) in std::mem::replace(&mut self.latest, rest) {
                if let Some(prev) = self.confirmed.insert(f, c) {
                    assert_eq!(prev, c, "final frame {f} changed");
                }
            }
        }
        self.drain_events();
    }

    fn drain_events(&mut self) {
        for e in self.s.events() {
            if let RollbackEvent::WaitRecommendation { skip_frames } = e {
                if self.honour_wait {
                    self.skip = u32::from(skip_frames);
                }
            }
            self.events.push(e);
        }
    }

    fn saw(&self, pred: impl Fn(&RollbackEvent) -> bool) -> bool {
        self.events.iter().any(pred)
    }

    fn close_reason(&self) -> Option<CloseReason> {
        self.events.iter().find_map(|e| match e {
            RollbackEvent::Disconnected { reason } => Some(reason.clone()),
            _ => None,
        })
    }
}

struct Sim {
    link: LoopbackLink,
    host: Peer,
    guest: Peer,
    tick: u64,
    host_budget: Box<dyn Fn(u64) -> u32>,
}

fn configs(delay: u8, max_prediction: u8) -> (RollbackConfig, RollbackConfig) {
    let mut hc = RollbackConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    hc.input_delay = delay;
    hc.max_prediction = max_prediction;
    hc.check_interval = 1;
    let mut gc = RollbackConfig::guest(CRC, TRAPS, COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED);
    // The host's delay and interval win; give the guest different requests.
    gc.input_delay = 3 - delay.min(3);
    gc.check_interval = 7;
    gc.max_prediction = max_prediction;
    (hc, gc)
}

impl Sim {
    fn new(link_cfg: LoopbackConfig, delay: u8, max_prediction: u8, seed: u64) -> Self {
        let (hc, gc) = configs(delay, max_prediction);
        Self::with(link_cfg, hc, gc, seed)
    }

    fn with(link_cfg: LoopbackConfig, hc: RollbackConfig, gc: RollbackConfig, seed: u64) -> Self {
        let (link, ta, tb) = loopback_pair(link_cfg, seed);
        Self {
            link,
            host: Peer::new(hc, ta, seed.wrapping_mul(0x9E37_79B9) ^ 0xA5),
            guest: Peer::new(gc, tb, seed.wrapping_mul(0x85EB_CA6B) ^ 0x5A),
            tick: 0,
            host_budget: Box::new(|_| 1),
        }
    }

    fn step(&mut self) {
        self.link.advance(1);
        let now = self.tick * TICK_MS;
        let hb = (self.host_budget)(self.tick);
        self.host.tick(now, hb);
        self.guest.tick(now, 1);
        self.tick += 1;
    }

    fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    fn run_confirmed(&mut self, frames: u32, max_ticks: u64) {
        let start = self.tick;
        while self.host.confirmed.len() < frames as usize
            || self.guest.confirmed.len() < frames as usize
        {
            assert!(
                self.tick - start < max_ticks,
                "no progress: host f={} c={} {:?} guest f={} c={} {:?}",
                self.host.s.frame(),
                self.host.confirmed.len(),
                self.host.close_reason(),
                self.guest.s.frame(),
                self.guest.confirmed.len(),
                self.guest.close_reason(),
            );
            self.step();
        }
    }

    /// Both sides' confirmed checksums equal the lockstep reference for every frame.
    fn check_against_reference(&self, frames: u32) {
        let reference = reference(&self.host.added, &self.guest.added, frames);
        for (f, want) in reference.iter().enumerate() {
            let f = f as u32;
            assert_eq!(self.host.confirmed.get(&f), Some(want), "host frame {f}");
            assert_eq!(self.guest.confirmed.get(&f), Some(want), "guest frame {f}");
        }
    }

    fn assert_healthy(&self) {
        for p in [&self.host, &self.guest] {
            assert_eq!(p.close_reason(), None, "{:?}", p.s.role());
            assert!(!p.saw(|e| matches!(e, RollbackEvent::DesyncDetected { .. })));
            assert!(p.s.is_running());
        }
    }
}

fn run_10k(link_cfg: LoopbackConfig, delay: u8, seed: u64) -> Sim {
    let mut sim = Sim::new(link_cfg, delay, 8, seed);
    sim.run_confirmed(10_000, 40_000);
    sim.assert_healthy();
    sim.check_against_reference(10_000);
    for p in [&sim.host, &sim.guest] {
        assert_eq!(p.s.input_delay(), delay);
        assert_eq!(p.s.check_interval(), 1);
        let st = p.s.stats();
        assert!(st.checksums_ok >= 9_000, "{st:?}");
        assert!(st.max_prediction_depth <= 8, "{st:?}");
        assert!(st.max_rollback_depth <= 8, "{st:?}");
    }
    sim
}

#[test]
fn rollback_10k_perfect_link() {
    let sim = run_10k(LoopbackConfig::default(), 2, 1);
    // One tick of latency and delay 2 hides it: no rollbacks needed.
    assert_eq!(sim.host.s.stats().rollbacks, 0);
}

#[test]
fn rollback_10k_latency_jitter_loss_reorder_delay2() {
    let cfg = LoopbackConfig {
        latency_ticks: 3,
        jitter_ticks: 4,
        loss_per_mille: 100,
        connect_after_ticks: 5,
    };
    let sim = run_10k(cfg, 2, 7);
    assert!(sim.link.dropped() > 0);
    for p in [&sim.host, &sim.guest] {
        let st = p.s.stats();
        assert!(st.rollbacks > 100, "{st:?}");
        assert!(p.loads == st.rollbacks && p.replays == st.rollback_frames);
        eprintln!("delay2 {:?}: {st:?}", p.s.role());
    }
}

#[test]
fn rollback_10k_zero_delay_harsh_link() {
    let cfg = LoopbackConfig {
        latency_ticks: 5,
        jitter_ticks: 6,
        loss_per_mille: 200,
        connect_after_ticks: 0,
    };
    let sim = run_10k(cfg, 0, 99);
    for p in [&sim.host, &sim.guest] {
        let st = p.s.stats();
        assert!(st.rollbacks > 500, "{st:?}");
        eprintln!("delay0 {:?}: {st:?}", p.s.role());
    }
}

#[test]
fn rollback_10k_delay3_no_prediction_behaves_like_lockstep() {
    let cfg = LoopbackConfig {
        latency_ticks: 2,
        jitter_ticks: 2,
        loss_per_mille: 50,
        connect_after_ticks: 0,
    };
    let (hc, gc) = configs(3, 0);
    let mut sim = Sim::with(cfg, hc, gc, 5);
    sim.run_confirmed(10_000, 60_000);
    sim.assert_healthy();
    sim.check_against_reference(10_000);
    for p in [&sim.host, &sim.guest] {
        assert_eq!(p.s.stats().rollbacks, 0);
        assert_eq!(p.replays, 0);
    }
}

#[test]
fn misprediction_rolls_back_to_reference() {
    let cfg = LoopbackConfig {
        latency_ticks: 4,
        ..LoopbackConfig::default()
    };
    let (hc, gc) = configs(0, 8);
    let mut sim = Sim::with(cfg, hc, gc, 3);
    // Guest holds nothing, then Right from frame 200, then A+Right from 400.
    sim.guest.pads = Box::new(|f, _| match f {
        0..200 => 0,
        200..400 => 0x01,
        _ => 0x81,
    });
    sim.host.pads = Box::new(|_, _| 0x40);
    sim.run_confirmed(600, 5_000);
    sim.assert_healthy();
    sim.check_against_reference(600);
    let st = sim.host.s.stats();
    // Exactly the two input changes were mispredicted; everything else repeated.
    assert_eq!(st.rollbacks, 2, "{st:?}");
    assert_eq!(sim.host.loads, 2);
    assert!(sim.host.replays >= 2);
    // Final state equals lockstep over the same pads.
    let frames = sim.host.game.frame.min(sim.guest.game.frame);
    assert!(frames >= 600);
    let reference = reference(&sim.host.added, &sim.guest.added, 600);
    assert_eq!(sim.host.confirmed[&599], reference[599]);
}

#[test]
fn prediction_window_stalls_then_recovers() {
    let cfg = LoopbackConfig {
        latency_ticks: 1,
        ..LoopbackConfig::default()
    };
    let mut sim = Sim::new(cfg, 1, 8, 11);
    sim.run_confirmed(100, 1_000);
    sim.link.set_blackout(true);
    sim.run_ticks(40);
    let frame = sim.host.s.frame();
    let confirmed = sim.host.s.confirmed_frame();
    assert_eq!(frame - confirmed, 8, "ran exactly to the prediction window");
    sim.run_ticks(60);
    assert_eq!(
        sim.host.s.frame(),
        frame,
        "stalled instead of predicting further"
    );
    assert_eq!(sim.guest.s.frame() - sim.guest.s.confirmed_frame(), 8);
    let st = sim.host.s.stats();
    assert!(st.stalled_advances >= 60, "{st:?}");
    assert_eq!(st.prediction_depth, 8);
    assert!(sim
        .host
        .saw(|e| matches!(e, RollbackEvent::NetworkInterrupted { .. })));
    sim.link.set_blackout(false);
    sim.run_confirmed(1_000, 5_000);
    assert!(sim.host.saw(|e| matches!(e, RollbackEvent::NetworkResumed)));
    sim.assert_healthy();
    sim.check_against_reference(1_000);
}

#[test]
fn time_sync_recommends_wait_to_the_side_ahead() {
    let cfg = LoopbackConfig {
        latency_ticks: 2,
        ..LoopbackConfig::default()
    };
    let mut sim = Sim::new(cfg, 2, 8, 21);
    sim.host.honour_wait = false;
    // The host's clock runs fast: two frames every other tick.
    sim.host_budget = Box::new(|t| 1 + u32::from(t.is_multiple_of(2)));
    sim.run_ticks(600);
    let h = sim.host.s.stats();
    let g = sim.guest.s.stats();
    assert!(h.wait_recommendations >= 1, "{h:?}");
    assert_eq!(g.wait_recommendations, 0, "{g:?}");
    assert!(sim.host.saw(
        |e| matches!(e, RollbackEvent::WaitRecommendation { skip_frames } if *skip_frames >= 2)
    ));
    assert!(h.local_lead > 0 && g.local_lead < 0, "{h:?} {g:?}");

    // Honouring the recommendation with equal clocks brings the lead back down.
    sim.host.honour_wait = true;
    sim.host_budget = Box::new(|_| 1);
    sim.run_ticks(600);
    let h = sim.host.s.stats();
    assert!(h.local_lead.abs() <= 3, "{h:?}");
    sim.assert_healthy();
    let n = sim.host.confirmed.len().min(sim.guest.confirmed.len()) as u32;
    sim.check_against_reference(n);
}

#[test]
fn confirmed_checksum_mismatch_is_detected_on_both_sides() {
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 8, 4);
    sim.guest.corrupt_checksum_at = Some(120);
    sim.run_ticks(400);
    for p in [&sim.host, &sim.guest] {
        assert!(
            p.saw(|e| matches!(e, RollbackEvent::DesyncDetected { frame: 120, .. })),
            "{:?}",
            p.events
        );
        assert_eq!(p.close_reason(), Some(CloseReason::Desync));
        assert_eq!(p.s.state(), RollbackState::Closed);
    }
}

#[test]
fn handshake_adopts_host_parameters_and_wram() {
    let (mut hc, gc) = configs(1, 8);
    hc.check_interval = 30;
    hc.wram = Some((0..WRAM_LEN).map(|i| i as u8).collect());
    let mut sim = Sim::with(LoopbackConfig::default(), hc, gc, 8);
    sim.run_ticks(10);
    for p in [&sim.host, &sim.guest] {
        assert!(p.saw(|e| matches!(e, RollbackEvent::Synchronizing { progress: 33 })));
        let running = p.events.iter().find_map(|e| match e {
            RollbackEvent::Running {
                input_delay,
                coop_flags,
                check_interval,
                wram,
            } => Some((*input_delay, *coop_flags, *check_interval, wram.len())),
            _ => None,
        });
        assert_eq!(running, Some((1, COOP_TWO_LINKS, 30, WRAM_LEN)));
    }
}

fn mismatch_case(
    edit_host: impl Fn(&mut RollbackConfig),
    edit_guest: impl Fn(&mut RollbackConfig),
) -> (Option<CloseReason>, Option<CloseReason>) {
    let (mut hc, mut gc) = configs(2, 8);
    edit_host(&mut hc);
    edit_guest(&mut gc);
    let mut sim = Sim::with(LoopbackConfig::default(), hc, gc, 9);
    sim.run_ticks(20);
    for p in [&sim.host, &sim.guest] {
        assert!(!p.saw(|e| matches!(e, RollbackEvent::Running { .. })));
        assert_eq!(p.s.state(), RollbackState::Closed);
    }
    (sim.host.close_reason(), sim.guest.close_reason())
}

#[test]
fn handshake_mismatches_refuse_to_start() {
    let rom = Some(CloseReason::Mismatch(MismatchKind::Rom));
    assert_eq!(
        mismatch_case(|_| {}, |g| g.rom_crc32 ^= 1),
        (rom.clone(), rom)
    );
    let traps = Some(CloseReason::Mismatch(MismatchKind::TrapSet));
    assert_eq!(
        mismatch_case(|h| h.trapset_id = 1, |_| {}),
        (traps.clone(), traps)
    );
    let flags = Some(CloseReason::Mismatch(MismatchKind::CoopFlags));
    assert_eq!(
        mismatch_case(
            |h| h.coop_flags = COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED,
            |g| g.coop_flags = COOP_TWO_LINKS
        ),
        (flags.clone(), flags)
    );
    let role = Some(CloseReason::RoleConflict);
    assert_eq!(
        mismatch_case(|_| {}, |g| g.role = z2_net::Role::Host),
        (role.clone(), role)
    );
}

#[test]
fn lockstep_and_rollback_peers_refuse_each_other() {
    let (link, ta, mut tb) = loopback_pair(LoopbackConfig::default(), 1);
    let mut rb = Peer::new(configs(2, 8).0, ta, 1);
    let mut ls = Session::new(SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
    let mut ls_events = Vec::new();
    for t in 0..20 {
        link.advance(1);
        rb.tick(t * TICK_MS, 1);
        ls.update(&mut tb, t * TICK_MS);
        ls_events.extend(ls.take_events());
    }
    assert_eq!(
        rb.close_reason(),
        Some(CloseReason::Mismatch(MismatchKind::NetMode))
    );
    assert!(
        ls_events.contains(&SessionEvent::Closed(CloseReason::Mismatch(
            MismatchKind::NetMode
        )))
    );
}

#[test]
fn disconnect_quit_and_timeout_close_reasons() {
    // Peer vanishes.
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 8, 12);
    sim.run_confirmed(50, 500);
    sim.link.disconnect(Side::B);
    sim.run_ticks(3);
    assert_eq!(sim.host.close_reason(), Some(CloseReason::PeerLeft));
    assert!(matches!(
        sim.guest.close_reason(),
        Some(CloseReason::Transport(_))
    ));
    let now = sim.tick * TICK_MS;
    sim.host.s.add_local_input(0);
    assert_eq!(
        sim.host.s.advance(now),
        Err(RollbackError::Closed(CloseReason::PeerLeft))
    );

    // Orderly quit.
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 8, 13);
    sim.run_confirmed(50, 500);
    sim.host.s.close();
    sim.run_ticks(5);
    assert_eq!(sim.host.close_reason(), Some(CloseReason::LocalQuit));
    assert_eq!(sim.guest.close_reason(), Some(CloseReason::PeerQuit));

    // Silence past the disconnect timeout.
    let (mut hc, mut gc) = configs(2, 8);
    hc.disconnect_timeout_ms = 1_000;
    gc.disconnect_timeout_ms = 1_000;
    let mut sim = Sim::with(LoopbackConfig::default(), hc, gc, 14);
    sim.run_confirmed(50, 500);
    sim.link.set_blackout(true);
    sim.run_ticks(1_000 / TICK_MS + 5);
    for p in [&sim.host, &sim.guest] {
        assert!(p.saw(|e| matches!(e, RollbackEvent::NetworkInterrupted { .. })));
        assert_eq!(p.close_reason(), Some(CloseReason::Timeout));
    }
}

#[test]
fn handshake_timeout() {
    let (mut hc, gc) = configs(2, 8);
    hc.handshake_timeout_ms = 500;
    let (link, ta, _tb) = loopback_pair(LoopbackConfig::default(), 2);
    let mut host = Peer::new(hc, ta, 1);
    let _ = gc;
    for t in 0..40 {
        link.advance(1);
        host.tick(t * TICK_MS, 1);
    }
    assert_eq!(host.close_reason(), Some(CloseReason::Timeout));
}

#[test]
fn config_validation() {
    let (link, ta, _tb) = loopback_pair(LoopbackConfig::default(), 1);
    let _ = link;
    let mut c = RollbackConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    assert_eq!(c.input_delay, 2);
    assert_eq!(c.max_prediction, 8);
    c.input_delay = 4;
    assert_eq!(
        RollbackSession::new(c.clone(), ta).err(),
        Some(RollbackConfigError::DelayOutOfRange(4))
    );
    let (_l, ta, _tb) = loopback_pair(LoopbackConfig::default(), 1);
    c.input_delay = 0;
    c.max_prediction = 31;
    assert_eq!(
        RollbackSession::new(c.clone(), ta).err(),
        Some(RollbackConfigError::PredictionOutOfRange(31))
    );
    let (_l, ta, _tb) = loopback_pair(LoopbackConfig::default(), 1);
    c.max_prediction = 8;
    c.coop_flags = 0;
    assert_eq!(
        RollbackSession::new(c, ta).err(),
        Some(RollbackConfigError::NoCoopFlags)
    );
}

#[test]
fn missing_local_input_is_an_error() {
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 8, 30);
    sim.run_confirmed(10, 200);
    let now = sim.tick * TICK_MS;
    let err = sim.host.s.advance(now).unwrap_err();
    assert!(matches!(err, RollbackError::MissingLocalInput { .. }));
}

// ---- sync test ----------------------------------------------------------

/// Hidden state that is neither saved nor restored: replays diverge.
#[derive(Default)]
struct LeakyGame {
    inner: MockGame,
    hidden: u64,
}

impl RollbackGame for LeakyGame {
    type State = MockGame;
    fn save(&self) -> MockGame {
        self.inner
    }
    fn load(&mut self, state: &MockGame) {
        self.inner = *state;
    }
    fn advance(&mut self, pad1: u8, pad2: u8) {
        self.hidden += 1;
        self.inner.advance(pad1 ^ self.hidden as u8, pad2);
    }
    fn checksum(&self) -> u64 {
        self.inner.checksum()
    }
}

fn run_sync_test<G: RollbackGame>(game: &mut G, distance: u8, frames: u32) -> SyncTestSession {
    let mut s = SyncTestSession::new(distance);
    let mut ring = StateRing::new(usize::from(distance) + 2);
    let mut rng = Rng(0xC0FFEE);
    for _ in 0..frames {
        let reqs = s.advance(rng.next() as u8, rng.next() as u8);
        apply_requests(game, &mut ring, &reqs, &mut s).expect("state held");
    }
    s
}

#[test]
fn sync_test_passes_deterministic_game() {
    for d in [1, 2, 8] {
        let mut g = MockGame::default();
        let mut s = run_sync_test(&mut g, d, 2_000);
        assert_eq!(s.mismatches(), 0);
        assert!(s.events().is_empty());
        assert_eq!(g.frame, 2_000);
        assert!(s.needs_checksum(0));
    }
}

#[test]
fn sync_test_detects_non_deterministic_game() {
    let mut g = LeakyGame::default();
    let mut s = run_sync_test(&mut g, 2, 100);
    assert!(s.mismatches() > 0);
    assert!(s
        .events()
        .iter()
        .any(|e| matches!(e, RollbackEvent::DesyncDetected { .. })));
}

#[test]
fn state_ring_keys_by_frame() {
    let mut r = StateRing::new(3);
    r.save(4, 'a');
    r.save(5, 'b');
    assert_eq!(r.get(4), Some(&'a'));
    r.save(7, 'c'); // same slot as 4
    assert_eq!(r.get(4), None);
    assert_eq!(r.get(7), Some(&'c'));
    assert_eq!(r.get(5), Some(&'b'));
}
