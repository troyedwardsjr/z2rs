//! Session state machine over the deterministic loopback transport (ROM-free).

use z2_net::{
    hash_state, loopback_pair, ByeReason, Channel, CloseReason, Fnv64, Hello, LinkState,
    LoopbackConfig, LoopbackLink, LoopbackTransport, Message, MismatchKind, Role, Session,
    SessionConfig, SessionEvent, SessionState, Side, Start, Transport, TransportError,
    COOP_SPRITE_UNLIMITED, COOP_TWO_LINKS, MAX_DELAY, PROTO_VERSION, WRAM_LEN,
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

/// One peer plus a deterministic stand-in "game": an FNV accumulator over pads.
struct Peer {
    s: Session,
    t: LoopbackTransport,
    rng: Rng,
    stream: Vec<(u32, u8, u8)>,
    latched: Vec<u8>,
    game: Fnv64,
    game_hist: Vec<u64>,
    events: Vec<SessionEvent>,
    corrupt_hash_at: Option<u32>,
}

impl Peer {
    fn new(cfg: SessionConfig, t: LoopbackTransport, seed: u64) -> Self {
        Self {
            s: Session::new(cfg).expect("valid config"),
            t,
            rng: Rng(seed | 1),
            stream: Vec::new(),
            latched: Vec::new(),
            game: Fnv64::new(),
            game_hist: Vec::new(),
            events: Vec::new(),
            corrupt_hash_at: None,
        }
    }

    fn tick(&mut self, now_ms: u64, budget: u32) {
        self.s.update(&mut self.t, now_ms);
        for _ in 0..budget {
            let pad = self.rng.next() as u8;
            if self.s.latch_local(pad) {
                self.latched.push(pad);
            }
            let Some((frame, p1, p2)) = self.s.poll() else {
                break;
            };
            assert_eq!(frame as usize, self.stream.len(), "in order, no skip/dup");
            self.stream.push((frame, p1, p2));
            self.game.write(&[p1, p2]);
            self.game_hist.push(self.game.finish());
            if self.s.needs_hash(frame) {
                let mut h = hash_state(&[&self.game.finish().to_le_bytes(), &frame.to_le_bytes()]);
                if self.corrupt_hash_at == Some(frame) {
                    h ^= 1;
                }
                self.s.report_hash(frame, h);
            }
        }
        self.s.update(&mut self.t, now_ms);
        self.events.extend(self.s.take_events());
    }

    fn saw(&self, pred: impl Fn(&SessionEvent) -> bool) -> bool {
        self.events.iter().any(pred)
    }

    fn assert_no_close(&self) {
        let closes: Vec<_> = self
            .events
            .iter()
            .filter(|e| matches!(e, SessionEvent::Closed(_)))
            .collect();
        assert!(
            closes.is_empty(),
            "{:?}: unexpected {closes:?}",
            self.s.role()
        );
    }
}

struct Sim {
    link: LoopbackLink,
    host: Peer,
    guest: Peer,
    tick: u64,
}

impl Sim {
    fn new(cfg: LoopbackConfig, delay: u8, seed: u64) -> Self {
        let (link, ta, tb) = loopback_pair(cfg, seed);
        let mut hc = SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
        hc.delay = delay;
        let gc = SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED);
        Self::with_configs(link, ta, tb, hc, gc, seed)
    }

    fn with_configs(
        link: LoopbackLink,
        ta: LoopbackTransport,
        tb: LoopbackTransport,
        hc: SessionConfig,
        gc: SessionConfig,
        seed: u64,
    ) -> Self {
        Self {
            link,
            host: Peer::new(hc, ta, seed.wrapping_mul(0x9E37_79B9) ^ 0xA5),
            guest: Peer::new(gc, tb, seed.wrapping_mul(0x85EB_CA6B) ^ 0x5A),
            tick: 0,
        }
    }

    fn step(&mut self) {
        self.link.advance(1);
        let now = self.tick * TICK_MS;
        // Uneven, deterministic frame budgets so the peers drift and catch up.
        let hb = 1 + u32::from(self.tick.is_multiple_of(3));
        let gb = 1 + u32::from(self.tick.is_multiple_of(5));
        self.host.tick(now, hb);
        self.guest.tick(now, gb);
        self.tick += 1;
    }

    fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    fn run_until(&mut self, max_ticks: u64, done: impl Fn(&Sim) -> bool) {
        let limit = self.tick + max_ticks;
        while !done(self) {
            assert!(
                self.tick < limit,
                "gave up at tick {}: host {:?} {:?} f={} guest {:?} {:?} f={}",
                self.tick,
                self.host.s.state(),
                self.host.s.close_reason(),
                self.host.stream.len(),
                self.guest.s.state(),
                self.guest.s.close_reason(),
                self.guest.stream.len()
            );
            self.step();
        }
    }

    fn run_frames(&mut self, frames: usize, max_ticks: u64) {
        self.run_until(max_ticks, |s| {
            s.host.stream.len() >= frames && s.guest.stream.len() >= frames
        });
    }

    fn check_streams(&self, frames: usize) {
        let (h, g) = (&self.host, &self.guest);
        assert_eq!(h.stream[..frames], g.stream[..frames]);
        assert_eq!(h.game_hist[..frames], g.game_hist[..frames]);
        let d = usize::from(h.s.delay());
        assert_eq!(d, usize::from(g.s.delay()));
        for &(f, p1, p2) in &h.stream[..frames] {
            let f = f as usize;
            if f < d {
                assert_eq!((p1, p2), (0, 0), "frames before the delay use pad 0");
            } else {
                assert_eq!(p1, h.latched[f - d], "pad1 is the host's pad at frame {f}");
                assert_eq!(p2, g.latched[f - d], "pad2 is the guest's pad at frame {f}");
            }
        }
    }
}

fn lockstep_10k(cfg: LoopbackConfig, delay: u8, seed: u64) -> Sim {
    let mut sim = Sim::new(cfg, delay, seed);
    sim.run_frames(10_000, 400_000);
    sim.check_streams(10_000);
    sim.host.assert_no_close();
    sim.guest.assert_no_close();
    for p in [&mut sim.host, &mut sim.guest] {
        assert!(
            p.s.stats().hashes_ok >= 150,
            "hashes compared: {:?}",
            p.s.stats()
        );
    }
    sim
}

#[test]
fn lockstep_perfect_link() {
    let sim = lockstep_10k(LoopbackConfig::default(), 2, 1);
    assert!(sim.host.s.stats().rtt_ms.is_some());
}

#[test]
fn lockstep_latency_jitter_loss_reorder() {
    let cfg = LoopbackConfig {
        latency_ticks: 2,
        jitter_ticks: 6,
        loss_per_mille: 250,
        connect_after_ticks: 3,
    };
    let sim = lockstep_10k(cfg, 2, 7);
    assert!(sim.link.dropped() > 0);
}

#[test]
fn lockstep_zero_delay_harsh_link() {
    let cfg = LoopbackConfig {
        latency_ticks: 3,
        jitter_ticks: 10,
        loss_per_mille: 500,
        connect_after_ticks: 0,
    };
    lockstep_10k(cfg, 0, 99);
}

#[test]
fn lockstep_max_delay_high_latency() {
    let cfg = LoopbackConfig {
        latency_ticks: 8,
        jitter_ticks: 3,
        loss_per_mille: 100,
        connect_after_ticks: 20,
    };
    lockstep_10k(cfg, MAX_DELAY, 12345);
}

#[test]
fn reliable_fallback_carries_inputs_when_unreliable_is_dead() {
    let cfg = LoopbackConfig {
        latency_ticks: 1,
        jitter_ticks: 0,
        loss_per_mille: 1000,
        connect_after_ticks: 0,
    };
    let mut sim = Sim::new(cfg, 2, 5);
    sim.run_frames(600, 100_000);
    sim.check_streams(600);
    assert!(sim.host.s.stats().reliable_resends > 0);
    sim.host.assert_no_close();
    sim.guest.assert_no_close();
}

#[test]
fn stall_and_resume_after_two_way_blackout() {
    let cfg = LoopbackConfig {
        latency_ticks: 1,
        ..LoopbackConfig::default()
    };
    let mut sim = Sim::new(cfg, 2, 3);
    sim.run_frames(300, 10_000);
    sim.link.set_blackout(true);
    sim.run_ticks(60); // ~1 s: every in-flight unreliable input is lost both ways
    for p in [&mut sim.host, &mut sim.guest] {
        assert_eq!(p.s.state(), SessionState::Stalled);
        assert!(p.saw(|e| matches!(e, SessionEvent::Stalled { .. })));
    }
    sim.link.set_blackout(false);
    sim.run_frames(1_000, 20_000);
    sim.check_streams(1_000);
    for p in [&mut sim.host, &mut sim.guest] {
        assert!(
            p.saw(|e| matches!(e, SessionEvent::Resumed { stalled_ms, .. } if *stalled_ms >= 250))
        );
        assert!(p.s.is_live());
        p.assert_no_close();
    }
}

#[test]
fn stall_timeout_closes_both() {
    let (link, ta, tb) = loopback_pair(LoopbackConfig::default(), 11);
    let mut hc = SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    hc.stall_timeout_ms = 1_000;
    let mut gc = SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS);
    gc.stall_timeout_ms = 1_000;
    let mut sim = Sim::with_configs(link, ta, tb, hc, gc, 11);
    sim.run_frames(100, 10_000);
    sim.link.set_blackout(true);
    sim.run_until(1_000, |s| !s.host.s.is_live() && !s.guest.s.is_live());
    for p in [&mut sim.host, &mut sim.guest] {
        assert_eq!(p.s.state(), SessionState::Closed);
        assert_eq!(p.s.close_reason(), Some(&CloseReason::Timeout));
        assert_eq!(p.s.poll(), None);
    }
}

#[test]
fn desync_detected_on_both_peers() {
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 21);
    sim.guest.corrupt_hash_at = Some(119);
    let ended = |p: &Peer| matches!(p.s.state(), SessionState::Desynced | SessionState::Closed);
    sim.run_until(2_000, |s| ended(&s.host) && ended(&s.guest));
    for p in [&mut sim.host, &mut sim.guest] {
        assert_eq!(p.s.state(), SessionState::Desynced);
        assert_eq!(p.s.close_reason(), Some(&CloseReason::Desync));
        assert_eq!(p.s.poll(), None);
        assert_eq!(p.s.frames_ready(), 0);
    }
    assert!(
        sim.host
            .saw(|e| matches!(e, SessionEvent::Desync { frame: 119, .. }))
            || sim
                .guest
                .saw(|e| matches!(e, SessionEvent::Desync { frame: 119, .. }))
    );
    assert_eq!(sim.host.s.stats().hashes_ok, 1, "frame 59 matched");
}

#[test]
fn bye_quit_reaches_peer() {
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 31);
    sim.run_frames(100, 5_000);
    sim.host.s.close();
    sim.run_until(100, |s| !s.guest.s.is_live());
    assert_eq!(sim.host.s.close_reason(), Some(&CloseReason::LocalQuit));
    assert_eq!(sim.guest.s.close_reason(), Some(&CloseReason::PeerQuit));
    assert!(sim
        .guest
        .saw(|e| *e == SessionEvent::Closed(CloseReason::PeerQuit)));
}

#[test]
fn peer_left_closes() {
    let mut sim = Sim::new(LoopbackConfig::default(), 2, 41);
    sim.run_frames(100, 5_000);
    sim.link.disconnect(Side::B);
    sim.run_until(10, |s| !s.host.s.is_live() && !s.guest.s.is_live());
    assert_eq!(sim.host.s.close_reason(), Some(&CloseReason::PeerLeft));
    assert!(matches!(
        sim.guest.s.close_reason(),
        Some(CloseReason::Transport(_))
    ));
}

#[test]
fn handshake_adopts_host_parameters_and_wram() {
    let (link, ta, tb) = loopback_pair(LoopbackConfig::default(), 51);
    let wram: Vec<u8> = (0..WRAM_LEN).map(|i| (i ^ (i >> 8)) as u8).collect();
    let mut hc = SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    hc.delay = 3;
    hc.hash_interval = 30;
    hc.wram = Some(wram.clone());
    let mut gc = SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED);
    gc.delay = 6;
    let mut sim = Sim::with_configs(link, ta, tb, hc, gc, 51);
    sim.run_frames(90, 1_000);
    sim.check_streams(90);
    for p in [&mut sim.host, &mut sim.guest] {
        assert_eq!(p.s.delay(), 3);
        assert_eq!(p.s.hash_interval(), 30);
        assert_eq!(p.s.coop_flags(), COOP_TWO_LINKS);
        let started = p
            .events
            .iter()
            .find_map(|e| match e {
                SessionEvent::Started {
                    delay,
                    coop_flags,
                    hash_interval,
                    wram,
                } => Some((*delay, *coop_flags, *hash_interval, wram.clone())),
                _ => None,
            })
            .expect("Started");
        assert_eq!(started, (3, COOP_TWO_LINKS, 30, wram.clone()));
        assert!(p.saw(|e| matches!(e, SessionEvent::PeerHello { .. })));
        // Frames 29 and 59 compared; 89's remote hash may still be in flight.
        assert!(p.s.stats().hashes_ok >= 2, "{:?}", p.s.stats());
    }
}

#[test]
fn role_conflict_and_rom_mismatch_over_loopback() {
    let cases = [
        (
            SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS),
            SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS),
            CloseReason::RoleConflict,
        ),
        (
            SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS),
            SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS),
            CloseReason::RoleConflict,
        ),
        (
            SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS),
            SessionConfig::guest(CRC ^ 1, TRAPS, COOP_TWO_LINKS),
            CloseReason::Mismatch(MismatchKind::Rom),
        ),
        (
            SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS),
            SessionConfig::guest(CRC, TRAPS ^ 1, COOP_TWO_LINKS),
            CloseReason::Mismatch(MismatchKind::TrapSet),
        ),
        (
            SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED),
            SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS),
            CloseReason::Mismatch(MismatchKind::CoopFlags),
        ),
    ];
    for (i, (a, b, expect)) in cases.into_iter().enumerate() {
        let (link, ta, tb) = loopback_pair(LoopbackConfig::default(), 60 + i as u64);
        let mut sim = Sim::with_configs(link, ta, tb, a, b, 60 + i as u64);
        sim.run_ticks(10);
        for p in [&mut sim.host, &mut sim.guest] {
            assert_eq!(p.s.state(), SessionState::Closed, "case {i}");
            assert_eq!(p.s.close_reason(), Some(&expect), "case {i}");
            assert!(p.stream.is_empty());
        }
    }
}

/// Scripted raw peer.
struct Stub {
    state: LinkState,
    inbox: Vec<(Channel, Vec<u8>)>,
    sent: Vec<(Channel, Vec<u8>)>,
}

impl Stub {
    fn connected(inbox: Vec<Vec<u8>>) -> Self {
        Self {
            state: LinkState::Connected,
            inbox: inbox.into_iter().map(|b| (Channel::Reliable, b)).collect(),
            sent: Vec::new(),
        }
    }

    fn sent_messages(&self) -> Vec<Message> {
        self.sent
            .iter()
            .map(|(_, b)| Message::decode(b).expect("session sends valid packets"))
            .collect()
    }
}

impl Transport for Stub {
    fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), TransportError> {
        self.sent.push((channel, bytes.to_vec()));
        Ok(())
    }
    fn recv(&mut self) -> Vec<(Channel, Vec<u8>)> {
        std::mem::take(&mut self.inbox)
    }
    fn state(&self) -> LinkState {
        self.state.clone()
    }
}

fn host_hello() -> Hello {
    Hello {
        proto: PROTO_VERSION,
        role: Role::Host,
        rom_crc32: CRC,
        trapset_id: TRAPS,
        coop_flags: COOP_TWO_LINKS,
        delay: 2,
        hash_interval: 60,
    }
}

fn guest_against(packets: Vec<Vec<u8>>) -> (Session, Stub) {
    let mut s = Session::new(SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
    let mut t = Stub::connected(packets);
    s.update(&mut t, 0);
    (s, t)
}

#[test]
fn handshake_mismatches_send_bye() {
    let hello = |f: &dyn Fn(&mut Hello)| {
        let mut h = host_hello();
        f(&mut h);
        Message::Hello(h).encode()
    };
    let cases: Vec<(Vec<u8>, CloseReason, Option<ByeReason>)> = vec![
        (
            hello(&|h| h.proto = PROTO_VERSION + 1),
            CloseReason::Mismatch(MismatchKind::Proto),
            Some(ByeReason::VersionMismatch),
        ),
        (
            hello(&|h| h.rom_crc32 = 0),
            CloseReason::Mismatch(MismatchKind::Rom),
            Some(ByeReason::RomMismatch),
        ),
        (
            hello(&|h| h.trapset_id = 0),
            CloseReason::Mismatch(MismatchKind::TrapSet),
            Some(ByeReason::TrapSetMismatch),
        ),
        (
            hello(&|h| h.role = Role::Guest),
            CloseReason::RoleConflict,
            Some(ByeReason::RoleConflict),
        ),
        (
            hello(&|h| h.coop_flags = 0),
            CloseReason::Mismatch(MismatchKind::CoopFlags),
            Some(ByeReason::CoopFlagsMismatch),
        ),
        (
            hello(&|h| h.coop_flags = COOP_SPRITE_UNLIMITED),
            CloseReason::Mismatch(MismatchKind::CoopFlags),
            Some(ByeReason::CoopFlagsMismatch),
        ),
        (
            hello(&|h| h.delay = MAX_DELAY + 1),
            CloseReason::Protocol("host delay 9 out of range".into()),
            Some(ByeReason::Protocol),
        ),
        (
            {
                let mut b = host_hello_bytes();
                b[0] = 9;
                b
            },
            CloseReason::Mismatch(MismatchKind::WireVersion),
            Some(ByeReason::VersionMismatch),
        ),
        (
            Message::Bye(ByeReason::RomMismatch).encode(),
            CloseReason::Mismatch(MismatchKind::Rom),
            None,
        ),
    ];
    for (i, (packet, reason, bye)) in cases.into_iter().enumerate() {
        let (s, t) = guest_against(vec![packet]);
        assert_eq!(s.state(), SessionState::Closed, "case {i}");
        assert_eq!(s.close_reason(), Some(&reason), "case {i}");
        let sent = t.sent_messages();
        assert!(matches!(sent.first(), Some(Message::Hello(_))), "case {i}");
        assert_eq!(
            sent.iter().find_map(|m| match m {
                Message::Bye(r) => Some(*r),
                _ => None,
            }),
            bye,
            "case {i}"
        );
    }
}

fn host_hello_bytes() -> Vec<u8> {
    Message::Hello(host_hello()).encode()
}

#[test]
fn malformed_and_contradictory_start_are_protocol_errors() {
    let start = |f: &dyn Fn(&mut Start)| {
        let mut s = Start {
            frame: 0,
            delay: 2,
            coop_flags: COOP_TWO_LINKS,
            hash_interval: 60,
            wram: Vec::new(),
        };
        f(&mut s);
        Message::Start(s).encode()
    };
    let bad: Vec<Vec<Vec<u8>>> = vec![
        vec![vec![1, 0x7F]],
        vec![start(&|_| {})], // Start before Hello
        vec![host_hello_bytes(), start(&|s| s.delay = MAX_DELAY + 1)],
        vec![host_hello_bytes(), start(&|s| s.coop_flags = 3)],
        vec![host_hello_bytes(), start(&|s| s.frame = 5)],
    ];
    for (i, packets) in bad.into_iter().enumerate() {
        let (s, _) = guest_against(packets);
        assert!(
            matches!(s.close_reason(), Some(CloseReason::Protocol(_))),
            "case {i}: {:?}",
            s.close_reason()
        );
    }
    let (s, _) = guest_against(vec![host_hello_bytes(), start(&|_| {})]);
    assert_eq!(s.state(), SessionState::Running);

    let mut host = Session::new(SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
    let mut t = Stub::connected(vec![start(&|_| {})]);
    host.update(&mut t, 0);
    assert!(matches!(
        host.close_reason(),
        Some(CloseReason::Protocol(_))
    ));
}

#[test]
fn handshake_timeout() {
    let mut s = Session::new(SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
    let mut t = Stub::connected(Vec::new());
    s.update(&mut t, 1_000);
    assert_eq!(s.state(), SessionState::Handshake);
    s.update(&mut t, 30_999);
    assert_eq!(s.state(), SessionState::Handshake);
    s.update(&mut t, 31_000);
    assert_eq!(s.close_reason(), Some(&CloseReason::Timeout));
    assert!(t
        .sent_messages()
        .contains(&Message::Bye(ByeReason::Timeout)));
}

#[test]
fn idle_waits_for_peer_without_timing_out() {
    let mut s = Session::new(SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
    let mut t = Stub::connected(Vec::new());
    t.state = LinkState::Connecting;
    s.update(&mut t, 0);
    s.update(&mut t, 10 * 60 * 1000);
    assert_eq!(s.state(), SessionState::Idle);
    assert!(t.sent.is_empty());
    assert!(!s.latch_local(1));
    assert_eq!(s.poll(), None);
}

#[test]
fn link_failures_close() {
    for (link, reason) in [
        (LinkState::RoomFull, CloseReason::RoomFull),
        (LinkState::PeerLeft, CloseReason::PeerLeft),
        (
            LinkState::Closed("boom".into()),
            CloseReason::Transport("boom".into()),
        ),
    ] {
        let mut s = Session::new(SessionConfig::guest(CRC, TRAPS, COOP_TWO_LINKS)).unwrap();
        let mut t = Stub::connected(Vec::new());
        t.state = link;
        s.update(&mut t, 0);
        assert_eq!(s.close_reason(), Some(&reason));
        assert_eq!(s.take_events(), vec![SessionEvent::Closed(reason)]);
    }
}

#[test]
fn rewritten_remote_input_is_a_protocol_error() {
    let input = |pads: Vec<u8>| {
        Message::Input {
            ack: 2,
            first: 2,
            pads,
        }
        .encode()
    };
    let (mut s, mut t) = guest_against(vec![
        host_hello_bytes(),
        Message::Start(Start {
            frame: 0,
            delay: 2,
            coop_flags: COOP_TWO_LINKS,
            hash_interval: 60,
            wram: Vec::new(),
        })
        .encode(),
        input(vec![5, 6]),
        input(vec![5, 6]), // duplicate: fine
    ]);
    assert!(s.is_live());
    assert_eq!(s.frames_ready(), 2);
    t.inbox.push((Channel::Unreliable, input(vec![5, 7])));
    s.update(&mut t, 16);
    assert!(matches!(s.close_reason(), Some(CloseReason::Protocol(_))));
}

#[test]
fn config_validation() {
    use z2_net::ConfigError;
    assert_eq!(
        Session::new(SessionConfig::host(CRC, TRAPS, 0)).unwrap_err(),
        ConfigError::NoCoopFlags
    );
    let mut c = SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    c.delay = MAX_DELAY + 1;
    assert_eq!(
        Session::new(c).unwrap_err(),
        ConfigError::DelayOutOfRange(MAX_DELAY + 1)
    );
    let mut c = SessionConfig::host(CRC, TRAPS, COOP_TWO_LINKS);
    c.wram = Some(vec![0; 10]);
    assert_eq!(Session::new(c).unwrap_err(), ConfigError::BadWramLen(10));
}

#[test]
fn hash_schedule() {
    let (s, _) = guest_against(vec![host_hello_bytes()]);
    assert!(!s.needs_hash(0));
    assert!(s.needs_hash(59));
    assert!(!s.needs_hash(60));
    assert!(s.needs_hash(119));
}
