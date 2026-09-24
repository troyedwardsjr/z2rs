//! Rollback integration: two independent games, each driven by its own
//! `RollbackSession` through the frontend's [`netplay::rollback_tick`], over
//! the deterministic loopback transport with latency, jitter and loss. Every
//! frame both peers consider final must have the identical full-state
//! checksum, and the sessions' own confirmed-frame checksums must match.
//!
//! The ROM-backed runs self-skip unless `Z2_ROM` names an existing file; a
//! synthetic-cartridge run always executes so the wiring is covered in
//! public CI.

mod common;

use std::collections::BTreeMap;

use z2_native::app::{self, Emu, Features};
use z2_native::netplay::{self, RollbackLink};
use z2_net::{
    loopback_pair, LoopbackConfig, LoopbackTransport, RollbackConfig, RollbackEvent,
    COOP_SPRITE_UNLIMITED, COOP_TWO_LINKS,
};

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

/// Held random pads (a new pad every 2..=25 frames, Start/Select rare).
fn scripted_pads(seed: u64, frames: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let mut pad = rng.next() as u8;
        if !rng.next().is_multiple_of(8) {
            pad &= !0x30;
        }
        let hold = 2 + (rng.next() % 24) as usize;
        out.extend(std::iter::repeat_n(pad, hold));
    }
    out.truncate(frames);
    out
}

type Rebuild = Box<dyn Fn(u32, &[u8]) -> Emu>;

struct Peer {
    name: &'static str,
    link: RollbackLink<LoopbackTransport>,
    emu: Emu,
    rebuild: Rebuild,
    /// Pad for each session frame (indexed by the frame it applies to).
    pads: Vec<u8>,
    /// Full-state checksums keyed by the frame the state is at: the live game
    /// at each tick end, and the ring's saves for frames a rollback replayed.
    latest: BTreeMap<u32, u64>,
    /// Checksums of states no rollback can change any more.
    final_sums: BTreeMap<u32, u64>,
    advanced: u64,
    replayed: u64,
}

impl Peer {
    fn tick(&mut self, now_ms: u64) {
        let target = self.link.session.frame() + u32::from(self.link.session.input_delay());
        let pad = self.pads.get(target as usize).copied().unwrap_or(0);
        let out = netplay::rollback_tick(&mut self.link, &mut self.emu, pad, now_ms, None);
        assert!(
            self.link.last_error.is_none(),
            "{}: {:?}",
            self.name,
            self.link.last_error
        );
        assert!(
            self.emu.game.apu.log.is_empty(),
            "{}: APU writes left undrained after a tick",
            self.name
        );
        self.advanced += u64::from(out.advanced);
        self.replayed += u64::from(out.replayed);
        if let Some(r) = out.loaded_from {
            // States after the restored frame were re-simulated within this
            // tick. The replay saved each of them into the ring again, so
            // re-record them from there: only dropping them leaves a hole of
            // up to `max_prediction` frames per rollback, and while both pads
            // change often the two peers' holes leave no frame in common.
            let _ = self.latest.split_off(&(r + 1));
            for f in r + 1..self.link.session.frame() {
                let state = self.link.ring.get(f).unwrap_or_else(|| {
                    panic!("{}: replayed state {f} is not in the ring", self.name)
                });
                self.latest.insert(f, netplay::rollback_checksum(state));
            }
        }
        for ev in self.link.take_events() {
            match ev {
                RollbackEvent::Running {
                    coop_flags, wram, ..
                } => {
                    self.emu = (self.rebuild)(coop_flags, &wram);
                    self.link.started = true;
                }
                RollbackEvent::WaitRecommendation { skip_frames } => {
                    self.link.skip_ticks = u32::from(skip_frames);
                }
                RollbackEvent::DesyncDetected {
                    frame,
                    local,
                    remote,
                } => panic!(
                    "{}: desync at frame {frame} (local {local:#018x}, remote {remote:#018x})",
                    self.name
                ),
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
            // The state at frame f is final once every frame before it is.
            let settled = self.link.session.final_frame();
            let rest = self.latest.split_off(&(settled + 1));
            for (frame, c) in std::mem::replace(&mut self.latest, rest) {
                self.final_sums.insert(frame, c);
            }
        }
    }
}

struct Net {
    name: &'static str,
    cfg: LoopbackConfig,
    delay: u8,
    seed: u64,
}

fn run_pair(
    net: &Net,
    make: &dyn Fn() -> (Emu, Rebuild),
    host_pads: Vec<u8>,
    guest_pads: Vec<u8>,
    frames: u32,
) {
    let (line, ta, tb) = loopback_pair(net.cfg, net.seed);
    let (host_emu, host_rebuild) = make();
    let (guest_emu, guest_rebuild) = make();
    assert_eq!(host_emu.trapset_id, guest_emu.trapset_id);
    let crc = z2_assets::rom::EXPECTED_BODY_CRC32;

    let mut hcfg = RollbackConfig::host(crc, host_emu.trapset_id, COOP_TWO_LINKS);
    hcfg.input_delay = net.delay;
    hcfg.check_interval = 30;
    hcfg.wram = Some(host_emu.game.wram().to_vec());
    let gcfg = RollbackConfig::guest(
        crc,
        guest_emu.trapset_id,
        COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED,
    );
    let mut host = Peer {
        name: "host",
        link: RollbackLink::new(hcfg, ta, "loop").expect("host config"),
        emu: host_emu,
        rebuild: host_rebuild,
        pads: host_pads,
        latest: BTreeMap::new(),
        final_sums: BTreeMap::new(),
        advanced: 0,
        replayed: 0,
    };
    let mut guest = Peer {
        name: "guest",
        link: RollbackLink::new(gcfg, tb, "loop").expect("guest config"),
        emu: guest_emu,
        rebuild: guest_rebuild,
        pads: guest_pads,
        latest: BTreeMap::new(),
        final_sums: BTreeMap::new(),
        advanced: 0,
        replayed: 0,
    };

    let settled = |p: &Peer| p.link.session.final_frame();
    let max_ticks = u64::from(frames) * 4 + 2_000;
    let mut tick = 0u64;
    while (settled(&host) < frames || settled(&guest) < frames) && tick < max_ticks {
        line.advance(1);
        let now = tick * TICK_MS;
        host.tick(now);
        guest.tick(now);
        tick += 1;
    }
    let (hs, gs) = (host.link.session.stats(), guest.link.session.stats());
    eprintln!(
        "rollback_loopback [{}]: {tick} ticks; host settled {} rollbacks {} replayed {} \
         max depth {} checksums ok {} waits {}; guest settled {} rollbacks {} replayed {} \
         max depth {} checksums ok {} waits {}",
        net.name,
        settled(&host),
        hs.rollbacks,
        hs.rollback_frames,
        hs.max_prediction_depth,
        hs.checksums_ok,
        hs.wait_recommendations,
        settled(&guest),
        gs.rollbacks,
        gs.rollback_frames,
        gs.max_prediction_depth,
        gs.checksums_ok,
        gs.wait_recommendations,
    );
    assert!(
        settled(&host) >= frames && settled(&guest) >= frames,
        "[{}] stalled: host {} guest {} of {frames} after {tick} ticks",
        net.name,
        settled(&host),
        settled(&guest)
    );
    assert_eq!(host.emu.game.exec_errors, 0, "host faulted");
    assert_eq!(guest.emu.game.exec_errors, 0, "guest faulted");

    // Each peer records the state at every frame from 1 on (a tick ends one
    // frame after the last, or on the same frame while stalled; a rollback
    // re-records what it replayed), so the whole run is compared, not a sample.
    let final_sum = |p: &Peer, f: u32| {
        *p.final_sums.get(&f).unwrap_or_else(|| {
            panic!(
                "[{}] {} recorded no final state at frame {f}",
                net.name, p.name
            )
        })
    };
    for f in 1..=frames {
        assert_eq!(
            final_sum(&host, f),
            final_sum(&guest, f),
            "[{}] state at frame {f} differs",
            net.name
        );
    }
    let min_checks = frames / 30 - 4;
    assert!(
        hs.checksums_ok >= min_checks && gs.checksums_ok >= min_checks,
        "[{}] session checksums: host {} guest {}",
        net.name,
        hs.checksums_ok,
        gs.checksums_ok
    );
    assert!(host.advanced >= u64::from(frames) && guest.advanced >= u64::from(frames));
    eprintln!(
        "rollback_loopback [{}]: final states at frames 1..={frames} identical",
        net.name
    );
}

fn nets() -> Vec<Net> {
    vec![
        Net {
            name: "lat 2 ticks, jitter 3, loss 5%, delay 2",
            cfg: LoopbackConfig {
                latency_ticks: 2,
                jitter_ticks: 3,
                loss_per_mille: 50,
                connect_after_ticks: 3,
            },
            delay: 2,
            seed: 11,
        },
        Net {
            name: "lat 5 ticks, jitter 6, loss 15%, delay 1",
            cfg: LoopbackConfig {
                latency_ticks: 5,
                jitter_ticks: 6,
                loss_per_mille: 150,
                connect_after_ticks: 7,
            },
            delay: 1,
            seed: 29,
        },
        Net {
            name: "lat 1 tick, jitter 2, loss 2%, delay 0",
            cfg: LoopbackConfig {
                latency_ticks: 1,
                jitter_ticks: 2,
                loss_per_mille: 20,
                connect_after_ticks: 2,
            },
            delay: 0,
            seed: 41,
        },
    ]
}

/// A minimal cartridge that boots into an `RTS` loop.
fn synthetic_emu() -> Emu {
    let mut img = vec![0u8; 16 + 8 * 0x4000 + 8 * 0x2000];
    img[0..4].copy_from_slice(b"NES\x1a");
    img[4] = 8;
    img[5] = 8;
    img[6] = 0x10;
    let prg_len = 8 * 0x4000;
    img[16] = 0x60;
    let v = 16 + prg_len - 4;
    img[v] = 0x00;
    img[v + 1] = 0xC0;
    let mut game = z2_core::game::Game::from_ines(&img).expect("synthetic boots");
    game.set_coop(true);
    game.reset();
    Emu {
        game,
        apu: z2_apu::Apu::new(44_100),
        trapset_id: 1,
        trapset_base: 1,
    }
}

/// Run `body` on a thread with an explicit stack. An `Emu` is 83 KiB and an
/// unoptimized build moves it by value through `run_pair`, `Peer::tick` and
/// the constructors behind `Rebuild`; measured on a debug build, the ROM run
/// needs more than the 2 MiB a test thread gets and the synthetic run more
/// than 1.25 MiB of it. A panic in `body` fails the test with its own message.
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
fn synthetic_games_agree_under_rollback_over_loopback() {
    tall_stack(synthetic_run);
}

fn synthetic_run() {
    for net in nets() {
        let make = || -> (Emu, Rebuild) {
            (
                synthetic_emu(),
                Box::new(|_, wram: &[u8]| {
                    let mut e = synthetic_emu();
                    e.game.wram.copy_from_slice(wram);
                    e
                }),
            )
        };
        run_pair(
            &net,
            &make,
            scripted_pads(3, 600),
            scripted_pads(5, 600),
            300,
        );
    }
}

#[test]
fn rom_games_agree_under_rollback_over_loopback() {
    let test = "rom_games_agree_under_rollback_over_loopback";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    tall_stack(move || rom_run(&raw));
}

fn rom_run(raw: &[u8]) {
    let body = z2_assets::rom::strip_ines_header(raw).to_vec();
    let frames: u32 = if cfg!(debug_assertions) { 300 } else { 3_000 };
    let n = frames as usize + 64;
    // Player 1 replays the any% movie (indexed by the frame each pad applies
    // to) when the corpus is present, so the run reaches side-view and the
    // overworld; player 2 plays held random pads.
    let host_pads = common::var_present("Z2_CORPUS")
        .map(|c| std::path::Path::new(&c).join("movies").join("anypct.bk2"))
        .filter(|p| p.is_file())
        .map(|p| app::load_movie_track(&p).expect("anypct.bk2 parses"))
        .unwrap_or_else(|| scripted_pads(7, n));
    let make = || -> (Emu, Rebuild) {
        let feats = Features {
            coop: true,
            wide_gameplay: None,
            record: false,
            margin_sprites: false,
        };
        let emu = app::emu_from_rom_body_with(&body, 44_100, feats).expect("emulator");
        let body = body.clone();
        (
            emu,
            Box::new(move |flags, wram: &[u8]| {
                netplay::session_emu(&body, 44_100, false, None, flags, wram)
                    .expect("session emulator")
            }),
        )
    };
    for net in nets() {
        run_pair(&net, &make, host_pads.clone(), scripted_pads(13, n), frames);
    }
}
