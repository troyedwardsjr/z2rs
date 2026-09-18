//! Two-player co-op (`z2_core::coop`): `Game::step2`, default-off and
//! registration pins (synthetic), plus ROM-gated parity / behaviour /
//! determinism runs over the any% corpus movie.
//!
//! ROM-gated tests self-skip unless `Z2_ROM` points at a file; the movie comes
//! from `Z2_CORPUS_MOVIES` (directory holding `anypct.bk2`, default the
//! out-of-tree corpus) and also self-skips when absent. No ROM bytes or
//! derived data are stored in the tree.

#![cfg(feature = "interp")]

mod common;

use z2_core::coop::{self, CoopOptions, COOP_TRAP_ADDRS, P2_OAM_SLOTS};
use z2_core::game::{Game, BTN_A, BTN_B, BTN_LEFT, BTN_RIGHT, BTN_UP};

// ------------------------------------------------------------ helpers

/// Default trap groups in the frontends' order (native `register_native_traps`,
/// xtask `TRAP_GROUPS`).
fn register_default_groups(game: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(game);
    z2_core::sideview_traps::register_sideview_traps(game);
    z2_core::sideview_traps::register_overworld_traps(game);
    z2_core::player_traps::register_player_traps(game);
    z2_core::enemy_traps::register_enemy_traps(game);
    z2_core::town_traps::register_town_traps(game);
    z2_core::palace_traps::register_palace_traps(game);
    z2_core::title_traps::register_title_traps(game);
    z2_core::boot_traps::register_boot_traps(game);
}

fn rom_bytes() -> Option<Vec<u8>> {
    common::rom_bytes("coop_tests")
}

fn anypct_track() -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .unwrap_or_else(|| "/Volumes/Holy Drive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(
        &std::path::Path::new(&dir).join("anypct.bk2"),
        "coop_tests any% movie",
    )?;
    let zip = std::fs::read(&path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
    Some(movie.pad1_track())
}

/// ROM + any% track, or `None` (skip).
fn fixtures() -> Option<(Vec<u8>, Vec<u8>)> {
    let rom = rom_bytes()?;
    let track = anypct_track()?;
    Some((rom, track))
}

/// Game with the default groups; `coop` = `Some(opts)` enables co-op.
fn rom_game(rom: &[u8], coop: Option<CoopOptions>) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    if let Some(opts) = coop {
        g.set_coop_options(opts);
        g.set_coop(true);
    }
    g.reset();
    g
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn state_hash(g: &Game) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, g.ram());
    fnv(&mut h, g.wram());
    fnv(&mut h, g.oam());
    fnv(&mut h, g.palette());
    fnv(&mut h, &g.frame[..]);
    fnv(&mut h, &g.coop_hash().to_le_bytes());
    h
}

/// RAM bytes an inert P2 may legitimately change: scratch `$00-$0F`, `$D9`,
/// the stack page, P2's OAM slots, SFX requests `$E9-$EF`, sound RAM
/// `$07C0-$07FF`.
fn masked(addr: usize) -> bool {
    let p2_oam = P2_OAM_SLOTS.iter().any(|&s| {
        let base = 0x200 + usize::from(s) * 4;
        (base..base + 4).contains(&addr)
    });
    addr < 0x10
        || addr == 0xD9
        || (0x100..0x200).contains(&addr)
        || p2_oam
        || (0xE9..=0xEF).contains(&addr)
        || (0x7C0..0x800).contains(&addr)
}

/// WRAM bytes that echo masked RAM: during area transitions a loader stashes
/// the stack page and OAM RAM into WRAM at a fixed `+$7C01` offset (first
/// seen any% frame 896, mode `$00`), so dead stack bytes and P2's spare OAM
/// slots show up there too.
fn wram_masked(addr: usize) -> bool {
    let r = addr.wrapping_sub(0x7C01);
    (0x100..0x300).contains(&r) && masked(r)
}

fn p2_world_x(g: &Game) -> u16 {
    let st = g.coop_status().expect("co-op enabled");
    (u16::from(st.p2_page) << 8) | u16::from(st.p2_x)
}

fn camera_left(g: &Game) -> u16 {
    (u16::from(g.ram[0x072A]) << 8) | u16::from(g.ram[0x072C])
}

/// Deterministic pad-2 script (64-bit LCG over the button bits).
struct Lcg(u64);
impl Lcg {
    fn pad(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let v = (self.0 >> 40) as u8;
        // Mostly walking/jumping/slashing; never Start/Select.
        v & (BTN_A | BTN_B | BTN_LEFT | BTN_RIGHT | BTN_UP)
    }
}

// ------------------------------------------------------------ synthetic

/// Strobe both pads every loop, shift 8 bits of each into `$00` (pad 2) /
/// `$01` (pad 1), then publish complete bytes to `$10` / `$11` (bit-reversed
/// pad bytes: the first bit read, A, ends in bit 7); `$12` counts loops.
const PAD_PROGRAM: [u8; 37] = [
    0xA9, 0x01, 0x8D, 0x16, 0x40, // LDA #1 : STA $4016
    0xA9, 0x00, 0x8D, 0x16, 0x40, // LDA #0 : STA $4016
    0xA2, 0x08, // LDX #8
    0xAD, 0x17, 0x40, 0x4A, 0x26, 0x00, // loop: LDA $4017 : LSR : ROL $00
    0xAD, 0x16, 0x40, 0x4A, 0x26, 0x01, // LDA $4016 : LSR : ROL $01
    0xCA, 0xD0, 0xF1, // DEX : BNE loop
    0xA5, 0x00, 0x85, 0x10, // LDA $00 : STA $10
    0xA5, 0x01, 0x85, 0x11, // LDA $01 : STA $11
    0xE6, 0x12, // INC $12
];

fn pad_program() -> Game {
    let mut blob = PAD_PROGRAM.to_vec();
    blob.extend_from_slice(&[0x4C, 0x00, 0x80]); // JMP $8000
    let mut g = Game::with_test_program(0x8000, &blob);
    g.reset();
    g
}

#[test]
fn step2_with_idle_pad2_matches_step() {
    let mut a = pad_program();
    let mut b = pad_program();
    for f in 0..40u8 {
        let input = f.wrapping_mul(37);
        a.step(input);
        b.step2(input, 0);
        assert_eq!(a.ram(), b.ram(), "frame {f}");
        assert_eq!(a.cpu_state(), b.cpu_state(), "frame {f}");
        assert_eq!(a.cpu.cycles, b.cpu.cycles, "frame {f}");
    }
    assert_eq!(
        a.ram()[0x11],
        39u8.wrapping_mul(37).reverse_bits(),
        "pad 1 reached the program"
    );
}

#[test]
fn step2_latches_pad2_and_masks_save_chord_only_with_coop() {
    let mut g = pad_program();
    g.step2(0, 0xFF);
    g.step2(0, 0xFF);
    assert_eq!(g.ram()[0x10], 0xFF);
    g.step2(0, BTN_UP | BTN_A);
    g.step(0);
    assert_eq!(
        g.ram()[0x10],
        0x88,
        "co-op off: the save chord reaches $4017"
    );
    g.set_coop(true);
    g.step2(0, BTN_UP | BTN_A);
    g.step2(0, BTN_UP | BTN_A);
    assert_eq!(g.ram()[0x10], BTN_A.reverse_bits(), "co-op on: Up dropped");
    let chord_b = BTN_UP | BTN_A | BTN_B;
    g.step2(0, chord_b);
    g.step2(0, chord_b);
    assert_eq!(g.ram()[0x10], chord_b.reverse_bits(), "not the exact chord");
}

#[test]
fn coop_is_off_by_default_and_not_in_default_groups() {
    let mut g = Game::default();
    assert!(!g.coop.enabled);
    assert!(g.coop_status().is_none());
    register_default_groups(&mut g);
    for addr in [0xD5A7u16, 0xEBF0, 0xE4D9] {
        assert!(!g.traps.is_trapped(addr), "${addr:04X} trapped by default");
    }
    let originals = [
        (0xD6C1u16, "bank7_Link_Collision_Detection"),
        (0xE558, "bank7_code39"),
    ];
    for (addr, name) in originals {
        assert_eq!(g.traps.get(addr).map(|t| t.name), Some(name));
    }
    let sword_cost = g.traps.get(0xE677).map(|t| t.cycles).expect("sword trap");
    let code39_cost = g.traps.get(0xE558).map(|t| t.cycles).unwrap();

    g.set_coop(true);
    for addr in COOP_TRAP_ADDRS {
        let name = g.traps.get(addr).map(|t| t.name).unwrap_or("");
        assert!(name.starts_with("coop_"), "${addr:04X} = {name}");
    }
    assert_eq!(g.traps.get(0xE677).unwrap().cycles, sword_cost);
    assert_eq!(g.traps.get(0xE558).unwrap().cycles, code39_cost);
    assert!(g.coop_status().is_some());
    g.set_coop(false);
    assert!(g.coop_status().is_none());
}

#[test]
fn coop_reset_area_drops_p2() {
    let mut g = Game::new();
    g.set_coop(true);
    g.coop.active = true;
    g.coop.respawn_timer = 5;
    g.coop_reset_area();
    let st = g.coop_status().unwrap();
    assert!(!st.active && !st.p2_alive && st.respawn_frames == 0);
}

// ------------------------------------------------------------ ROM-gated

/// Registered-but-disabled co-op group is byte-identical to the default set
/// (RAM incl. stack, WRAM, OAM, palette, frame, CPU clock).
#[test]
fn rom_coop_registered_but_disabled_is_byte_identical() {
    let Some((rom, track)) = fixtures() else {
        return;
    };
    let mut a = rom_game(&rom, None);
    let mut b = rom_game(&rom, None);
    coop::register_coop_traps(&mut b);
    for (f, &p) in track.iter().take(3000).enumerate() {
        a.step(p);
        b.step2(p, 0);
        assert!(a.ram() == b.ram(), "RAM differs at frame {f}");
        assert!(a.wram() == b.wram(), "WRAM differs at frame {f}");
        assert_eq!(a.oam(), b.oam(), "OAM frame {f}");
        assert_eq!(a.palette(), b.palette(), "palette frame {f}");
        assert!(a.frame[..] == b.frame[..], "frame buffer {f}");
        assert_eq!(a.cpu.cycles, b.cpu.cycles, "cycles frame {f}");
    }
}

/// `(address, off, co-op)` bytes of `b` (co-op) differing from `a` (off):
/// RAM outside the documented masks, then WRAM.
fn masked_diff(a: &Game, b: &Game) -> (Diff, Diff) {
    let ram = (0..0x800)
        .filter(|&i| !masked(i) && a.ram()[i] != b.ram()[i])
        .map(|i| (i as u16, a.ram()[i], b.ram()[i]))
        .collect();
    let wram = (0..0x2000)
        .filter(|&i| !wram_masked(0x6000 + i) && a.wram()[i] != b.wram()[i])
        .map(|i| ((0x6000 + i) as u16, a.wram()[i], b.wram()[i]))
        .collect();
    (ram, wram)
}

/// Differing bytes as `(address, co-op off, co-op on)`.
type Diff = Vec<(u16, u8, u8)>;
type FirstDiff = Option<(usize, Diff)>;

#[derive(Debug)]
struct LeakReport {
    ram_diverged_frames: u64,
    wram_diverged_frames: u64,
    first_ram: FirstDiff,
    first_wram: FirstDiff,
    active_frames: u64,
    p2_sprite_frames: u64,
    updates: u64,
    first_p2_contact: Option<usize>,
    diverged_before_contact: u64,
}

fn run_inert(rom: &[u8], track: &[u8], opts: CoopOptions, frames: usize) -> LeakReport {
    let mut a = rom_game(rom, None);
    let mut b = rom_game(rom, Some(opts));
    let mut r = LeakReport {
        ram_diverged_frames: 0,
        wram_diverged_frames: 0,
        first_ram: None,
        first_wram: None,
        active_frames: 0,
        p2_sprite_frames: 0,
        updates: 0,
        first_p2_contact: None,
        diverged_before_contact: 0,
    };
    for (f, &p) in track.iter().take(frames).enumerate() {
        a.step(p);
        b.step2(p, 0);
        if r.first_p2_contact.is_none() && b.coop.n_p2_contacts > 0 {
            r.first_p2_contact = Some(f);
        }
        let st = b.coop_status().unwrap();
        if b.ram()[0x0736] != 0x0B {
            assert!(!st.active, "P2 must be hidden outside sideview (frame {f})");
        }
        if st.active {
            r.active_frames += 1;
            let shown = P2_OAM_SLOTS
                .iter()
                .any(|&s| b.oam()[usize::from(s) * 4] < 0xF0);
            r.p2_sprite_frames += u64::from(shown);
        }
        let (dr, dw) = masked_diff(&a, &b);
        if (!dr.is_empty() || !dw.is_empty()) && r.first_p2_contact.is_none() {
            r.diverged_before_contact += 1;
        }
        if !dr.is_empty() {
            r.ram_diverged_frames += 1;
            if r.first_ram.is_none() {
                r.first_ram = Some((f, dr));
            }
        }
        if !dw.is_empty() {
            r.wram_diverged_frames += 1;
            if r.first_wram.is_none() {
                r.first_wram = Some((f, dw));
            }
        }
    }
    r.updates = b.coop.n_update;
    r
}

/// Inert P2 as a ghost (no contact passes): P1-relevant RAM and all of WRAM
/// identical to a co-op-off run over any% 6000 frames.
#[test]
fn rom_inert_p2_ghost_zero_p1_divergence() {
    let Some((rom, track)) = fixtures() else {
        return;
    };
    let opts = CoopOptions {
        p2_contact: false,
        ..CoopOptions::default()
    };
    let r = run_inert(&rom, &track, opts, 6000);
    eprintln!("ghost: {r:?}");
    assert!(r.active_frames > 1000, "P2 was active in sideview");
    assert!(
        r.p2_sprite_frames > 0,
        "P2 sprites landed in the spare slots"
    );
    assert_eq!(r.first_p2_contact, None, "ghost P2 never makes contact");
    assert_eq!(r.ram_diverged_frames, 0, "{r:?}");
    assert_eq!(r.wram_diverged_frames, 0, "{r:?}");
}

/// Inert P2 with the default options (contact passes on): identical to the
/// co-op-off run until P2's first contact event (an enemy walking into an
/// idle P2 legitimately changes shared enemy state from then on).
#[test]
fn rom_inert_p2_default_no_divergence_before_p2_contact() {
    let Some((rom, track)) = fixtures() else {
        return;
    };
    let r = run_inert(&rom, &track, CoopOptions::default(), 6000);
    eprintln!("default: {r:?}");
    assert!(r.active_frames > 1000);
    assert_eq!(r.diverged_before_contact, 0, "{r:?}");
}

/// P2 given walking input moves (vs an idle P2 under the same P1 input) and
/// stays inside the camera window every frame.
#[test]
fn rom_p2_walks_and_stays_on_screen() {
    let Some((rom, track)) = fixtures() else {
        return;
    };
    let mut idle = rom_game(&rom, Some(CoopOptions::default()));
    let mut walk = rom_game(&rom, Some(CoopOptions::default()));
    let mut active_since: Option<usize> = None;
    let mut moved_frames = 0u64;
    let mut checked = 0u64;
    for (f, &p) in track.iter().take(3000).enumerate() {
        let st = walk.coop_status().unwrap();
        let pad2 = match active_since {
            Some(s) if st.active => {
                // 120 frames right, 120 frames left, repeating.
                if (f - s) % 240 < 120 {
                    BTN_RIGHT
                } else {
                    BTN_LEFT
                }
            }
            _ => 0,
        };
        idle.step2(p, 0);
        walk.step2(p, pad2);
        let st = walk.coop_status().unwrap();
        if st.active && active_since.is_none() {
            active_since = Some(f);
        }
        let ist = idle.coop_status().unwrap();
        if st.active && st.p2_alive {
            let x = p2_world_x(&walk);
            let left = camera_left(&walk);
            assert!(
                x >= left && x <= left + 240,
                "frame {f}: P2 x {x:#06X} outside [{left:#06X}, +240]"
            );
            checked += 1;
            if ist.active && p2_world_x(&idle) != x {
                moved_frames += 1;
            }
        }
    }
    eprintln!("walk: active_since={active_since:?} checked={checked} moved_frames={moved_frames}");
    assert!(active_since.is_some(), "P2 never activated");
    assert!(checked > 500);
    assert!(moved_frames > 100, "P2 input moved P2");
}

/// Two co-op games fed the same (pad1, pad2) streams stay hash-equal; a
/// one-frame pad-2 change diverges.
#[test]
fn rom_coop_determinism() {
    let Some((rom, track)) = fixtures() else {
        return;
    };
    let mut a = rom_game(&rom, Some(CoopOptions::default()));
    let mut b = rom_game(&rom, Some(CoopOptions::default()));
    let mut c = rom_game(&rom, Some(CoopOptions::default()));
    let (mut la, mut lb, mut lc) = (Lcg(0x5EED), Lcg(0x5EED), Lcg(0x5EED));
    let mut c_diverged = false;
    let mut p2_updates_seen = false;
    for (f, &p) in track.iter().take(3000).enumerate() {
        let (pa, pb) = (la.pad(), lb.pad());
        let mut pc = lc.pad();
        if f == 1500 {
            pc ^= BTN_B;
        }
        a.step2(p, pa);
        b.step2(p, pb);
        c.step2(p, pc);
        assert_eq!(state_hash(&a), state_hash(&b), "hash differs at frame {f}");
        c_diverged |= state_hash(&a) != state_hash(&c);
        p2_updates_seen |= a.coop.n_update > 0;
    }
    eprintln!(
        "determinism: updates={} deaths={} c_diverged={c_diverged}",
        a.coop.n_update, a.coop.p2_deaths
    );
    assert!(p2_updates_seen);
    assert!(c_diverged, "a pad-2 change must reach the state");
}
