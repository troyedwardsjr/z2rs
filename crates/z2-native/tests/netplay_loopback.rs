//! Lockstep integration: two independent games driven through two `z2-net`
//! sessions over the deterministic loopback transport must stay bit-identical.
//!
//! This is the test that would actually catch a desync bug in the frontend
//! wiring (wrong pad order, a dropped frame, a hash taken at the wrong moment),
//! because it runs the real session state machine and the real per-frame hash
//! the app uses — only the transport is replaced.
//!
//! The ROM-backed case self-skips unless `Z2_ROM` names an existing file; a
//! synthetic-cartridge case always runs so the wiring is covered in public CI.

use z2_native::app::{self, Features};
use z2_native::netplay::{self, NetLink};
use z2_net::{
    loopback_pair, LoopbackConfig, LoopbackTransport, SessionConfig, SessionEvent,
    COOP_SPRITE_UNLIMITED, COOP_TWO_LINKS,
};

/// Deterministic pad script: mostly neutral with a periodic Start press, so the
/// run is reproducible and still exercises non-zero input.
fn pad_for(frame: u32) -> u8 {
    if frame % 128 < 4 {
        z2_core::game::BTN_START
    } else {
        0
    }
}

/// The verified ROM body, or `None` when the ROM is not available.
fn rom_body() -> Option<Vec<u8>> {
    let path = app::env_path_if_usable("Z2_ROM")?;
    let raw = std::fs::read(&path).ok()?;
    Some(z2_assets::rom::strip_ines_header(&raw).to_vec())
}

/// A minimal cartridge that boots into an `RTS` loop: enough to step real
/// frames through `Game::step2` with no ROM present.
fn synthetic_ines() -> Vec<u8> {
    let mut img = vec![0u8; 16 + 8 * 0x4000 + 8 * 0x2000];
    img[0..4].copy_from_slice(b"NES\x1a");
    img[4] = 8;
    img[5] = 8;
    img[6] = 0x10; // mapper 1 (MMC1)
    let prg_base = 16;
    let prg_len = 8 * 0x4000;
    img[prg_base] = 0x60; // RTS at $C000
    let v = prg_base + prg_len - 4;
    img[v] = 0x00;
    img[v + 1] = 0xC0; // RESET -> $C000
    img
}

/// One peer: its session, its game, and its own frame counter.
struct Peer {
    link: NetLink<LoopbackTransport>,
    game: z2_core::game::Game,
    frames: u32,
}

impl Peer {
    /// Apply protocol events. `Started` would rebuild the game in the real
    /// frontend; here both peers already start from the same power-on state, so
    /// only the WRAM snapshot has to be adopted.
    fn apply_events(&mut self) {
        for ev in self.link.take_events() {
            match ev {
                SessionEvent::Started { delay, wram, .. } => {
                    if wram.len() == self.game.wram.len() {
                        self.game.wram.copy_from_slice(&wram);
                        self.game.coop_reset_area();
                    }
                    self.link.delay = delay;
                    self.link.started = true;
                }
                SessionEvent::Desync { frame, .. } => panic!("desync at frame {frame}"),
                SessionEvent::Closed(r) => panic!("session closed early: {r}"),
                _ => {}
            }
        }
    }

    /// One tick: pump, apply events, step the confirmed frames, flush.
    fn tick(&mut self, now_ms: u64, budget: u32) {
        self.link.update(now_ms);
        self.apply_events();
        let pad = pad_for(self.frames);
        let game = &mut self.game;
        let out = netplay::step_session(&mut self.link, budget, pad, |p1, p2, want| {
            game.step2(p1, p2);
            want.then(|| netplay::state_hash(game))
        });
        self.frames += out.stepped;
        self.link.update(now_ms);
    }
}

/// Run both peers until each has stepped `frames` frames, then compare.
fn run_pair(mut host_game: z2_core::game::Game, mut guest_game: z2_core::game::Game, frames: u32) {
    let trapset = 0xDEAD_BEEF_u64; // identical on both peers: that is all it must be
    let crc = z2_assets::rom::EXPECTED_BODY_CRC32;
    let (line, ta, tb) = loopback_pair(LoopbackConfig::default(), 7);

    let mut hcfg = SessionConfig::host(crc, trapset, COOP_TWO_LINKS);
    hcfg.wram = Some(host_game.wram().to_vec());
    let host_link = NetLink::new(hcfg, ta, "loop").expect("host config");
    let gcfg = SessionConfig::guest(crc, trapset, COOP_TWO_LINKS | COOP_SPRITE_UNLIMITED);
    let guest_link = NetLink::new(gcfg, tb, "loop").expect("guest config");

    // The games are moved into the peers; take the initial hash first.
    let h0 = netplay::state_hash(&host_game);
    assert_eq!(
        h0,
        netplay::state_hash(&guest_game),
        "both peers must start from identical state"
    );
    host_game.coop_reset_area();
    guest_game.coop_reset_area();

    let mut host = Peer {
        link: host_link,
        game: host_game,
        frames: 0,
    };
    let mut guest = Peer {
        link: guest_link,
        game: guest_game,
        frames: 0,
    };

    let mut tick = 0u64;
    // Generous bound: 2 frames per tick means ~frames/2 ticks are needed.
    let max_ticks = u64::from(frames) * 8 + 4_000;
    while (host.frames < frames || guest.frames < frames) && tick < max_ticks {
        line.advance(1);
        let now_ms = tick * 16;
        host.tick(now_ms, 2);
        guest.tick(now_ms, 2);
        tick += 1;
    }

    assert!(
        host.frames >= frames && guest.frames >= frames,
        "lockstep stalled: host {} guest {} of {frames} frames after {tick} ticks",
        host.frames,
        guest.frames
    );
    assert_eq!(host.frames, guest.frames, "peers stepped a different count");
    // The real assertion: identical state, region by region so a failure says
    // which one drifted, then the combined hash the protocol itself compares.
    assert_eq!(host.game.ram(), guest.game.ram(), "RAM diverged");
    assert_eq!(host.game.wram(), guest.game.wram(), "WRAM diverged");
    assert_eq!(host.game.oam(), guest.game.oam(), "OAM diverged");
    assert_eq!(
        host.game.coop_hash(),
        guest.game.coop_hash(),
        "co-op state diverged"
    );
    assert_eq!(
        netplay::state_hash(&host.game),
        netplay::state_hash(&guest.game),
        "state hash diverged"
    );
    assert_ne!(
        netplay::state_hash(&host.game),
        h0,
        "the run must actually have advanced the state"
    );
}

/// Synthetic cartridge, always runs: proves the session/step/hash wiring.
#[test]
fn synthetic_games_stay_in_lockstep_over_loopback() {
    let mut a = z2_core::game::Game::from_ines(&synthetic_ines()).expect("synthetic boots");
    let mut b = z2_core::game::Game::from_ines(&synthetic_ines()).expect("synthetic boots");
    a.reset();
    b.reset();
    a.set_coop(true);
    b.set_coop(true);
    run_pair(a, b, 400);
}

/// Real ROM: a few thousand frames of the actual game on both peers.
///
/// Skips (rather than fails) without an existing `Z2_ROM`. The frame count is
/// reduced in debug builds, where the interpreter runs a few hundred frames per
/// second rather than thousands.
#[test]
fn rom_games_stay_in_lockstep_over_loopback() {
    let Some(body) = rom_body() else {
        eprintln!("SKIP rom_games_stay_in_lockstep_over_loopback: Z2_ROM unset or not a file");
        return;
    };
    let frames: u32 = if cfg!(debug_assertions) { 400 } else { 3_000 };
    let feats = Features {
        coop: true,
        wide_gameplay: None,
        record: false,
        margin_sprites: false,
    };
    let host = app::emu_from_rom_body_with(&body, 44_100, feats).expect("host emulator");
    let guest = app::emu_from_rom_body_with(&body, 44_100, feats).expect("guest emulator");
    // Both frontends must agree on the trap-set identity or the handshake would
    // refuse; assert that before using a placeholder id in the session.
    assert_eq!(
        host.trapset_id, guest.trapset_id,
        "two identically built emulators must have the same trap-set id"
    );
    assert_ne!(host.trapset_id, 0, "a ROM build registers traps");
    run_pair(host.game, guest.game, frames);
}
