//! Native end-to-end: in-process z2-signal + two MatchboxTransports on 127.0.0.1.
//!
//! Needs only the loopback interface (host ICE candidates); the default STUN
//! servers being unreachable does not prevent a localhost connection.

#![cfg(all(feature = "matchbox", not(target_arch = "wasm32")))]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use z2_net::{
    hash_state, room_url, CloseReason, ConnectStage, IceConfig, LinkState, MatchboxTransport,
    RollbackConfig, RollbackError, RollbackEvent, RollbackRequest, RollbackSession, Session,
    SessionConfig, SessionEvent, StateRing, Transport, COOP_TWO_LINKS, SIGNALLING_TIMEOUT_MS,
};

const CRC: u32 = 0xBA32_2865;

fn start_signal() -> SocketAddr {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async move {
            let mut server = z2_signal::build_server(SocketAddr::from(([127, 0, 0, 1], 0)), false);
            let addr = server.bind().expect("bind signalling server");
            tx.send(addr).expect("report address");
            let _ = server.serve().await;
        });
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("signalling server did not start")
}

struct Peer {
    s: Session,
    t: MatchboxTransport,
    frames: Vec<(u32, u8, u8)>,
    hello: bool,
}

impl Peer {
    fn tick(&mut self, now_ms: u64) {
        self.s.update(&mut self.t, now_ms);
        let role = self.s.role().as_u8() as u32;
        for _ in 0..4 {
            let pad = ((self.s.frame() * 7 + role * 3) & 0xFF) as u8;
            self.s.latch_local(pad);
            let Some(fr) = self.s.poll() else { break };
            self.frames.push(fr);
            if self.s.needs_hash(fr.0) {
                self.s.report_hash(fr.0, hash_state(&[&[fr.1, fr.2]]));
            }
        }
        self.s.update(&mut self.t, now_ms);
        for e in self.s.take_events() {
            match e {
                SessionEvent::PeerHello { .. } => self.hello = true,
                SessionEvent::Closed(r) => panic!("{:?} closed: {r}", self.s.role()),
                _ => {}
            }
        }
    }
}

#[test]
fn matchbox_pair_runs_120_frames_and_third_peer_is_refused() {
    let addr = start_signal();
    let url = room_url(&format!("ws://{addr}"), "itest").unwrap();
    let mut host = Peer {
        s: Session::new(SessionConfig::host(CRC, 42, COOP_TWO_LINKS)).unwrap(),
        t: MatchboxTransport::connect(&url, None),
        frames: Vec::new(),
        hello: false,
    };
    let mut guest = Peer {
        s: Session::new(SessionConfig::guest(CRC, 42, COOP_TWO_LINKS)).unwrap(),
        t: MatchboxTransport::connect(&url, None),
        frames: Vec::new(),
        hello: false,
    };
    let clock = Instant::now();
    let now = || clock.elapsed().as_millis() as u64;
    let deadline = Duration::from_secs(60);

    while host.frames.len() < 120 || guest.frames.len() < 120 {
        assert!(
            clock.elapsed() < deadline,
            "timeout: host {:?}/{:?} f={} guest {:?}/{:?} f={}",
            host.s.state(),
            host.t.state(),
            host.frames.len(),
            guest.s.state(),
            guest.t.state(),
            guest.frames.len()
        );
        let t = now();
        host.tick(t);
        guest.tick(t);
        std::thread::sleep(Duration::from_millis(4));
    }
    assert!(host.hello && guest.hello);
    assert_eq!(host.frames[..120], guest.frames[..120]);
    assert!(host.s.stats().hashes_ok >= 1 && guest.s.stats().hashes_ok >= 1);

    // A third peer in the same room is refused by z2-signal (HTTP 409).
    let mut third = MatchboxTransport::connect(&url, None);
    while !third.state().is_terminal() {
        assert!(clock.elapsed() < deadline * 2, "third peer never refused");
        let t = now();
        host.tick(t);
        guest.tick(t);
        let _ = third.recv();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(third.state(), LinkState::RoomFull);
    assert!(host.s.is_live() && guest.s.is_live());

    // Orderly quit reaches the guest.
    host.s.close();
    host.s.update(&mut host.t, now());
    let quit_by = Instant::now();
    while guest.s.is_live() {
        assert!(
            quit_by.elapsed() < Duration::from_secs(20),
            "guest never saw the quit"
        );
        guest.s.update(&mut guest.t, now());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        guest.s.close_reason(),
        Some(CloseReason::PeerQuit | CloseReason::PeerLeft)
    ));
}

/// Pump both transports until `done` holds, recording every stage each one
/// reports (deduplicated), with a deadline.
fn pump_until(
    a: &mut MatchboxTransport,
    b: &mut MatchboxTransport,
    limit: Duration,
    stages: &mut [Vec<ConnectStage>; 2],
    done: impl Fn(&MatchboxTransport, &MatchboxTransport) -> bool,
) {
    let t0 = Instant::now();
    while !done(a, b) {
        assert!(
            t0.elapsed() < limit,
            "deadline: a {:?}/{:?} b {:?}/{:?} (stages seen {stages:?})",
            a.stage(),
            a.state(),
            b.stage(),
            b.state()
        );
        for (i, t) in [&mut *a, &mut *b].into_iter().enumerate() {
            let _ = t.recv();
            if stages[i].last() != Some(&t.stage()) {
                stages[i].push(t.stage());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn stages_progress_in_order_and_no_ice_servers_connects_fast() {
    let addr = start_signal();
    let url = room_url(&format!("ws://{addr}"), "stages").unwrap();
    let mut first = MatchboxTransport::connect(&url, Some(IceConfig::none()));
    assert_eq!(first.stage(), ConnectStage::Signalling);
    let mut stages = [vec![ConnectStage::Signalling], Vec::new()];

    // Alone in the room: the first peer settles on "waiting for the other player".
    let t0 = Instant::now();
    while first.stage() != ConnectStage::WaitingForPeer {
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "never joined: {:?}",
            first.state()
        );
        let _ = first.recv();
        std::thread::sleep(Duration::from_millis(5));
    }
    stages[0].push(ConnectStage::WaitingForPeer);

    let mut second = MatchboxTransport::connect(&url, Some(IceConfig::none()));
    let linked = Instant::now();
    pump_until(
        &mut first,
        &mut second,
        Duration::from_secs(10),
        &mut stages,
        |a, b| a.state() == LinkState::Connected && b.state() == LinkState::Connected,
    );
    assert!(
        linked.elapsed() < Duration::from_secs(5),
        "a loopback link with no ICE servers took {:?}",
        linked.elapsed()
    );
    assert_eq!(
        stages[0],
        [
            ConnectStage::Signalling,
            ConnectStage::WaitingForPeer,
            ConnectStage::EstablishingLink,
            ConnectStage::Connected
        ]
    );
    assert_eq!(stages[1].last(), Some(&ConnectStage::Connected));
    for w in stages.iter() {
        assert!(
            w.windows(2).all(|p| p[0] < p[1]),
            "stages went backwards: {w:?}"
        );
    }

    // Leaving ends the other side's link, and closing is idempotent.
    first.close();
    first.close();
    let _ = first.recv();
    assert_eq!(first.stage(), ConnectStage::Ended);
    drop(first);
    let t0 = Instant::now();
    while !second.state().is_terminal() {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "peer never saw the leave"
        );
        let _ = second.recv();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(second.stage(), ConnectStage::Ended);
}

#[test]
fn unreachable_signal_server_fails_with_an_actionable_reason() {
    // Bind then drop a listener: nothing listens on that port any more.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = room_url(&format!("ws://127.0.0.1:{port}"), "nobody").unwrap();
    let mut t = MatchboxTransport::connect(&url, None);
    let t0 = Instant::now();
    while !t.state().is_terminal() {
        assert!(
            t0.elapsed() < Duration::from_millis(SIGNALLING_TIMEOUT_MS + 5_000),
            "never gave up: {:?}",
            t.stage()
        );
        assert_eq!(t.stage(), ConnectStage::Signalling);
        let _ = t.recv();
        std::thread::sleep(Duration::from_millis(20));
    }
    let LinkState::Closed(reason) = t.state() else {
        panic!("expected Closed, got {:?}", t.state());
    };
    assert!(
        reason.contains("z2-signal"),
        "reason should say what to check: {reason}"
    );
    assert_eq!(t.stage(), ConnectStage::Ended);
}

/// Rollback peer over real WebRTC with an FNV stand-in game `(frame, acc)`.
struct RbPeer {
    s: RollbackSession<MatchboxTransport>,
    game: (u32, u64),
    ring: StateRing<(u32, u64)>,
    latest: BTreeMap<u32, u64>,
    final_sums: BTreeMap<u32, u64>,
}

impl RbPeer {
    fn new(cfg: RollbackConfig, url: &str) -> Self {
        let cap = usize::from(cfg.max_prediction) + 2;
        Self {
            s: RollbackSession::new(cfg, MatchboxTransport::connect(url, None)).unwrap(),
            game: (0, 0),
            ring: StateRing::new(cap),
            latest: BTreeMap::new(),
            final_sums: BTreeMap::new(),
        }
    }

    fn sum(&self) -> u64 {
        hash_state(&[&self.game.0.to_le_bytes(), &self.game.1.to_le_bytes()])
    }

    fn tick(&mut self, now_ms: u64) {
        let role = self.s.role().as_u8() as u32;
        let pad = ((self.s.frame() / 16 * 7 + role * 3) & 0xFF) as u8;
        self.s.add_local_input(pad);
        let reqs = match self.s.advance(now_ms) {
            Ok(r) => r,
            Err(RollbackError::Closed(r)) => panic!("{:?} closed: {r}", self.s.role()),
            Err(e) => panic!("{e}"),
        };
        for r in reqs {
            match r {
                RollbackRequest::SaveState { frame } => {
                    if self.s.needs_checksum(frame) {
                        let c = self.sum();
                        self.s.report_checksum(frame, c);
                    }
                    self.ring.save(frame, self.game);
                }
                RollbackRequest::LoadState { frame } => {
                    self.game = *self.ring.get(frame).expect("state held");
                }
                RollbackRequest::Advance {
                    frame, pad1, pad2, ..
                } => {
                    assert_eq!(self.game.0, frame);
                    self.game.1 = hash_state(&[&self.game.1.to_le_bytes(), &[pad1, pad2]]);
                    self.game.0 += 1;
                    self.latest.insert(frame, self.sum());
                }
            }
        }
        let rest = self.latest.split_off(&self.s.final_frame());
        for (f, c) in std::mem::replace(&mut self.latest, rest) {
            self.final_sums.insert(f, c);
        }
        for e in self.s.events() {
            if let RollbackEvent::Disconnected { reason } = e {
                panic!("{:?} disconnected: {reason}", self.s.role());
            }
        }
    }
}

#[test]
fn matchbox_rollback_pair_confirms_300_frames() {
    let addr = start_signal();
    let url = room_url(&format!("ws://{addr}"), "rbtest").unwrap();
    let mut hc = RollbackConfig::host(CRC, 42, COOP_TWO_LINKS);
    hc.input_delay = 1;
    hc.check_interval = 10;
    let mut host = RbPeer::new(hc, &url);
    let mut guest = RbPeer::new(RollbackConfig::guest(CRC, 42, COOP_TWO_LINKS), &url);
    let clock = Instant::now();
    let now = || clock.elapsed().as_millis() as u64;
    let deadline = Duration::from_secs(60);
    while host.final_sums.len() < 300 || guest.final_sums.len() < 300 {
        assert!(
            clock.elapsed() < deadline,
            "timeout: host {:?}/{:?} f={} guest {:?}/{:?} f={}",
            host.s.state(),
            host.s.transport().state(),
            host.s.frame(),
            guest.s.state(),
            guest.s.transport().state(),
            guest.s.frame()
        );
        let t = now();
        host.tick(t);
        guest.tick(t);
        std::thread::sleep(Duration::from_millis(4));
    }
    let n = 300;
    let h: Vec<_> = host.final_sums.range(..n).collect();
    let g: Vec<_> = guest.final_sums.range(..n).collect();
    assert_eq!(h, g);
    assert!(host.s.stats().checksums_ok >= 20 && guest.s.stats().checksums_ok >= 20);

    host.s.close();
    let quit_by = Instant::now();
    while guest.s.is_running() {
        assert!(
            quit_by.elapsed() < Duration::from_secs(20),
            "guest never saw the quit"
        );
        guest.s.poll(now());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(
        guest.s.close_reason(),
        Some(CloseReason::PeerQuit | CloseReason::PeerLeft)
    ));
}
