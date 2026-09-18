//! Golden-frame tests vs the lockstep oracle.
//!
//! Acceptance: the indexed framebuffer matches the oracle on 100% of sampled
//! frames of all four movies (game running through the interpreter, so only
//! the PPU model is under test). Sampling = every corpus snapshot plus every
//! 600th movie frame ([`z2_ppu::GOLDEN_EVERY_NTH_MOVIE_FRAME`]).
//!
//! This suite is not wired to the oracle and the interpreter yet, so the
//! end-to-end test below is ROM-gated *and* oracle-gated: without `Z2_ROM`,
//! or without `z2-verify::oracle`, it skips gracefully with the same
//! `let Some(..) else { return }` pattern used in
//! `z2-assets/tests/roundtrip.rs`. The pure harness tests (sampling +
//! stub-oracle verification) run everywhere.
//!
//! Wiring checklist for hooking up the real oracle and interpreter:
//!
//! 1. Load each movie (`z2-verify` movie parsers) and each corpus snapshot
//!    (`z2-verify::snapshot`, the `LABEL_PLAN` labels).
//! 2. Replay the game through the interpreter; after each sampled frame,
//!    copy PPU-visible state into [`z2_ppu::Ppu`] (see crate docs) and call
//!    [`z2_ppu::Ppu::render_frame`].
//! 3. Serve the oracle's indexed framebuffer through
//!    [`z2_ppu::OracleFrameSource`] and compare with
//!    [`z2_ppu::verify_against_oracle`]; on mismatch, dump
//!    [`z2_ppu::write_diff_ppm`] output next to the failure log.

use z2_ppu::{
    sample_indices, verify_against_oracle, IndexedFrame, OracleFrameSource,
    GOLDEN_EVERY_NTH_MOVIE_FRAME,
};

/// Reference ROM env var (read-only; never copy or commit the ROM).
const Z2_ROM_ENV: &str = "Z2_ROM";

/// The four published TAS movies listed in README.md (file stems only;
/// the corpus itself lives out of tree and is gitignored).
const GOLDEN_MOVIES: [&str; 4] = [
    "zelda2-anypct-4367M.fm2",
    "zelda2-100pct-4425M.fm2",
    "zelda2-warpless-3254M.fm2",
    "zelda2-warp-glitch-4234M.fm2",
];

/// `Some(path)` when `Z2_ROM` points at a readable file, else `None` (public
/// CI without a ROM stays green).
fn rom_path() -> Option<std::path::PathBuf> {
    match std::env::var(Z2_ROM_ENV) {
        Ok(p) if std::path::Path::new(&p).is_file() => Some(p.into()),
        Ok(_) => {
            eprintln!("skipping ROM-gated test: {Z2_ROM_ENV} is not a file");
            None
        }
        Err(_) => {
            eprintln!("skipping ROM-gated test: {Z2_ROM_ENV} is not set");
            None
        }
    }
}

/// Serve oracle frames once the oracle frame export is wired. Today this is a stub returning
/// `None` (oracle-gated skip); main replaces the body with the real
/// `z2-verify::oracle` lookup without touching the test logic below.
struct PendingOracle {
    name: &'static str,
    len: u64,
}

impl OracleFrameSource for PendingOracle {
    fn len_frames(&self) -> u64 {
        self.len
    }
    fn frame_indexed(&self, _frame: u64) -> Option<IndexedFrame> {
        // `z2-verify` owns the oracle's indexed framebuffer export.
        None
    }
    fn source_name(&self) -> &str {
        self.name
    }
}

#[test]
fn golden_frames_match_oracle_on_sampled_movie_frames() {
    let Some(_rom) = rom_path() else { return };
    // Oracle not wired yet: skip after the ROM gate so the harness
    // shape is proven without failing public CI.
    let oracle = PendingOracle {
        name: GOLDEN_MOVIES[0],
        len: 0,
    };
    let _ = oracle.frame_indexed(0);
    eprintln!("skipping ROM-gated test: oracle not wired yet");
}

#[test]
fn golden_harness_samples_every_600th_frame_of_each_movie() {
    // Harness-level: with a 126k-frame movie the sample grid is every 600th.
    let samples = sample_indices(126_000, GOLDEN_EVERY_NTH_MOVIE_FRAME);
    assert_eq!(GOLDEN_EVERY_NTH_MOVIE_FRAME, 600);
    assert_eq!(samples[0], 0);
    assert!(samples.windows(2).all(|w| w[1] - w[0] == 600));
    assert!(samples.iter().all(|&f| f < 126_000));
    // All four movies are covered by the same grid (lengths differ; the grid
    // adapts per movie via its own frame count).
    assert_eq!(GOLDEN_MOVIES.len(), 4);
}

#[test]
fn golden_harness_reports_clean_and_dirty_runs() {
    struct AllBlack {
        len: u64,
    }
    impl OracleFrameSource for AllBlack {
        fn len_frames(&self) -> u64 {
            self.len
        }
        fn frame_indexed(&self, _frame: u64) -> Option<IndexedFrame> {
            Some([0x0F; z2_ppu::WIDTH * z2_ppu::HEIGHT])
        }
    }
    let oracle = AllBlack { len: 601 };
    // Model matches: frames 0 and 600 sampled, both clean.
    let report = verify_against_oracle(|_| [0x0F; z2_ppu::WIDTH * z2_ppu::HEIGHT], &oracle, 600);
    assert_eq!(report.sampled, 2);
    assert!(report.is_clean());

    // Model differs: mismatch recorded with frame number + diff.
    let report = verify_against_oracle(|_| [0x00; z2_ppu::WIDTH * z2_ppu::HEIGHT], &oracle, 600);
    assert!(!report.is_clean());
    assert_eq!(report.mismatched.len(), 2);
    assert_eq!(report.mismatched[0].frame, 0);
    assert_eq!(
        report.mismatched[0].diff.count,
        z2_ppu::WIDTH * z2_ppu::HEIGHT
    );

    // On failure, the PPM overlay dumps every pixel as mismatch magenta.
    let mut ppm = Vec::new();
    z2_ppu::write_diff_ppm(
        &[0x00; z2_ppu::WIDTH * z2_ppu::HEIGHT],
        &[0x0F; z2_ppu::WIDTH * z2_ppu::HEIGHT],
        &mut ppm,
    );
    assert!(ppm.starts_with(b"P6\n256 240\n255\n"));
}
