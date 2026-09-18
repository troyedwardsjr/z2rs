//! `xtask fuzz`: divergence fuzz from snapshots.
//!
//! ```
//! cargo xtask fuzz --snapshot S.snap [--seeds N] [--frames M] [--dut game|oracle] [--rom PATH]
//! ```
//!
//! Replays the snapshot's history prefix on both sides (reaching the
//! snapshot state from power-on), then feeds `M` pseudo-random inputs
//! (deterministic xorshift per seed, so failures reproduce) and reports
//! the first [`Divergence`](z2_verify::lockstep::Divergence) per seed.
//! Exit 0 when all seeds run clean, 1 on any divergence, 2 on usage errors.

use z2_verify::lockstep::Lockstep;
use z2_verify::oracle::{Oracle, TetanesOracle};

use super::verify::GamePort;

fn usage() -> &'static str {
    "cargo xtask fuzz --snapshot <S.snap> [--seeds N] [--frames M] [--dut game|oracle] [--rom PATH]\n\
     defaults: --seeds 50 --frames 600 --dut game"
}

/// Deterministic xorshift64* stream; seed 0 maps to a fixed nonzero constant
/// so every seed (including 0) yields a distinct reproducible track.
struct XorShift(u64);

impl XorShift {
    fn seed(seed: u64) -> Self {
        Self(
            seed.wrapping_mul(0x9E3779B97F4A7C15)
                .wrapping_add(0x243F6A8885A308D3),
        )
    }
    fn next_u8(&mut self) -> u8 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545F4914F6CDD1D) >> 56) as u8
    }
}

/// Bias toward sparse inputs (mostly 0x00, occasional buttons + D-pad runs):
/// full-white-noise desyncs into walls immediately and covers less logic.
fn random_track(rng: &mut XorShift, frames: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames);
    let mut held = 0u8;
    for _ in 0..frames {
        let r = rng.next_u8();
        if r < 0x14 {
            // Fresh sparse press (single button or single direction).
            held = 1u8 << (rng.next_u8() & 7);
        } else if r < 0x1E {
            held = 0; // release
        }
        out.push(held);
    }
    out
}

pub fn run(args: &[String]) -> i32 {
    let mut snapshot: Option<String> = None;
    let mut seeds: u64 = 50;
    let mut frames: usize = 600;
    let mut dut = "game".to_string();
    let mut rom: Option<String> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--snapshot" => snapshot = it.next().cloned(),
            "--seeds" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => seeds = n,
                None => {
                    eprintln!("xtask fuzz: --seeds needs a number\n\n{0}", usage());
                    return 2;
                }
            },
            // Unlike `xtask verify`, 0 is not a whole-track sentinel here:
            // a random track has no natural length, so 0 would fuzz
            // nothing and still report green.
            "--frames" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) if n > 0 => frames = n,
                _ => {
                    eprintln!(
                        "xtask fuzz: --frames needs a positive number\n\n{0}",
                        usage()
                    );
                    return 2;
                }
            },
            "--dut" => {
                dut = it.next().cloned().unwrap_or_default();
                if dut != "oracle" && dut != "game" {
                    eprintln!("xtask fuzz: --dut wants oracle|game\n\n{0}", usage());
                    return 2;
                }
            }
            "--rom" => rom = it.next().cloned(),
            "-h" | "--help" => {
                println!("{0}", usage());
                return 0;
            }
            other => {
                eprintln!("xtask fuzz: unknown arg '{other}'\n\n{0}", usage());
                return 2;
            }
        }
    }
    let Some(snap_path) = snapshot else {
        eprintln!("xtask fuzz: need --snapshot S.snap\n\n{0}", usage());
        return 2;
    };

    let rom = match super::verify::rom_path(rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask fuzz: {e}");
            return 2;
        }
    };
    let bytes = match std::fs::read(&snap_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("xtask fuzz: read {snap_path}: {e}");
            return 2;
        }
    };
    let snap = match z2_verify::snapshot::Snapshot::decode(&bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask fuzz: decode {snap_path}: {}", e.0);
            return 2;
        }
    };
    let upto = super::verify::history_prefix_len(&snap);
    let prefix = snap.input_history[..upto].to_vec();

    let mut oracle = match TetanesOracle::load_rom(&rom) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask fuzz: load ROM {}: {e}", rom.display());
            return 2;
        }
    };

    let mut diverged = 0u64;
    for seed in 0..seeds {
        // Fresh instances per seed so one seed's divergence cannot poison
        // the next (both sides rebuild from power-on).
        let mut o = match TetanesOracle::load_rom(&rom) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("xtask fuzz: reload ROM: {e}");
                return 2;
            }
        };
        if let Err(d) = Lockstep::run(&mut oracle, &mut o, &prefix, prefix.len()) {
            println!("seed {seed}: PREFIX divergence (snapshot unreachable): {d}");
            diverged += 1;
            continue;
        }
        let mut rng = XorShift::seed(seed);
        let track = random_track(&mut rng, frames);
        if dut == "game" {
            let raw = match std::fs::read(&rom) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("xtask fuzz: read {}: {e}", rom.display());
                    return 2;
                }
            };
            if z2_assets::rom::open_at(&rom).is_err() {
                eprintln!("xtask fuzz: ROM gate rejected {}", rom.display());
                return 2;
            }
            let mut game = match z2_core::game::Game::from_ines(&raw) {
                Ok(g) => GamePort(g),
                Err(e) => {
                    eprintln!("xtask fuzz: build Game: {e:?}");
                    return 2;
                }
            };
            game.0.reset();
            register_all(&mut game.0);
            // Prefix on the game side too (reaches the snapshot state).
            if let Err(d) = Lockstep::run(&mut o, &mut game, &prefix, prefix.len()) {
                println!("seed {seed}: PREFIX game divergence: {d}");
                diverged += 1;
                continue;
            }
            match Lockstep::run(&mut o, &mut game, &track, frames) {
                Ok(()) => {}
                Err(d) => {
                    println!("seed {seed}: DIVERGENCE {d}");
                    diverged += 1;
                }
            }
        } else {
            let mut o2 = match TetanesOracle::load_rom(&rom) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("xtask fuzz: reload ROM: {e}");
                    return 2;
                }
            };
            if let Err(d) = Lockstep::run(&mut o, &mut o2, &prefix, prefix.len()) {
                println!("seed {seed}: PREFIX divergence: {d}");
                diverged += 1;
                continue;
            }
            match Lockstep::run(&mut o, &mut o2, &track, frames) {
                Ok(()) => {}
                Err(d) => {
                    println!("seed {seed}: DIVERGENCE {d}");
                    diverged += 1;
                }
            }
        }
    }
    if diverged == 0 {
        println!(
            "fuzz ok: '{label}' × {seeds} seeds × {frames} frames clean",
            label = snap.label
        );
        0
    } else {
        println!("fuzz: {diverged}/{seeds} seeds diverged (see above)");
        1
    }
}

fn register_all(game: &mut z2_core::game::Game) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic boot snapshot (empty history, zeroed RAM, valid label):
    /// exercises the whole fuzz path without a minted corpus.
    fn boot_snapshot_bytes() -> Vec<u8> {
        let snap = z2_verify::snapshot::Snapshot::new(
            vec![0u8; 0x800],
            vec![0u8; 0x2000],
            vec![],
            vec![],
            vec![],
            vec![],
            "boot-title",
            "synthetic",
            0,
            0,
        )
        .expect("synthetic snapshot");
        snap.encode().expect("encode")
    }

    #[test]
    fn xorshift_tracks_are_deterministic() {
        for seed in [0, 1, 42] {
            let (a, b) = (
                random_track(&mut XorShift::seed(seed), 600),
                random_track(&mut XorShift::seed(seed), 600),
            );
            assert_eq!(a, b, "seed {seed} reproduces");
            assert!(a.iter().any(|&b| b != 0), "seed {seed} presses buttons");
        }
    }

    /// Oracle-vs-oracle fuzz over blank input must be clean (needs Z2_ROM;
    /// skips gracefully without it like every other ROM-gated test).
    #[test]
    fn oracle_self_fuzz_is_clean() {
        // Absent = unset, empty, or not an existing file (public CI used to
        // export an empty `Z2_ROM`).
        let rom_present = std::env::var(z2_assets::rom::ROM_ENV_VAR)
            .is_ok_and(|p| !p.trim().is_empty() && std::path::Path::new(&p).is_file());
        if !rom_present {
            eprintln!("skipping oracle_self_fuzz_is_clean: Z2_ROM not set to an existing file");
            return;
        }
        let dir = std::env::temp_dir().join("z2rs-fuzz-selftest");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("boot.snap");
        std::fs::write(&path, boot_snapshot_bytes()).expect("write snapshot");
        let args = [
            "--snapshot".to_string(),
            path.to_string_lossy().into_owned(),
            "--seeds".to_string(),
            "2".to_string(),
            "--frames".to_string(),
            "5".to_string(),
            "--dut".to_string(),
            "oracle".to_string(),
        ];
        assert_eq!(run(&args), 0, "oracle self-fuzz clean");
        std::fs::remove_file(&path).ok();
    }
}
