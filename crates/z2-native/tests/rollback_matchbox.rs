//! Desktop rollback end to end on one machine: an in-process z2-signal server
//! and two real games, each driven by the frontend's
//! [`netplay::rollback_tick`] over its own WebRTC `MatchboxTransport` on
//! 127.0.0.1. Both peers must agree on every settled frame they recorded and
//! on the sessions' confirmed-frame checksums.
//!
//! ROM-gated (self-skips unless `Z2_ROM` names an existing file) and needs
//! the `netplay` feature (the default).

#![cfg(feature = "netplay")]

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use z2_native::app::{self, Emu, Features};
use z2_native::netplay::{self, RollbackLink};
use z2_net::{
    room_url, MatchboxTransport, RollbackConfig, RollbackEvent, COOP_SPRITE_UNLIMITED,
    COOP_TWO_LINKS,
};

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
    name: &'static str,
    link: RollbackLink<MatchboxTransport>,
    emu: Emu,
    body: Vec<u8>,
    seed: u32,
    latest: BTreeMap<u32, u64>,
    final_sums: BTreeMap<u32, u64>,
}

impl Peer {
    fn tick(&mut self, now_ms: u64) {
        let target = self.link.session.frame() + u32::from(self.link.session.input_delay());
        // Held pads that change every 12 frames, different per peer.
        let pad = ((target / 12)
            .wrapping_mul(2_654_435_761)
            .wrapping_add(self.seed)
            >> 13) as u8
            & !0x30;
        let out = netplay::rollback_tick(&mut self.link, &mut self.emu, pad, now_ms, None);
        assert!(
            self.link.last_error.is_none(),
            "{}: {:?}",
            self.name,
            self.link.last_error
        );
        if let Some(r) = out.loaded_from {
            let _ = self.latest.split_off(&(r + 1));
        }
        for ev in self.link.take_events() {
            match ev {
                RollbackEvent::Running {
                    coop_flags, wram, ..
                } => {
                    self.emu = netplay::session_emu(&self.body, 44_100, false, coop_flags, &wram)
                        .expect("session emulator");
                    self.link.started = true;
                }
                RollbackEvent::WaitRecommendation { skip_frames } => {
                    self.link.skip_ticks = u32::from(skip_frames);
                }
                RollbackEvent::DesyncDetected { frame, .. } => {
                    panic!("{}: desync at frame {frame}", self.name)
                }
                RollbackEvent::Disconnected { reason } => {
                    panic!("{}: disconnected: {reason}", self.name)
                }
                _ => {}
            }
        }
        if self.link.started && (out.advanced > 0 || out.replayed > 0) {
            let f = self.link.session.frame();
            let sum = netplay::rollback_checksum(&self.emu.game.save_state());
            self.latest.insert(f, sum);
            let settled = self.link.session.final_frame();
            let rest = self.latest.split_off(&(settled + 1));
            for (frame, c) in std::mem::replace(&mut self.latest, rest) {
                self.final_sums.insert(frame, c);
            }
        }
    }
}

/// Run `body` on a thread with an explicit stack. An `Emu` is 83 KiB and an
/// unoptimized build moves it by value through `run_pair`, `Peer::tick` and
/// the constructors behind `session_emu`; measured on a debug build, that
/// needs more than the 2 MiB a test thread gets. A panic in `body` fails the
/// test with its own message.
fn tall_stack(body: impl FnOnce() + Send + 'static) {
    let name = std::thread::current().name().unwrap_or("test").to_string();
    let worker = std::thread::Builder::new()
        .name(name)
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .expect("spawn tall-stack test thread");
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn desktop_rollback_pair_over_localhost_matchbox() {
    let test = "desktop_rollback_pair_over_localhost_matchbox";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    tall_stack(move || run_pair(&raw));
}

fn run_pair(raw: &[u8]) {
    let body = z2_assets::rom::strip_ines_header(raw).to_vec();
    let frames: u32 = if cfg!(debug_assertions) { 120 } else { 600 };
    let feats = Features {
        coop: true,
        record: false,
    };
    let host_emu = app::emu_from_rom_body_with(&body, 44_100, feats).expect("host emulator");
    let guest_emu = app::emu_from_rom_body_with(&body, 44_100, feats).expect("guest emulator");
    let crc = z2_assets::rom::EXPECTED_BODY_CRC32;

    let addr = start_signal();
    let url = room_url(&format!("ws://{addr}"), "desktop-rb").expect("room url");
    let mut hcfg = RollbackConfig::host(crc, host_emu.trapset_id, COOP_TWO_LINKS);
    hcfg.check_interval = 20;
    // No input delay: every remote press arrives after its frame ran, so
    // pad changes are corrected by real rollbacks.
    hcfg.input_delay = 0;
    hcfg.wram = Some(host_emu.game.wram().to_vec());
    let gcfg = RollbackConfig::guest(
        crc,
        guest_emu.trapset_id,
        COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED,
    );
    let mut host = Peer {
        name: "host",
        link: RollbackLink::new(hcfg, MatchboxTransport::connect(&url, None), "desktop-rb")
            .expect("host link"),
        emu: host_emu,
        body: body.clone(),
        seed: 0x1111,
        latest: BTreeMap::new(),
        final_sums: BTreeMap::new(),
    };
    let mut guest = Peer {
        name: "guest",
        link: RollbackLink::new(gcfg, MatchboxTransport::connect(&url, None), "desktop-rb")
            .expect("guest link"),
        emu: guest_emu,
        body,
        seed: 0x7777,
        latest: BTreeMap::new(),
        final_sums: BTreeMap::new(),
    };

    let clock = Instant::now();
    let deadline = Duration::from_secs(120);
    let settled = |p: &Peer| p.link.session.final_frame();
    // One tick per 16 ms of wall clock, like the windowed loop at 60 Hz.
    let mut next = Duration::ZERO;
    while settled(&host) < frames || settled(&guest) < frames {
        assert!(
            clock.elapsed() < deadline,
            "timeout: host {:?} f={} guest {:?} f={}",
            host.link.session.state(),
            host.link.session.frame(),
            guest.link.session.state(),
            guest.link.session.frame()
        );
        let now = clock.elapsed();
        if now < next {
            host.link.session.poll(now.as_millis() as u64);
            guest.link.session.poll(now.as_millis() as u64);
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        next += Duration::from_millis(16);
        let ms = now.as_millis() as u64;
        host.tick(ms);
        guest.tick(ms);
    }
    let mut common_frames = 0;
    for (f, h) in host.final_sums.range(..=frames) {
        if let Some(g) = guest.final_sums.get(f) {
            assert_eq!(h, g, "state at frame {f} differs");
            common_frames += 1;
        }
    }
    let (hs, gs) = (host.link.session.stats(), guest.link.session.stats());
    eprintln!(
        "rollback_matchbox: {frames} frames settled; {common_frames} common frames identical; \
         host rtt {:?} rollbacks {} checksums ok {}; guest rtt {:?} rollbacks {} checksums ok {}",
        hs.rtt_ms, hs.rollbacks, hs.checksums_ok, gs.rtt_ms, gs.rollbacks, gs.checksums_ok
    );
    assert!(common_frames >= frames / 3, "{common_frames} common frames");
    let min = frames / 20 - 3;
    assert!(hs.checksums_ok >= min && gs.checksums_ok >= min);

    host.link
        .close_and_flush(clock.elapsed().as_millis() as u64);
    let quit_by = Instant::now();
    while guest.link.session.is_running() {
        assert!(
            quit_by.elapsed() < Duration::from_secs(20),
            "guest never saw the quit"
        );
        guest.link.session.poll(clock.elapsed().as_millis() as u64);
        std::thread::sleep(Duration::from_millis(10));
    }
}
