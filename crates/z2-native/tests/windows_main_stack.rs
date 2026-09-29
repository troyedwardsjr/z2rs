//! Regression: a rollback session start must fit the 1 MiB stack Windows
//! gives its main thread.
//!
//! Windows players crashed with "thread 'main' has overflowed its stack"
//! (exit code 0xC00000FD) right after "netplay: synchronizing (66%)": the
//! `Running` event rebuilds the emulator with `netplay::session_emu`, and
//! with the 60 KiB framebuffer inline in `Game` that constructor moved an
//! 83 KiB `Emu` by value half a dozen times (about 600 KiB of a release
//! build's stack, 1.6 MiB of a debug build's) on top of the event loop.
//! macOS and Linux give the main thread 8 MiB, so it never showed there.
//!
//! Each test runs the handshake, the rebuild at `Running` and a stretch of
//! rollback frames on a thread with exactly 1 MiB of stack, in the order the
//! windowed loop does them. A synthetic cartridge always runs; the real game
//! runs when `Z2_ROM` names the ROM.

mod common;

use z2_core::game::Game;
use z2_native::app::{self, Emu, Features};
use z2_native::netplay::{self, RollbackLink};
use z2_net::{
    loopback_pair, LoopbackConfig, LoopbackTransport, RollbackConfig, RollbackEvent, COOP_TWO_LINKS,
};

/// The main-thread stack reservation of a Windows executable linked with
/// the default `/STACK`.
const WINDOWS_MAIN_STACK: usize = 1 << 20;

const TICK_MS: u64 = 16;

/// Run `body` on a thread with the Windows main-thread stack. An overflow
/// aborts the whole test binary, which fails the run just as loudly.
fn on_windows_main_stack(body: impl FnOnce() + Send + 'static) {
    let worker = std::thread::Builder::new()
        .name("windows-main-stack".into())
        .stack_size(WINDOWS_MAIN_STACK)
        .spawn(body)
        .expect("spawn 1 MiB test thread");
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}

type Rebuild<'a> = dyn Fn(u32, &[u8]) -> Emu + 'a;

struct Peer {
    link: RollbackLink<LoopbackTransport>,
    emu: Emu,
    progress: Vec<u8>,
    running: bool,
}

impl Peer {
    fn tick(&mut self, now_ms: u64, pad: u8, rebuild: &Rebuild<'_>) {
        let _ = netplay::rollback_tick(&mut self.link, &mut self.emu, pad, now_ms, None);
        assert!(self.link.last_error.is_none(), "{:?}", self.link.last_error);
        for ev in self.link.take_events() {
            match ev {
                RollbackEvent::Synchronizing { progress } => self.progress.push(progress),
                // What the windowed loop does on `Running`: build a fresh
                // emulator with the host's save and replace the live one.
                RollbackEvent::Running {
                    coop_flags, wram, ..
                } => {
                    let fresh = rebuild(coop_flags, &wram);
                    self.emu = fresh;
                    self.link.started = true;
                    self.running = true;
                }
                RollbackEvent::WaitRecommendation { skip_frames } => {
                    self.link.skip_ticks = u32::from(skip_frames);
                }
                RollbackEvent::DesyncDetected { frame, .. } => panic!("desync at frame {frame}"),
                RollbackEvent::Disconnected { reason } => panic!("disconnected: {reason}"),
                _ => {}
            }
        }
    }
}

/// Connect two peers over a lossy loopback, start the session and play
/// `frames` settled frames.
fn session(make: &dyn Fn() -> Emu, rebuild: &Rebuild<'_>, frames: u32) {
    let cfg = LoopbackConfig {
        latency_ticks: 2,
        jitter_ticks: 2,
        loss_per_mille: 30,
        connect_after_ticks: 3,
    };
    let (line, ta, tb) = loopback_pair(cfg, 17);
    let host_emu = make();
    let guest_emu = make();
    let crc = z2_assets::rom::EXPECTED_BODY_CRC32;
    let mut hcfg = RollbackConfig::host(crc, host_emu.trapset_id, COOP_TWO_LINKS);
    hcfg.input_delay = 2;
    hcfg.check_interval = 30;
    hcfg.wram = Some(host_emu.game.wram().to_vec());
    let gcfg = RollbackConfig::guest(crc, guest_emu.trapset_id, COOP_TWO_LINKS);
    let mut host = Peer {
        link: RollbackLink::new(hcfg, ta, "stack").expect("host config"),
        emu: host_emu,
        progress: Vec::new(),
        running: false,
    };
    let mut guest = Peer {
        link: RollbackLink::new(gcfg, tb, "stack").expect("guest config"),
        emu: guest_emu,
        progress: Vec::new(),
        running: false,
    };
    let max_ticks = u64::from(frames) * 4 + 2_000;
    let mut tick = 0u64;
    let settled = |p: &Peer| p.link.session.final_frame();
    while (settled(&host) < frames || settled(&guest) < frames) && tick < max_ticks {
        line.advance(1);
        let now = tick * TICK_MS;
        // Held pads that change every 16 frames, walking and jumping.
        let pad = [0x00, 0x01, 0x81, 0x02, 0x42][(tick / 16 % 5) as usize];
        host.tick(now, pad, rebuild);
        guest.tick(now, pad ^ 0x03, rebuild);
        tick += 1;
    }
    for (name, p) in [("host", &host), ("guest", &guest)] {
        assert!(p.running, "{name} never reached Running");
        assert_eq!(p.progress, [33, 66], "{name} handshake progress");
        assert!(
            settled(p) >= frames,
            "{name} settled {} of {frames} frames after {tick} ticks",
            settled(p)
        );
        assert_eq!(p.emu.game.exec_errors, 0, "{name} faulted");
    }
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
    let mut game = Game::from_ines(&img).expect("synthetic boots");
    game.set_coop(true);
    game.reset();
    Emu {
        game,
        apu: z2_apu::Apu::new(44_100),
        trapset_id: 1,
        trapset_base: 1,
    }
}

/// The framebuffer stays on the heap: an inline one makes every by-value
/// move of a `Game` or `Emu` cost 60 KiB of stack.
#[test]
fn emulator_state_is_small_enough_to_move() {
    let game = std::mem::size_of::<Game>();
    let emu = std::mem::size_of::<Emu>();
    assert!(game <= 32 * 1024, "Game is {game} bytes");
    assert!(emu <= 32 * 1024, "Emu is {emu} bytes");
}

#[test]
fn synthetic_session_start_fits_the_windows_main_stack() {
    on_windows_main_stack(|| {
        let rebuild = |_: u32, wram: &[u8]| {
            let mut e = synthetic_emu();
            e.game.wram.copy_from_slice(wram);
            e
        };
        session(&synthetic_emu, &rebuild, 120);
    });
}

#[test]
fn rom_session_start_fits_the_windows_main_stack() {
    let test = "rom_session_start_fits_the_windows_main_stack";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    on_windows_main_stack(move || {
        let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
        let feats = Features {
            coop: true,
            wide_gameplay: None,
            record: false,
            margin_sprites: false,
        };
        let make = || app::emu_from_rom_body_with(&body, 44_100, feats).expect("emulator");
        let rebuild = |flags: u32, wram: &[u8]| {
            netplay::session_emu(&body, 44_100, false, None, flags, wram).expect("session emulator")
        };
        let frames = if cfg!(debug_assertions) { 120 } else { 600 };
        session(&make, &rebuild, frames);
    });
}
