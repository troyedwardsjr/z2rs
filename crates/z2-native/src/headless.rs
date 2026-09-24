//! Headless automation CLI surface.
//!
//! `z2-native --headless [--snapshot S] [--movie M] [--frames N]
//! [--dump facts.json] [--dump-frame out.png] [--dump-at LIST,PREFIX]`
//!
//! Scope: this file owns the **CLI surface** —
//! argument parsing, exit codes and the windowless run used on CI.
//! Nothing here opens a window: the run only steps the standalone
//! [`z2_core::game::Game`] through the shared [`crate::app::step_frames`]
//! primitive (the same funnel as the windowed loop), exports
//! [`GameFacts`] JSON and/or a framebuffer PNG, and exits — so it runs on
//! a display-less CI runner with no ROM and no oracle.
//!
//! Wiring for `z2-native/src/main.rs`:
//! ```ignore
//! fn main() {
//!     let argv: Vec<String> = std::env::args().collect();
//!     if z2_native::headless::is_headless(&argv) {
//!         std::process::exit(z2_native::headless::run_argv(&argv));
//!     }
//!     // ... launch the windowed frontend
//! }
//! ```
//!
//! Exit codes: `0` ran (or `--help`), `2` usage error, `3` reserved
//! (historically "gated on the oracle/PPU" — nothing gates anymore, kept
//! so scripts matching on codes keep working), `4` I/O error.

use std::fmt;
use z2_core::facts::Game;
use z2_core::ram::Ram;

/// Parsed `--headless` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadlessArgs {
    /// INTERIM snapshot path: a raw 2048-byte NES CPU-RAM image, copied
    /// into the `Game`'s RAM before stepping (WRAM starts zeroed — the
    /// canonical snapshot schema with full state import does not exist
    /// yet, same limitation as `xtask verify --dut game --snapshot`).
    pub snapshot: Option<String>,
    /// Battery-RAM image (`sram.sav`, 8192 bytes) copied into WRAM before
    /// stepping — reproduces a windowed session that restored a save file.
    pub sram: Option<String>,
    /// Movie path (`.fm2`/`.bk2` via [`crate::app::load_movie_track`]).
    /// Oracle-free demo mode: the bytes just feed
    /// [`crate::app::step_frames`] — no lockstep verification.
    pub movie: Option<String>,
    /// Frames to run. `0` with no `--movie` is the smoke path (exports
    /// initial facts, steps nothing); `0` with `--movie` runs the whole
    /// track; otherwise exactly `N` frames (movie prefix + blank `0x00`
    /// padding past the end of the track).
    pub frames: u64,
    /// Where to write the [`GameFacts`] JSON dump.
    pub dump_facts: Option<String>,
    /// Where to write the final framebuffer PNG (indexed frame through
    /// the display-only NES palette, `std`-only encoder below).
    pub dump_frame: Option<String>,
    /// Raw `--dump-at` value, `FRAMES,PREFIX` (e.g. `450,472,palace`):
    /// after stepping each listed 1-based frame, write `<prefix>N.png`
    /// (same encoder as [`Self::dump_frame`]). Parsed by
    /// [`parse_dump_at`] once the run starts.
    pub dump_at: Option<String>,
    /// ROM path. Explicit `--rom` wins; else `$Z2_ROM`; with neither, the
    /// run uses the synthetic no-cartridge `Game`.
    pub rom: Option<String>,
    /// Widescreen preset (`off` | `16:10` | `16:9` | `N`). Applied before
    /// stepping, because the margin decoder needs the PPU render record to
    /// have been on during the run.
    pub widescreen: Option<String>,
    /// Where to write the composed widescreen PNG (needs a ROM for CHR).
    pub dump_wide: Option<String>,
    /// `--wide-gameplay on|off`: enemies spawn and live in the widescreen
    /// margins. Off unless asked for: headless runs replay movies, and this
    /// changes gameplay (a movie desyncs once an encounter moves).
    pub wide_gameplay: bool,
    /// Enable two-Link co-op so pad 2 drives a second Link.
    pub coop: bool,
    /// Hold this constant byte on pad 2 every frame (co-op only).
    pub p2_hold: Option<u8>,
    /// Pad 2 mirrors pad 1 every frame (co-op only).
    pub p2_mirror: bool,
    /// Where to write the co-op status JSON.
    pub dump_coop: Option<String>,
    /// HD graphics pack directory (`pack.json` inside).
    pub hd_pack: Option<String>,
    /// Output multiplier for [`Self::dump_present`] (1..=8, default 1).
    pub hd_scale: Option<u32>,
    /// Where to write the fully composed RGBA PNG (widescreen + HD pack +
    /// scale, exactly what the windowed frontend puts on screen).
    pub dump_present: Option<String>,
    /// `--margin-sprites on|off`: side-view objects outside the window in the
    /// widescreen margins (default on, like the windowed frontend).
    pub margin_sprites_flag: Option<bool>,
}

impl HeadlessArgs {
    /// Whether side-view objects are drawn into the margins (default on).
    fn margin_sprites(&self) -> bool {
        self.margin_sprites_flag.unwrap_or(true)
    }

    /// Margin tiles per side for this run.
    ///
    /// `--dump-wide` without `--widescreen` implies `16:9`, so the flag is
    /// useful on its own.
    fn wide_tiles(&self) -> u8 {
        match &self.widescreen {
            Some(p) => z2_ppu::preset_tiles(p).unwrap_or(0),
            None if self.dump_wide.is_some() => 11,
            None => 0,
        }
    }

    /// Visual settings for this run, shared with the windowed frontend.
    fn display_settings(&self) -> crate::app::DisplaySettings {
        crate::app::DisplaySettings {
            wide_tiles: self.wide_tiles(),
            scale: self.hd_scale.unwrap_or(1),
            // On by default for the same reason the app defaults them on:
            // the overworld blanks x0-7 and masks x248-255, which would
            // otherwise leave a black seam on each side of the play field.
            fill_left_clip: true,
            fill_right_clip: true,
            margin_sprites: self.margin_sprites(),
            pack_dir: self.hd_pack.as_deref().map(std::path::PathBuf::from),
            record_dir: None,
        }
    }

    /// Pad-2 byte for a given pad-1 byte.
    ///
    /// Note that `Game::step2` still filters the pad-2 manual-save chord, so
    /// `--p2-hold 0x11` (Up+A) reaches the game as A only.
    fn pad2_for(&self, p1: u8) -> u8 {
        match self.p2_hold {
            Some(h) => h,
            None if self.p2_mirror => p1,
            None => 0,
        }
    }
}

/// CLI usage text (also serves `--headless --help`).
pub const HEADLESS_USAGE: &str = "\
usage: z2-native --headless [--snapshot S] [--movie M] [--frames N] [--dump facts.json] [--dump-frame out.png]
  --snapshot S    raw 2048-byte CPU-RAM image as initial state (INTERIM format; WRAM starts zeroed)
  --sram F        8192-byte battery-RAM image (the app's sram.sav) loaded into WRAM before stepping
  --rom PATH      cartridge ROM (else $Z2_ROM, else synthetic no-ROM emu)
  --movie M       .fm2/.bk2 input track to play back (oracle-free: bytes feed Game::step, no verification)
  --frames N      frames to emulate (default 0: smoke when no --movie, whole track with --movie)
  --dump PATH     write GameFacts JSON of the final state here
  --dump-frame P  write final framebuffer PNG here (display palette, std-only encoder)
  --dump-at L,P   after each 1-based frame in LIST write PREFIX N .png
                  (`450,472,palace` writes palace450.png + palace472.png)
  --widescreen P  widescreen margins: off | 16:10 | 16:9 | N tiles per side (0-16)
  --dump-wide P   write the composed widescreen PNG here (needs a ROM for CHR;
                  implies --widescreen 16:9 when that flag is absent)
  --margin-sprites on|off
                  side-view enemies, NPCs and items outside the window in the
                  widescreen margins (display only; default on)
  --wide-gameplay on|off
                  enemies spawn and live in the widescreen margins (default off
                  here: it changes gameplay, so movies desync; needs a margin)
  --coop          enable two-Link co-op (pad 2 drives a second Link in side-view)
  --p2-hold MASK  hold this pad-2 byte every frame (decimal or 0x..; with --coop)
  --p2-mirror     pad 2 copies pad 1 every frame (with --coop)
  --dump-coop P   write the co-op status JSON here
  --hd-pack DIR   HD graphics pack directory (the one holding pack.json)
  --hd-scale N    output multiplier 1-8 for --dump-present (default 1)
  --dump-present P
                  write the fully composed RGBA PNG here (widescreen + HD pack +
                  scale: exactly what the windowed frontend presents)
  --help          print this text
exit codes: 0 ran/help, 2 usage error, 3 reserved (was: gated on oracle/PPU), 4 I/O error";

/// Result of classifying `argv`.
///
/// `Run` is much larger than the other two variants now that the argument
/// struct carries the widescreen/co-op flags, but this enum is built exactly
/// once per process, at startup, and immediately destructured — boxing it
/// would trade a pointless allocation for a size difference nothing pays for.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// No `--headless` flag: the caller should launch the GUI.
    NotHeadless,
    /// `--help` was passed: print [`HEADLESS_USAGE`].
    Help,
    /// Run windowless with these args.
    Run(HeadlessArgs),
}

/// True when `argv` asks for the headless surface (used by `main` to route).
pub fn is_headless(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--headless")
}

/// Parse `argv` (including `argv[0]`). Never touches the display.
pub fn parse_headless_args(argv: &[String]) -> Result<ParseOutcome, String> {
    if !is_headless(argv) {
        return Ok(ParseOutcome::NotHeadless);
    }
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(ParseOutcome::Help);
    }
    let mut args = HeadlessArgs::default();
    let mut it = argv.iter().skip(1).peekable();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "--headless" => {}
            "--snapshot" => args.snapshot = Some(value_of(&mut it, "--snapshot")?),
            "--sram" => args.sram = Some(value_of(&mut it, "--sram")?),
            "--movie" => args.movie = Some(value_of(&mut it, "--movie")?),
            "--frames" => {
                let raw = value_of(&mut it, "--frames")?;
                args.frames = raw
                    .parse::<u64>()
                    .map_err(|_| format!("--frames expects a non-negative integer, got '{raw}'"))?;
            }
            "--dump" => args.dump_facts = Some(value_of(&mut it, "--dump")?),
            "--dump-frame" => args.dump_frame = Some(value_of(&mut it, "--dump-frame")?),
            "--dump-at" => args.dump_at = Some(value_of(&mut it, "--dump-at")?),
            "--rom" => args.rom = Some(value_of(&mut it, "--rom")?),
            "--widescreen" => {
                let raw = value_of(&mut it, "--widescreen")?;
                if z2_ppu::preset_tiles(&raw).is_none() {
                    return Err(format!(
                        "--widescreen expects off | 16:10 | 16:9 | a number 0-16, got '{raw}'\n{HEADLESS_USAGE}"
                    ));
                }
                args.widescreen = Some(raw);
            }
            "--dump-wide" => args.dump_wide = Some(value_of(&mut it, "--dump-wide")?),
            "--margin-sprites" => {
                let raw = value_of(&mut it, "--margin-sprites")?;
                args.margin_sprites_flag = Some(match raw.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => {
                        return Err(format!(
                            "--margin-sprites expects on | off, got '{raw}'\n{HEADLESS_USAGE}"
                        ))
                    }
                });
            }
            "--wide-gameplay" => {
                let raw = value_of(&mut it, "--wide-gameplay")?;
                args.wide_gameplay = match raw.as_str() {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => {
                        return Err(format!(
                            "--wide-gameplay expects on|off, got '{raw}'\n{HEADLESS_USAGE}"
                        ))
                    }
                };
            }
            "--coop" => args.coop = true,
            "--p2-hold" => {
                let raw = value_of(&mut it, "--p2-hold")?;
                let parsed = match raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
                    Some(hex) => u8::from_str_radix(hex, 16),
                    None => raw.parse::<u8>(),
                };
                args.p2_hold = Some(parsed.map_err(|_| {
                    format!("--p2-hold expects a byte 0-255 (decimal or 0x..), got '{raw}'\n{HEADLESS_USAGE}")
                })?);
            }
            "--p2-mirror" => args.p2_mirror = true,
            "--dump-coop" => args.dump_coop = Some(value_of(&mut it, "--dump-coop")?),
            "--hd-pack" => args.hd_pack = Some(value_of(&mut it, "--hd-pack")?),
            "--dump-present" => args.dump_present = Some(value_of(&mut it, "--dump-present")?),
            "--hd-scale" => {
                let raw = value_of(&mut it, "--hd-scale")?;
                let n: u32 = raw.parse().unwrap_or(0);
                if n == 0 || n > z2_render::MAX_SCALE {
                    return Err(format!(
                        "--hd-scale expects 1-{}, got '{raw}'\n{HEADLESS_USAGE}",
                        z2_render::MAX_SCALE
                    ));
                }
                args.hd_scale = Some(n);
            }
            other => return Err(format!("unknown flag '{other}'\n{HEADLESS_USAGE}")),
        }
    }
    Ok(ParseOutcome::Run(args))
}

fn value_of<'a, I>(it: &mut std::iter::Peekable<I>, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = &'a String>,
{
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} expects a value\n{HEADLESS_USAGE}"))
}

/// Parse a [`HeadlessArgs::dump_at`] value: every comma item but the last
/// is a 1-based frame number, the last is the file-name prefix.
pub fn parse_dump_at(raw: &str) -> Result<(Vec<usize>, String), String> {
    let Some((frames_raw, prefix)) = raw.rsplit_once(',') else {
        return Err(format!(
            "--dump-at expects FRAMES,PREFIX (e.g. 450,472,palace), got '{raw}'"
        ));
    };
    if prefix.is_empty() || frames_raw.is_empty() {
        return Err(format!(
            "--dump-at expects FRAMES,PREFIX (e.g. 450,472,palace), got '{raw}'"
        ));
    }
    let mut frames = Vec::new();
    for item in frames_raw.split(',') {
        let n: usize = item
            .parse()
            .map_err(|_| format!("--dump-at: bad frame '{item}' in '{raw}'"))?;
        if n == 0 {
            return Err(format!(
                "--dump-at: frame numbers are 1-based, got '{item}' in '{raw}'"
            ));
        }
        frames.push(n);
    }
    Ok((frames, prefix.to_string()))
}

/// Per-frame PNG snapshots requested by `--dump-at`.
///
/// Owned by the stepping loop of [`run_headless`]; [`Self::observe`] runs
/// once per stepped frame and writes nothing unless the frame is a target
/// (the encoder is the same std-only 256x240 one as `--dump-frame`).
struct FrameDumps {
    targets: Vec<usize>,
    prefix: String,
    written: Vec<String>,
}

impl FrameDumps {
    /// No-dump instance (steps pass straight through `observe`).
    fn inactive() -> Self {
        Self {
            targets: Vec::new(),
            prefix: String::new(),
            written: Vec::new(),
        }
    }

    /// Parsed from the raw `--dump-at` value; `None` is [`Self::inactive`].
    fn parse(raw: Option<&String>) -> Result<Self, HeadlessError> {
        match raw {
            None => Ok(Self::inactive()),
            Some(raw) => {
                parse_dump_at(raw)
                    .map_err(HeadlessError::Usage)
                    .map(|(targets, prefix)| Self {
                        targets,
                        prefix,
                        written: Vec::new(),
                    })
            }
        }
    }

    /// After `count` frames stepped (1-based), write the PNG if `count` is
    /// a target.
    fn observe(&mut self, count: usize, emu: &mut crate::app::Emu) -> Result<(), HeadlessError> {
        if !self.targets.contains(&count) {
            return Ok(());
        }
        let path = format!("{}{count}.png", self.prefix);
        let png = encode_frame_png(emu.game.frame_indexed());
        std::fs::write(&path, png).map_err(HeadlessError::Io)?;
        self.written.push(path);
        Ok(())
    }
}

/// Windowless run report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessReport {
    /// Frames emulated through the shared [`crate::app::step_frames`] path.
    pub frames_run: u64,
    /// Facts dump written, if requested.
    pub facts_path: Option<String>,
    /// Frame dump written, if requested.
    pub frame_path: Option<String>,
    /// Multi-frame dumps written by `--dump-at`, in write order.
    pub dump_at_paths: Vec<String>,
    /// Widescreen PNG written, if requested.
    pub wide_path: Option<String>,
    /// Co-op status JSON written, if requested.
    pub coop_path: Option<String>,
    /// Composed RGBA PNG written, if requested.
    pub present_path: Option<String>,
    /// Human-readable note (input source, snapshot seeding).
    pub note: String,
}

/// Headless failure modes.
#[derive(Debug)]
pub enum HeadlessError {
    /// Bad flag combination or bad input file (parse errors, malformed
    /// snapshot).
    Usage(String),
    /// Historically: needs work that had not landed yet (`need` names the
    /// missing prerequisite). Retained for API/exit-code stability —
    /// [`run_headless`] no longer returns it (the standalone `Game` path
    /// it was waiting on has landed).
    OracleGated {
        /// Name of the prerequisite that is missing.
        need: &'static str,
        /// What specifically is unavailable.
        detail: String,
    },
    /// Filesystem failure.
    Io(std::io::Error),
}

impl fmt::Display for HeadlessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeadlessError::Usage(msg) => write!(f, "usage error: {msg}\n{HEADLESS_USAGE}"),
            HeadlessError::OracleGated { need, detail } => {
                write!(f, "gated on {need}: {detail}")
            }
            HeadlessError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for HeadlessError {}

/// Run windowless. No window, no GPU, no audio device: safe on display-less
/// CI. No ROM, no oracle needed — the synthetic (no-cartridge) `Game`.
///
/// * Builds the emulator via [`crate::app::new_emu`], seeds RAM from the
///   optional INTERIM `--snapshot` image, steps `--frames` frames of blank
///   (`0x00`) input or of the `--movie` track through the SAME shared
///   primitive as the windowed loop ([`crate::app::step_frames`]).
/// * `--dump` writes the final-state [`GameFacts`] JSON; `--dump-frame`
///   writes the final indexed framebuffer as PNG (display palette);
///   `--dump-at L,P` writes `<P>N.png` after each listed 1-based frame N
///   while the run steps.
pub fn run_headless(args: &HeadlessArgs) -> Result<HeadlessReport, HeadlessError> {
    // -- initial RAM (INTERIM 2048-byte image, as before) -------------------
    let snapshot_ram: Option<[u8; Ram::LEN]> = match &args.snapshot {
        None => None,
        Some(path) => {
            let bytes = std::fs::read(path).map_err(HeadlessError::Io)?;
            let ram = Ram::from_slice(&bytes).map_err(|e| {
                HeadlessError::Usage(format!(
                    "{path}: {e} (INTERIM snapshot format = raw 2048-byte CPU-RAM image)"
                ))
            })?;
            Some(*ram.as_slice())
        }
    };
    // -- input track (oracle-free demo playback, same loader as windowed) --
    let track: Vec<u8> = match &args.movie {
        None => Vec::new(),
        Some(path) => crate::app::load_movie_track(std::path::Path::new(path)).map_err(|e| {
            // `load_movie_track` folds I/O + parse errors into a string;
            // read failures start with "read <path>:" — keep the exit-code
            // contract (4 = I/O, 2 = usage) by splitting on that prefix.
            if e.starts_with("read ") {
                HeadlessError::Io(std::io::Error::other(e))
            } else {
                HeadlessError::Usage(format!("--movie {path}: {e}"))
            }
        })?,
    };
    // -- build + seed the emulator -----------------------------------------
    // ROM cartridge when available (explicit --rom, else $Z2_ROM);
    // otherwise the synthetic no-ROM emu (boot never runs there).
    // An explicit `--rom` is used as given (a bad path is a loud error);
    // `$Z2_ROM` counts as absent when unset, empty, or not an existing file.
    let rom_path: Option<std::path::PathBuf> = match &args.rom {
        Some(p) => Some(p.into()),
        None => crate::app::env_path_if_usable(z2_assets::rom::ROM_ENV_VAR),
    };
    // Features go in at construction: co-op traps must be registered before
    // `Game::reset`, and the widescreen record must be armed before the first
    // stepped frame or there is nothing to compose margins from.
    let display_settings = args.display_settings();
    let mut feats = display_settings.features(args.coop);
    if args.wide_gameplay {
        if display_settings.wide_tiles == 0 {
            return Err(HeadlessError::Usage(
                "--wide-gameplay on needs a widescreen margin (--widescreen P or --dump-wide)"
                    .into(),
            ));
        }
        feats.wide_gameplay = Some(display_settings.wide_tiles);
    }
    if args.dump_wide.is_some() && rom_path.is_none() {
        return Err(HeadlessError::Usage(
            "--dump-wide needs a ROM (--rom PATH or $Z2_ROM): margins are decoded from \
             cartridge CHR"
                .into(),
        ));
    }
    let mut emu = match &rom_path {
        Some(path) => {
            crate::app::emu_from_rom_file_with(path, crate::audio::RATE_44100, feats)
                .map_err(HeadlessError::Usage)?
                .0
        }
        None => crate::app::new_emu_with(crate::audio::RATE_44100, feats),
    };
    if let Some(path) = &args.sram {
        let bytes = std::fs::read(path)
            .map_err(|e| HeadlessError::Io(std::io::Error::other(format!("{path}: {e}"))))?;
        if bytes.len() != emu.game.wram.len() {
            return Err(HeadlessError::Usage(format!(
                "{path}: {} bytes, want {}",
                bytes.len(),
                emu.game.wram.len()
            )));
        }
        emu.game.wram.copy_from_slice(&bytes);
    }
    if let Some(img) = &snapshot_ram {
        emu.game.ram.copy_from_slice(img);
    }
    // `--sram` / `--snapshot` replaced memory under a possibly live co-op
    // state, so drop the second Link's parked block: it describes RAM that no
    // longer exists. It re-anchors beside player 1 on the next side-view frame.
    emu.game.coop_reset_area();
    // -- step through the shared primitive (never duplicated here) ---------
    let want = usize::try_from(args.frames).unwrap_or(usize::MAX);
    let mut dumps = FrameDumps::parse(args.dump_at.as_ref())?;
    let mut stepped: usize = 0;
    if track.is_empty() {
        stepped += step_blank(&mut emu, want, args, &mut dumps)?;
    } else if want == 0 {
        // `--movie` with `--frames 0` (or default): the whole track.
        stepped += step_track(&mut emu, &track, args, &mut dumps)?;
    } else {
        let head = want.min(track.len());
        stepped += step_track(&mut emu, &track[..head], args, &mut dumps)?;
        stepped += step_blank(&mut emu, want - head, args, &mut dumps)?;
    }
    // -- dumps of the final state ------------------------------------------
    let mut report = HeadlessReport {
        frames_run: stepped as u64,
        facts_path: None,
        frame_path: None,
        dump_at_paths: Vec::new(),
        wide_path: None,
        coop_path: None,
        present_path: None,
        note: format!(
            "stepped {stepped} frame(s) via shared app::step_frames ({}; {}; {})",
            match &args.movie {
                Some(p) => format!("movie {p}"),
                None => "blank input".to_string(),
            },
            match &args.snapshot {
                Some(p) => format!("RAM seeded from snapshot {p}"),
                None => "power-on RAM".to_string(),
            },
            match &args.rom {
                Some(p) => format!("ROM {p}"),
                None => match crate::app::env_path_if_usable(z2_assets::rom::ROM_ENV_VAR) {
                    Some(p) => format!("ROM ${{Z2_ROM}}={}", p.display()),
                    None => "no ROM (synthetic emu)".to_string(),
                },
            }
        ),
    };
    if let Some(path) = &args.dump_facts {
        // `Game::ram` is always exactly 2048 bytes, so this cannot fail.
        let ram = Ram::from_slice(&emu.game.ram)
            .map_err(|e| HeadlessError::Usage(format!("internal RAM image: {e}")))?;
        let json = Game::new(ram).facts().to_json_pretty();
        std::fs::write(path, json).map_err(HeadlessError::Io)?;
        report.facts_path = Some(path.clone());
    }
    if let Some(path) = &args.dump_frame {
        // Deliberately the plain 256x240 encoder: `--dump-frame` is the
        // verification-shaped dump and must not change shape with widescreen.
        let png = encode_frame_png(emu.game.frame_indexed());
        std::fs::write(path, png).map_err(HeadlessError::Io)?;
        report.frame_path = Some(path.clone());
    }
    if let Some(path) = &args.dump_wide {
        // The INDEXED widescreen dump: palette indices straight out of
        // `compose_wide`, no scaling and no pack. `--dump-present` is the RGBA
        // dump that matches the window.
        let tiles = args.wide_tiles();
        if tiles == 0 {
            return Err(HeadlessError::Usage(format!(
                "--dump-wide needs a widescreen margin > 0 (got --widescreen {})",
                args.widescreen.as_deref().unwrap_or("off")
            )));
        }
        let mut margins = z2_ppu::Margins::new(tiles);
        margins.fill_left_clip = true;
        margins.fill_right_clip = true;
        margins.fill_left_sprites = args.margin_sprites();
        let mut wide = z2_ppu::WideFrame::new(tiles);
        let armed = emu.game.compose_wide(tiles, &mut margins, &mut wide);
        if !armed {
            return Err(HeadlessError::Usage(
                "--dump-wide: the PPU render record was not armed for this run".into(),
            ));
        }
        let png = z2_ppu::encode_indexed_png_wh(wide.width, wide.height, &wide.pixels);
        std::fs::write(path, png).map_err(HeadlessError::Io)?;
        report.wide_path = Some(path.clone());
    }
    if let Some(path) = &args.dump_present {
        // Exactly the windowed present path: one `app::Display`, one frame.
        let mut display =
            crate::app::Display::new(display_settings.clone()).map_err(HeadlessError::Usage)?;
        let (w, h) = display.size();
        let rgba = display
            .present(&emu.game)
            .map_err(HeadlessError::Usage)?
            .to_vec();
        let png = z2_render::encode_png_rgba(w, h, &rgba, &[])
            .map_err(|e| HeadlessError::Usage(format!("--dump-present: encode PNG: {e}")))?;
        std::fs::write(path, png).map_err(HeadlessError::Io)?;
        report.present_path = Some(path.clone());
    }
    if let Some(path) = &args.dump_coop {
        let json = match emu.game.coop_status() {
            Some(st) => serde_json::to_string_pretty(&st)
                .map_err(|e| HeadlessError::Usage(format!("encode co-op status: {e}")))?,
            None => "null".to_string(),
        };
        std::fs::write(path, json).map_err(HeadlessError::Io)?;
        report.coop_path = Some(path.clone());
    }
    report.dump_at_paths = std::mem::take(&mut dumps.written);
    // Interpreter health (see Game::exec_errors): nonzero faults mean the
    // run wedged somewhere — the frame dumps above are frozen states.
    match emu.game.last_exec_error {
        Some(e) => eprintln!(
            "headless: interpreter faults: {} (last: {e} at frame {})",
            emu.game.exec_errors,
            emu.game.frame_count()
        ),
        None => eprintln!("headless: interpreter faults: 0"),
    }
    Ok(report)
}

/// Step `n` blank (`0x00`) frames. Returns frames stepped.
fn step_blank(
    emu: &mut crate::app::Emu,
    n: usize,
    args: &HeadlessArgs,
    dumps: &mut FrameDumps,
) -> Result<usize, HeadlessError> {
    let mut done = 0;
    for _ in 0..n {
        step_one_frame(emu, 0, args);
        done += 1;
        dumps.observe(done, emu)?;
    }
    Ok(done)
}

/// Step exactly one frame with the scripted pad-2 policy.
fn step_one_frame(emu: &mut crate::app::Emu, pad: u8, args: &HeadlessArgs) -> usize {
    if args.coop {
        // Deliberately `step2` only under `--coop`: the single-pad path must
        // not touch pad 2 even by writing a zero.
        crate::app::step_frames2(emu, &[(pad, args.pad2_for(pad))], None)
    } else {
        crate::app::step_frames(emu, &[pad], None)
    }
}

/// Step a pad-1 track. Returns frames stepped.
fn step_track(
    emu: &mut crate::app::Emu,
    pads: &[u8],
    args: &HeadlessArgs,
    dumps: &mut FrameDumps,
) -> Result<usize, HeadlessError> {
    let mut done = 0;
    for &p in pads {
        step_one_frame(emu, p, args);
        done += 1;
        dumps.observe(done, emu)?;
    }
    Ok(done)
}

/// Encode the indexed framebuffer as a true-colour PNG (std-only).
///
/// Thin wrapper over [`z2_ppu::encode_indexed_png`] (display-only NES
/// palette — never used for verification).
fn encode_frame_png(indexed: &[u8; crate::app::FRAME_LEN]) -> Vec<u8> {
    z2_ppu::encode_indexed_png(indexed)
}

/// Parity probe: step `frames` synthetic `Game` frames through the
/// SAME shared primitive as the windowed loop
/// ([`crate::app::new_emu`] + [`crate::app::step_frames`], with per-frame
/// APU audio pushed into a [`crate::audio::SharedAudio`] ring).
///
/// Returns `(game_frames, audio_depth)`.
pub fn shared_step_selftest(frames: usize, audio_rate: u32) -> (u64, usize) {
    let mut emu = crate::app::new_emu(audio_rate);
    let ring = crate::audio::SharedAudio::new(audio_rate);
    let inputs = vec![0u8; frames];
    let _ = crate::app::step_frames(&mut emu, &inputs, Some(&ring));
    (emu.game.frame_count(), ring.depth())
}

/// `main`-ready entry point: parse, run, print; returns the exit code.
/// `NotHeadless` prints usage to stderr and returns 2 (the caller normally
/// routes that case to the GUI instead of calling this).
pub fn run_argv(argv: &[String]) -> i32 {
    match parse_headless_args(argv) {
        Err(msg) => {
            eprintln!("{msg}");
            2
        }
        Ok(ParseOutcome::NotHeadless) => {
            eprintln!("{HEADLESS_USAGE}");
            2
        }
        Ok(ParseOutcome::Help) => {
            println!("{HEADLESS_USAGE}");
            0
        }
        Ok(ParseOutcome::Run(args)) => match run_headless(&args) {
            Ok(report) => {
                println!(
                    "headless ok: frames={} note: {}",
                    report.frames_run, report.note
                );
                if let Some(p) = &report.facts_path {
                    println!("headless ok: facts -> {p}");
                }
                if let Some(p) = &report.frame_path {
                    println!("headless ok: frame -> {p}");
                }
                for p in &report.dump_at_paths {
                    println!("headless ok: frame -> {p}");
                }
                0
            }
            Err(HeadlessError::OracleGated { need, detail }) => {
                eprintln!("headless gated on {need}: {detail}");
                3
            }
            Err(HeadlessError::Usage(msg)) => {
                eprintln!("{msg}");
                2
            }
            Err(HeadlessError::Io(e)) => {
                eprintln!("headless I/O error: {e}");
                4
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// Unique scratch dir per test (tests run parallel in one process, so
    /// the process id alone is not unique — the `tag` must differ).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("z2-headless-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// Minimal 2-frame `.fm2` fixture (same shape as the `app` movie test:
    /// neutral then A). Returns the file path.
    fn fm2_fixture(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("t.fm2");
        std::fs::write(
            &p,
            "version 3\npalFlag 0\nromFilename Zelda II.nes\n|0|........|\n|0|.......A|\n",
        )
        .unwrap();
        p
    }

    #[test]
    fn routes_gui_when_flag_absent() {
        assert_eq!(
            parse_headless_args(&argv(&["z2-native"])),
            Ok(ParseOutcome::NotHeadless)
        );
        assert!(!is_headless(&argv(&["z2-native"])));
    }

    #[test]
    fn parses_full_surface() {
        let out = parse_headless_args(&argv(&[
            "z2-native",
            "--headless",
            "--snapshot",
            "s.bin",
            "--movie",
            "m.fm2",
            "--frames",
            "600",
            "--dump",
            "facts.json",
            "--dump-frame",
            "out.png",
            "--dump-at",
            "450,472,palace",
        ]))
        .expect("parses");
        assert_eq!(
            out,
            ParseOutcome::Run(HeadlessArgs {
                snapshot: Some("s.bin".into()),
                sram: None,
                movie: Some("m.fm2".into()),
                frames: 600,
                dump_facts: Some("facts.json".into()),
                dump_frame: Some("out.png".into()),
                dump_at: Some("450,472,palace".into()),
                rom: None,
                ..Default::default()
            })
        );
    }

    #[test]
    fn parses_dump_at_values_and_rejects_bad_ones() {
        let out = parse_headless_args(&argv(&["z2-native", "--headless", "--dump-at", "2,pal"]))
            .expect("parses");
        let ParseOutcome::Run(a) = out else {
            panic!("expected a run");
        };
        assert_eq!(
            parse_dump_at(a.dump_at.as_deref().expect("set")),
            Ok((vec![2], "pal".into()))
        );
        assert_eq!(
            parse_dump_at("450,472,palace"),
            Ok((vec![450, 472], "palace".into()))
        );
        // Bad values are usage errors at run time (`FrameDumps::parse`),
        // not argument-parse time — mirror `--snapshot`'s late validation.
        for bad in ["palace", ",palace", "0,palace", "10,two,palace", "1,"] {
            assert!(
                matches!(
                    run_headless(&HeadlessArgs {
                        dump_at: Some(bad.into()),
                        ..Default::default()
                    }),
                    Err(HeadlessError::Usage(_))
                ),
                "must reject --dump-at '{bad}'"
            );
        }
        assert!(parse_dump_at("all frames").is_err());
        assert!(parse_dump_at("1,").is_err());
    }

    #[test]
    fn parses_widescreen_and_coop_flags() {
        let out = parse_headless_args(&argv(&[
            "z2-native",
            "--headless",
            "--widescreen",
            "16:9",
            "--dump-wide",
            "w.png",
            "--coop",
            "--p2-hold",
            "0x11",
            "--dump-coop",
            "coop.json",
        ]))
        .expect("parses");
        let ParseOutcome::Run(a) = out else {
            panic!("expected a run");
        };
        assert_eq!(a.widescreen.as_deref(), Some("16:9"));
        assert_eq!(a.wide_tiles(), 11, "16:9 is 11 tiles per side");
        assert_eq!(a.dump_wide.as_deref(), Some("w.png"));
        assert!(a.coop);
        assert_eq!(a.p2_hold, Some(0x11));
        assert_eq!(a.pad2_for(0x00), 0x11, "held byte wins");
        assert_eq!(a.dump_coop.as_deref(), Some("coop.json"));
        assert!(a.margin_sprites(), "margin sprites default on");
        assert!(a.display_settings().features(false).margin_sprites);
    }

    #[test]
    fn parses_margin_sprites() {
        let parse = |v: &str| {
            parse_headless_args(&argv(&["z2-native", "--headless", "--margin-sprites", v]))
        };
        let Ok(ParseOutcome::Run(a)) = parse("off") else {
            panic!("expected a run");
        };
        assert!(!a.margin_sprites());
        assert!(!a.display_settings().features(false).margin_sprites);
        let Ok(ParseOutcome::Run(a)) = parse("on") else {
            panic!("expected a run");
        };
        assert!(a.margin_sprites());
        assert!(
            !a.display_settings().features(false).margin_sprites,
            "no widescreen: nothing to draw into"
        );
        assert!(parse("maybe").is_err());
    }

    #[test]
    fn dump_wide_implies_sixteen_nine_and_mirror_copies_pad1() {
        let a = HeadlessArgs {
            dump_wide: Some("w.png".into()),
            ..Default::default()
        };
        assert_eq!(a.wide_tiles(), 11, "--dump-wide alone implies 16:9");
        let m = HeadlessArgs {
            coop: true,
            p2_mirror: true,
            ..Default::default()
        };
        assert_eq!(m.pad2_for(0x25), 0x25, "mirror copies pad 1");
        let off = HeadlessArgs::default();
        assert_eq!(off.wide_tiles(), 0);
        assert_eq!(off.pad2_for(0xFF), 0, "pad 2 idle by default");
    }

    #[test]
    fn rejects_bad_widescreen_and_pad2_values() {
        for bad in [
            vec!["z2-native", "--headless", "--widescreen", "21:9"],
            vec!["z2-native", "--headless", "--widescreen", "17"],
            vec!["z2-native", "--headless", "--p2-hold", "300"],
            vec!["z2-native", "--headless", "--p2-hold", "zz"],
        ] {
            assert!(
                parse_headless_args(&argv(&bad)).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    /// `--dump-wide` without a cartridge is a usage error, not a panic: the
    /// margins are decoded from CHR the synthetic emulator does not have.
    #[test]
    fn dump_wide_without_a_rom_is_a_usage_error() {
        let dir = scratch("widenorom");
        let out = dir.join("w.png");
        let saved = std::env::var("Z2_ROM").ok();
        std::env::remove_var("Z2_ROM");
        let r = run_headless(&HeadlessArgs {
            frames: 1,
            dump_wide: Some(out.to_string_lossy().into_owned()),
            ..Default::default()
        });
        assert!(matches!(r, Err(HeadlessError::Usage(_))), "got {r:?}");
        if let Some(v) = saved {
            std::env::set_var("Z2_ROM", v);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Co-op stepping runs with no ROM and no display, and `--dump-coop`
    /// writes parseable JSON (`null` when co-op is off).
    #[test]
    fn coop_steps_and_dumps_status_windowless() {
        let dir = scratch("coopdump");
        let out = dir.join("coop.json");
        let report = run_headless(&HeadlessArgs {
            frames: 3,
            coop: true,
            p2_mirror: true,
            dump_coop: Some(out.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("co-op smoke runs windowless");
        assert_eq!(report.frames_run, 3);
        assert_eq!(
            report.coop_path.as_deref(),
            Some(out.to_string_lossy().as_ref())
        );
        let text = std::fs::read_to_string(&out).expect("status written");
        // A cartridge-less emulator never enters side-view, so P2 is inactive;
        // the point is that the JSON is well formed and co-op stepped.
        let v: serde_json::Value = serde_json::from_str(&text).expect("status is JSON");
        assert!(v.is_object() || v.is_null(), "got {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unknown_flags_and_bad_frames() {
        assert!(parse_headless_args(&argv(&["z2-native", "--headless", "--bogus"])).is_err());
        assert!(parse_headless_args(&argv(&["z2-native", "--headless", "--frames"])).is_err());
        assert!(
            parse_headless_args(&argv(&["z2-native", "--headless", "--frames", "ten"])).is_err()
        );
    }

    /// Ungated: `--frames N` steps the standalone `Game` through
    /// the shared `app::step_frames` path — no oracle, no gate.
    #[test]
    fn frames_step_the_shared_game_path() {
        let report = run_headless(&HeadlessArgs {
            frames: 5,
            ..Default::default()
        })
        .expect("frames run ungated");
        assert_eq!(report.frames_run, 5);
        assert!(report.note.contains('5'));
        // Smoke (no movie, N=0) still steps nothing.
        let smoke = run_headless(&HeadlessArgs::default()).expect("smoke runs windowless");
        assert_eq!(smoke.frames_run, 0);
    }

    /// Ungated: `--movie` feeds the track (whole track at
    /// `--frames 0`, truncated/padded to N otherwise).
    #[test]
    fn movie_track_feeds_frames() {
        let dir = scratch("movie");
        let movie = fm2_fixture(&dir);
        let m = movie.to_string_lossy().into_owned();
        // Whole 2-frame track at N=0.
        let whole = run_headless(&HeadlessArgs {
            movie: Some(m.clone()),
            ..Default::default()
        })
        .expect("movie runs ungated");
        assert_eq!(whole.frames_run, 2);
        // Truncated to N.
        let trunc = run_headless(&HeadlessArgs {
            movie: Some(m.clone()),
            frames: 1,
            ..Default::default()
        })
        .expect("movie truncates");
        assert_eq!(trunc.frames_run, 1);
        // Padded with blank past the end of the track.
        let padded = run_headless(&HeadlessArgs {
            movie: Some(m),
            frames: 5,
            ..Default::default()
        })
        .expect("movie pads");
        assert_eq!(padded.frames_run, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ungated: `--movie` + `--dump` exports final-state facts
    /// that parse back as `GameFacts`.
    #[test]
    fn movie_run_writes_facts_dump() {
        let dir = scratch("moviedump");
        let movie = fm2_fixture(&dir);
        let dump = dir.join("facts.json");
        let report = run_headless(&HeadlessArgs {
            movie: Some(movie.to_string_lossy().into_owned()),
            dump_facts: Some(dump.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("movie+dumps run");
        assert_eq!(report.frames_run, 2);
        assert!(report.facts_path.is_some());
        let text = std::fs::read_to_string(&dump).expect("dump written");
        let facts = z2_core::facts::GameFacts::from_json(&text).expect("dump parses as GameFacts");
        assert_eq!(facts.enemies.len(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ungated: `--dump-frame` writes a PNG with the right
    /// magic and 256x240 IHDR (std-only encoder, no image dependency).
    #[test]
    fn dump_frame_writes_valid_png() {
        let dir = scratch("png");
        let shot = dir.join("out.png");
        let report = run_headless(&HeadlessArgs {
            frames: 2,
            dump_frame: Some(shot.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("dump-frame runs ungated");
        assert_eq!(report.frames_run, 2);
        assert!(report.frame_path.is_some());
        let bytes = std::fs::read(&shot).expect("png written");
        assert_eq!(
            &bytes[0..8],
            &[137, 80, 78, 71, 13, 10, 26, 10],
            "PNG magic"
        );
        assert_eq!(&bytes[12..16], b"IHDR", "first chunk is IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 256);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 240);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--dump-at` writes one PNG per listed 1-based frame while the run
    /// steps, and `--dump-frame` (final frame) still works independently.
    #[test]
    fn dump_at_writes_pngs_at_each_frame() {
        let dir = scratch("dumpat");
        let final_png = dir.join("final.png");
        let prefix = dir.join("pal").to_string_lossy().into_owned();
        let report = run_headless(&HeadlessArgs {
            frames: 3,
            dump_at: Some(format!("2,3,{prefix}")),
            dump_frame: Some(final_png.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("dump-at runs ungated");
        assert_eq!(report.frames_run, 3);
        assert_eq!(report.dump_at_paths.len(), 2, "{report:?}");
        for n in [2, 3usize] {
            let bytes =
                std::fs::read(dir.join(format!("pal{n}.png"))).expect("per-frame png written");
            assert_eq!(
                &bytes[0..8],
                &[137, 80, 78, 71, 13, 10, 26, 10],
                "PNG magic in {n}"
            );
        }
        let bytes = std::fs::read(&final_png).expect("final png written");
        assert_eq!(
            &bytes[0..8],
            &[137, 80, 78, 71, 13, 10, 26, 10],
            "PNG magic"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--snapshot` seeds `Game` RAM before stepping: byte `$0774`
    /// (`link.hp`) set in the image shows up in the dumped facts.
    #[test]
    fn snapshot_seeds_ram_before_stepping() {
        let dir = scratch("seed");
        let snap = dir.join("seed.bin");
        let dump = dir.join("facts.json");
        let mut img = vec![0u8; 2048];
        img[0x0774] = 0x2A;
        std::fs::write(&snap, &img).unwrap();
        let report = run_headless(&HeadlessArgs {
            snapshot: Some(snap.to_string_lossy().into_owned()),
            sram: None,
            dump_facts: Some(dump.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("seeded smoke runs");
        assert_eq!(report.frames_run, 0);
        let text = std::fs::read_to_string(&dump).expect("dump written");
        let facts = z2_core::facts::GameFacts::from_json(&text).expect("dump parses");
        assert_eq!(facts.link.hp, 0x2A, "snapshot RAM seeds the Game");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Missing movie file is I/O (exit 4); unparseable movie is usage
    /// (exit 2) — the exit-code contract survives the ungate.
    #[test]
    fn bad_movie_maps_to_io_or_usage() {
        let missing = HeadlessArgs {
            movie: Some("/nonexistent-dir-z2rs/missing.fm2".into()),
            ..Default::default()
        };
        assert!(matches!(run_headless(&missing), Err(HeadlessError::Io(_))));
        let dir = scratch("badmovie");
        let bad = dir.join("bad.xyz");
        std::fs::write(&bad, b"nope").unwrap();
        let unknown = HeadlessArgs {
            movie: Some(bad.to_string_lossy().into_owned()),
            ..Default::default()
        };
        assert!(matches!(
            run_headless(&unknown),
            Err(HeadlessError::Usage(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CI windowless run: `--headless --frames 0 --dump facts.json` needs no
    /// display, no ROM, no oracle — and the dump parses back as GameFacts.
    #[test]
    fn smoke_run_writes_facts_dump_windowless() {
        let dir = scratch("smoke");
        let dump = dir.join("facts.json");
        let args = HeadlessArgs {
            dump_facts: Some(dump.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let report = run_headless(&args).expect("smoke runs windowless");
        assert_eq!(report.frames_run, 0);
        assert_eq!(
            report.facts_path.as_deref(),
            Some(dump.to_string_lossy().as_ref())
        );
        let text = std::fs::read_to_string(&dump).expect("dump written");
        let facts = z2_core::facts::GameFacts::from_json(&text).expect("dump parses as GameFacts");
        assert_eq!(facts.link.hp, 0);
        assert_eq!(facts.enemies.len(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smoke_run_rejects_malformed_snapshot() {
        let dir = scratch("bad");
        let snap = dir.join("short.bin");
        std::fs::write(&snap, [0u8; 100]).unwrap();
        let args = HeadlessArgs {
            snapshot: Some(snap.to_string_lossy().into_owned()),
            sram: None,
            ..Default::default()
        };
        assert!(matches!(run_headless(&args), Err(HeadlessError::Usage(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(run_argv(&argv(&["z2-native", "--headless", "--help"])), 0);
        assert_eq!(run_argv(&argv(&["z2-native", "--headless", "--bogus"])), 2);
        // Ungated: real frame runs exit 0 (was: 3 gated on the oracle).
        assert_eq!(
            run_argv(&argv(&["z2-native", "--headless", "--frames", "60"])),
            0
        );
        assert_eq!(
            run_argv(&argv(&[
                "z2-native",
                "--headless",
                "--movie",
                "/nonexistent-dir-z2rs/missing.fm2"
            ])),
            4
        );
    }
}
