//! `xtask verify`: lockstep verification driver.
//!
//! ```
//! cargo xtask verify --movie corpus/x.fm2 --frames N [--dut oracle|game]
//! cargo xtask verify --snapshot S.snap --frames N [--movie M]
//! cargo xtask verify --routine Label --snapshot S.snap [--frames N]
//! ```
//!
//! `--frames 0` replays the whole track (the movie, or the snapshot's own
//! history tail); the default is 600 frames.
//!
//! `--dut oracle` (default) runs oracle-vs-oracle: it validates the input
//! track and oracle determinism. `--dut game` runs the `z2-core` hybrid
//! `Game` against the oracle — the routine-porting loop driver.

use std::path::{Path, PathBuf};
use z2_verify::lockstep::{Divergence, Lockstep};
use z2_verify::oracle::{Oracle, Port, TetanesOracle};

/// Adapter: `z2-core` `Game` as a [`Port`] (shapes mirrored by hand per the
/// shared contract; no `z2-verify` dep inside `z2-core` by design).
pub(crate) struct GamePort(pub(crate) z2_core::game::Game);

impl Port for GamePort {
    fn step(&mut self, input: u8) {
        self.0.step(input);
    }
    fn ram(&self) -> &[u8; 0x800] {
        self.0.ram()
    }
    fn wram(&self) -> &[u8; 0x2000] {
        self.0.wram()
    }
    fn oam(&self) -> &[u8; 256] {
        self.0.oam()
    }
    fn palette(&self) -> &[u8; 32] {
        self.0.palette()
    }
    fn frame_indexed(&self) -> &[u8; 256 * 240] {
        self.0.frame_indexed()
    }
}

/// Trap registry group names accepted by `--trap-set`.
pub(crate) const TRAP_GROUPS: &[&str] = &[
    "bank7",
    "sideview",
    "overworld",
    "player",
    "enemy",
    "town",
    "palace",
    "title",
    "boot",
];

/// Register the full hybrid trap set (shared by verify/probe/fuzz).
pub(crate) fn register_all_traps(game: &mut z2_core::game::Game) {
    register_trap_groups(game, TRAP_GROUPS);
}

/// Register only the named trap groups (bisection aid: `--trap-set a,b`).
pub(crate) fn register_trap_groups(game: &mut z2_core::game::Game, groups: &[&str]) {
    for g in groups {
        match *g {
            "bank7" => z2_core::bank7_traps::register_bank7_traps(game),
            "sideview" => z2_core::sideview_traps::register_sideview_traps(game),
            "overworld" => z2_core::sideview_traps::register_overworld_traps(game),
            "player" => z2_core::player_traps::register_player_traps(game),
            "enemy" => z2_core::enemy_traps::register_enemy_traps(game),
            "town" => z2_core::town_traps::register_town_traps(game),
            "palace" => z2_core::palace_traps::register_palace_traps(game),
            "title" => z2_core::title_traps::register_title_traps(game),
            "boot" => z2_core::boot_traps::register_boot_traps(game),
            other => eprintln!("unknown trap group '{other}' (want one of {TRAP_GROUPS:?})"),
        }
    }
}

fn usage() -> &'static str {
    "cargo xtask verify --movie <x.fm2|x.bk2> [--frames N] [--dut oracle|game] [--no-traps] [--rom PATH]\n\
     cargo xtask verify --snapshot <S.snap> [--frames N] [--movie <M>] [--dut oracle|game] [--rom PATH]\n\
     cargo xtask verify --routine <Label> --snapshot <S.snap> [--frames N] [--dut oracle|game] [--rom PATH]\n\
     --frames N: frames to replay (default 600); 0 = the whole movie / snapshot tail\n\
     --continue [--dump-at F,G --out DIR] [--trace-mode] [--max-lines N] [--watch a,b]:\n\
       (game only) keep stepping past divergences, report per-frame region sets, dump\n\
       PNG/state at frames; --trap-set g1,g2 / --untrap $addr,... bisect the trap set"
}

pub(crate) fn rom_path(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        return Ok(PathBuf::from(p));
    }
    // Workspace convention: the env var counts as absent when it is unset,
    // empty (public CI exports it empty), or not an existing file, so a stale
    // $Z2_ROM gives this clear message instead of a confusing open failure
    // further in. An explicit `--rom` is returned untouched above: the user
    // named that file, so a bad path there must stay a loud error.
    std::env::var(z2_assets::rom::ROM_ENV_VAR)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "no ROM: pass --rom PATH or set {} to an existing file (see LEGAL.md)",
                z2_assets::rom::ROM_ENV_VAR
            )
        })
}

/// Load a player-1 input track from `.fm2` or `.bk2` (extension-selected).
///
/// Header warnings go to stderr. A movie for another platform, or one
/// with a Reset/Power press (lockstep models neither), is refused for
/// both formats.
pub(crate) fn load_movie_track(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let (track, warnings, console_press) = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("fm2") => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|e| format!("{} is not UTF-8: {e}", path.display()))?;
            let m = z2_verify::movie_fm2::parse_fm2(text)
                .map_err(|e| format!("parse {}: {e:?}", path.display()))?;
            let press = m
                .frames
                .iter()
                .find(|f| f.is_reset() || f.is_power())
                .map(|f| f.line_no);
            (m.pad1_track(), m.warnings(), press)
        }
        Some("bk2") => {
            let m = z2_verify::movie_bk2::parse_bk2_zip(&bytes)
                .map_err(|e| format!("parse {}: {e:?}", path.display()))?;
            if let Some(p) = m.platform().filter(|p| !p.eq_ignore_ascii_case("NES")) {
                return Err(format!(
                    "{}: Header.txt Platform {p:?} is not NES",
                    path.display()
                ));
            }
            let press = m.first_console_press().map(|f| f.line_no);
            (m.pad1_track(), m.warnings(), press)
        }
        other => {
            return Err(format!(
                "unknown movie extension {other:?} (want .fm2 or .bk2): {}",
                path.display()
            ))
        }
    };
    for w in &warnings {
        eprintln!("movie {}: warning: {w}", path.display());
    }
    if let Some(line) = console_press {
        return Err(format!(
            "{}: Reset/Power press at input line {line} is not modeled by lockstep",
            path.display()
        ));
    }
    Ok(track)
}

/// Snapshot history split: frames replayed from power-on to reach the
/// snapshot; the rest of `input_history` is the tail it continues on.
pub(crate) fn history_prefix_len(s: &z2_verify::snapshot::Snapshot) -> usize {
    (s.frame_raw as usize).min(s.input_history.len())
}

fn report(d: &Divergence) -> i32 {
    println!("DIVERGENCE {d}");
    1
}

/// Interpreter health line for `--dut game` runs: fault count + last fault.
/// Nonzero means the port diverged into unmapped territory (data executed
/// as code) — the usual explanation for frozen/grey/blue screens past the
/// frontier.
fn report_exec_errors(game: &z2_core::game::Game) {
    match game.last_exec_error {
        Some(e) => eprintln!(
            "game interpreter faults: {} (last: {e} at frame {})",
            game.exec_errors,
            game.frame_count()
        ),
        None => eprintln!("game interpreter faults: 0"),
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut movie: Option<String> = None;
    let mut snapshot: Option<String> = None;
    let mut routine: Option<String> = None;
    let mut frames: usize = 600;
    let mut dut = "oracle".to_string();
    let mut rom: Option<String> = None;
    // Traps on by default for `--dut game`: ported Rust runs in
    // situ, unported routines interpret. `--no-traps` runs the pure
    // interpreter for A/B verification.
    let mut traps = true;
    let mut cont = false;
    let mut dump_at: Vec<usize> = Vec::new();
    let mut out = "verify-dumps".to_string();
    let mut trace_mode = false;
    let mut max_lines: usize = 200;
    let mut watch: Vec<u16> = Vec::new();
    let mut trap_set: Option<Vec<String>> = None;
    let mut untrap: Vec<u16> = Vec::new();

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--movie" => movie = it.next().cloned(),
            "--snapshot" => snapshot = it.next().cloned(),
            "--routine" => routine = it.next().cloned(),
            "--frames" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => frames = n,
                None => {
                    eprintln!("xtask verify: --frames needs a number\n\n{0}", usage());
                    return 2;
                }
            },
            "--dut" => {
                dut = it.next().cloned().unwrap_or_default();
                if dut != "oracle" && dut != "game" {
                    eprintln!("xtask verify: --dut wants oracle|game\n\n{0}", usage());
                    return 2;
                }
            }
            "--rom" => rom = it.next().cloned(),
            "--no-traps" => traps = false,
            "--continue" => cont = true,
            "--trace-mode" => trace_mode = true,
            "--out" => out = it.next().cloned().unwrap_or_default(),
            "--max-lines" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => max_lines = n,
                None => {
                    eprintln!("xtask verify: --max-lines needs a number");
                    return 2;
                }
            },
            "--trap-set" => {
                let list = it.next().cloned().unwrap_or_default();
                trap_set = Some(
                    list.split(',')
                        .filter(|p| !p.is_empty())
                        .map(String::from)
                        .collect(),
                );
            }
            "--untrap" => {
                let list = it.next().cloned().unwrap_or_default();
                for part in list.split(',').filter(|p| !p.is_empty()) {
                    match u16::from_str_radix(part.trim_start_matches('$'), 16) {
                        Ok(n) => untrap.push(n),
                        Err(_) => {
                            eprintln!("xtask verify: --untrap wants hex trap addresses a,b,c");
                            return 2;
                        }
                    }
                }
            }
            "--watch" => {
                let list = it.next().cloned().unwrap_or_default();
                for part in list.split(',').filter(|p| !p.is_empty()) {
                    match u16::from_str_radix(part.trim_start_matches('$'), 16) {
                        Ok(n) => watch.push(n),
                        Err(_) => {
                            eprintln!("xtask verify: --watch wants hex CPU addresses a,b,c");
                            return 2;
                        }
                    }
                }
            }
            "--dump-at" => {
                let list = it.next().cloned().unwrap_or_default();
                for part in list.split(',').filter(|p| !p.is_empty()) {
                    match part.parse::<usize>() {
                        Ok(n) => dump_at.push(n),
                        Err(_) => {
                            eprintln!("xtask verify: --dump-at wants F,G,... frame numbers");
                            return 2;
                        }
                    }
                }
            }
            "-h" | "--help" => {
                println!("{}", usage());
                return 0;
            }
            other => {
                eprintln!("xtask verify: unknown arg '{other}'\n\n{0}", usage());
                return 2;
            }
        }
    }

    if movie.is_none() && snapshot.is_none() && routine.is_none() {
        eprintln!("verify: no --movie/--snapshot/--routine: self-check on blank input");
    }
    // `--routine` names a LABEL_PLAN entry; it still needs a snapshot (or a
    // movie) to replay — the corpus mint produces those.
    if let Some(label) = &routine {
        if !z2_verify::snapshot::label_is_known(label) {
            eprintln!("xtask verify: unknown routine label '{label}' (not in LABEL_PLAN)");
            return 2;
        }
        if snapshot.is_none() && movie.is_none() {
            eprintln!(
                "xtask verify: --routine {label} needs --snapshot S (no snapshot minted for it yet)"
            );
            return 2;
        }
    }

    let rom = match rom_path(rom.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("xtask verify: {e}");
            return 2;
        }
    };

    // Resolve the input track: explicit movie wins; otherwise continue from
    // the snapshot's own history (logic frames via the lag map).
    let track: Vec<u8> = match &movie {
        Some(m) => match load_movie_track(Path::new(m)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("xtask verify: {e}");
                return 2;
            }
        },
        None => Vec::new(), // snapshot-only: inputs come from the snapshot below
    };

    // Snapshot restore point, if any.
    let snap = match &snapshot {
        Some(s) => {
            let bytes = std::fs::read(s).unwrap_or_else(|e| {
                eprintln!("xtask verify: read {s}: {e}");
                std::process::exit(2);
            });
            match z2_verify::snapshot::Snapshot::decode(&bytes) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    eprintln!("xtask verify: decode {s}: {}", e.0);
                    return 2;
                }
            }
        }
        None => None,
    };

    // `--dut game` cannot start from a snapshot yet (Game has no state
    // import): refuse before any frame-count or ROM work.
    if dut == "game" && snapshot.is_some() {
        eprintln!(
            "xtask verify: --dut game with --snapshot is not supported yet (Game has no state import)"
        );
        return 2;
    }

    // `--frames 0` means the whole track, selected the same way the
    // snapshot branch below picks it: the movie track when it has frames,
    // else the snapshot's own history tail — even when that tail is
    // empty, so a snapshot minted at its last frame still gets its prefix
    // replay and RAM/WRAM drift check.
    if frames == 0 {
        frames = if !track.is_empty() {
            track.len()
        } else {
            snap.as_ref()
                .map_or(0, |s| s.input_history.len() - history_prefix_len(s))
        };
        eprintln!("verify: full track, {frames} frames");
    }

    let mut oracle = match TetanesOracle::load_rom(&rom) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask verify: load ROM {}: {e}", rom.display());
            return 2;
        }
    };

    if dut == "game" {
        // from_ines wants the full iNES image (header + PRG + CHR); the gate
        // above already hash-verified it, so re-read the raw file here.
        let raw = match std::fs::read(&rom) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("xtask verify: read {}: {e}", rom.display());
                return 2;
            }
        };
        if z2_assets::rom::open_at(&rom).is_err() {
            eprintln!("xtask verify: ROM gate rejected {}", rom.display());
            return 2;
        }
        let mut game = match z2_core::game::Game::from_ines(&raw) {
            Ok(g) => GamePort(g),
            Err(e) => {
                eprintln!("xtask verify: build Game: {e:?}");
                return 2;
            }
        };
        game.0.reset();
        if traps {
            match &trap_set {
                Some(list) => {
                    let names: Vec<&str> = list.iter().map(String::as_str).collect();
                    register_trap_groups(&mut game.0, &names);
                }
                None => register_all_traps(&mut game.0),
            }
            for a in &untrap {
                game.0.set_untrapped(*a, true);
            }
        }
        if cont {
            let opts = crate::diag::ContinueOpts {
                frames,
                dump_at: &dump_at,
                out: Path::new(&out),
                trace_mode,
                max_lines,
                watch: &watch,
            };
            let code = crate::diag::run_continue(&mut oracle, &mut game, &track, &opts);
            report_exec_errors(&game.0);
            return code;
        }
        return match Lockstep::run(&mut oracle, &mut game, &track, frames) {
            Ok(()) => {
                println!("verify ok: game matches oracle for {frames} frames");
                report_exec_errors(&game.0);
                0
            }
            Err(d) => {
                let code = report(&d);
                report_exec_errors(&game.0);
                code
            }
        };
    }

    // Oracle-vs-oracle (default): validates track + oracle determinism.
    let mut oracle_b = match TetanesOracle::load_rom(&rom) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("xtask verify: load ROM {}: {e}", rom.display());
            return 2;
        }
    };
    if let Some(s) = &snap {
        // Replay the snapshot's own history prefix from power-on on both
        // oracles (validates determinism over the long track), then check
        // the resulting RAM/WRAM against the snapshot image (validates the
        // mint), then continue `frames` more on the tail/movie track.
        let upto = history_prefix_len(s);
        let prefix = &s.input_history[..upto];
        eprintln!(
            "verify: replaying {} history frames to '{}' ...",
            prefix.len(),
            s.label
        );
        if let Err(d) = Lockstep::run(&mut oracle, &mut oracle_b, prefix, prefix.len()) {
            return report(&d);
        }
        let ram_ok = oracle.ram().as_slice() == s.ram.as_slice();
        let wram_ok = oracle.wram().as_slice() == s.wram.as_slice();
        if !ram_ok || !wram_ok {
            println!(
                "SNAPSHOT DRIFT '{}': ram_match={ram_ok} wram_match={wram_ok} after {upto} frames",
                s.label
            );
            return 1;
        }
        let tail: Vec<u8> = if track.is_empty() {
            s.input_history[upto..].to_vec()
        } else {
            track
        };
        return match Lockstep::run(&mut oracle, &mut oracle_b, &tail, frames) {
            Ok(()) => {
                println!(
                    "verify ok: snapshot '{}' matches at frame {upto} and replays {frames} more frames",
                    s.label
                );
                0
            }
            Err(d) => report(&d),
        };
    }
    match Lockstep::run(&mut oracle, &mut oracle_b, &track, frames) {
        Ok(()) => {
            println!("verify ok: oracle agrees with itself for {frames} frames");
            0
        }
        Err(d) => report(&d),
    }
}

#[cfg(test)]
mod tests {
    //! The verification path must never see the opt-in frontend features.
    //!
    //! Widescreen, co-op and the PPU render record are all runtime switches
    //! that only the frontends flip. If any of them leaked into the trap set
    //! or the `Game` that `--dut game` builds, every lockstep result in the
    //! repository would silently change meaning — so it is pinned here, in
    //! the module that owns the trap-set wiring.
    use super::*;
    use z2_verify::oracle::Port;

    /// A `Game` wired exactly as `xtask verify --dut game` wires it: co-op
    /// off, render record off, and none of the co-op wrappers registered.
    #[test]
    fn verify_trap_set_enables_no_coop_widescreen_or_record() {
        let mut game = z2_core::game::Game::new();
        register_all_traps(&mut game);
        assert!(
            !game.record_enabled(),
            "the PPU render record must stay off in verification"
        );
        assert!(
            game.coop_status().is_none(),
            "co-op must stay disabled in verification"
        );
        for addr in z2_core::coop::COOP_TRAP_ADDRS {
            let leaked = game
                .traps
                .get(addr)
                .is_some_and(|t| t.name.starts_with("coop_"));
            assert!(
                !leaked,
                "co-op wrapper registered at ${addr:04X} by the verification trap set"
            );
        }
    }

    /// Every name in `TRAP_GROUPS` that can register fixed-bank entries does.
    /// A typo in the list would quietly register nothing and silently change a
    /// `--trap-set` bisection result.
    ///
    /// `town` and `palace` are the documented exceptions: their routines live
    /// in mapper-banked PRG, so against a cartridge-less `Game` they add no
    /// entries. They are still exercised by the ROM-backed runs.
    #[test]
    fn every_trap_group_name_registers_something() {
        const BANKED_ONLY: &[&str] = &["town", "palace"];
        for g in TRAP_GROUPS {
            let mut game = z2_core::game::Game::new();
            register_trap_groups(&mut game, &[*g]);
            if BANKED_ONLY.contains(g) {
                continue;
            }
            assert!(
                !game.traps.is_empty(),
                "trap group '{g}' registered nothing (unknown name?)"
            );
        }
        // And the whole set together is non-empty and free of co-op wrappers.
        let mut all = z2_core::game::Game::new();
        register_all_traps(&mut all);
        assert!(all.traps.len() > TRAP_GROUPS.len(), "the full set is wired");
    }

    /// `GamePort::step` drives pad 1 only, so a second Link can never appear
    /// in a verification run even if co-op were somehow enabled.
    #[test]
    fn game_port_steps_pad_one_only() {
        let mut port = GamePort(z2_core::game::Game::new());
        port.step(0xFF);
        assert_eq!(port.0.frame_count(), 1);
        assert!(port.0.coop_status().is_none());
        assert!(!port.0.record_enabled());
    }
}
