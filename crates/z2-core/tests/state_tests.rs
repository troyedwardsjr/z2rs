//! Save-state determinism (`z2_core::state`): the rollback-netplay contract.
//!
//! A restored [`GameState`] must reproduce the original run bit for bit —
//! RAM, WRAM, OAM, palette, CPU registers, co-op state and the indexed
//! framebuffer — for every later frame.
//!
//! * Synthetic (ROM-free): a hand-assembled program that renders with NMI,
//!   polls both pads, writes nametables mid-frame, OAM DMA, MMC1 CHR switches
//!   over a patterned CHR image and APU registers.
//! * ROM-gated (self-skip unless `Z2_ROM` names a file and the any% movie is
//!   found at `$Z2_CORPUS/movies/anypct.bk2` or `$Z2_CORPUS_MOVIES/anypct.bk2`):
//!   checkpoints across title/overworld/side-view, 2000-frame rollback storms
//!   (1-frame and 8-frame mispredicted rollbacks), and the same storms with
//!   co-op driven through `Game::step2`.

mod common;

use std::hash::Hasher;
use std::time::Instant;

use z2_core::game::{Game, BTN_A, BTN_B, BTN_LEFT, BTN_RIGHT, BTN_START, BTN_UP};
use z2_core::state::{GameState, StateError};

// ------------------------------------------------------------ helpers

/// Hash of everything a peer could observe after a frame.
fn frame_hash(g: &Game) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(g.ram());
    h.write(g.wram());
    h.write(g.oam());
    h.write(g.palette());
    h.write(&g.frame_indexed()[..]);
    let (a, x, y, sp, pc, p) = g.cpu_state();
    h.write(&[a, x, y, sp, p]);
    h.write_u16(pc);
    h.write_u64(g.cpu.cycles);
    h.write_u64(g.frame_count());
    h.write_u64(g.coop_hash());
    h.finish()
}

/// Deterministic input script (64-bit LCG).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as u8
    }
    fn p2(&mut self) -> u8 {
        self.next() & (BTN_A | BTN_B | BTN_LEFT | BTN_RIGHT | BTN_UP)
    }
}

/// Inputs for one frame: `(pad1, pad2)`.
type Pads = (u8, u8);

fn advance(g: &mut Game, pads: Pads, two: bool) {
    if two {
        g.step2(pads.0, pads.1);
    } else {
        g.step(pads.0);
    }
}

/// Straight run over `inputs`, hash after every frame.
fn straight(g: &mut Game, inputs: &[Pads], two: bool) -> Vec<u64> {
    inputs
        .iter()
        .map(|&p| {
            advance(g, p, two);
            frame_hash(g)
        })
        .collect()
}

/// Every frame: save, advance, hash, load, advance again with the same
/// input, hash; both hashes must match the straight run.
fn storm_1(g: &mut Game, inputs: &[Pads], want: &[u64], two: bool, label: &str) {
    let mut st = GameState::new();
    for (i, &p) in inputs.iter().enumerate() {
        g.save_state_into(&mut st);
        advance(g, p, two);
        let first = frame_hash(g);
        g.load_state(&st);
        assert_eq!(g.frame_count(), st.frame_count());
        advance(g, p, two);
        let second = frame_hash(g);
        assert_eq!(first, want[i], "{label}: first pass diverged at step {i}");
        assert_eq!(second, want[i], "{label}: replay diverged at step {i}");
    }
}

/// Every 8 frames: save, run 8 frames on mispredicted inputs, load, re-run
/// the 8 with the real inputs; the re-simulated hashes must match.
fn storm_8(g: &mut Game, inputs: &[Pads], want: &[u64], two: bool, label: &str) {
    let mut st = GameState::new();
    let mut wrong = Lcg(0x5EED_0008);
    for (c, chunk) in inputs.chunks(8).enumerate() {
        g.save_state_into(&mut st);
        for _ in chunk {
            let w = (wrong.next() & !BTN_START, wrong.p2());
            advance(g, w, two);
        }
        g.load_state(&st);
        for (j, &p) in chunk.iter().enumerate() {
            advance(g, p, two);
            let i = c * 8 + j;
            assert_eq!(
                frame_hash(g),
                want[i],
                "{label}: re-sim diverged at step {i}"
            );
        }
    }
}

// ------------------------------------------------------------ synthetic

/// Tiny assembler: raw bytes plus backward `BNE`.
struct Asm(Vec<u8>);
impl Asm {
    fn b(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn here(&self) -> usize {
        self.0.len()
    }
    fn bne_back(&mut self, target: usize) -> &mut Self {
        let off = target as isize - (self.0.len() as isize + 2);
        self.b(&[0xD0, off as i8 as u8])
    }
}

/// Program at `$8000`: PPU on with NMI; the main loop polls both pads,
/// folds them into `$12`/`$13`, writes a nametable byte and an OAM RAM byte
/// and resets scroll; the NMI does `$4014` DMA, bumps `$20`, writes an APU
/// register, `$2005` scroll and clocks one bit of `$20 >> 3` into the MMC1
/// CHR-0 serial port (a bank switch every fifth frame).
fn synth_game() -> Game {
    let mut a = Asm(Vec::new());
    // RESET
    a.b(&[0x78, 0xA2, 0xFF, 0x9A]); // SEI : LDX #$FF : TXS
    a.b(&[0xA9, 0x00, 0x8D, 0x00, 0x20, 0x8D, 0x01, 0x20]);
    // MMC1 control = $1E (4 KiB CHR, PRG mode 3, horizontal).
    a.b(&[0xA9, 0x1E, 0xA0, 0x05]);
    let l = a.here();
    a.b(&[0x8D, 0x00, 0x80, 0x4A, 0x88]);
    a.bne_back(l);
    // Palette: $3F00.. = 0..31.
    a.b(&[0xA9, 0x3F, 0x8D, 0x06, 0x20, 0xA9, 0x00, 0x8D, 0x06, 0x20]);
    a.b(&[0xA2, 0x00]);
    let l = a.here();
    a.b(&[0x8A, 0x8D, 0x07, 0x20, 0xE8, 0xE0, 0x20]);
    a.bne_back(l);
    a.b(&[0xA9, 0x1E, 0x8D, 0x01, 0x20, 0xA9, 0x80, 0x8D, 0x00, 0x20]);
    // main:
    let main = 0x8000 + a.here() as u16;
    a.b(&[0xA9, 0x01, 0x8D, 0x16, 0x40, 0xA9, 0x00, 0x8D, 0x16, 0x40]);
    a.b(&[0xA2, 0x08]);
    let l = a.here();
    a.b(&[0xAD, 0x16, 0x40, 0x4A, 0x26, 0x10]);
    a.b(&[0xAD, 0x17, 0x40, 0x4A, 0x26, 0x11, 0xCA]);
    a.bne_back(l);
    a.b(&[0xA5, 0x12, 0x18, 0x65, 0x10, 0x85, 0x12]); // $12 += $10
    a.b(&[0x45, 0x11, 0x85, 0x13]); // $13 = A ^ $11
    a.b(&[0xA9, 0x20, 0x8D, 0x06, 0x20, 0xA5, 0x12, 0x8D, 0x06, 0x20]);
    a.b(&[0xA5, 0x13, 0x8D, 0x07, 0x20]); // PPU[$20xx] = $13
    a.b(&[0xA6, 0x13, 0xA5, 0x12, 0x9D, 0x00, 0x02]); // $0200,$13 = $12
    a.b(&[0x9D, 0x00, 0x61]); // WRAM $6100,X
    a.b(&[0xA9, 0x00, 0x8D, 0x05, 0x20, 0x8D, 0x05, 0x20]);
    a.b(&[0x4C, main as u8, (main >> 8) as u8]);
    // NMI:
    let nmi = 0x8000 + a.here() as u16;
    a.b(&[0x48, 0xA9, 0x02, 0x8D, 0x14, 0x40, 0xE6, 0x20]);
    a.b(&[0xA5, 0x20, 0x8D, 0x00, 0x40]); // APU $4000
    a.b(&[0x8D, 0x05, 0x20]); // $2005
                              // One MMC1 serial bit per frame, so the shift register and CHR bank
                              // carry across frame boundaries: bit ($20 >> 3) & 1 into $A000.
    a.b(&[0x4A, 0x4A, 0x4A, 0x29, 0x01, 0x8D, 0x00, 0xA0]);
    a.b(&[0x68, 0x40]); // PLA : RTI
    let mut g = Game::with_test_program(0x8000, &a.0);
    g.set_nmi_vector(nmi);
    // 32 KiB patterned CHR so bank switches change the picture.
    g.chr = (0..0x8000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    g.reset();
    g
}

fn synth_inputs(n: usize, seed: u64) -> Vec<Pads> {
    let mut r = Lcg(seed);
    (0..n).map(|_| (r.next(), r.next())).collect()
}

#[test]
fn synth_program_is_nontrivial() {
    let mut g = synth_game();
    let inputs = synth_inputs(120, 1);
    let hashes = straight(&mut g, &inputs, true);
    let distinct: std::collections::HashSet<_> = hashes.iter().collect();
    assert!(distinct.len() > 100, "frames should differ");
    assert_eq!(g.exec_errors, 0);
    assert!(g.frame_indexed().iter().any(|&b| b != g.frame_indexed()[0]));
    assert!(g.mmc1.chr0 != 0 || g.ram()[0x20] != 0);
}

#[test]
fn synth_save_run_load_rerun_matches() {
    let mut g = synth_game();
    let inputs = synth_inputs(40 + 120, 2);
    for &p in &inputs[..40] {
        g.step2(p.0, p.1);
    }
    let st = g.save_state();
    let first = straight(&mut g, &inputs[40..], true);
    assert!(!g.apu.log.is_empty());
    g.load_state(&st);
    assert!(g.apu.log.is_empty(), "APU log cleared on load");
    let second = straight(&mut g, &inputs[40..], true);
    assert_eq!(first, second);
}

#[test]
fn synth_state_transfers_to_a_fresh_game_and_through_bytes() {
    let mut a = synth_game();
    let inputs = synth_inputs(100, 3);
    for &p in &inputs[..50] {
        a.step2(p.0, p.1);
    }
    let bytes = a.save_state().to_bytes();
    let want = straight(&mut a, &inputs[50..], true);

    let mut b = synth_game();
    for &p in &synth_inputs(17, 99) {
        b.step2(p.0, p.1); // unrelated history
    }
    let st = GameState::from_bytes(&bytes).expect("decode");
    assert_eq!(st.to_bytes(), bytes, "byte image round-trips");
    b.load_state(&st);
    assert_eq!(straight(&mut b, &inputs[50..], true), want);

    assert_eq!(
        GameState::from_bytes(&bytes[..bytes.len() - 1]).unwrap_err(),
        StateError::Truncated
    );
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert_eq!(
        GameState::from_bytes(&bad).unwrap_err(),
        StateError::BadMagic
    );
}

#[test]
fn synth_rollback_storms_match_straight_run() {
    let n = if cfg!(debug_assertions) { 400 } else { 2000 };
    let inputs = synth_inputs(n, 4);
    let mut s = synth_game();
    let want = straight(&mut s, &inputs, true);
    let mut g = synth_game();
    storm_1(&mut g, &inputs, &want, true, "synth storm 1");
    assert_eq!(frame_hash(&g), frame_hash(&s));
    let mut g = synth_game();
    storm_8(&mut g, &inputs, &want, true, "synth storm 8");
}

// ------------------------------------------------------------ ROM-gated

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

fn anypct_track(test: &str) -> Option<Vec<u8>> {
    let dir = match common::var_present("Z2_CORPUS") {
        Some(c) => std::path::Path::new(&c).join("movies"),
        None => std::path::PathBuf::from(
            common::var_present("Z2_CORPUS_MOVIES")
                .unwrap_or_else(|| "/Volumes/Holy Drive/dev/z2-corpus/movies".to_string()),
        ),
    };
    let path = common::file_present(&dir.join("anypct.bk2"), test)?;
    let zip = std::fs::read(path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
    Some(movie.pad1_track())
}

fn fixtures(test: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let rom = common::rom_bytes(test)?;
    let track = anypct_track(test)?;
    Some((rom, track))
}

fn rom_game(rom: &[u8], coop: bool) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    if coop {
        g.set_coop(true);
    }
    g.reset();
    g
}

/// Movie pad 1 with pad 2 either idle or an LCG script.
fn rom_inputs(track: &[u8], n: usize, p2: bool) -> Vec<Pads> {
    let mut r = Lcg(0xC00F);
    (0..n)
        .map(|i| {
            let p1 = track.get(i).copied().unwrap_or(0);
            (p1, if p2 { r.p2() } else { 0 })
        })
        .collect()
}

#[test]
fn rom_checkpoints_restore_bit_exact() {
    let Some((rom, track)) = fixtures("rom_checkpoints_restore_bit_exact") else {
        return;
    };
    const K: usize = 120;
    // Title menus (mode $11), side-view ($0B), overworld ($05), an area
    // transition ($08 at 900), side-view, overworld, late game.
    let checkpoints = [60usize, 300, 700, 900, 1400, 2500, 5000];
    let inputs = rom_inputs(&track, 5000 + K, false);
    let mut g = rom_game(&rom, false);
    let mut f = 0;
    for &cp in &checkpoints {
        while f < cp {
            g.step(inputs[f].0);
            f += 1;
        }
        let mode = g.ram()[0x0736];
        let st = g.save_state();
        let first = straight(&mut g, &inputs[cp..cp + K], false);
        g.load_state(&st);
        let second = straight(&mut g, &inputs[cp..cp + K], false);
        assert_eq!(first, second, "checkpoint {cp}");
        eprintln!("checkpoint {cp}: mode ${mode:02X}, {K} frames bit-exact");
        f = cp + K;
    }
    assert_eq!(g.exec_errors, 0);
}

fn rom_storms(coop: bool) {
    let test = if coop {
        "rom_coop_rollback_storms"
    } else {
        "rom_rollback_storms"
    };
    let Some((rom, track)) = fixtures(test) else {
        return;
    };
    const START: usize = 1000;
    const N: usize = 2000;
    let inputs = rom_inputs(&track, START + N, coop);
    let mut base = rom_game(&rom, coop);
    for &p in &inputs[..START] {
        advance(&mut base, p, coop);
    }
    let prefix = base.save_state();
    let mut want = Vec::with_capacity(N);
    let mut p2_active = 0usize;
    for &p in &inputs[START..] {
        advance(&mut base, p, coop);
        want.push(frame_hash(&base));
        p2_active += usize::from(base.coop_status().is_some_and(|s| s.active));
    }
    if coop {
        assert!(p2_active > 500, "P2 should be live for much of the window");
    }

    let mut g = rom_game(&rom, coop);
    g.load_state(&prefix);
    storm_1(&mut g, &inputs[START..], &want, coop, test);
    let mut g = rom_game(&rom, coop);
    g.load_state(&prefix);
    storm_8(&mut g, &inputs[START..], &want, coop, test);
    if coop {
        eprintln!("{test}: {N} frames from {START} bit-exact ({p2_active} with P2 live)");
    } else {
        eprintln!("{test}: {N} frames from {START} bit-exact");
    }
}

#[test]
fn rom_rollback_storms() {
    rom_storms(false);
}

#[test]
fn rom_coop_rollback_storms() {
    rom_storms(true);
}

/// Prints native timings (no wall-clock assertions: they flake under load).
#[test]
fn rom_state_perf_report() {
    let Some((rom, track)) = fixtures("rom_state_perf_report") else {
        return;
    };
    let inputs = rom_inputs(&track, 3000, false);
    let mut g = rom_game(&rom, false);
    for &p in &inputs[..2500] {
        g.step(p.0);
    }
    let mut st = GameState::new();
    const REPS: u32 = 2000;
    let t = Instant::now();
    for _ in 0..REPS {
        g.save_state_into(&mut st);
    }
    let save = t.elapsed() / REPS;
    let t = Instant::now();
    for _ in 0..REPS {
        g.load_state(&st);
    }
    let load = t.elapsed() / REPS;
    let t = Instant::now();
    let bytes = st.to_bytes();
    let ser = t.elapsed();
    let t = Instant::now();
    let back = GameState::from_bytes(&bytes).expect("decode");
    let de = t.elapsed();
    assert_eq!(back.frame_count(), st.frame_count());
    let t = Instant::now();
    for _ in 0..400 {
        g.load_state(&st);
        for &p in &inputs[2500..2508] {
            g.step(p.0);
        }
    }
    let resim8 = t.elapsed() / 400;
    let t = Instant::now();
    for &p in &inputs[2500..3000] {
        g.step(p.0);
    }
    let frame = t.elapsed() / 500;
    eprintln!(
        "state perf: save {save:?}, load {load:?}, in-memory ~{} bytes, serialized {} bytes, \
         one frame {frame:?}, load+8 frames {resim8:?}, to_bytes {ser:?}, from_bytes {de:?}",
        GameState::approx_bytes(),
        bytes.len()
    );
}
