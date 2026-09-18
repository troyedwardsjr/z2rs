//! Rollback sync test against the real game: every frame rolls back
//! `check_distance` frames, re-simulates them and compares each re-saved
//! state's full checksum with the first one taken for that frame.
//!
//! This is the gate for enabling rollback netplay: a save state that misses
//! any emulation-affecting byte, or a step that is not a pure function of
//! (state, pads), shows up here as a mismatch with no network involved.
//!
//! ROM-gated (self-skips unless `Z2_ROM` names an existing file). Player 1
//! replays `$Z2_CORPUS/movies/anypct.bk2` when available, so the run spans the
//! title screen, the first side-view palace and the overworld; player 2 plays
//! a deterministic pseudo-random held-button script throughout.

mod common;

use z2_native::app::{self, Emu, Features};
use z2_native::netplay::{self, GameStateRing};
use z2_net::{apply_requests, RollbackEvent, RollbackGame, SyncTestSession};

/// Deterministic xorshift for the pad scripts.
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

/// Held-button script: a new random pad every 4..=35 frames, Start/Select
/// masked off most of the time so the game is not paused constantly.
fn scripted_pads(seed: u64, frames: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let mut pad = rng.next() as u8;
        if !rng.next().is_multiple_of(8) {
            pad &= !0x30; // Start ($10) and Select ($20) rarely
        }
        let hold = 4 + (rng.next() % 32) as usize;
        out.extend(std::iter::repeat_n(pad, hold));
    }
    out.truncate(frames);
    out
}

fn build(body: &[u8]) -> Emu {
    app::emu_from_rom_body_with(
        body,
        44_100,
        Features {
            coop: true,
            record: false,
        },
    )
    .expect("emulator builds from the verified ROM")
}

/// The run's pads: (pad1, pad2) per frame.
fn pads(frames: usize) -> (Vec<(u8, u8)>, bool) {
    let movie = common::var_present("Z2_CORPUS")
        .map(|c| std::path::Path::new(&c).join("movies").join("anypct.bk2"))
        .filter(|p| p.is_file());
    let (p1, from_movie) = match movie {
        Some(path) => {
            let mut t = app::load_movie_track(&path).expect("anypct.bk2 parses");
            let script = scripted_pads(0x1234_5678, frames);
            if t.len() < frames {
                t.extend_from_slice(&script[t.len()..]);
            }
            t.truncate(frames);
            (t, true)
        }
        None => {
            eprintln!("rollback_sync: no $Z2_CORPUS/movies/anypct.bk2, player 1 is scripted");
            (scripted_pads(0x1234_5678, frames), false)
        }
    };
    let p2 = scripted_pads(0x9E37_79B9_7F4A_7C15, frames);
    (p1.into_iter().zip(p2).collect(), from_movie)
}

fn frames() -> usize {
    if cfg!(debug_assertions) {
        300
    } else {
        3_000
    }
}

/// Where the reference run spent its frames.
#[derive(Debug, Default)]
struct Coverage {
    first_sideview: Option<usize>,
    sideview: usize,
    overworld_after_sideview: usize,
    modes: Vec<(usize, u8)>,
}

/// Straight run with no rollback: the final checksum every rollback run must
/// reproduce, plus where the run went.
fn reference(body: &[u8], pads: &[(u8, u8)]) -> (u64, Coverage) {
    let mut emu = build(body);
    let mut cov = Coverage::default();
    for (f, &(p1, p2)) in pads.iter().enumerate() {
        emu.game.step2(p1, p2);
        emu.game.apu.log.clear();
        let active = emu.game.coop_status().is_some_and(|s| s.active);
        if active {
            cov.sideview += 1;
            cov.first_sideview.get_or_insert(f);
        } else if cov.first_sideview.is_some() {
            cov.overworld_after_sideview += 1;
        }
        let mode = emu.game.ram()[0x0736];
        if cov.modes.last().is_none_or(|&(_, m)| m != mode) {
            cov.modes.push((f, mode));
        }
    }
    assert_eq!(emu.game.exec_errors, 0, "reference run faulted");
    (emu.checksum(), cov)
}

/// Drive `pads` through a `SyncTestSession` with the frontend's own request
/// executor (preallocated ring, silent replays). Returns the final checksum.
fn run_frontend_path(body: &[u8], pads: &[(u8, u8)], distance: u8) -> u64 {
    let mut emu = build(body);
    let mut sync = SyncTestSession::new(distance);
    let mut ring = GameStateRing::new(usize::from(distance) + 2);
    let mut pcm = Vec::new();
    let (mut advanced, mut replayed) = (0usize, 0usize);
    for &(p1, p2) in pads {
        let reqs = sync.advance(p1, p2);
        let out =
            netplay::apply_rollback_requests(&reqs, &mut emu, &mut ring, &mut sync, &mut pcm, None)
                .expect("ring holds every state the sync test loads");
        advanced += out.advanced as usize;
        replayed += out.replayed as usize;
        // Replayed frames' register writes are dropped, never left for the
        // synth to pick up with the next frame.
        assert!(emu.game.apu.log.is_empty(), "APU log not drained");
        if let Some(RollbackEvent::DesyncDetected {
            frame,
            local,
            remote,
        }) = sync.events().into_iter().next()
        {
            panic!(
                "distance {distance}: state at frame {frame} differs after rollback \
                 (first {local:#018x}, re-simulated {remote:#018x})"
            );
        }
    }
    assert_eq!(sync.mismatches(), 0);
    let d = usize::from(distance);
    assert_eq!(advanced, pads.len(), "one audible frame per tick");
    assert_eq!(
        replayed,
        (pads.len() - d) * d,
        "every rollback replays silently"
    );
    assert_eq!(emu.game.exec_errors, 0, "distance {distance}: run faulted");
    emu.checksum()
}

#[test]
fn real_game_survives_rollback_every_frame() {
    let test = "real_game_survives_rollback_every_frame";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
    let n = frames();
    let (pads, from_movie) = pads(n);
    let (want, cov) = reference(&body, &pads);
    eprintln!(
        "rollback_sync: {n} frames, movie={from_movie}, first side-view frame {:?}, \
         side-view frames {}, non-side-view frames after it {}, game-mode changes {:?}",
        cov.first_sideview, cov.sideview, cov.overworld_after_sideview, cov.modes
    );
    if from_movie && !cfg!(debug_assertions) {
        assert!(cov.sideview > 0, "run never reached side-view");
        assert!(
            cov.overworld_after_sideview > 0,
            "run never left side-view for the overworld"
        );
    }
    for distance in [1u8, 2, 8] {
        let got = run_frontend_path(&body, &pads, distance);
        assert_eq!(
            got, want,
            "distance {distance}: final state differs from the straight run"
        );
        eprintln!("rollback_sync: distance {distance}: {n} frames, 0 mismatches");
    }
}

/// The same check through `z2_net::apply_requests` and the `RollbackGame`
/// impl for the real game (allocating saves), at the widest distance.
#[test]
fn real_game_rollback_game_impl_is_deterministic() {
    let test = "real_game_rollback_game_impl_is_deterministic";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
    let n = frames();
    let (pads, _) = pads(n);
    let (want, _) = reference(&body, &pads);
    let mut emu = build(&body);
    let mut sync = SyncTestSession::new(8);
    let mut ring = z2_net::StateRing::new(10);
    for &(p1, p2) in &pads {
        let reqs = sync.advance(p1, p2);
        apply_requests(&mut emu, &mut ring, &reqs, &mut sync).expect("state held");
    }
    assert!(sync.events().is_empty(), "desync through RollbackGame impl");
    assert_eq!(sync.mismatches(), 0);
    assert_eq!(emu.checksum(), want);
}

/// Checksum sink shaped like a live session: a checksum every 60 frames.
struct EverySixty;

impl z2_net::ChecksumSink for EverySixty {
    fn needs_checksum(&self, frame: u32) -> bool {
        frame.is_multiple_of(60)
    }
    fn report_checksum(&mut self, _frame: u32, _checksum: u64) {}
}

/// Per-tick cost of the frontend's rollback path at prediction depths 0, 4
/// and 8 on the real game (run explicitly, in release:
/// `cargo test -p z2-native --release --test rollback_sync -- --ignored --nocapture`).
///
/// A depth-`d` tick is what a live session issues when a remote pad `d`
/// frames back proves mispredicted: load, `d` silent re-simulated frames with
/// their saves, then the new frame's save and step (audio synthesised).
#[test]
#[ignore = "timing measurement; run explicitly in release"]
fn rollback_tick_cost_at_prediction_depths() {
    let test = "rollback_tick_cost_at_prediction_depths";
    let Some(raw) = common::rom_bytes(test) else {
        return;
    };
    let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
    let warm = 400usize;
    let measured = 2_600usize;
    let (pads, _) = pads(warm + measured);
    for depth in [0u8, 4, 8] {
        let mut emu = build(&body);
        let mut ring = GameStateRing::new(usize::from(depth) + 2);
        let mut sync = SyncTestSession::new(depth.max(1));
        let mut pcm = Vec::new();
        let mut sink = EverySixty;
        let mut times = Vec::with_capacity(measured);
        for (f, &(p1, p2)) in pads.iter().enumerate() {
            let reqs = if depth == 0 {
                vec![
                    z2_net::RollbackRequest::SaveState { frame: f as u32 },
                    z2_net::RollbackRequest::Advance {
                        frame: f as u32,
                        pad1: p1,
                        pad2: p2,
                        confirmed: true,
                        replay: false,
                    },
                ]
            } else {
                sync.advance(p1, p2)
            };
            let t0 = std::time::Instant::now();
            netplay::apply_rollback_requests(&reqs, &mut emu, &mut ring, &mut sink, &mut pcm, None)
                .expect("state held");
            let dt = t0.elapsed();
            if f >= warm {
                times.push(dt.as_secs_f64() * 1e3);
            }
        }
        times.sort_by(f64::total_cmp);
        let pct = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        eprintln!(
            "rollback tick cost, depth {depth}: {measured} ticks, mean {mean:.3} ms, \
             median {:.3} ms, p99 {:.3} ms, max {:.3} ms",
            pct(0.5),
            pct(0.99),
            times[times.len() - 1]
        );
        assert!(
            pct(0.99) < 16.7,
            "depth {depth}: p99 tick {:.3} ms does not fit a 60 Hz frame",
            pct(0.99)
        );
    }
}
