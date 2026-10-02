//! `z2-rando`: a Zelda II randomizer written from scratch for z2rs.
//!
//! The player's own verified ROM goes in, a patched ROM body comes out.
//! Nothing here ships ROM bytes, upstream randomizer code, room data, text or
//! art: every feature is an original reimplementation of documented
//! behaviour, all vanilla data is read from the player's ROM at run time, and
//! 6502 patches are written as source for the crate's own assembler
//! ([`asm`]). See README.md and `LEGAL.md`.
//!
//! # Entry point
//!
//! [`randomize`] (or [`randomize_with`] for bring-your-own extras) verifies
//! the vanilla body, then runs the pipeline: a fixed list of modules
//! ([`PIPELINE`]), each `fn apply(ctx: &mut Ctx) -> Result<(), RandoError>`.
//! A module that cannot finish with the current random choices returns
//! [`RandoError::Retry`]; the whole pipeline then starts over from the
//! vanilla image with a fresh attempt number (up to [`MAX_ATTEMPTS`]).
//!
//! # Determinism
//!
//! Same vanilla ROM + seed text + flags + [`GENERATOR_VERSION`] gives a
//! byte-identical body on every platform. The master seed is SHA-1 of the
//! seed text and [`flags::Flags::seed_flag_string`]; each module draws from
//! its own stream ([`rng::Rng::derive`] with the module name and attempt
//! number), so one module changing how much it draws does not reshuffle the
//! others. Never iterate a `HashMap` for anything that affects output.
//!
//! With vanilla flags every module is a no-op and the output body equals
//! the input byte for byte (a test pins this).
//!
//! # Adding a pipeline step
//!
//! Put the work in the module you own (`src/<module>.rs`), keep `apply` as
//! the entry point, and read your options from `ctx.flags.<module>`. Shared
//! world/logic results go into [`State`] (add typed fields there, additive
//! edits only). If you need a brand-new module, add its file, a `mod` line
//! below and one entry in [`PIPELINE`] at the right position.

#![forbid(unsafe_code)]

pub mod asm;
pub mod flags;
pub mod ips;
pub mod rng;
pub mod rom;
pub mod sideview;
pub mod spoiler;
pub mod text;
pub mod trap_policy;
pub mod world;

pub mod asm_features;
pub mod cosmetic;
pub mod drops;
pub mod enemies;
pub mod hints;
pub mod items;
pub mod overworld;
pub mod palaces;
pub mod qol;
pub mod spells;
pub mod start;
pub mod stats;
pub mod towns;

use std::fmt;

use flags::{Flags, Tri};
use rng::Rng;
use rom::Rom;
use spoiler::Spoiler;

/// Bump whenever a change makes the same seed and flags produce a different
/// ROM. It is folded into the hash code (not the RNG), so players can tell
/// that two builds disagree.
pub const GENERATOR_VERSION: u32 = 2;

/// Whole-pipeline attempts before giving up.
pub const MAX_ATTEMPTS: u32 = 64;

/// Randomizer failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandoError {
    /// The input is not the expected vanilla ROM, or a ROM access was out of
    /// range.
    Rom(String),
    /// The flag string or option combination is invalid.
    Flags(String),
    /// A module could not finish with this attempt's random choices; the
    /// pipeline restarts with the next attempt.
    Retry(String),
    /// Every attempt failed.
    GaveUp {
        /// Attempts made.
        attempts: u32,
        /// Last retry reason.
        last: String,
    },
    /// Assembler error in a patch.
    Asm(String),
    /// Dialog text could not be encoded.
    Text(String),
    /// A bring-your-own IPS patch is malformed.
    Ips(String),
    /// Anything else a module wants to report.
    Other(String),
}

impl fmt::Display for RandoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RandoError::Rom(m) => write!(f, "ROM: {m}"),
            RandoError::Flags(m) => write!(f, "flags: {m}"),
            RandoError::Retry(m) => write!(f, "retry: {m}"),
            RandoError::GaveUp { attempts, last } => {
                write!(f, "no valid seed after {attempts} attempts (last: {last})")
            }
            RandoError::Asm(m) => write!(f, "patch assembly: {m}"),
            RandoError::Text(m) => write!(f, "text: {m}"),
            RandoError::Ips(m) => write!(f, "IPS: {m}"),
            RandoError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for RandoError {}

impl From<asm::AsmError> for RandoError {
    fn from(e: asm::AsmError) -> Self {
        RandoError::Asm(e.to_string())
    }
}

impl From<flags::FlagError> for RandoError {
    fn from(e: flags::FlagError) -> Self {
        RandoError::Flags(e.to_string())
    }
}

/// Bring-your-own inputs that are not flags (files the player supplies).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras {
    /// Raw IPS patch for the player sprite (applied by the `cosmetic`
    /// module).
    pub sprite_ips: Option<Vec<u8>>,
}

/// Shared world and logic state that modules hand to later modules.
///
/// Modules add typed fields here (additive edits only), for example
/// the resolved palace layouts, item placements or the spoiler-facing
/// location names. It starts empty on every attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Spell menu order and the resolved Fire option (`spells` module).
    pub spells: spells::SpellState,
    /// The dialog table, loaded on first use by [`text::table`] and written
    /// once after the last module ([`text::flush`]).
    pub text: Option<text::TextTable>,
    /// Magic containers the New Kasuto basement needs, when the `towns`
    /// module changed it (`None` = vanilla 7).
    pub new_kasuto_containers: Option<u8>,
    /// What every item location holds, for the hints (see
    /// [`hints::Spot`]). Only used when [`State::world`] was not built (the
    /// hints prefer the world model); when both are empty the hints
    /// describe the vanilla layout ([`hints::vanilla_spots`]).
    pub hint_spots: Vec<hints::Spot>,
    /// The game world as the logic sees it: overworld spots, item
    /// locations, requirements, starting inventory (see [`world`]). Built
    /// from the vanilla ROM by the `start` module (first in the pipeline);
    /// later modules edit it and `items` validates it with
    /// [`world::World::beatable`].
    pub world: world::World,
    /// The generated palaces: per palace its rooms (map, sideview and
    /// enemy pointers, item byte offsets), the new-room-data ranges used in
    /// banks 4/5, and the crystal count (`palaces` module).
    pub palaces: palaces::PalaceState,
}

/// Everything a module sees while it runs.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// The image being patched.
    pub rom: Rom,
    /// The untouched vanilla image (read-only reference).
    pub vanilla: Rom,
    /// This module's random stream (reset per module, see crate docs).
    pub rng: Rng,
    /// The options.
    pub flags: Flags,
    /// Seed text as typed.
    pub seed: String,
    /// Pipeline attempt (0-based).
    pub attempt: u32,
    /// Shared results.
    pub state: State,
    /// Spoiler log.
    pub spoiler: Spoiler,
    /// Developer log lines (returned in [`Output::log`]).
    pub log: Vec<String>,
    /// Bring-your-own inputs.
    pub extras: Extras,
}

impl Ctx {
    /// Resolve a [`Tri`] switch: `Random` comes out on with
    /// [`flags::StartFlags::random_flag_rate`].
    pub fn tri(&mut self, t: Tri) -> bool {
        match t {
            Tri::Off => false,
            Tri::On => true,
            Tri::Random => {
                let (n, d) = self.flags.start.random_flag_rate.ratio();
                self.rng.chance(n, d)
            }
        }
    }

    /// Append a developer log line.
    pub fn log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
    }
}

/// A module entry point.
pub type ApplyFn = fn(&mut Ctx) -> Result<(), RandoError>;

/// The pipeline, in run order. Order matters: later modules read what
/// earlier ones decided (`ctx.state`) and the ROM they left. QoL,
/// `asm_features` and cosmetics run last and never feed back into the world.
pub const PIPELINE: &[(&str, ApplyFn)] = &[
    ("start", start::apply),
    ("palaces", palaces::apply),
    ("overworld", overworld::apply),
    ("towns", towns::apply),
    ("spells", spells::apply),
    ("stats", stats::apply),
    ("items", items::apply),
    ("enemies", enemies::apply),
    ("drops", drops::apply),
    ("hints", hints::apply),
    ("qol", qol::apply),
    ("asm_features", asm_features::apply),
    ("cosmetic", cosmetic::apply),
];

/// Randomizer result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Headerless body (PRG then CHR), vanilla or expanded layout.
    pub body: Vec<u8>,
    /// PRG size in 16 KiB units (8 vanilla, 16 expanded).
    pub prg_units: u8,
    /// CHR size in 8 KiB units.
    pub chr_units: u8,
    /// CRC32 of [`Output::body`].
    pub crc32: u32,
    /// Six-character seed identifier (digits 1-5, 7-9 and A-X), the same for
    /// everyone with the same seed, gameplay flags and generator version.
    pub hash_code: String,
    /// Spoiler log text.
    pub spoiler: String,
    /// Developer log lines.
    pub log: Vec<String>,
    /// Fixed-bank CPU addresses whose bytes differ from vanilla (input to
    /// [`trap_policy::untrap_list`]).
    pub fixed_bank_changes: Vec<u16>,
    /// Pipeline attempts used.
    pub attempts: u32,
    /// Which module wrote which bytes (from the write-ownership guard, see
    /// [`rom::Rom::set_writer`]).
    pub write_runs: Vec<rom::WriteRun>,
}

impl Output {
    /// Full iNES image (synthetic MMC1 header + body).
    #[must_use]
    pub fn ines(&self) -> Vec<u8> {
        let mut v = rom::ines_header(self.prg_units, self.chr_units).to_vec();
        v.extend_from_slice(&self.body);
        v
    }
}

/// Master seed for `seed` + the seed-relevant flags.
#[must_use]
pub fn master_seed(seed: &str, flags: &Flags) -> u64 {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"z2rs-rando seed\0");
    msg.extend_from_slice(seed.as_bytes());
    msg.push(0);
    msg.extend_from_slice(flags.seed_flag_string().as_bytes());
    let d = z2_assets::rom::sha1_digest(&msg);
    u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

const HASH_ALPHABET: &[u8; 32] = b"12345789ABCDEFGHIJKLMNOPQRSTUVWX";

/// The six-character hash code shown in window titles and save names.
#[must_use]
pub fn hash_code(seed: &str, flags: &Flags) -> String {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"z2rs-rando hash\0");
    msg.extend_from_slice(&GENERATOR_VERSION.to_le_bytes());
    msg.extend_from_slice(seed.as_bytes());
    msg.push(0);
    msg.extend_from_slice(flags.seed_flag_string().as_bytes());
    let d = z2_assets::rom::sha1_digest(&msg);
    let mut v = u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
    let mut s = String::with_capacity(6);
    for _ in 0..6 {
        s.push(HASH_ALPHABET[(v & 31) as usize] as char);
        v >>= 5;
    }
    s
}

/// Randomize `vanilla_body` (headerless, verified against the known ROM).
pub fn randomize(vanilla_body: &[u8], seed: &str, flags: &Flags) -> Result<Output, RandoError> {
    randomize_with(vanilla_body, seed, flags, &Extras::default())
}

/// [`randomize`] with bring-your-own extras.
pub fn randomize_with(
    vanilla_body: &[u8],
    seed: &str,
    flags: &Flags,
    extras: &Extras,
) -> Result<Output, RandoError> {
    z2_assets::rom::verify_body(vanilla_body).map_err(|e| RandoError::Rom(e.to_string()))?;
    randomize_unverified(vanilla_body, seed, flags, extras)
}

/// The pipeline without the hash gate (synthetic images in tests). Callers
/// outside tests should use [`randomize`].
pub fn randomize_unverified(
    vanilla_body: &[u8],
    seed: &str,
    flags: &Flags,
    extras: &Extras,
) -> Result<Output, RandoError> {
    let mut flags = flags.clone();
    flags.normalize();
    let vanilla = Rom::from_body(vanilla_body)?;
    let master = Rng::new(master_seed(seed, &flags));
    let hash = hash_code(seed, &flags);
    let mut last = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        let mut ctx = Ctx {
            rom: vanilla.clone(),
            vanilla: vanilla.clone(),
            rng: master.clone(),
            flags: flags.clone(),
            seed: seed.to_string(),
            attempt,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: extras.clone(),
        };
        ctx.spoiler.set_header("Seed", seed);
        ctx.spoiler.set_header("Flags", flags.to_flag_string());
        ctx.spoiler.set_header("Hash", hash.clone());
        ctx.spoiler.set_header(
            "Generator",
            format!("z2rs {} (v{GENERATOR_VERSION})", env!("CARGO_PKG_VERSION")),
        );
        match run_pipeline(&mut ctx, &master) {
            Ok(()) => {
                let options = flags.describe_changes();
                if !options.is_empty() {
                    let sec = ctx.spoiler.section(spoiler::OPTIONS_SECTION);
                    for (m, f, v) in options {
                        sec.push(format!("{m}.{f} = {v}"));
                    }
                }
                let body = ctx.rom.body();
                let crc32 = z2_assets::rom::crc32_ieee(&body);
                let fixed_bank_changes = trap_policy::changed_fixed_bank_addrs(vanilla_body, &body);
                let write_runs = ctx.rom.write_runs();
                return Ok(Output {
                    prg_units: ctx.rom.prg_units(),
                    chr_units: ctx.rom.chr_units(),
                    body,
                    crc32,
                    hash_code: hash,
                    spoiler: ctx.spoiler.render(),
                    log: ctx.log,
                    fixed_bank_changes,
                    attempts: attempt + 1,
                    write_runs,
                });
            }
            Err(RandoError::Retry(why)) => last = why,
            Err(e) => return Err(e),
        }
    }
    Err(RandoError::GaveUp {
        attempts: MAX_ATTEMPTS,
        last,
    })
}

fn run_pipeline(ctx: &mut Ctx, master: &Rng) -> Result<(), RandoError> {
    for (name, apply) in PIPELINE {
        ctx.rng = master.derive(&format!("{name}#{}", ctx.attempt));
        ctx.rom.set_writer(name);
        apply(ctx).map_err(|e| match e {
            RandoError::Retry(m) => RandoError::Retry(format!("{name}: {m}")),
            other => other,
        })?;
        check_ownership(ctx)?;
    }
    ctx.rom.set_writer("text");
    text::flush(ctx)?;
    check_ownership(ctx)
}

/// The write-ownership guard: two modules writing the same byte is a bug
/// (one of them silently undoes the other), so it fails the run outright.
fn check_ownership(ctx: &Ctx) -> Result<(), RandoError> {
    match ctx.rom.conflicts() {
        [] => Ok(()),
        all => {
            let list: Vec<String> = all.iter().take(8).map(ToString::to_string).collect();
            Err(RandoError::Other(format!(
                "write-ownership conflict ({} bytes): {}",
                all.len(),
                list.join("; ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flags::Preset;

    fn synthetic_body() -> Vec<u8> {
        let mut rng = Rng::new(1);
        (0..rom::VANILLA_BODY_LEN)
            .map(|_| rng.next_u32() as u8)
            .collect()
    }

    #[test]
    fn vanilla_flags_leave_the_body_untouched() {
        let body = synthetic_body();
        for seed in ["", "0", "12345", "hello world"] {
            let out =
                randomize_unverified(&body, seed, &Flags::default(), &Extras::default()).unwrap();
            assert_eq!(out.body, body, "seed {seed:?}");
            assert_eq!(out.prg_units, 8);
            assert_eq!(out.chr_units, 16);
            assert!(out.fixed_bank_changes.is_empty());
            assert_eq!(out.crc32, z2_assets::rom::crc32_ieee(&body));
        }
    }

    #[test]
    fn same_inputs_same_output() {
        let body = synthetic_body();
        let f = Preset::MaxRando.flags();
        let a = randomize_unverified(&body, "race", &f, &Extras::default()).unwrap();
        let b = randomize_unverified(&body, "race", &f, &Extras::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn rejects_a_body_that_is_not_the_vanilla_rom() {
        let err = randomize(&synthetic_body(), "x", &Flags::default()).unwrap_err();
        assert!(matches!(err, RandoError::Rom(_)), "{err}");
    }

    #[test]
    fn hash_code_shape_and_sensitivity() {
        let f = Flags::default();
        let h = hash_code("seed", &f);
        assert_eq!(h.len(), 6);
        assert!(h.bytes().all(|b| HASH_ALPHABET.contains(&b)));
        assert!(!h.contains('0') && !h.contains('6'));
        assert_eq!(h, hash_code("seed", &f));
        assert_ne!(h, hash_code("seed2", &f));
        assert_ne!(h, hash_code("seed", &Preset::Standard.flags()));
        // Cosmetic and QoL choices do not change it.
        let mut g = Flags::default();
        g.cosmetic.disable_music = true;
        g.qol.fast_text = true;
        assert_eq!(h, hash_code("seed", &g));
        assert_eq!(master_seed("seed", &f), master_seed("seed", &g));
    }

    #[test]
    fn tri_resolution_uses_the_rate() {
        let body = synthetic_body();
        let vanilla = Rom::from_body(&body).unwrap();
        let mut ctx = Ctx {
            rom: vanilla.clone(),
            vanilla,
            rng: Rng::new(5),
            flags: Flags::default(),
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        };
        assert!(!ctx.tri(Tri::Off));
        assert!(ctx.tri(Tri::On));
        ctx.flags.start.random_flag_rate = flags::RandomFlagRate::NinetyPercent;
        let ons = (0..1000).filter(|_| ctx.tri(Tri::Random)).count();
        assert!((850..950).contains(&ons), "{ons}");
    }

    #[test]
    fn pipeline_lists_every_module_once() {
        let mut names: Vec<&str> = PIPELINE.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PIPELINE.len());
        assert_eq!(PIPELINE.len(), 13);
    }

    /// ROM-gated: vanilla flags reproduce the player's ROM exactly, for any
    /// seed, and two runs agree.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_vanilla_flags_identity_and_determinism() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        for seed in ["", "1", "test", "a much longer seed text 123"] {
            let out = randomize(&body, seed, &Flags::default()).unwrap();
            assert_eq!(out.body, body, "seed {seed:?}");
            assert_eq!(out.crc32, z2_assets::rom::EXPECTED_BODY_CRC32);
        }
        for p in Preset::ALL {
            let a = randomize(&body, "determinism", &p.flags()).unwrap();
            let b = randomize(&body, "determinism", &p.flags()).unwrap();
            assert_eq!(a, b, "{p:?}");
        }
    }
}
