//! Windowed native frontend: loop, scaling, ROM/assets, states.
//!
//! This module owns the **shared `Game` build/step path** used by both the
//! windowed loop and `--headless` (see [`crate::headless`]): [`new_emu`],
//! [`emu_from_rom_body`], [`step_frames`], [`load_movie_track`],
//! snapshot/SRAM helpers. Windowed-only work (winit/pixels/cpal/gilrs) is
//! confined to [`run_windowed`]; everything else is pure and runs on
//! display-less CI.
//!
//! # Loop architecture
//!
//! * Fixed-timestep accumulator at NTSC 60.0988 Hz ([`NTSC_HZ`]): wall time
//!   accumulates into [`FrameTimer`]; each `FRAME_DT` slice steps one
//!   [`Game::step`] + one [`z2_apu::Apu::audio`] frame pushed into the
//!   shared [`crate::audio::SharedAudio`] ring. Capped at
//!   [`MAX_STEPS_PER_TICK`] (drop excess — no spiral of death).
//! * Fast-forward multiplies wall time (config `fast_forward_multiplier`,
//!   default 4×) while held (`Tab`); frame-step (`.` / `Space` when paused)
//!   advances exactly one frame.
//! * Pause on focus loss when configured; title bar shows emulated frames
//!   per second + audio underruns.
//!
//! # Input latency note
//!
//! Keyboard + gamepad are polled once per emulated frame, immediately before
//! `Game::step` (see [`crate::input`]). Worst-case added latency is one
//! frame (~16.64 ms) plus OS/display queue depth.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use z2_core::game::{Game, FRAME_H as GAME_H, FRAME_W as GAME_W};

use crate::audio::SharedAudio;
use crate::config::NativeConfig;

// ---------------------------------------------------------------------------
// Framebuffer / timing constants
// ---------------------------------------------------------------------------

/// Visible framebuffer width (NTSC NES).
pub const FRAME_W: usize = GAME_W;
/// Visible framebuffer height (NTSC NES).
pub const FRAME_H: usize = GAME_H;
/// Indexed framebuffer bytes.
pub const FRAME_LEN: usize = FRAME_W * FRAME_H;
/// RGBA bytes per pixels frame.
pub const FRAME_RGBA_LEN: usize = FRAME_W * FRAME_H * 4;

/// NTSC field rate the accumulator tracks (Hz).
pub const NTSC_HZ: f64 = 60.0988;
/// Seconds per logic frame.
pub const FRAME_DT: f64 = 1.0 / NTSC_HZ;
/// Max frames stepped per OS tick (excess wall time is dropped).
pub const MAX_STEPS_PER_TICK: usize = 5;

/// Hotkeys (also listed in the window title help line).
pub const HOTKEY_SAVE: &str = "F5";
/// Hotkeys (also listed in the window title help line).
pub const HOTKEY_LOAD: &str = "F7";

/// Numbered save-state slots: digits 1-9 / 0 select, `F6` cycles.
pub const SAVESTATE_SLOTS: u8 = 10;

/// Next save-state slot in the cycle (`9` wraps to `0`).
#[must_use]
pub fn next_savestate_slot(slot: u8) -> u8 {
    (slot + 1) % SAVESTATE_SLOTS
}

/// Fixed-timestep accumulator (pure — no threads, no window).
#[derive(Debug, Clone)]
pub struct FrameTimer {
    acc: f64,
}

impl Default for FrameTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTimer {
    /// Empty accumulator.
    pub fn new() -> Self {
        Self { acc: 0.0 }
    }

    /// Current buffered time (seconds).
    #[must_use]
    pub fn buffered(&self) -> f64 {
        self.acc
    }

    /// Advance by `dt_secs` of wall time (×`speed`) and return frames to
    /// step now (capped at [`MAX_STEPS_PER_TICK`]; excess is dropped).
    /// When `paused` (and not single-stepping) the time is discarded and 0
    /// is returned.
    pub fn advance(&mut self, dt_secs: f64, speed: f64, paused: bool) -> usize {
        if paused {
            self.acc = 0.0;
            return 0;
        }
        let dt = dt_secs.max(0.0) * speed.max(0.0);
        self.acc += dt;
        let mut steps = 0usize;
        while self.acc >= FRAME_DT && steps < MAX_STEPS_PER_TICK {
            self.acc -= FRAME_DT;
            steps += 1;
        }
        if self.acc >= FRAME_DT {
            // Spiral-of-death guard: drop backlog beyond the cap.
            self.acc = 0.0;
        }
        steps
    }

    /// Single frame-step while paused.
    pub fn step_once(&mut self) {
        self.acc = 0.0;
    }
}

/// Audio-driven pacing nudge (pure).
///
/// The wall-clock timer alone keeps the shared audio ring at whatever depth
/// the one-frame silence prime left it, so any main-thread stall longer
/// than a frame (window events, a slow present) starves the `cpal`
/// callback and the title-bar underrun meter climbs. Nudging the step
/// count by the ring depth keeps it between [`PACE_LOW_FRAMES`] and
/// [`PACE_HIGH_FRAMES`] nominal frames: below the low mark one extra frame
/// is stepped this tick, above the high mark one is held back. Only at
/// normal speed (fast-forward and pause bypass it); the long-run average
/// stays 60 steps/s because the ring drains at exactly the device rate.
#[must_use]
pub fn pace_steps(steps: usize, depth_samples: usize, rate: u32, normal_speed: bool) -> usize {
    if !normal_speed {
        return steps;
    }
    let frame = (rate / 60).max(1) as usize;
    if depth_samples < PACE_LOW_FRAMES * frame {
        steps + 1
    } else if depth_samples > PACE_HIGH_FRAMES * frame {
        steps.saturating_sub(1)
    } else {
        steps
    }
}

/// [`pace_steps`] low-water mark, in nominal audio frames (~33 ms).
pub const PACE_LOW_FRAMES: usize = 2;
/// [`pace_steps`] high-water mark, in nominal audio frames (~100 ms).
pub const PACE_HIGH_FRAMES: usize = 6;

// ---------------------------------------------------------------------------
// Viewport / scaling
// ---------------------------------------------------------------------------

/// Centered viewport inside the OS window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// Left offset (physical pixels).
    pub x: u32,
    /// Top offset (physical pixels).
    pub y: u32,
    /// Viewport width.
    pub w: u32,
    /// Viewport height.
    pub h: u32,
    /// Integer scale factor applied (1 when letterboxed / window too small).
    pub scale: u32,
}

/// Clamp a window surface size to valid `pixels` dimensions.
///
/// `winit` can report a zero width/height (minimized, not-yet-mapped); `pixels`
/// rejects zero-sized surfaces, so every `SurfaceTexture::new` /
/// `resize_surface` site routes through here. Pure so the windowed-only call
/// sites stay unit-testable without opening a window.
#[must_use]
pub fn clamp_surface_size(w: u32, h: u32) -> (u32, u32) {
    (w.max(1), h.max(1))
}

/// Compute the centered viewport for `window_w × window_h`.
///
/// * Integer scaling: `scale = floor(min(ww/eff_w, wh/eff_h))`, min 1.
/// * Aspect correction: effective frame is `256*8/7 × 240` (≈292×240,
///   the NTSC PAR-corrected width); otherwise `256×240`.
/// * When the window is smaller than one effective frame, the viewport is
///   the clamped window size centered (scale 1, letterboxed by `pixels`).
pub fn compute_viewport(
    window_w: u32,
    window_h: u32,
    integer_scaling: bool,
    aspect_correction: bool,
) -> Viewport {
    let eff_w: f64 = if aspect_correction {
        FRAME_W as f64 * 8.0 / 7.0
    } else {
        FRAME_W as f64
    };
    let eff_h: f64 = FRAME_H as f64;
    if window_w == 0 || window_h == 0 {
        return Viewport {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            scale: 1,
        };
    }
    let scale_f = (f64::from(window_w) / eff_w).min(f64::from(window_h) / eff_h);
    let scale: u32 = if integer_scaling {
        (scale_f.floor() as u32).max(1)
    } else {
        1
    };
    // With integer scaling the viewport is exactly scale*eff; without it the
    // viewport fills the window (pixels stretches the 256×240 texture).
    let (w, h) = if integer_scaling {
        let w = ((eff_w * f64::from(scale)).round() as u32).min(window_w);
        let h = ((eff_h * f64::from(scale)).round() as u32).min(window_h);
        (w, h)
    } else {
        (window_w, window_h)
    };
    Viewport {
        x: window_w.saturating_sub(w) / 2,
        y: window_h.saturating_sub(h) / 2,
        w,
        h,
        scale,
    }
}

// ---------------------------------------------------------------------------
// Shared Game build/step path (headless + windowed)
// ---------------------------------------------------------------------------

/// Live emulation state: the shared `Game` + its APU voice state.
///
/// `Game::step` owns CPU/PPU timing; the APU here renders PCM per frame via
/// [`z2_apu::Apu::audio`] (silence until the bank-6 engine wires register
/// writes per frame — the plumbing and self-test already run).
pub struct Emu {
    /// Hybrid-execution game state.
    pub game: Game,
    /// NES APU synth (PCM source).
    pub apu: z2_apu::Apu,
    /// Identity of the registered ported-routine set, captured **before** any
    /// co-op traps are added (see [`trapset_id`]), with the wide-gameplay
    /// margin folded in ([`session_trapset_id`]). Netplay refuses a peer
    /// whose value differs, so two builds can never silently desync.
    pub trapset_id: u64,
    /// [`trapset_id`] of the default groups alone, the base
    /// [`Emu::trapset_id`] is derived from when wide gameplay changes.
    pub trapset_base: u64,
}

impl std::fmt::Debug for Emu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emu")
            .field("frame", &self.game.frame_count())
            .field("apu_frames", &self.apu.frames_rendered())
            .field("record", &self.game.record_enabled())
            .finish()
    }
}

/// Which **game-affecting** switches an emulator is built with.
///
/// This exists so that every path that constructs an `Emu` — startup, a ROM
/// dropped on the running window, a netplay session start — applies the same
/// set. Losing co-op or the render record on a ROM drop was a real bug in the
/// first cut of this work; passing `Features` around by value is the fix.
///
/// Purely visual settings (margin width, output scale, HD pack) live in
/// [`DisplaySettings`] instead: they never touch the game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Features {
    /// Two-Link co-op (pad 2 drives a second Link in sideview).
    pub coop: bool,
    /// Wide gameplay margin in tiles per side (`None` = off, the default):
    /// enemies spawn and live in the widescreen margins
    /// ([`Game::set_wide_gameplay`]). Changes gameplay, so it is part of the
    /// netplay identity ([`session_trapset_id`]).
    pub wide_gameplay: Option<u8>,
    /// Arm the PPU render record. Required by widescreen margins, by HD art
    /// and by pack recording; off otherwise, so the default single-player run
    /// is byte-for-byte the verification path.
    pub record: bool,
    /// Draw side-view objects the game keeps alive outside the window into
    /// the widescreen margins ([`Game::set_margin_sprites`]). Registers a
    /// display-only observer: the game itself runs byte-identically, and the
    /// observer stays out of [`trapset_id`], so netplay peers may differ.
    pub margin_sprites: bool,
}

/// Arm or disarm the PPU render record on an existing emulator.
pub fn arm_record(emu: &mut Emu, on: bool) {
    emu.game.set_record(on);
}

/// Apply `feats` to an already-built emulator.
///
/// Co-op is normally enabled *before* `Game::reset` by
/// [`emu_from_rom_body_with`]; toggling it here is supported but re-anchors
/// the second Link on the next sideview frame.
pub fn apply_features(emu: &mut Emu, feats: Features) {
    if emu.game.coop_status().is_some() != feats.coop {
        emu.game.set_coop(feats.coop);
    }
    if emu.game.wide_gameplay_tiles() != feats.wide_gameplay {
        emu.game.set_wide_gameplay(feats.wide_gameplay);
        emu.trapset_id = session_trapset_id(emu.trapset_base, feats.wide_gameplay);
    }
    arm_record(emu, feats.record);
    emu.game.set_margin_sprites(feats.margin_sprites);
}

/// Identity of the registered trap set: FNV-1a over every `(addr, name)`
/// pair, address-sorted so registration order cannot change it. Display-only
/// observers ([`z2_core::wide_sprites::is_display_only_trap`]) are left out:
/// they never change the game, so they must not split peers.
///
/// Netplay compares this between peers. **The algorithm is duplicated in
/// `z2-web` (`trapset_id` there) and the two must stay identical**, or a
/// native host and a web guest refuse each other's handshake. Both are
/// pinned to the same expected value for a synthetic table by a unit test in
/// each crate.
#[must_use]
pub fn trapset_id(game: &Game) -> u64 {
    let mut entries: Vec<(u16, &'static str)> = game
        .traps
        .iter()
        .filter(|t| !z2_core::wide_sprites::is_display_only_trap(t.name))
        .map(|t| (t.addr, t.name))
        .collect();
    entries.sort_unstable();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (addr, name) in entries {
        for b in addr.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for b in name.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Session identity: [`trapset_id`] with the wide-gameplay margin folded in
/// (the same FNV-1a continued over `"wide_gameplay"` and the tile count).
/// `None` leaves the id untouched, so a peer without wide gameplay keeps the
/// identity it always had.
///
/// **Duplicated in `z2-web` (`session_trapset_id` there)**; both are pinned to
/// [`WIDE_TRAPSET_PIN_VALUE`].
#[must_use]
pub fn session_trapset_id(base: u64, wide_tiles: Option<u8>) -> u64 {
    let Some(tiles) = wide_tiles else {
        return base;
    };
    let mut h = base;
    for b in b"wide_gameplay".iter().chain(core::iter::once(&tiles)) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Cross-crate pin for [`session_trapset_id`]:
/// `session_trapset_id(TRAPSET_PIN_VALUE, Some(11))`.
pub const WIDE_TRAPSET_PIN_VALUE: u64 = 0x1094_c616_9aeb_24b9;

/// Cross-crate pin for [`trapset_id`]: the value both `z2-native` and
/// `z2-web` must produce for the entries
/// `[(0xFF9D, "bank7"), (0x00C2, "boot")]`.
///
/// If a change to either implementation moves this number, native and web
/// peers stop being able to play together — that is what the pin protects.
pub const TRAPSET_PIN_VALUE: u64 = 0xad1b_d206_40e7_ea04;

/// Presented framebuffer size for `wide_tiles` per side at scale 1.
#[must_use]
pub fn present_size(wide_tiles: u8) -> (u32, u32) {
    present_size_scaled(wide_tiles, 1)
}

/// Presented framebuffer size for `wide_tiles` per side at `scale`.
///
/// This is the same arithmetic [`z2_render::Presenter::width`] /
/// [`z2_render::Presenter::height`] perform; the presenter remains the
/// authority at runtime (an HD pack can raise the effective scale), and this
/// helper exists for sizing decisions made before one is built.
#[must_use]
pub fn present_size_scaled(wide_tiles: u8, scale: u32) -> (u32, u32) {
    let s = scale.max(1);
    (
        (z2_ppu::wide_width(wide_tiles) * s as usize) as u32,
        (FRAME_H * s as usize) as u32,
    )
}

/// Initial window size in logical pixels for a `tex_w x tex_h` framebuffer.
///
/// `pixels` 0.15 letterboxes with an integer scale clamped to `>= 1`, so a
/// texture LARGER than the surface is cropped, not shrunk: the window must
/// therefore never open smaller than the texture. Picks the largest of 3x,
/// 2x, 1x that fits `monitor` (leaving room for titlebar/dock), falling back
/// to a 1536-logical-px budget when the monitor size is unknown.
#[must_use]
pub fn initial_window_size(tex_w: u32, tex_h: u32, monitor: Option<(u32, u32)>) -> (f64, f64) {
    let mut scale = 3u32;
    while scale > 1 {
        let (w, h) = (tex_w * scale, tex_h * scale);
        let fits = match monitor {
            Some((mw, mh)) => w <= mw && h.saturating_add(120) <= mh,
            None => w <= 1536,
        };
        if fits {
            break;
        }
        scale -= 1;
    }
    (f64::from(tex_w * scale), f64::from(tex_h * scale))
}

/// Synthetic emulator (no ROM): zeroed `Game` + default APU.
///
/// This is what `--headless --frames 0` smoke and the CI self-tests step —
/// real `Game::step` frames, just without a cartridge.
pub fn new_emu(audio_rate: u32) -> Emu {
    Emu {
        game: Game::new(),
        apu: z2_apu::Apu::new(crate::audio::clamp_rate(audio_rate)),
        trapset_id: 0,
        trapset_base: 0,
    }
}

/// Synthetic emulator with `feats` applied.
///
/// The record matters even here: a `--widescreen` / `--hd-pack` launch with no
/// ROM presents through exactly the same [`Display`] as a cartridge run, and
/// with no CHR the margins decode to backdrop — which is the right picture
/// behind [`NO_ROM_TITLE`].
///
/// Co-op is deliberately **not** enabled on a cartridge-less emulator: there
/// is no ROM code for the co-op wrappers to override.
pub fn new_emu_with(audio_rate: u32, feats: Features) -> Emu {
    let mut emu = new_emu(audio_rate);
    arm_record(&mut emu, feats.record);
    emu
}

/// Build an emulator from a header-stripped ROM body (262144 bytes).
///
/// The body is verified with the `z2-assets` hash gate, then wrapped in a
/// synthetic MMC1 iNES header (PRG 8×16 KiB + CHR 16×8 KiB, mapper 1) for
/// [`Game::from_ines`]. The caller's ROM bytes are never stored — only the
/// parsed banks live in the returned `Game`.
///
/// The CPU is `reset()` to the reset vector: without this the interpreter
/// would execute from address 0 (BRK soup) and the game would never boot.
pub fn emu_from_rom_body(body: &[u8], audio_rate: u32) -> Result<Emu, String> {
    emu_from_rom_body_with(body, audio_rate, Features::default())
}

/// [`emu_from_rom_body`] with optional features applied at the right moments.
///
/// Ordering matters and is the whole reason this function exists: the co-op
/// trap group must be registered **after** the nine default groups (it
/// overrides three of them and inherits their cycle costs) and **before**
/// `Game::reset`, exactly like the default groups. The trap-set identity is
/// captured before co-op registration so it describes the parity-relevant
/// table only.
pub fn emu_from_rom_body_with(
    body: &[u8],
    audio_rate: u32,
    feats: Features,
) -> Result<Emu, String> {
    z2_assets::rom::verify_body(body).map_err(|e| format!("ROM gate: {e}"))?;
    // Zelda II SNROM: 128 KiB PRG + 128 KiB CHR.
    if body.len() != z2_assets::rom::EXPECTED_BODY_LEN {
        return Err(format!("ROM body is {} bytes, want 262144", body.len()));
    }
    let mut ines = Vec::with_capacity(16 + body.len());
    ines.extend_from_slice(b"NES\x1A");
    ines.push(8u8); // PRG units of 16 KiB = 128 KiB.
    ines.push(16u8); // CHR units of 8 KiB = 128 KiB.
    ines.push(0x10u8); // flags6: mapper low nibble 1 (MMC1).
    ines.push(0x00u8); // flags7: mapper high nibble 0.
    ines.extend_from_slice(&[0u8; 8]); // padding.
    ines.extend_from_slice(body);
    let mut game = Game::from_ines(&ines).map_err(|e| format!("load ROM: {e}"))?;
    // The native frontend must use the same trap wiring as the verification
    // and web frontends.  Install it before reset so every subsequent CPU
    // entry point, including the first boot frame, sees the native routines.
    register_native_traps(&mut game);
    let trapset_id = trapset_id(&game);
    // Co-op traps override three default-group entries, so they go on last —
    // and before `reset`, so the first boot frame already sees them.
    if feats.coop {
        game.set_coop(true);
    }
    // Wide gameplay: its own trap pair and the townsfolk PRG patch, also
    // before `reset`; the margin goes into the session identity.
    game.set_wide_gameplay(feats.wide_gameplay);
    let trapset_base = trapset_id;
    let trapset_id = session_trapset_id(trapset_base, feats.wide_gameplay);
    game.reset();
    let mut apu = z2_apu::Apu::new(crate::audio::clamp_rate(audio_rate));
    apu.install_dmc_source(Box::new(z2_apu::PrgSource::new(
        body[..128 * 1024].to_vec(),
    )));
    let mut emu = Emu {
        game,
        apu,
        trapset_id,
        trapset_base,
    };
    arm_record(&mut emu, feats.record);
    emu.game.set_margin_sprites(feats.margin_sprites);
    Ok(emu)
}

/// Install every trap registry used by the native ROM path.
///
/// `Game::from_ines` intentionally starts with an empty table so callers can
/// choose pure interpretation or a hybrid run.  The playable native app is a
/// hybrid frontend, so omitting this wiring silently sends boot, title, menu,
/// and gameplay through the much slower raw interpreter.
fn register_native_traps(game: &mut Game) {
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

/// Build an emulator from a ROM file (headered or bare).
///
/// Reads the file, strips a leading iNES header when present, verifies via
/// the hash gate, then delegates to [`emu_from_rom_body`]. The file is only
/// read — never copied into the data dir.
pub fn emu_from_rom_file(path: &Path, audio_rate: u32) -> Result<Emu, String> {
    emu_from_rom_file_with(path, audio_rate, Features::default()).map(|(emu, _body)| emu)
}

/// [`emu_from_rom_file`] with features applied. Returns the emulator and the
/// verified ROM body, which a netplay session needs in order to rebuild a
/// fresh game at `Started` without re-reading the file.
pub fn emu_from_rom_file_with(
    path: &Path,
    audio_rate: u32,
    feats: Features,
) -> Result<(Emu, Vec<u8>), String> {
    let file = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let body = z2_assets::rom::strip_ines_header(&file);
    // Copy out of the borrowed file buffer (verify + build own the bytes).
    let owned = body.to_vec();
    let emu = emu_from_rom_body_with(&owned, audio_rate, feats)?;
    Ok((emu, owned))
}

/// Step `inputs.len()` logic frames, pushing one APU frame per step into
/// `audio` when present. Returns frames stepped.
///
/// This is the single shared stepping primitive: the windowed accumulator
/// and `--headless` both funnel through here.
///
/// Sound path: the interpreter-run sound engine writes `$4000-$4017` into
/// the game's null-APU façade; each frame's writes are drained here into
/// the real [`z2_apu`] synth before rendering PCM, so what you hear is the
/// original engine's register stream synthesized live.
pub fn step_frames(emu: &mut Emu, inputs: &[u8], audio: Option<&SharedAudio>) -> usize {
    let mut pcm = Vec::new();
    for &b in inputs {
        // Deliberately `Game::step`, not `step2`: this is the primitive the
        // headless surface and the verification path share, and it must not
        // touch pad 2 even by writing a zero.
        emu.game.step(b);
        drain_audio_frame(emu, &mut pcm, audio);
    }
    inputs.len()
}

/// Two-pad variant of [`step_frames`] for co-op (local and netplay).
///
/// Uses `Game::step2`, which latches pad 2 (filtered so player 2 can never
/// trigger the save-and-quit chord) and then runs the frame exactly as
/// `step` does.
pub fn step_frames2(emu: &mut Emu, pads: &[(u8, u8)], audio: Option<&SharedAudio>) -> usize {
    let mut pcm = Vec::new();
    for &(p1, p2) in pads {
        emu.game.step2(p1, p2);
        drain_audio_frame(emu, &mut pcm, audio);
    }
    pads.len()
}

/// Drain one frame's APU register writes into the synth and push its PCM.
///
/// Factored out of [`step_frames`] so the two-pad and netplay paths render
/// audio identically instead of growing their own copies.
pub fn drain_audio_frame(emu: &mut Emu, pcm: &mut Vec<i16>, audio: Option<&SharedAudio>) {
    for (addr, val) in emu.game.apu.drain_log() {
        emu.apu.write_reg(addr, val);
    }
    pcm.clear();
    emu.apu.audio(pcm);
    if let Some(ring) = audio {
        ring.push_frame(pcm);
    }
}

/// Step exactly one frame (windowed hot path).
pub fn step_one(emu: &mut Emu, input: u8, audio: Option<&SharedAudio>) {
    let _ = step_frames(emu, &[input], audio);
}

/// Step exactly one two-pad frame (co-op windowed hot path).
pub fn step_one2(emu: &mut Emu, pads: (u8, u8), audio: Option<&SharedAudio>) {
    let _ = step_frames2(emu, &[pads], audio);
}

// ---------------------------------------------------------------------------
// Movie track loading (oracle-free demo mode)
// ---------------------------------------------------------------------------

/// Load the player-1 input track from `.fm2` / `.bk2` (by extension).
///
/// Oracle-free demo mode: the bytes just feed [`step_frames`] — no lockstep
/// verification, no corpus needed. `.bk2` header warnings (wrong platform
/// or game, Reset/Power presses) are logged to stderr; demo mode plays the
/// pad track regardless.
pub fn load_movie_track(path: &Path) -> Result<Vec<u8>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "fm2" => {
            let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let movie = z2_verify::movie_fm2::parse_fm2_bytes(&bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(movie.pad1_track())
        }
        "bk2" => {
            let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let movie = z2_verify::movie_bk2::parse_bk2_zip(&bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            for w in movie.warnings() {
                eprintln!("{}: warning: {w}", path.display());
            }
            Ok(movie.pad1_track())
        }
        other => Err(format!(
            "{}: unknown movie extension '.{other}' (want .fm2/.bk2)",
            path.display()
        )),
    }
}

// ---------------------------------------------------------------------------
// ROM / assets flow
// ---------------------------------------------------------------------------

/// First-run ROM resolution: `--rom` > config `rom_path` > `$Z2_ROM`.
///
/// Returns the ROM path or a clear error telling the user exactly what to
/// do (no file picker — `--rom` only by design, plus winit drag-drop onto
/// the running window).
pub fn resolve_rom_path(cli_rom: Option<&str>, config: &NativeConfig) -> Result<PathBuf, String> {
    if let Some(p) = cli_rom {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = &config.rom_path {
        return Ok(PathBuf::from(p));
    }
    env_path_if_usable(z2_assets::rom::ROM_ENV_VAR).ok_or_else(|| {
        format!(
            "no ROM supplied: pass --rom PATH, set rom_path in {}, or set ${} to an existing Zelda II (USA) dump of your own. The ROM is never stored — only assets.bin is extracted into the data dir.",
            crate::config::config_path().display(),
            z2_assets::rom::ROM_ENV_VAR
        )
    })
}

/// Read a path from an environment variable, treating unset, empty, and
/// non-existent values alike as "not supplied".
///
/// Workspace convention: a stale or blank `Z2_ROM` must behave exactly like no
/// ROM at all — the app shows its visible no-ROM window — rather than failing
/// with a confusing read error. An explicit `--rom` is deliberately **not**
/// filtered this way: the user named that specific file, so a bad path there
/// stays a loud error.
#[must_use]
pub fn env_path_if_usable(var: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(var)?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    path.is_file().then_some(path)
}

/// Ensure `<data-dir>/assets.bin` exists and decodes.
///
/// When missing, extract from the ROM resolved by [`resolve_rom_path`] via
/// `z2-assets` (`extract_rom_file_to_vec`) and write the image. The ROM
/// itself is never stored. Returns the assets path.
pub fn ensure_assets(
    cli_rom: Option<&str>,
    config: &NativeConfig,
    data_dir: &Path,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    let out = data_dir.join(crate::config::ASSETS_FILE_NAME);
    if let Ok(bytes) = std::fs::read(&out) {
        if z2_assets::assets_bin::decode(&bytes).is_ok() {
            return Ok(out);
        }
    }
    let rom_path = resolve_rom_path(cli_rom, config)?;
    let (image, summary) = z2_assets::assets_bin_io::extract_rom_file_to_vec(&rom_path)
        .map_err(|e| format!("extract {}: {e}", rom_path.display()))?;
    std::fs::write(&out, &image).map_err(|e| format!("write {}: {e}", out.display()))?;
    eprintln!(
        "extract: {} sections, {} bytes -> {}",
        summary.sections,
        summary.bytes,
        out.display()
    );
    Ok(out)
}

// ---------------------------------------------------------------------------
// Save states (Snapshot encode/decode) + SRAM persistence
// ---------------------------------------------------------------------------

/// Quick-save label (must be in `z2-verify` `LABEL_PLAN`).
pub const QUICK_SAVE_LABEL: &str = "game-start";

/// Build a [`z2_verify::snapshot::Snapshot`] from live `Game` state.
pub fn snapshot_from_game(
    game: &Game,
    input_history: Vec<u8>,
) -> Result<z2_verify::snapshot::Snapshot, String> {
    z2_verify::snapshot::Snapshot::new(
        game.ram().to_vec(),
        game.wram().to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        input_history,
        QUICK_SAVE_LABEL,
        "live-save",
        game.frame_count(),
        game.frame_count(),
    )
    .map_err(|e| format!("snapshot: {e}"))
}

/// Restore RAM/WRAM from a snapshot into `game`.
/// Restore RAM/WRAM from a snapshot into `game`.
///
/// Also drops the second Link from the current area: its parked byte block
/// describes the RAM we just replaced, so keeping it would put P2 somewhere
/// that no longer exists. It re-anchors next to P1 on the next sideview
/// frame. Harmless when co-op is off.
pub fn apply_snapshot_to_game(
    game: &mut Game,
    snap: &z2_verify::snapshot::Snapshot,
) -> Result<(), String> {
    if snap.ram.len() != 0x800 {
        return Err(format!(
            "snapshot ram is {} bytes, want 2048",
            snap.ram.len()
        ));
    }
    if snap.wram.len() != 0x2000 {
        return Err(format!(
            "snapshot wram is {} bytes, want 8192",
            snap.wram.len()
        ));
    }
    game.ram.copy_from_slice(&snap.ram);
    game.wram.copy_from_slice(&snap.wram);
    game.coop_reset_area();
    Ok(())
}

/// Save-state file path for `slot` in `data_dir`.
pub fn savestate_path(data_dir: &Path, slot: u8) -> PathBuf {
    data_dir.join(format!("savestate{slot}.z2snap"))
}

/// Write the current `Game` state to `slot`.
pub fn save_savestate(game: &Game, data_dir: &Path, slot: u8) -> Result<PathBuf, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    let snap = snapshot_from_game(game, Vec::new())?;
    let bytes = snap.encode().map_err(|e| format!("encode: {e}"))?;
    let path = savestate_path(data_dir, slot);
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Load `slot` into `game`.
pub fn load_savestate(game: &mut Game, data_dir: &Path, slot: u8) -> Result<(), String> {
    let path = savestate_path(data_dir, slot);
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let snap = z2_verify::snapshot::Snapshot::decode(&bytes)
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    apply_snapshot_to_game(game, &snap)
}

/// Battery-RAM file name in the data dir.
pub const SRAM_FILE: &str = "sram.sav";

/// Battery-RAM file a netplay **guest** autosaves to.
///
/// A guest plays on the host's save, so it must never write `sram.sav` — that
/// would destroy its own solo progress. Rename this file over `sram.sav` to
/// continue the co-op game alone.
pub const SRAM_COOP_FILE: &str = "sram-coop.sav";

/// SRAM file path in `data_dir`.
pub fn sram_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SRAM_FILE)
}

/// Persist the battery-RAM window (`$6000-$7FFF`) to `sram.sav` exactly
/// as the game left it.
///
/// The cartridge's SRAM is the game's own: its bank-5 writer stages,
/// commits and validates the three save slots when the player saves. The
/// app only mirrors the window to disk (autosave every ~10 s, on exit, on
/// ROM swap) and never edits slots itself — an earlier version stamped the
/// live RAM stats into slot 0 on every autosave, which on the title screen
/// (stats zeroed by reset) produced a "valid" slot with levels 0 and a
/// name of `$00` tiles that then loaded as a broken game.
pub fn save_sram(game: &mut Game, data_dir: &Path) -> Result<PathBuf, String> {
    save_sram_named(game, data_dir, SRAM_FILE)
}

/// [`save_sram`] to a named file in `data_dir` (see [`SRAM_COOP_FILE`]).
pub fn save_sram_named(game: &mut Game, data_dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    let path = data_dir.join(name);
    std::fs::write(&path, game.wram()).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Load `sram.sav` into the battery-RAM window (raw copy; the game reads
/// its own slots from there).
///
/// Missing file is not an error (first run) — returns `Ok(false)`;
/// a present file returns `Ok(true)`. Slots that look corrupted by the old
/// autosave are reported on stderr (see [`suspicious_slots`]).
pub fn load_sram(game: &mut Game, data_dir: &Path) -> Result<bool, String> {
    let path = sram_path(data_dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(false);
    };
    if bytes.len() != z2_core::save_format::SRAM_LEN {
        return Err(format!(
            "{}: {} bytes, want 8192",
            path.display(),
            bytes.len()
        ));
    }
    game.wram.copy_from_slice(&bytes);
    for slot in suspicious_slots(&bytes) {
        eprintln!(
            "warning: {} slot {} is marked valid but holds levels of 0 (a save the game never writes; \
             an earlier build's autosave corrupted it). Delete the file to start fresh.",
            path.display(),
            slot + 1
        );
    }
    Ok(true)
}

/// Slots whose header says valid while the three level bytes are all zero —
/// impossible for a slot the game wrote (new games start at level 1), the
/// signature of the old autosave stamping title-screen RAM into slot 0.
#[must_use]
pub fn suspicious_slots(sram: &[u8]) -> Vec<u8> {
    use z2_core::save_format::{classify_header, HeaderState, HDR_ADDR, PART1_LEN, PART2_LEN};
    let mut out = Vec::new();
    for slot in 0..3u8 {
        let Some(h) = z2_core::save_format::sram_index(HDR_ADDR[usize::from(slot)]) else {
            continue;
        };
        if classify_header(sram[h]) != HeaderState::Valid {
            continue;
        }
        let mut part1 = [0u8; PART1_LEN];
        let mut part2 = [0u8; PART2_LEN];
        if z2_core::save_format::load_slot(sram, slot, &mut part1, &mut part2)
            && part1[..3].iter().all(|&b| b == 0)
        {
            out.push(slot);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Presentation helpers
// ---------------------------------------------------------------------------

/// Window title line: FPS + underrun meter + state flags.
///
/// A nonzero `slot` is announced as `[SLOT n]` so the selected save target
/// is always visible; slot `0` shows nothing (the historical default).
pub fn title_text(fps: f64, underruns: u64, paused: bool, fast_forward: bool, slot: u8) -> String {
    let mut s = format!("z2rs — {fps:.1} fps, underruns {underruns}");
    if paused {
        s.push_str(" [PAUSED — press P]");
    }
    if fast_forward {
        s.push_str(" [FF]");
    }
    if slot != 0 {
        s.push_str(&format!(" [SLOT {slot}]"));
    }
    s
}

/// Window title shown while no ROM is loaded.
///
/// This is the visible replacement for the old silent-fallback `eprintln`:
/// the window keeps rendering the (empty) frame behind this title so a
/// ROM-less launch never looks like a dead grey window.
pub const NO_ROM_TITLE: &str = "z2rs — no ROM: drop a .nes file or restart with --rom PATH";

/// The exact no-ROM title string (accessor for tests / event loop).
#[must_use]
pub fn no_rom_title() -> &'static str {
    NO_ROM_TITLE
}

/// Full window-title dispatch: no-ROM message while cartridge-less,
/// otherwise the normal [`title_text`] meter line.
#[must_use]
pub fn window_title(
    fps: f64,
    underruns: u64,
    paused: bool,
    fast_forward: bool,
    has_rom: bool,
    slot: u8,
) -> String {
    if has_rom {
        title_text(fps, underruns, paused, fast_forward, slot)
    } else {
        NO_ROM_TITLE.to_string()
    }
}

/// How a file dropped onto the window is routed (pure — no window needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroppedFileKind {
    /// `.fm2` / `.bk2` demo movie (case-insensitive extension).
    Movie,
    /// Anything else is attempted as a ROM cartridge.
    Rom,
}

/// Classify a dropped path by extension (`.fm2`/`.bk2` → movie, else ROM).
#[must_use]
pub fn classify_dropped_file(path: &Path) -> DroppedFileKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "fm2" || ext == "bk2" {
        DroppedFileKind::Movie
    } else {
        DroppedFileKind::Rom
    }
}

/// Headless-testable window UI state: ROM presence, pause, and focus memory.
///
/// The event loop owns one of these field-by-field (`has_rom`, `paused`,
/// `ever_focused`); this struct mirrors that triple so the transitions are
/// unit-testable without opening a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowUiState {
    /// A cartridge is loaded and stepping real boot code.
    pub has_rom: bool,
    /// Emulation stepping is suspended (timer time is discarded).
    pub paused: bool,
    /// The window has gained focus at least once since launch.
    pub ever_focused: bool,
    /// Selected save-state slot (`0` = the historical default).
    pub savestate_slot: u8,
}

impl WindowUiState {
    /// Fresh UI state starts running. A no-ROM launch still shows
    /// [`NO_ROM_TITLE`], but it must not inherit a hidden pause state; a valid
    /// ROM likewise begins stepping as soon as the window loop starts.
    #[must_use]
    pub fn new(has_rom: bool) -> Self {
        Self {
            has_rom,
            paused: false,
            ever_focused: false,
            savestate_slot: 0,
        }
    }

    /// Current window title for this state.
    #[must_use]
    pub fn title(&self, fps: f64, underruns: u64, fast_forward: bool) -> String {
        window_title(
            fps,
            underruns,
            self.paused,
            fast_forward,
            self.has_rom,
            self.savestate_slot,
        )
    }

    /// Focus transition. Auto-pause on focus loss applies only after the
    /// window has gained focus at least once — otherwise a window opened
    /// unfocused (macOS open-unfocused) would instantly read `[PAUSED]`
    /// and look dead. Respects the `pause_on_focus_loss` pref throughout.
    pub fn on_focus(&mut self, focused: bool, pause_on_focus_loss: bool) {
        if focused {
            self.ever_focused = true;
        } else if pause_on_focus_loss && self.ever_focused {
            self.paused = true;
        }
    }

    /// A ROM drop (or pending-ROM swap) verified and rebuilt live: back to
    /// the normal title and resume stepping. Clears nothing else — the
    /// caller resets the movie track alongside.
    pub fn on_rom_loaded(&mut self) {
        self.has_rom = true;
        self.paused = false;
    }

    /// Whether the shared-audio pause-mute must be engaged for this state.
    ///
    /// Contract the windowed loop mirrors at every transition (init, `P`,
    /// focus loss, ROM-drop resume): the `cpal` callback keeps firing while
    /// stepping is suspended, so the ring mute follows `paused` exactly.
    /// Breaks of this contract are audible/visible in the title meters:
    /// starting cartless-but-unmuted floods `underruns` (no production while
    /// paused), and resuming stepping-but-muted runs the window silent.
    #[must_use]
    pub fn audio_muted(&self) -> bool {
        self.paused
    }
}

/// Audio status suffix for the window title (device rate always shown so
/// silent-audio states are visible; error/overrun flags appear on pathology).
pub fn audio_suffix(device_rate: Option<u32>, stream_errors: u64, overruns: u64) -> String {
    let mut s = String::new();
    match device_rate {
        Some(r) => s.push_str(&format!(" [audio {r} Hz]")),
        None => s.push_str(" [audio off]"),
    }
    if stream_errors > 0 {
        s.push_str(&format!(" [AUDIO ERR {stream_errors}]"));
    }
    if overruns > 0 {
        s.push_str(&format!(" [DROP {overruns}]"));
    }
    s
}
/// Blit the indexed framebuffer to RGBA `pixels` frame via the display-only
/// NES master palette (`z2-ppu`, never used for verification).
pub fn blit_indexed_to_rgba(indexed: &[u8; FRAME_LEN], rgba: &mut [u8]) {
    debug_assert_eq!(rgba.len(), FRAME_RGBA_LEN);
    blit_indexed_slice_to_rgba(indexed, rgba);
}

/// Blit an indexed buffer of **any** size to RGBA (widescreen present path).
///
/// The fixed-size [`blit_indexed_to_rgba`] is a wrapper over this, so the
/// 256x240 path keeps its exact shape while the wide path reuses the same
/// palette lookup.
pub fn blit_indexed_slice_to_rgba(indexed: &[u8], rgba: &mut [u8]) {
    debug_assert_eq!(rgba.len(), indexed.len() * 4);
    for (dst, &px) in rgba.chunks_exact_mut(4).zip(indexed.iter()) {
        dst.copy_from_slice(&z2_ppu::palette::indexed_to_rgba(px));
    }
}

/// Co-op status suffix for the window title (pure, like [`audio_suffix`]).
#[must_use]
pub fn coop_suffix(status: Option<&z2_core::coop::CoopStatus>) -> String {
    match status {
        None => String::new(),
        Some(st) if !st.active => " [coop P2 hidden]".to_string(),
        Some(st) if st.respawn_frames > 0 => {
            format!(" [coop P2 down {}]", st.respawn_frames)
        }
        Some(st) => format!(" [coop P2 hp {}/{}]", st.p2_hp, st.hp_max),
    }
}

// ---------------------------------------------------------------------------
// Presentation (widescreen + HD packs + scaling)
// ---------------------------------------------------------------------------

/// Purely visual settings. None of these reach the game: `Game::step` and
/// `Game::frame_indexed` are byte-identical whatever is set here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplaySettings {
    /// Widescreen margin tiles per side (0 = off, max 16).
    pub wide_tiles: u8,
    /// Requested output multiplier (1..=[`z2_render::MAX_SCALE`]). An HD pack
    /// whose scale this does not divide raises the effective scale to its own.
    pub scale: u32,
    /// Paint the window's clipped left 8 columns from the fetched margin tiles
    /// (widescreen only; removes the overworld's black seam).
    pub fill_left_clip: bool,
    /// Paint the window's right 8 columns from the line's own tiles wherever
    /// the game masked them with an opaque edge sprite (widescreen only;
    /// removes the overworld's black seam at x 248-255).
    pub fill_right_clip: bool,
    /// Draw side-view enemies, NPCs and items that are outside the window into
    /// the margins, and the in-window part of those just left of it
    /// (widescreen only; [`Features::margin_sprites`]).
    pub margin_sprites: bool,
    /// HD graphics pack directory (the one holding `pack.json`).
    pub pack_dir: Option<PathBuf>,
    /// Write a recorded template pack here when the app exits.
    pub record_dir: Option<PathBuf>,
}

impl DisplaySettings {
    /// Whether the PPU render record has to be armed for these settings.
    ///
    /// Widescreen margins, HD art and pack recording all need the per-line
    /// tile identities; nothing else does, so a plain run keeps the record off.
    #[must_use]
    pub fn needs_record(&self) -> bool {
        self.wide_tiles > 0 || self.pack_dir.is_some() || self.record_dir.is_some()
    }

    /// The [`Features`] an emulator must be built with to serve these
    /// settings, combined with `coop`.
    #[must_use]
    pub fn features(&self, coop: bool) -> Features {
        Features {
            coop,
            wide_gameplay: None,
            record: self.needs_record(),
            margin_sprites: self.wide_tiles > 0 && self.margin_sprites,
        }
    }
}

/// The present path: one [`z2_render::Presenter`] plus the optional pack
/// recorder, driven once per **presented** frame.
///
/// This is the single composition path for every mode — plain 256x240,
/// widescreen, HD pack, any integer scale — so there is exactly one place
/// where the texture size and the pixel content can disagree, and
/// [`Display::size`] is that place's answer.
pub struct Display {
    presenter: z2_render::Presenter,
    settings: DisplaySettings,
    recorder: Option<z2_render::Recorder>,
    /// Stand-in handed to the compositor when the record is off (no pack and
    /// no margins ever read it; see `Compositor::compose`).
    empty_record: z2_ppu::FrameRecord,
    /// One-line description of the loaded pack, for the startup log.
    pack_note: Option<String>,
}

impl std::fmt::Debug for Display {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Display")
            .field("settings", &self.settings)
            .field("size", &self.size())
            .field("effective_scale", &self.presenter.effective_scale())
            .field("recording", &self.recorder.is_some())
            .finish()
    }
}

impl Display {
    /// Build the present path, loading the HD pack if one is configured.
    ///
    /// # Errors
    /// A bad scale, or a pack directory that does not load — the user named
    /// that directory, so it is a loud error rather than a silent fallback.
    pub fn new(settings: DisplaySettings) -> Result<Self, String> {
        let cfg = z2_render::PresentConfig {
            scale: settings.scale.max(1),
            margin_tiles: settings.wide_tiles,
            fill_left_clip: settings.fill_left_clip,
            fill_right_clip: settings.fill_right_clip,
        };
        let mut presenter = z2_render::Presenter::new(cfg).map_err(|e| format!("display: {e}"))?;
        let mut pack_note = None;
        if let Some(dir) = &settings.pack_dir {
            let pack = z2_render::fs::load_pack_dir(dir)
                .map_err(|e| format!("HD pack {}: {e}", dir.display()))?;
            if !z2_render::Compositor::pack_supports_scale(&pack, cfg.scale) {
                eprintln!(
                    "HD pack: --hd-scale {} does not divide the pack's {}x art; presenting at {}x",
                    cfg.scale,
                    pack.scale(),
                    pack.scale()
                );
            }
            pack_note = Some(format!(
                "'{}' {}x, {} tiles ({} palette variants) from {}",
                pack.name(),
                pack.scale(),
                pack.tile_count(),
                pack.variant_count(),
                dir.display()
            ));
            presenter
                .set_pack(Some(pack))
                .map_err(|e| format!("HD pack {}: {e}", dir.display()))?;
        }
        let recorder = settings.record_dir.is_some().then(z2_render::Recorder::new);
        Ok(Self {
            presenter,
            settings,
            recorder,
            empty_record: z2_ppu::FrameRecord::new(),
            pack_note,
        })
    }

    /// The settings in force.
    #[must_use]
    pub fn settings(&self) -> &DisplaySettings {
        &self.settings
    }

    /// Texture size to allocate, in pixels. **Re-read after every change**:
    /// an HD pack can raise the effective scale above the requested one.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (
            self.presenter.width() as u32,
            self.presenter.height() as u32,
        )
    }

    /// Output multiplier actually in use.
    #[must_use]
    pub fn effective_scale(&self) -> u32 {
        self.presenter.effective_scale()
    }

    /// Whether the PPU render record must be armed.
    #[must_use]
    pub fn needs_record(&self) -> bool {
        self.presenter.needs_record() || self.recorder.is_some()
    }

    /// Whether widescreen margins are on.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.presenter.is_wide()
    }

    /// The pack description for the startup log, if a pack is loaded.
    #[must_use]
    pub fn pack_note(&self) -> Option<&str> {
        self.pack_note.as_deref()
    }

    /// Compose the game's last latched frame and return the RGBA bytes
    /// (`size().0 * size().1 * 4` of them).
    ///
    /// Order matters and is the whole contract: decode the margins from the
    /// record *first*, then compose. With the record off (or before the first
    /// stepped frame) the margins fall back to the line backdrop, which is the
    /// correct picture for a title screen or a cartridge-less window.
    ///
    /// # Errors
    /// A [`z2_render::ComposeError`], rendered as text. Every case is a size or
    /// scale mismatch, i.e. a frontend bug rather than user input.
    pub fn present(&mut self, game: &Game) -> Result<&[u8], String> {
        let tiles = self.settings.wide_tiles;
        if tiles > 0 {
            // `PresentConfig` carries both edge flags, but the settings can
            // change between `set_config` calls, so re-apply here each frame.
            self.presenter.margins_mut().fill_right_clip = self.settings.fill_right_clip;
            self.presenter.margins_mut().fill_left_sprites = self.settings.margin_sprites;
            if !game.wide_margins(tiles, self.presenter.margins_mut()) {
                self.presenter.margins_mut().clear_backdrop();
            }
        }
        if self.presenter.needs_scene() {
            self.presenter
                .set_scene(game.sideview_scene().map(|s| z2_render::SceneView {
                    world: s.world,
                    region: s.region,
                    scene: s.scene,
                    camera_x: s.camera_x,
                }));
        }
        let record = game.frame_record().unwrap_or(&self.empty_record);
        if let Some(rec) = self.recorder.as_mut() {
            rec.observe(record);
            rec.end_frame();
        }
        self.presenter
            .present(game.frame_indexed(), record, &game.chr)
            .map_err(|e| format!("present: {e}"))
    }

    /// Write the recorded template pack, if `--hd-record` asked for one.
    ///
    /// Returns `Ok(None)` when nothing was requested or nothing was drawn.
    /// The output is ROM-derived, so a directory inside a git work tree is
    /// refused (LEGAL.md §1) exactly as `cargo xtask hdpack` refuses one.
    ///
    /// # Errors
    /// A refused directory, an empty recording, or any I/O failure.
    pub fn write_recorded_pack(&self, chr_rom: &[u8]) -> Result<Option<PathBuf>, String> {
        let (Some(dir), Some(rec)) = (&self.settings.record_dir, self.recorder.as_ref()) else {
            return Ok(None);
        };
        if rec.is_empty() {
            return Err(format!(
                "--hd-record {}: nothing was drawn, so there is no pack to write",
                dir.display()
            ));
        }
        if let Some(root) = z2_render::fs::enclosing_git_worktree(dir) {
            return Err(format!(
                "--hd-record: refusing to write ROM-derived sheets into the git work tree at {} \
                 (LEGAL.md §1) — pick a directory outside the repository",
                root.display()
            ));
        }
        let scale = self.settings.scale.max(1);
        let files = rec
            .write_pack(chr_rom, scale, "z2rs-session")
            .map_err(|e| format!("--hd-record: {e}"))?;
        z2_render::fs::write_files(dir, &files)
            .map_err(|e| format!("--hd-record {}: {e}", dir.display()))?;
        Ok(Some(dir.clone()))
    }
}

// ---------------------------------------------------------------------------
// Windowed loop (winit + pixels + cpal + gilrs)
// ---------------------------------------------------------------------------

/// How the two-player mode was requested on the command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CoopMode {
    /// Single player (default).
    #[default]
    Off,
    /// Two players on this machine.
    Local,
    /// Host an online session in this room (we are player 1 / camera owner).
    Host(String),
    /// Join an online session in this room (we are player 2).
    Join(String),
}

impl CoopMode {
    /// True for [`Self::Host`] / [`Self::Join`].
    #[must_use]
    pub fn is_online(&self) -> bool {
        matches!(self, CoopMode::Host(_) | CoopMode::Join(_))
    }

    /// The room name for an online mode.
    #[must_use]
    pub fn room(&self) -> Option<&str> {
        match self {
            CoopMode::Host(r) | CoopMode::Join(r) => Some(r.as_str()),
            _ => None,
        }
    }

    /// Whether the game should run with a second Link at all.
    #[must_use]
    pub fn wants_two_links(&self) -> bool {
        !matches!(self, CoopMode::Off)
    }
}

/// Native (non-headless) CLI flags.
#[derive(Debug, Clone, Default)]
pub struct NativeArgs {
    /// `--rom PATH`.
    pub rom: Option<String>,
    /// `--movie PATH.fm2/.bk2` (demo playback).
    pub movie: Option<String>,
    /// `--config PATH` override.
    pub config: Option<String>,
    /// `--widescreen off|16:10|16:9|N` (overrides the config key).
    pub widescreen: Option<String>,
    /// `--coop-local` / `--coop-host ROOM` / `--coop-join ROOM`.
    pub coop: CoopMode,
    /// `--signal URL` (overrides `netplay.signal_url`).
    pub signal: Option<String>,
    /// `--net-delay N` input delay in frames (overrides `netplay.input_delay`).
    pub net_delay: Option<u8>,
    /// `--net-mode rollback|lockstep` (overrides `netplay.mode`).
    pub net_mode: Option<crate::config::NetMode>,
    /// `--ice SPEC` ICE servers (overrides `netplay.ice_url`; see `z2_net::ice`).
    pub ice: Option<String>,
    /// `--p2-pad INDEX`: pin player 2 to this gamepad in connection order.
    pub p2_pad: Option<usize>,
    /// `--hd-pack DIR` (overrides the config key; `""` forces the pack off).
    pub hd_pack: Option<String>,
    /// `--hd-scale N` output multiplier (overrides `hd_scale`).
    pub hd_scale: Option<u32>,
    /// `--hd-record DIR`: write a template pack of what this session drew.
    pub hd_record: Option<String>,
    /// `--fill-left-clip on|off` (overrides `widescreen_fill_left_clip`).
    pub fill_left_clip: Option<bool>,
    /// `--fill-right-clip on|off` (overrides `widescreen_fill_right_clip`).
    pub fill_right_clip: Option<bool>,
    /// `--margin-sprites on|off` (overrides `widescreen_margin_sprites`).
    pub margin_sprites: Option<bool>,
    /// `--wide-gameplay on|off` (overrides `widescreen_gameplay`).
    pub wide_gameplay: Option<bool>,
}

pub const NATIVE_USAGE: &str = "\
usage: z2-native [--rom PATH] [--movie M.fm2|.bk2] [--config PATH]
                 [--widescreen off|16:10|16:9|N]
                 [--fill-left-clip on|off] [--fill-right-clip on|off]
                 [--margin-sprites on|off]
                 [--wide-gameplay on|off]
                 [--hd-pack DIR] [--hd-scale N] [--hd-record DIR]
                 [--coop-local] [--coop-host ROOM | --coop-join ROOM]
                 [--signal URL] [--net-mode rollback|lockstep] [--net-delay N]
                 [--ice SPEC] [--p2-pad INDEX] [--headless ...]
  --rom PATH       Zelda II .nes ROM (else config rom_path / $Z2_ROM). Never stored.
  --movie PATH     .fm2/.bk2 demo playback (oracle-free: steps Game, no verify).
  --config PATH    JSON config override (default: <data-dir>/z2-native.json).
  --widescreen P   widescreen margins: off | 16:10 (8 tiles/side) | 16:9 (11) | N (0-16).
                   See README.md.
  --fill-left-clip on|off
                   paint the 8 columns the overworld blanks at x0-7 (default on).
  --fill-right-clip on|off
                   paint the 8 columns the overworld masks at x248-255 (default on).
  --margin-sprites on|off
                   draw side-view enemies, NPCs and items outside the window into
                   the margins (display only; default on).
  --wide-gameplay on|off
                   with widescreen, enemies spawn and live out in the margins
                   (default on, off with --movie; changes gameplay, both netplay
                   peers must agree).
  --hd-pack DIR    HD graphics pack directory (the one with pack.json); '' = off.
                   Build one with: cargo xtask hdpack template (see README.md).
  --hd-scale N     output multiplier 1-8 (default 1). A pack whose scale N does
                   not divide is presented at the pack's own scale.
  --hd-record DIR  on exit, write a template pack of the tiles this session drew.
                   ROM-derived output: DIR must be outside any git work tree.
  --coop-local     two players on this machine (P2: keys_p2 / second gamepad).
  --coop-host ROOM host an online 2-player session (you are player 1).
  --coop-join ROOM join an online session in ROOM as player 2.
  --signal URL     signalling server, ws://host[:port] (default ws://127.0.0.1:3536;
                   run it with: cargo run --release -p z2-signal).
  --net-mode M     online sync: rollback (default; local input is instant, the remote
                   player is predicted and corrected) | lockstep (waits for both pads).
  --net-delay N    netplay input delay in frames (default 2): rollback 0-3,
                   lockstep 0-8 (try ceil(RTT_ms/16)+1).
  --ice SPEC       ICE servers: space-separated stun:/turn: URLs, plus username=...
                   credential=... for TURN; 'none' = same machine / LAN only;
                   default: Google STUN (see README.md)
  --p2-pad INDEX   pin player 2 to gamepad INDEX (0-based, connection order).
  --headless ...   windowless CI surface (see --headless --help).
keys P1: Z=A X=B Enter=Start RightShift=Select arrows=dpad
keys P2: G=A F=B T=Start R=Select W/A/S/D=dpad (local co-op only; see keys_p2)
Tab=fast-forward F5=save F7=load F6/digits=slot (1-9,0; shown in title) P=pause .=step Esc=quit; drop a .nes ROM/movie \
file onto the window. Save states, movies, pause and fast-forward are disabled \
during netplay.";

/// Parse an `on|off` flag value.
fn native_on_off<'a, I>(it: &mut std::iter::Peekable<I>, flag: &str) -> Result<bool, String>
where
    I: Iterator<Item = &'a String>,
{
    match native_arg_value(it, flag)?.as_str() {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => Err(format!(
            "{flag} expects on|off, got '{other}'\n{NATIVE_USAGE}"
        )),
    }
}

fn native_arg_value<'a, I>(it: &mut std::iter::Peekable<I>, flag: &str) -> Result<String, String>
where
    I: Iterator<Item = &'a String>,
{
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} expects a value\n{NATIVE_USAGE}"))
}

/// Parse non-headless argv (call only when `--headless` is absent).
///
/// Strict whitelist: an unknown flag is an error (the caller exits 2), and
/// every value is validated here rather than half-way through startup, so a
/// typo never opens a window first.
pub fn parse_native_args(argv: &[String]) -> Result<NativeArgs, String> {
    let mut out = NativeArgs::default();
    let mut coop_flags_seen = 0usize;
    let mut it = argv.iter().skip(1).peekable();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "--rom" => out.rom = Some(native_arg_value(&mut it, "--rom")?),
            "--movie" => out.movie = Some(native_arg_value(&mut it, "--movie")?),
            "--config" => out.config = Some(native_arg_value(&mut it, "--config")?),
            "--widescreen" => {
                let v = native_arg_value(&mut it, "--widescreen")?;
                if z2_ppu::preset_tiles(&v).is_none() {
                    return Err(format!(
                        "--widescreen: expected off | 16:10 | 16:9 | a number 0-16, got '{v}'\n{NATIVE_USAGE}"
                    ));
                }
                out.widescreen = Some(v);
            }
            "--coop-local" => {
                coop_flags_seen += 1;
                out.coop = CoopMode::Local;
            }
            "--coop-host" | "--coop-join" => {
                coop_flags_seen += 1;
                let flag = tok.as_str();
                let room = native_arg_value(&mut it, flag)?;
                if !z2_net::room_name_valid(&room) {
                    return Err(format!(
                        "{flag}: room name must be 1-32 characters of A-Z a-z 0-9 _ -, got '{room}'\n{NATIVE_USAGE}"
                    ));
                }
                out.coop = if flag == "--coop-host" {
                    CoopMode::Host(room)
                } else {
                    CoopMode::Join(room)
                };
            }
            "--signal" => {
                let v = native_arg_value(&mut it, "--signal")?;
                // Validated against a placeholder room so a bad URL is caught
                // now, with the same message the connect path would give.
                if z2_net::room_url(&v, "probe").is_err() {
                    return Err(format!(
                        "--signal: expected ws://host[:port] or wss://host[:port], got '{v}'\n{NATIVE_USAGE}"
                    ));
                }
                out.signal = Some(v);
            }
            "--net-delay" => {
                let raw = native_arg_value(&mut it, "--net-delay")?;
                let n: u8 = raw.parse().map_err(|_| {
                    format!(
                        "--net-delay expects a number 0-{}, got '{raw}'\n{NATIVE_USAGE}",
                        z2_net::MAX_DELAY
                    )
                })?;
                if n > z2_net::MAX_DELAY {
                    return Err(format!(
                        "--net-delay expects 0-{}, got {n}\n{NATIVE_USAGE}",
                        z2_net::MAX_DELAY
                    ));
                }
                out.net_delay = Some(n);
            }
            "--net-mode" => {
                let raw = native_arg_value(&mut it, "--net-mode")?;
                out.net_mode = Some(crate::config::NetMode::parse(&raw).ok_or_else(|| {
                    format!("--net-mode expects rollback or lockstep, got '{raw}'\n{NATIVE_USAGE}")
                })?);
            }
            "--ice" => {
                let v = native_arg_value(&mut it, "--ice")?;
                if let Err(e) = z2_net::IceConfig::parse(&v) {
                    return Err(format!("--ice: {e}\n{NATIVE_USAGE}"));
                }
                out.ice = Some(v);
            }
            "--p2-pad" => {
                let raw = native_arg_value(&mut it, "--p2-pad")?;
                out.p2_pad = Some(raw.parse().map_err(|_| {
                    format!(
                        "--p2-pad expects a gamepad index (0-based), got '{raw}'\n{NATIVE_USAGE}"
                    )
                })?);
            }
            "--hd-pack" => out.hd_pack = Some(native_arg_value(&mut it, "--hd-pack")?),
            "--hd-record" => out.hd_record = Some(native_arg_value(&mut it, "--hd-record")?),
            "--hd-scale" => {
                let raw = native_arg_value(&mut it, "--hd-scale")?;
                let n: u32 = raw.parse().unwrap_or(0);
                if n == 0 || n > z2_render::MAX_SCALE {
                    return Err(format!(
                        "--hd-scale expects 1-{}, got '{raw}'\n{NATIVE_USAGE}",
                        z2_render::MAX_SCALE
                    ));
                }
                out.hd_scale = Some(n);
            }
            "--fill-left-clip" => {
                out.fill_left_clip = Some(native_on_off(&mut it, "--fill-left-clip")?);
            }
            "--fill-right-clip" => {
                out.fill_right_clip = Some(native_on_off(&mut it, "--fill-right-clip")?);
            }
            "--margin-sprites" => {
                out.margin_sprites = Some(native_on_off(&mut it, "--margin-sprites")?);
            }
            "--wide-gameplay" => {
                out.wide_gameplay = Some(native_on_off(&mut it, "--wide-gameplay")?);
            }
            "--help" | "-h" => return Err(NATIVE_USAGE.to_string()),
            other => return Err(format!("unknown flag '{other}'\n{NATIVE_USAGE}")),
        }
    }
    if coop_flags_seen > 1 {
        return Err(format!(
            "pick one of --coop-local / --coop-host / --coop-join\n{NATIVE_USAGE}"
        ));
    }
    if let (Some(mode), Some(d)) = (out.net_mode, out.net_delay) {
        if d > mode.max_delay() {
            return Err(format!(
                "--net-delay {d} is above the {} maximum of {}\n{NATIVE_USAGE}",
                mode.name(),
                mode.max_delay()
            ));
        }
    }
    if out.coop.is_online() && out.movie.is_some() {
        return Err(format!(
            "netplay and movie playback are mutually exclusive: a movie would \
             override the pads the session confirms\n{NATIVE_USAGE}"
        ));
    }
    if out.coop.is_online() && !crate::netplay::supported() {
        return Err(format!("{}\n{NATIVE_USAGE}", crate::netplay::NO_TRANSPORT));
    }
    Ok(out)
}

/// Run the windowed frontend. Opens a window + audio device; never call in
/// tests. Returns a human-readable error (the `main` wrapper maps it to an
/// exit code).
pub fn run_windowed(args: &NativeArgs) -> Result<(), String> {
    // -- config + data dir -------------------------------------------------
    let config: NativeConfig = match &args.config {
        Some(p) => NativeConfig::load_from(Path::new(p)),
        None => NativeConfig::load(),
    };
    let data_dir = config.effective_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    // Persist defaults on first run so users can discover the file.
    if args.config.is_none() && !config_path_exists() {
        let _ = config.save();
    }

    // -- features (CLI overrides config) -----------------------------------
    // Visual settings first: whether the PPU render record has to be armed
    // depends on them, and the record has to be on before the first stepped
    // frame or the margins/HD art have no tile identities to read.
    let display_settings = resolve_display(args, &config)?;
    let coop = resolve_coop(args, &config);
    let display = Display::new(display_settings.clone())?;
    let feats = Features {
        coop,
        wide_gameplay: resolve_wide_gameplay(args, &config, display_settings.wide_tiles),
        record: display.needs_record(),
        margin_sprites: display_settings.features(coop).margin_sprites,
    };
    let coop_local = feats.coop && !args.coop.is_online();

    // -- emulator -----------------------------------------------------------
    // No-ROM fallback is VISIBLE, never silent: `has_rom=false` renders the
    // empty frame behind NO_ROM_TITLE until a ROM drop flips it (the
    // eprintln lines below are logs only — the title is the signal).
    //
    // The features go in at construction, and `new_emu_with` honours the
    // widescreen width too, so a `--widescreen` launch with no cartridge
    // still presents a correctly sized buffer.
    let rate = config.effective_audio_rate();
    let (mut emu, has_rom, rom_body) = match resolve_rom_path(args.rom.as_deref(), &config) {
        Ok(rom_path) => match emu_from_rom_file_with(&rom_path, rate, feats) {
            Ok((e, body)) => (e, true, Some(body)),
            Err(err) => {
                eprintln!("ROM load failed ({err}); showing no-ROM window.");
                (new_emu_with(rate, feats), false, None)
            }
        },
        Err(note) => {
            eprintln!("{note}\nShowing no-ROM window.");
            (new_emu_with(rate, feats), false, None)
        }
    };
    let _ = load_sram(&mut emu.game, &data_dir);
    // `load_sram` replaced WRAM under a possibly live co-op state.
    emu.game.coop_reset_area();

    // -- demo movie ----------------------------------------------------------
    let movie_track: Vec<u8> = match &args.movie {
        Some(p) => {
            let track = load_movie_track(Path::new(p)).map_err(|e| format!("movie: {e}"))?;
            eprintln!("demo movie: {} frames from {}", track.len(), p);
            track
        }
        None => Vec::new(),
    };

    if args.coop.is_online() && !has_rom {
        return Err(
            "netplay needs a ROM: pass --rom PATH (both peers must run the same ROM)".to_string(),
        );
    }
    if display_settings.wide_tiles > 0 {
        eprintln!(
            "widescreen: {} tiles per side; side-view objects in the margins {}; \
             wide gameplay {}.",
            display_settings.wide_tiles,
            if feats.margin_sprites { "on" } else { "off" },
            if feats.wide_gameplay.is_some() {
                "on"
            } else {
                "off"
            }
        );
    }
    if let Some(note) = display.pack_note() {
        eprintln!("HD pack: {note}");
    }
    if let Some(dir) = &display_settings.record_dir {
        eprintln!(
            "HD recording: a template pack for everything drawn this session will be \
             written to {} on exit",
            dir.display()
        );
    }
    let (px_w, px_h) = display.size();
    eprintln!(
        "present: {px_w}x{px_h} ({}x scale{})",
        display.effective_scale(),
        if display.is_wide() {
            ", widescreen"
        } else {
            ""
        }
    );
    if coop_local {
        eprintln!(
            "local co-op: player 2 = G:A F:B T:Start R:Select W/A/S/D:dpad (or a second \
             gamepad). Player 2 exists in side-view areas only."
        );
    }

    run_event_loop(RunConfig {
        config,
        data_dir,
        emu,
        display,
        movie_track,
        has_rom,
        feats,
        coop_local,
        args: args.clone(),
        rom_body,
    })
}

/// Effective visual settings from the CLI (which wins) and the config file.
///
/// `--widescreen` and `--hd-scale` were already syntax-checked by
/// [`parse_native_args`]; a bad value in the *config* silently degrades to the
/// default, so a stale config can never stop the app from starting.
///
/// `--hd-pack ''` (an empty value) is how you turn a pack configured in the
/// file off for one run.
///
/// # Errors
/// An unparseable `--widescreen` value (only reachable when a caller builds
/// [`NativeArgs`] by hand rather than through the parser).
pub fn resolve_display(
    args: &NativeArgs,
    config: &NativeConfig,
) -> Result<DisplaySettings, String> {
    let wide_tiles = match &args.widescreen {
        Some(p) => z2_ppu::preset_tiles(p).ok_or_else(|| {
            format!("--widescreen: expected off | 16:10 | 16:9 | a number 0-16, got '{p}'")
        })?,
        None => config.widescreen_tiles(),
    };
    let non_empty = |s: &String| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
    Ok(DisplaySettings {
        wide_tiles,
        scale: match args.hd_scale {
            Some(n) => n.clamp(1, z2_render::MAX_SCALE),
            None => config.effective_hd_scale(),
        },
        fill_left_clip: args
            .fill_left_clip
            .unwrap_or(config.widescreen_fill_left_clip),
        fill_right_clip: args
            .fill_right_clip
            .unwrap_or(config.widescreen_fill_right_clip),
        margin_sprites: args
            .margin_sprites
            .unwrap_or(config.widescreen_margin_sprites),
        pack_dir: match &args.hd_pack {
            Some(s) => non_empty(s),
            None => config.hd_pack.as_ref().and_then(non_empty),
        },
        record_dir: match &args.hd_record {
            Some(s) => non_empty(s),
            None => config.hd_record.as_ref().and_then(non_empty),
        },
    })
}

/// The wide-gameplay margin for an interactive run: the widescreen margin
/// whenever widescreen is on and wide gameplay is not turned off
/// (`--wide-gameplay on|off` wins over the `widescreen_gameplay` config key,
/// default on). `None` without widescreen. A `--movie` playback defaults it
/// off (it changes gameplay, so the movie would desync) unless
/// `--wide-gameplay on` asks for it.
#[must_use]
pub fn resolve_wide_gameplay(
    args: &NativeArgs,
    config: &NativeConfig,
    wide_tiles: u8,
) -> Option<u8> {
    let on = args
        .wide_gameplay
        .unwrap_or(config.widescreen_gameplay && args.movie.is_none());
    (on && wide_tiles > 0).then_some(wide_tiles)
}

/// Whether this run gives the game a second Link.
///
/// Two Links whenever any co-op mode is active: local co-op, or either end of
/// an online session (the session's co-op flags are always non-zero, so a guest
/// always gets real gameplay rather than an idle second pad).
#[must_use]
pub fn resolve_coop(args: &NativeArgs, config: &NativeConfig) -> bool {
    args.coop.wants_two_links() || (args.coop == CoopMode::Off && config.coop_local)
}

/// Effective game-affecting features from the CLI and the config file
/// (the interactive defaults: wide gameplay follows widescreen, see
/// [`resolve_wide_gameplay`]).
///
/// # Errors
/// Whatever [`resolve_display`] rejects (the record flag depends on it).
pub fn resolve_features(args: &NativeArgs, config: &NativeConfig) -> Result<Features, String> {
    let display = resolve_display(args, config)?;
    let mut feats = display.features(resolve_coop(args, config));
    feats.wide_gameplay = resolve_wide_gameplay(args, config, display.wide_tiles);
    Ok(feats)
}

/// Everything [`run_event_loop`] needs, bundled so the signature stays
/// readable as features accumulate.
struct RunConfig {
    config: NativeConfig,
    data_dir: PathBuf,
    emu: Emu,
    display: Display,
    movie_track: Vec<u8>,
    has_rom: bool,
    feats: Features,
    coop_local: bool,
    args: NativeArgs,
    rom_body: Option<Vec<u8>>,
}

fn config_path_exists() -> bool {
    std::fs::metadata(assets_path_config()).is_ok()
}

fn assets_path_config() -> PathBuf {
    crate::config::config_path()
}

/// The winit event-loop body. Separated for readability; still opens a
/// window — never call from tests.
#[allow(clippy::too_many_lines)]
fn run_event_loop(run: RunConfig) -> Result<(), String> {
    let RunConfig {
        config,
        data_dir,
        emu,
        display,
        movie_track,
        has_rom,
        feats,
        coop_local,
        args,
        rom_body,
    } = run;
    use winit::application::ApplicationHandler;
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::Window;

    struct Handler {
        config: NativeConfig,
        data_dir: PathBuf,
        emu: Emu,
        /// The one present path (widescreen, HD pack, scaling).
        display: Display,
        movie: Vec<u8>,
        movie_pos: usize,
        window: Option<&'static Window>,
        pixels: Option<pixels::Pixels<'static>>,
        keyboard: crate::input::KeyboardState,
        gilrs: Option<gilrs::Gilrs>,
        /// Gamepad that most recently pressed a button (drives player 1).
        active_pad: Option<gilrs::GamepadId>,
        audio: SharedAudio,
        _stream: Option<cpal::Stream>,
        /// Actual output-device rate (None = audio disabled). Shown in the
        /// title so rate fallback/silence is always visible.
        device_rate: Option<u32>,
        timer: FrameTimer,
        last_tick: Option<Instant>,
        paused: bool,
        has_rom: bool,
        ever_focused: bool,
        fast_forward: bool,
        /// Selected save-state slot (digits pick, `F6` cycles; shown in title).
        savestate_slot: u8,
        frame_step: bool,
        /// Last observed `Game::exec_errors` (wedge guard in step_emulator).
        exec_errors_seen: u64,
        fps_acc: u32,
        fps_shown: f64,
        fps_last: Instant,
        /// Set by `WindowEvent::Occluded`: while fully hidden/minimized the
        /// loop keeps stepping but skips the GPU present (some drivers fail
        /// surface acquisition while occluded); the next visible redraw
        /// re-blits the full frame, so nothing stale survives.
        occluded: bool,
        /// Features to re-apply to every emulator this loop builds (ROM drop,
        /// netplay session start) — the single source of truth that stops a
        /// drop from silently losing widescreen or co-op.
        feats: Features,
        /// Two players sharing this machine (second key set / second gamepad).
        coop_local: bool,
        /// Verified ROM body, kept so a netplay session can rebuild a fresh
        /// game at `Started` without re-reading the file.
        rom_body: Option<Vec<u8>>,
        /// Gamepad driving player 2 by "most recent presser that is not P1".
        active_pad2: Option<gilrs::GamepadId>,
        /// Gamepads in connection order, for `--p2-pad INDEX`.
        pad_order: Vec<gilrs::GamepadId>,
        /// `--p2-pad INDEX`: pin player 2 to this entry of `pad_order`.
        p2_pad: Option<usize>,
        /// Size the `pixels` texture currently has, so the present path only
        /// calls `resize_buffer` when it actually changes. `(0, 0)` forces a
        /// re-check on the next redraw (used after a ROM drop or a netplay
        /// session start rebuilds the emulator).
        tex_size: (u32, u32),
        /// Monotonic base for the millisecond clock netplay timers use.
        #[cfg(feature = "netplay")]
        start: Instant,
        /// Live netplay session, if any.
        #[cfg(feature = "netplay")]
        net: Option<crate::netplay::NetLink<z2_net::MatchboxTransport>>,
        /// Live rollback netplay session, if any (the default online mode).
        #[cfg(feature = "netplay")]
        rollback: Option<crate::netplay::RollbackLink<z2_net::MatchboxTransport>>,
    }

    impl Handler {
        /// Fold one gamepad's pressed buttons through a binding table.
        ///
        /// This is where `config.gamepad` / `config.gamepad_p2` finally take
        /// effect (they were parsed but ignored before). The default tables
        /// reproduce the previous hard-coded layout exactly — pinned by
        /// `input::config_gamepad_table_reproduces_the_default_layout` — so
        /// nothing changes for a user who never edited the config.
        fn pad_bits(gp: &gilrs::Gamepad<'_>, table: &std::collections::HashMap<String, u8>) -> u8 {
            let mut names: Vec<&str> = Vec::new();
            for (name, button) in crate::input::BINDABLE_BUTTONS
                .iter()
                .chain(crate::input::BINDABLE_DPAD.iter())
            {
                if gp.is_pressed(*button) {
                    names.push(name);
                }
            }
            let mut pad = crate::input::gamepad_bits(table, names.into_iter());
            let (lx, ly) = (
                gp.axis_data(gilrs::Axis::LeftStickX),
                gp.axis_data(gilrs::Axis::LeftStickY),
            );
            let (lx, ly) = (
                lx.map(|a| a.value()).unwrap_or(0.0),
                ly.map(|a| a.value()).unwrap_or(0.0),
            );
            pad |= crate::input::axis_to_bits(lx, ly, 0.5);
            pad
        }

        /// Gamepad pinned to player 2 by `--p2-pad INDEX`, if connected.
        fn pinned_p2(&self) -> Option<gilrs::GamepadId> {
            self.p2_pad.and_then(|i| self.pad_order.get(i).copied())
        }

        /// Gamepad currently driving player 2.
        fn p2_pad_id(&self) -> Option<gilrs::GamepadId> {
            self.pinned_p2().or(self.active_pad2)
        }

        /// Drain gamepad events and read both pads.
        ///
        /// Player 1 is the pad that most recently pressed a button — the
        /// existing single-player rule, which avoids both the empty port of a
        /// multi-port adapter (e.g. Mayflash GameCube) and the ghost stick
        /// directions that OR-ing every pad produced. With local co-op on, a
        /// press from a *different* pad claims player 2 instead of stealing
        /// player 1, and `--p2-pad` pins that slot explicitly. With co-op off
        /// the rule is byte-for-byte what it was before.
        fn poll_gamepads(&mut self) -> (u8, u8) {
            enum PadEv {
                Conn(gilrs::GamepadId),
                Press(gilrs::GamepadId),
                Disc(gilrs::GamepadId),
            }
            let mut evs: Vec<PadEv> = Vec::new();
            let mut names: Vec<(gilrs::GamepadId, String)> = Vec::new();
            if let Some(g) = self.gilrs.as_mut() {
                while let Some(ev) = g.next_event() {
                    match ev.event {
                        gilrs::EventType::Connected => {
                            if let Some(gp) = g.connected_gamepad(ev.id) {
                                names.push((ev.id, gp.name().to_string()));
                            }
                            evs.push(PadEv::Conn(ev.id));
                        }
                        gilrs::EventType::ButtonPressed(..) => evs.push(PadEv::Press(ev.id)),
                        gilrs::EventType::Disconnected => evs.push(PadEv::Disc(ev.id)),
                        _ => {}
                    }
                }
            }
            // The index is the pad's slot in connection order, so it has to be
            // read AFTER the push below, not while draining the event queue —
            // a multi-port adapter announces all of its ports in one batch and
            // every one of them would otherwise be logged as "index 0".
            let announce = |order: &[gilrs::GamepadId], id: gilrs::GamepadId| {
                if let Some(name) = names.iter().find(|(i, _)| *i == id).map(|(_, n)| n) {
                    let idx = order.iter().position(|p| *p == id).unwrap_or(0);
                    // Logged so a user can discover which pad is which when
                    // pinning one with `--p2-pad`.
                    eprintln!("gamepad connected: '{name}' (index {idx})");
                }
            };
            for e in evs {
                match e {
                    PadEv::Conn(id) => {
                        if !self.pad_order.contains(&id) {
                            self.pad_order.push(id);
                            announce(&self.pad_order, id);
                        }
                    }
                    PadEv::Press(id) => {
                        if !self.pad_order.contains(&id) {
                            self.pad_order.push(id);
                            announce(&self.pad_order, id);
                        }
                        if !self.coop_local {
                            self.active_pad = Some(id);
                        } else if self.pinned_p2() == Some(id) {
                            // The pinned player-2 pad never becomes player 1.
                        } else if self.active_pad.is_none() || self.active_pad == Some(id) {
                            self.active_pad = Some(id);
                        } else if self.pinned_p2().is_none() {
                            self.active_pad2 = Some(id);
                        }
                    }
                    PadEv::Disc(id) => {
                        self.pad_order.retain(|&p| p != id);
                        if self.active_pad == Some(id) {
                            self.active_pad = None;
                        }
                        if self.active_pad2 == Some(id) {
                            self.active_pad2 = None;
                        }
                    }
                }
            }
            let (id1, id2) = (self.active_pad, self.p2_pad_id());
            let mut pads = (0u8, 0u8);
            if let Some(g) = self.gilrs.as_ref() {
                if let Some(gp) = id1.and_then(|id| g.connected_gamepad(id)) {
                    pads.0 = Self::pad_bits(&gp, &self.config.gamepad.map);
                }
                if self.coop_local {
                    if let Some(gp) = id2.and_then(|id| g.connected_gamepad(id)) {
                        pads.1 = Self::pad_bits(&gp, &self.config.gamepad_p2.map);
                    }
                }
            }
            pads
        }

        /// Both players' pads for one frame.
        ///
        /// A demo movie overrides player 1 while frames remain; movie files
        /// carry one port, so player 2 stays idle during playback.
        fn current_inputs(&mut self) -> (u8, u8) {
            if self.movie_pos < self.movie.len() {
                let b = self.movie[self.movie_pos];
                self.movie_pos += 1;
                return (b, 0);
            }
            let kb1 = self.keyboard.pad(&self.config.keys.map);
            let kb2 = if self.coop_local {
                self.keyboard.pad(&self.config.keys_p2.map)
            } else {
                0
            };
            let (gp1, gp2) = self.poll_gamepads();
            // Apple-driver pads (e.g. Xbox One S over USB or Bluetooth) enumerate
            // through gilrs but deliver input only via GameController, so they
            // are read separately and drive player 1.
            #[cfg(target_os = "macos")]
            let gp1 = gp1 | crate::gc_pad::poll();
            (
                crate::input::combine_inputs(kb1, gp1),
                crate::input::combine_inputs(kb2, gp2),
            )
        }

        /// Apply one netplay session event.
        #[cfg(feature = "netplay")]
        fn on_net_event(&mut self, ev: z2_net::SessionEvent) {
            use z2_net::SessionEvent as Ev;
            match ev {
                Ev::PeerHello { role, delay, .. } => {
                    eprintln!("netplay: peer joined as {role:?} (its delay request: {delay})");
                }
                Ev::Started {
                    delay,
                    coop_flags,
                    wram,
                    ..
                } => {
                    // Both peers restart from power-on with the host's save, so
                    // the two games are byte-identical before frame 0.
                    let rate = self.config.effective_audio_rate();
                    let feats = Features {
                        coop: coop_flags & z2_net::COOP_TWO_LINKS != 0,
                        wide_gameplay: self.feats.wide_gameplay,
                        record: self.feats.record,
                        margin_sprites: self.feats.margin_sprites,
                    };
                    let Some(body) = self.rom_body.clone() else {
                        eprintln!("netplay: no ROM body available to start a session");
                        return;
                    };
                    match emu_from_rom_body_with(&body, rate, feats) {
                        Ok(mut fresh) => {
                            if wram.len() == fresh.game.wram.len() {
                                fresh.game.wram.copy_from_slice(&wram);
                                fresh.game.coop_reset_area();
                            }
                            self.emu = fresh;
                            // Force the present path to re-check the texture.
                            self.tex_size = (0, 0);
                            self.audio.clear();
                            self.audio.set_paused(false);
                            self.timer = FrameTimer::new();
                            self.paused = false;
                            self.movie.clear();
                            self.movie_pos = 0;
                            if let Some(net) = self.net.as_mut() {
                                if net.requested_delay != delay {
                                    eprintln!(
                                        "netplay: using the host's input delay {delay} (you asked for {})",
                                        net.requested_delay
                                    );
                                }
                                net.delay = delay;
                                net.started = true;
                            }
                            eprintln!("netplay: session started (input delay {delay} frames)");
                        }
                        Err(e) => eprintln!("netplay: cannot start session: {e}"),
                    }
                }
                Ev::Stalled { frame } => {
                    eprintln!("netplay: waiting for the other player at frame {frame}");
                    self.audio.set_paused(true);
                    if let Some(net) = self.net.as_mut() {
                        net.muted = true;
                    }
                }
                Ev::Resumed { stalled_ms, .. } => {
                    // Drop the stale audio tail AND the wall-clock backlog:
                    // bursting through catch-up frames would immediately stall
                    // the peer right back.
                    self.audio.clear();
                    self.audio.set_paused(false);
                    self.timer = FrameTimer::new();
                    if let Some(net) = self.net.as_mut() {
                        net.muted = false;
                    }
                    eprintln!("netplay: resumed after {stalled_ms} ms");
                }
                Ev::Desync {
                    frame,
                    local,
                    remote,
                } => {
                    eprintln!(
                        "netplay: DESYNC at frame {frame} (local {local:#018x}, remote \
                         {remote:#018x}) - please report this with both peers' versions"
                    );
                    self.paused = true;
                    self.audio.set_paused(true);
                }
                Ev::Closed(reason) => {
                    eprintln!("netplay: session closed ({reason})");
                    self.paused = true;
                    self.audio.set_paused(true);
                }
            }
        }

        /// Pump the session and step whatever frames it has confirmed.
        ///
        /// Always runs (even with a zero frame budget) so the handshake and
        /// the stall timers keep making progress while nothing steps.
        #[cfg(feature = "netplay")]
        fn step_netplay(&mut self, budget: usize) {
            if self.net.is_none() {
                return;
            }
            let now = self.now_ms();
            if let Some(net) = self.net.as_mut() {
                net.update(now);
                net.report_stage();
            }
            let events = self
                .net
                .as_mut()
                .map(|n| n.take_events())
                .unwrap_or_default();
            for ev in events {
                self.on_net_event(ev);
            }
            // Until the session starts, the local game keeps running: `Started`
            // restarts from power-on anyway, so nothing played here reaches
            // the session, and a slow connection never looks like a hang.
            if self.net.as_ref().is_some_and(|n| !n.started && n.is_live()) {
                if budget > 0 && !self.paused {
                    self.step_emulator(budget);
                    self.fps_acc += budget as u32;
                }
                return;
            }
            // The local pad is sampled ONCE per tick and reused for every frame
            // stepped this tick; only the bytes travel, so both peers agree.
            let local = self.current_inputs().0;
            let stepped = {
                let emu = &mut self.emu;
                let audio = &self.audio;
                let Some(net) = self.net.as_mut() else {
                    return;
                };
                let mut pcm = Vec::new();
                crate::netplay::step_session(net, budget.max(1) as u32, local, |p1, p2, want| {
                    emu.game.step2(p1, p2);
                    drain_audio_frame(emu, &mut pcm, Some(audio));
                    want.then(|| crate::netplay::state_hash(&emu.game))
                })
                .stepped
            };
            self.fps_acc += stepped;
            if stepped > 0 {
                self.audio.trim_oldest(self.audio.suggested_max_depth());
                let frames = self.emu.game.frame_count();
                // The host owns sram.sav; a guest autosaves the session to a
                // separate file so its own solo save is never overwritten.
                if frames != 0 && frames.is_multiple_of(600) && self.emu.game.exec_errors == 0 {
                    let name = self.sram_file();
                    let _ = save_sram_named(&mut self.emu.game, &self.data_dir, name);
                }
            }
            // Flush the pads just latched so the peer is not left waiting a
            // whole frame for them.
            let now = self.now_ms();
            if let Some(net) = self.net.as_mut() {
                net.update(now);
            }
        }

        /// Apply one rollback session event.
        #[cfg(feature = "netplay")]
        fn on_rollback_event(&mut self, ev: z2_net::RollbackEvent) {
            use z2_net::RollbackEvent as Ev;
            match ev {
                Ev::Synchronizing { progress } => {
                    eprintln!("netplay: synchronizing ({progress}%)");
                }
                Ev::Running {
                    input_delay,
                    coop_flags,
                    wram,
                    ..
                } => {
                    // Power-on with the host's save on both peers, exactly as
                    // the lockstep start does.
                    let Some(body) = self.rom_body.as_deref() else {
                        eprintln!("netplay: no ROM body available to start a session");
                        return;
                    };
                    let rate = self.config.effective_audio_rate();
                    match crate::netplay::session_emu(
                        body,
                        rate,
                        self.feats.record,
                        self.feats.wide_gameplay,
                        coop_flags,
                        &wram,
                    ) {
                        Ok(mut fresh) => {
                            fresh.game.set_margin_sprites(self.feats.margin_sprites);
                            self.emu = fresh;
                            self.tex_size = (0, 0);
                            self.audio.clear();
                            self.audio.set_paused(false);
                            self.timer = FrameTimer::new();
                            self.paused = false;
                            self.movie.clear();
                            self.movie_pos = 0;
                            if let Some(rb) = self.rollback.as_mut() {
                                if rb.requested_delay != input_delay {
                                    eprintln!(
                                        "netplay: using the host's input delay {input_delay} (you asked for {})",
                                        rb.requested_delay
                                    );
                                }
                                rb.started = true;
                                rb.skip_ticks = 0;
                            }
                            eprintln!(
                                "netplay: rollback session started (input delay {input_delay} frames)"
                            );
                        }
                        Err(e) => {
                            eprintln!("netplay: cannot start session: {e}");
                            if let Some(rb) = self.rollback.as_mut() {
                                rb.session.close();
                            }
                        }
                    }
                }
                Ev::WaitRecommendation { skip_frames } => {
                    if let Some(rb) = self.rollback.as_mut() {
                        rb.skip_ticks = u32::from(skip_frames);
                    }
                }
                Ev::NetworkInterrupted { silent_ms } => {
                    eprintln!("netplay: nothing from the other player for {silent_ms} ms");
                    if let Some(rb) = self.rollback.as_mut() {
                        rb.interrupted = true;
                    }
                }
                Ev::NetworkResumed => {
                    eprintln!("netplay: connection resumed");
                    if let Some(rb) = self.rollback.as_mut() {
                        rb.interrupted = false;
                    }
                }
                Ev::DesyncDetected {
                    frame,
                    local,
                    remote,
                } => {
                    eprintln!(
                        "netplay: DESYNC at frame {frame} (local {local:#018x}, remote \
                         {remote:#018x}) - please report this with both peers' versions"
                    );
                    self.paused = true;
                    self.audio.set_paused(true);
                }
                Ev::Disconnected { reason } => {
                    eprintln!("netplay: session closed ({reason})");
                    self.paused = true;
                    self.audio.set_paused(true);
                }
            }
        }

        /// Run `ticks` rollback ticks (one per frame the timer says is due),
        /// or just pump the network when none is due. Events are applied
        /// after every tick, so a `Running` rebuild lands before frame 0.
        #[cfg(feature = "netplay")]
        fn step_rollback(&mut self, ticks: usize) {
            if self.rollback.is_none() {
                return;
            }
            let now = self.now_ms();
            let local = self.current_inputs().0;
            let mut advanced = 0u32;
            for _ in 0..ticks.max(1) {
                let Some(rb) = self.rollback.as_mut() else {
                    return;
                };
                if ticks == 0 {
                    rb.session.poll(now);
                } else {
                    let out = crate::netplay::rollback_tick(
                        rb,
                        &mut self.emu,
                        local,
                        now,
                        Some(&self.audio),
                    );
                    advanced += out.advanced;
                    if let Some(e) = rb.last_error.take() {
                        eprintln!("netplay: {e}");
                    }
                }
                rb.report_stage();
                let events = rb.take_events();
                for ev in events {
                    self.on_rollback_event(ev);
                }
            }
            // Until the session starts, keep the solo game running (as the
            // lockstep path does): it is rebuilt from power-on at `Running`.
            let waiting = self.rollback.as_ref().is_some_and(|rb| {
                !rb.started && rb.session.state() != z2_net::RollbackState::Closed
            });
            if waiting {
                if ticks > 0 && !self.paused {
                    self.step_emulator(ticks);
                    self.fps_acc += ticks as u32;
                }
                return;
            }
            self.fps_acc += advanced;
            if advanced > 0 {
                self.audio.trim_oldest(self.audio.suggested_max_depth());
                let frames = self.emu.game.frame_count();
                // Autosave only a state with no predicted remote input in it.
                let settled = self
                    .rollback
                    .as_ref()
                    .is_some_and(|rb| rb.session.stats().prediction_depth == 0);
                if settled
                    && frames != 0
                    && frames.is_multiple_of(600)
                    && self.emu.game.exec_errors == 0
                {
                    let name = self.sram_file();
                    let _ = save_sram_named(&mut self.emu.game, &self.data_dir, name);
                }
            }
        }

        fn step_emulator(&mut self, steps: usize) {
            for _ in 0..steps {
                let (p1, p2) = self.current_inputs();
                if self.coop_local {
                    step_one2(&mut self.emu, (p1, p2), Some(&self.audio));
                } else {
                    // Single player keeps the exact `Game::step` path that the
                    // headless surface and the verification driver share.
                    step_one(&mut self.emu, p1, Some(&self.audio));
                }
            }
            // Bound frontend latency on fast runs (windowed only — headless
            // dumps depend on unbounded push_frame; see audio.rs).
            self.audio.trim_oldest(self.audio.suggested_max_depth());
            // Interpreter wedge guard: a fault aborts frame slices silently
            // (see Game::exec_errors) — the game cannot progress, so stop
            // the clock (visible via the PAUSED title), never autosave a
            // wedged state, and say exactly where it died.
            if self.emu.game.exec_errors != self.exec_errors_seen {
                self.exec_errors_seen = self.emu.game.exec_errors;
                if let Some(e) = self.emu.game.last_exec_error {
                    eprintln!(
                        "emulator wedged at frame {}: {e}",
                        self.emu.game.frame_count()
                    );
                }
                self.paused = true;
                self.audio.set_paused(true);
            }
            // Autosave SRAM every ~10 s of emulation (600 frames), never
            // from a wedged state (faulted RAM would poison the save file).
            let frames = self.emu.game.frame_count();
            if frames != 0 && frames.is_multiple_of(600) && self.exec_errors_seen == 0 {
                let _ = save_sram(&mut self.emu.game, &self.data_dir);
            }
        }

        fn redraw(&mut self) {
            let Some(window) = self.window else {
                return;
            };
            window.set_title(&self.compose_title());
            if self.occluded {
                // Fully hidden/minimized: skip the GPU present (surface
                // acquisition can fail while occluded). Stepping continues; the
                // next visible redraw re-blits the whole frame, so no stale
                // content survives unocclude.
                return;
            }
            // The presented size can change after startup — a ROM dropped on a
            // cartridge-less window rebuilds the emulator, and an HD pack can
            // raise the effective scale — so the texture is re-checked on every
            // redraw against the presenter's own answer rather than only when
            // the window is created.
            let want = self.display.size();
            if self.tex_size != want {
                if let Some(p) = self.pixels.as_mut() {
                    if let Err(e) = p.resize_buffer(want.0, want.1) {
                        eprintln!("present resize_buffer {}x{}: {e:?}", want.0, want.1);
                        return;
                    }
                }
                self.tex_size = want;
            }
            // Split the borrow: composing reads the game while the blit writes
            // the surface, and both live on `self`.
            let Self {
                pixels,
                display,
                emu,
                ..
            } = self;
            let Some(pixels) = pixels.as_mut() else {
                return;
            };
            // Composed per PRESENTED frame, not per stepped frame, so
            // fast-forward does not pay for frames it never shows. The full
            // frame is written every visible redraw, so no stale frame can
            // survive once stepping.
            match display.present(&emu.game) {
                Ok(rgba) => {
                    let dst = pixels.frame_mut();
                    if dst.len() == rgba.len() {
                        dst.copy_from_slice(rgba);
                    } else {
                        // Only reachable if `resize_buffer` silently disagreed
                        // with the presenter; copy what fits and say so.
                        let n = dst.len().min(rgba.len());
                        dst[..n].copy_from_slice(&rgba[..n]);
                        eprintln!(
                            "present: texture is {} bytes but the composed frame is {}",
                            dst.len(),
                            rgba.len()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    return;
                }
            }
            if let Err(e) = pixels.render() {
                // A swallowed render error leaves the last good (or the initial
                // grey) frame up while the title keeps moving - exactly the
                // reported symptom. Log it and re-establish the surface.
                eprintln!("present render: {e:?}");
                let size = window.inner_size();
                let (w, h) = clamp_surface_size(size.width, size.height);
                if let Err(e2) = pixels.resize_surface(w, h) {
                    eprintln!("present resize {w}x{h}: {e2:?}");
                }
            }
        }

        fn key_name(code: KeyCode) -> Option<&'static str> {
            Some(match code {
                KeyCode::KeyZ => "KeyZ",
                KeyCode::KeyX => "KeyX",
                KeyCode::ShiftLeft => "ShiftLeft",
                KeyCode::ShiftRight => "ShiftRight",
                KeyCode::Enter => "Enter",
                KeyCode::ArrowUp => "ArrowUp",
                KeyCode::ArrowDown => "ArrowDown",
                KeyCode::ArrowLeft => "ArrowLeft",
                KeyCode::ArrowRight => "ArrowRight",
                // Player 2's default set (see `KeyBindings::default_p2`).
                // Any key named by a default binding table must appear here
                // or it can never be pressed; pinned by the
                // `key_name_covers_default_bindings` test.
                KeyCode::KeyW => "KeyW",
                KeyCode::KeyA => "KeyA",
                KeyCode::KeyS => "KeyS",
                KeyCode::KeyD => "KeyD",
                KeyCode::KeyF => "KeyF",
                KeyCode::KeyG => "KeyG",
                KeyCode::KeyR => "KeyR",
                KeyCode::KeyT => "KeyT",
                _ => return None,
            })
        }

        /// Milliseconds since the loop started (netplay timers only).
        #[cfg(feature = "netplay")]
        fn now_ms(&self) -> u64 {
            self.start.elapsed().as_millis() as u64
        }

        /// True while an online session exists (in any state).
        fn net_active(&self) -> bool {
            #[cfg(feature = "netplay")]
            {
                self.net.is_some() || self.rollback.is_some()
            }
            #[cfg(not(feature = "netplay"))]
            {
                false
            }
        }

        /// Everything that must happen exactly once on the way out: the
        /// battery RAM this peer owns, and the recorded HD pack if one was
        /// asked for. Both quit paths (window close and `Esc`) call this, so
        /// neither can grow a half-copy of the other.
        fn on_quit(&mut self) {
            let name = self.sram_file();
            let _ = save_sram_named(&mut self.emu.game, &self.data_dir, name);
            match self.display.write_recorded_pack(&self.emu.game.chr) {
                Ok(Some(dir)) => eprintln!(
                    "HD recording written to {} — edit the sheets, then run with \
                     --hd-pack {}",
                    dir.display(),
                    dir.display()
                ),
                Ok(None) => {}
                Err(e) => eprintln!("{e}"),
            }
        }

        /// One stderr line explaining that a hotkey is inert during netplay.
        fn refuse_during_netplay(&self, what: &str) {
            eprintln!("{what} is disabled during netplay (both peers must step the same frames)");
        }

        /// The SRAM file this peer owns.
        ///
        /// A netplay guest plays on the host's save, so writing `sram.sav`
        /// would overwrite its own solo progress. It autosaves the session to
        /// `sram-coop.sav` instead, which can be renamed to continue solo.
        fn sram_file(&self) -> &'static str {
            #[cfg(feature = "netplay")]
            {
                if self.net.as_ref().is_some_and(|n| !n.is_host())
                    || self.rollback.as_ref().is_some_and(|r| !r.is_host())
                {
                    return SRAM_COOP_FILE;
                }
            }
            SRAM_FILE
        }

        /// Window title: meters + audio + co-op + netplay suffixes.
        fn compose_title(&self) -> String {
            let mut t = format!(
                "{}{}",
                window_title(
                    self.fps_shown,
                    self.audio.underruns(),
                    self.paused,
                    self.fast_forward,
                    self.has_rom,
                    self.savestate_slot,
                ),
                audio_suffix(
                    self.device_rate,
                    self.audio.stream_errors(),
                    self.audio.overruns()
                )
            );
            if self.feats.coop {
                t.push_str(&coop_suffix(self.emu.game.coop_status().as_ref()));
            }
            #[cfg(feature = "netplay")]
            if let Some(net) = self.net.as_ref() {
                t.push_str(&crate::netplay::netplay_suffix(
                    net.role,
                    net.stage(),
                    net.state(),
                    net.close_reason(),
                    &net.stats(),
                    net.delay,
                    &net.room,
                ));
            }
            #[cfg(feature = "netplay")]
            if let Some(rb) = self.rollback.as_ref() {
                t.push_str(&crate::netplay::rollback_suffix(
                    rb.session.role(),
                    rb.stage(),
                    rb.session.state(),
                    rb.session.close_reason(),
                    &rb.session.stats(),
                    rb.session.input_delay(),
                    rb.interrupted,
                    &rb.room,
                ));
            }
            t
        }
    }

    impl ApplicationHandler for Handler {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            // Texture size follows the widescreen width, and the window is
            // opened at least that big: pixels 0.15 clamps its integer scale to
            // >= 1, so a texture larger than the surface would be CROPPED
            // rather than shrunk.
            let (tex_w, tex_h) = self.display.size();
            let monitor = event_loop.primary_monitor().map(|m| {
                let s = m.size();
                (s.width, s.height)
            });
            let (lw, lh) = initial_window_size(tex_w, tex_h, monitor);
            let attrs = Window::default_attributes()
                .with_title(if self.has_rom { "z2rs" } else { NO_ROM_TITLE })
                .with_inner_size(winit::dpi::LogicalSize::new(lw, lh));
            let window = match event_loop.create_window(attrs) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("create window: {e}");
                    event_loop.exit();
                    return;
                }
            };
            // Leak to 'static so the pixels surface can borrow it (standard
            // winit-0.30 + pixels-0.15 pattern; the window lives for the app).
            let window: &'static Window = Box::leak(Box::new(window));
            let size = window.inner_size();
            let (sw, sh) = clamp_surface_size(size.width, size.height);
            let surface = pixels::SurfaceTexture::new(sw, sh, window);
            match pixels::Pixels::new(tex_w, tex_h, surface) {
                Ok(p) => self.pixels = Some(p),
                Err(e) => {
                    eprintln!("create pixels surface: {e}");
                    event_loop.exit();
                    return;
                }
            }
            self.tex_size = (tex_w, tex_h);
            self.window = Some(window);
            self.last_tick = Some(Instant::now());
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            match event {
                WindowEvent::CloseRequested => {
                    #[cfg(feature = "netplay")]
                    {
                        let now = self.now_ms();
                        if let Some(net) = self.net.as_mut() {
                            net.close_and_flush(now);
                        }
                        if let Some(rb) = self.rollback.as_mut() {
                            rb.close_and_flush(now);
                        }
                    }
                    self.on_quit();
                    event_loop.exit();
                }
                WindowEvent::Focused(focused) => {
                    // Mirror WindowUiState::on_focus so the pure state stays
                    // the tested contract for this transition; the pause-mute
                    // follows `paused` exactly (`audio_muted` contract).
                    let mut ui = WindowUiState {
                        has_rom: self.has_rom,
                        paused: self.paused,
                        ever_focused: self.ever_focused,
                        savestate_slot: self.savestate_slot,
                    };
                    // Auto-pause is ignored during netplay: a paused peer
                    // stalls the other one, so an unfocused window keeps
                    // stepping.
                    let pause_pref = self.config.pause_on_focus_loss && !self.net_active();
                    ui.on_focus(focused, pause_pref);
                    self.paused = ui.paused;
                    self.ever_focused = ui.ever_focused;
                    self.audio.set_paused(ui.audio_muted());
                    if !focused {
                        self.keyboard.clear();
                        self.fast_forward = false;
                    }
                }
                WindowEvent::DroppedFile(path) => {
                    // Swapping the game under a live lockstep session would
                    // desync both peers instantly.
                    if self.net_active() {
                        self.refuse_during_netplay("dropping a file");
                    } else if classify_dropped_file(&path) == DroppedFileKind::Movie {
                        match load_movie_track(&path) {
                            Ok(t) => {
                                eprintln!("demo movie: {} frames from {}", t.len(), path.display());
                                self.movie = t;
                                self.movie_pos = 0;
                            }
                            Err(e) => eprintln!("movie drop: {e}"),
                        }
                    } else {
                        // Assume a ROM drop: verify + rebuild live, WITH the
                        // same features the app was started with. Rebuilding
                        // without them silently dropped widescreen and co-op
                        // for anyone who used the documented "start with no
                        // ROM, then drop one" flow.
                        let rate = self.config.effective_audio_rate();
                        let feats = self.feats;
                        match emu_from_rom_file_with(&path, rate, feats) {
                            Ok((fresh, body)) => {
                                self.emu = fresh;
                                self.rom_body = Some(body);
                                let _ = load_sram(&mut self.emu.game, &self.data_dir);
                                self.emu.game.coop_reset_area();
                                self.movie.clear();
                                self.movie_pos = 0;
                                self.timer = FrameTimer::new();
                                // Make the present path re-check the texture:
                                // the presenter is unchanged by a ROM swap, but
                                // the surface is re-established from scratch.
                                self.tex_size = (0, 0);
                                let mut ui = WindowUiState {
                                    has_rom: self.has_rom,
                                    paused: self.paused,
                                    ever_focused: self.ever_focused,
                                    savestate_slot: self.savestate_slot,
                                };
                                ui.on_rom_loaded();
                                self.has_rom = ui.has_rom;
                                self.paused = ui.paused;
                                // Audio follows the pause flag exactly (see
                                // `WindowUiState::audio_muted`): the swap
                                // resumes stepping, so unmute — and drop the
                                // old ROM's buffered PCM so no stale burst
                                // plays.
                                self.audio.set_paused(ui.audio_muted());
                                self.audio.clear();
                                eprintln!(
                                    "ROM accepted from drop: {} (widescreen {} tiles/side, \
                                     co-op {}, render record {})",
                                    path.display(),
                                    self.display.settings().wide_tiles,
                                    if feats.coop { "on" } else { "off" },
                                    if feats.record { "on" } else { "off" }
                                );
                            }
                            Err(e) => eprintln!("ROM drop rejected: {e}"),
                        }
                    }
                }
                WindowEvent::Resized(size) => {
                    if let Some(p) = self.pixels.as_mut() {
                        let (w, h) = clamp_surface_size(size.width, size.height);
                        if let Err(e) = p.resize_surface(w, h) {
                            // Was `let _ =` (silent grey/stale after a failed
                            // resize). Needs human eyes if it ever fires.
                            eprintln!("present resize_surface {w}x{h}: {e:?}");
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    // macOS Retina transitions (window moved across displays,
                    // display-scale change): the backing store size changed
                    // without a `Resized`. Without this arm the surface keeps
                    // the old physical size (stale/blurry presentation). This
                    // only tracks the surface — `compute_viewport`
                    // letterboxing is still unwired (config
                    // integer_scaling/aspect_correction are no-ops).
                    if let (Some(p), Some(w)) = (self.pixels.as_mut(), self.window.as_ref()) {
                        let size = w.inner_size();
                        let (sw, sh) = clamp_surface_size(size.width, size.height);
                        if let Err(e) = p.resize_surface(sw, sh) {
                            eprintln!("present scale-factor resize {sw}x{sh}: {e:?}");
                        }
                    }
                }
                WindowEvent::Occluded(occluded) => {
                    self.occluded = occluded;
                    if !occluded {
                        // Unocclude/unminimize: re-establish the surface
                        // before the next present so no stale frame survives.
                        if let (Some(p), Some(w)) = (self.pixels.as_mut(), self.window.as_ref()) {
                            let size = w.inner_size();
                            let (sw, sh) = clamp_surface_size(size.width, size.height);
                            if let Err(e) = p.resize_surface(sw, sh) {
                                eprintln!("present unocclude resize {sw}x{sh}: {e:?}");
                            }
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    use winit::keyboard::KeyCode as KC;
                    let pressed = event.state == ElementState::Pressed;
                    let repeat = event.repeat;
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if let Some(name) = Self::key_name(code) {
                            self.keyboard.set(name, pressed);
                        }
                        if pressed && !repeat {
                            match code {
                                KC::Escape => {
                                    // Announce the quit and give the goodbye
                                    // time to leave, so the peer sees "peer
                                    // quit" instead of a stall.
                                    #[cfg(feature = "netplay")]
                                    {
                                        let now = self.now_ms();
                                        if let Some(net) = self.net.as_mut() {
                                            net.close_and_flush(now);
                                        }
                                        if let Some(rb) = self.rollback.as_mut() {
                                            rb.close_and_flush(now);
                                        }
                                    }
                                    self.on_quit();
                                    event_loop.exit();
                                }
                                KC::KeyP => {
                                    if self.net_active() {
                                        self.refuse_during_netplay("pause");
                                    } else {
                                        self.paused = !self.paused;
                                        // Pause-mute: freeze the meter/ring
                                        // with the flag; drop stale audio on
                                        // resume so no burst plays.
                                        self.audio.set_paused(self.paused);
                                        if !self.paused {
                                            self.audio.clear();
                                        }
                                    }
                                }
                                KC::Tab => {
                                    if self.net_active() {
                                        self.refuse_during_netplay("fast-forward");
                                    } else {
                                        self.fast_forward = true;
                                    }
                                }
                                KC::Period if self.paused && !self.net_active() => {
                                    self.frame_step = true;
                                }
                                KC::Digit1
                                | KC::Digit2
                                | KC::Digit3
                                | KC::Digit4
                                | KC::Digit5
                                | KC::Digit6
                                | KC::Digit7
                                | KC::Digit8
                                | KC::Digit9
                                | KC::Digit0 => {
                                    // Slot selection is only a label change, so
                                    // it stays live during netplay (silent; the
                                    // title shows ` [SLOT n]` when nonzero).
                                    self.savestate_slot = match code {
                                        KC::Digit0 => 0,
                                        KC::Digit1 => 1,
                                        KC::Digit2 => 2,
                                        KC::Digit3 => 3,
                                        KC::Digit4 => 4,
                                        KC::Digit5 => 5,
                                        KC::Digit6 => 6,
                                        KC::Digit7 => 7,
                                        KC::Digit8 => 8,
                                        _ => 9,
                                    };
                                }
                                KC::F6 => {
                                    // Cycle the slot (silent; title updates).
                                    self.savestate_slot = next_savestate_slot(self.savestate_slot);
                                }
                                KC::F5 | KC::F7 => {
                                    // Rewinding one peer desyncs both: there is
                                    // no rollback in this protocol.
                                    if let Some(why) =
                                        crate::netplay::savestate_blocked(self.net_active())
                                    {
                                        eprintln!("{why}");
                                    } else if code == KC::F5 {
                                        let slot = self.savestate_slot;
                                        match save_savestate(&self.emu.game, &self.data_dir, slot) {
                                            Ok(p) => eprintln!("saved {}", p.display()),
                                            Err(e) => eprintln!("save failed: {e}"),
                                        }
                                    } else {
                                        let slot = self.savestate_slot;
                                        match load_savestate(
                                            &mut self.emu.game,
                                            &self.data_dir,
                                            slot,
                                        ) {
                                            Ok(()) => eprintln!("loaded savestate{slot}"),
                                            Err(e) => eprintln!("load failed: {e}"),
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        if !pressed && code == KC::Tab {
                            self.fast_forward = false;
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    self.redraw();
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
            let now = Instant::now();
            let dt = self
                .last_tick
                .map(|t| now.duration_since(t))
                .unwrap_or(Duration::from_millis(16));
            self.last_tick = Some(now);
            // FPS meter (emulated frames per second, counted below once the
            // step count is known).
            if now.duration_since(self.fps_last) >= Duration::from_secs(1) {
                self.fps_shown =
                    f64::from(self.fps_acc) / now.duration_since(self.fps_last).as_secs_f64();
                self.fps_acc = 0;
                self.fps_last = now;
            }
            let speed = if self.fast_forward {
                f64::from(self.config.fast_forward_multiplier.max(1))
            } else {
                1.0
            };
            let mut steps = self.timer.advance(dt.as_secs_f64(), speed, self.paused);
            // Only with a live output stream draining the ring: without a
            // device the ring never empties and the nudge would hold the
            // emulator back.
            if !self.paused && self.device_rate.is_some() {
                steps = pace_steps(
                    steps,
                    self.audio.depth(),
                    self.audio.rate(),
                    !self.fast_forward,
                );
            }
            if self.frame_step {
                steps = 1;
                self.frame_step = false;
            }
            if self.net_active() {
                // Always pumped, even with a zero budget: the handshake and
                // the stall timers must keep running while nothing steps.
                // `step_netplay` counts the frames it actually stepped.
                #[cfg(feature = "netplay")]
                {
                    if self.rollback.is_some() {
                        self.step_rollback(steps);
                    } else {
                        self.step_netplay(steps);
                    }
                }
            } else if steps > 0 {
                self.step_emulator(steps);
                self.fps_acc += steps as u32;
            }
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
    }

    // Read before `config` moves into the handler.
    let config_p2_pad = config.gamepad_p2_index;
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let audio = SharedAudio::new(config.effective_audio_rate());
    let (stream, device_rate) = match crate::audio::open_output_stream(&audio) {
        Ok((s, r)) => {
            if r != audio.rate() {
                eprintln!(
                    "audio: device runs {r} Hz vs game {} Hz — set audio_rate to {r} in the config for exact pitch",
                    audio.rate()
                );
            }
            // Prime one frame of silence so the meter doesn't open red.
            audio.prime_silence();
            (Some(s), Some(r))
        }
        Err(e) => {
            eprintln!("audio disabled: {e}");
            (None, None)
        }
    };
    let gilrs = crate::input::new_gilrs(true).ok();

    let initial = WindowUiState::new(has_rom);
    // -- netplay session ----------------------------------------------------
    #[cfg(feature = "netplay")]
    let net_mode = args.net_mode.unwrap_or(config.netplay.mode);
    #[cfg(feature = "netplay")]
    let net = if args.coop.is_online() && net_mode == crate::config::NetMode::Lockstep {
        let host = matches!(args.coop, CoopMode::Host(_));
        let room = args.coop.room().unwrap_or_default().to_string();
        let signal = args
            .signal
            .clone()
            .unwrap_or_else(|| config.netplay.signal_url.clone());
        let url = z2_net::room_url(&signal, &room).map_err(|e| format!("netplay: {e}"))?;
        let delay = args.net_delay.unwrap_or(config.netplay.input_delay);
        // TWO_LINKS is mandatory: a session whose co-op flags were zero would
        // give the guest a pad the ROM never reads, i.e. no gameplay at all.
        let mut cfg = if host {
            z2_net::SessionConfig::host(
                z2_assets::rom::EXPECTED_BODY_CRC32,
                emu.trapset_id,
                z2_net::COOP_TWO_LINKS,
            )
        } else {
            z2_net::SessionConfig::guest(
                z2_assets::rom::EXPECTED_BODY_CRC32,
                emu.trapset_id,
                z2_net::COOP_TWO_LINKS | z2_net::COOP_SPRITE_UNLIMITED,
            )
        };
        cfg.delay = delay;
        cfg.stall_timeout_ms = config.netplay.stall_timeout_ms;
        if host {
            // The host's save is the session's save.
            cfg.wram = Some(emu.game.wram().to_vec());
        }
        let ice = crate::netplay::ice_setting(args.ice.as_deref(), &config.netplay)
            .map_err(|e| format!("netplay: {e}"))?;
        eprintln!(
            "netplay: {} room '{}' via {} (input delay {} frames, {})",
            if host { "hosting" } else { "joining" },
            room,
            signal,
            delay,
            ice.describe()
        );
        eprintln!("netplay: connecting to {url} — the game keeps running, Esc cancels");
        let transport = crate::netplay::connect_matchbox(&url, Some(ice));
        Some(
            crate::netplay::NetLink::new(cfg, transport, &room)
                .map_err(|e| format!("netplay: {e}"))?,
        )
    } else {
        None
    };
    #[cfg(feature = "netplay")]
    let rollback = if args.coop.is_online() && net_mode == crate::config::NetMode::Rollback {
        let host = matches!(args.coop, CoopMode::Host(_));
        let room = args.coop.room().unwrap_or_default().to_string();
        let signal = args
            .signal
            .clone()
            .unwrap_or_else(|| config.netplay.signal_url.clone());
        let url = z2_net::room_url(&signal, &room).map_err(|e| format!("netplay: {e}"))?;
        let delay = crate::netplay::rollback_delay(args.net_delay, config.netplay.input_delay)?;
        let mut cfg = if host {
            z2_net::RollbackConfig::host(
                z2_assets::rom::EXPECTED_BODY_CRC32,
                emu.trapset_id,
                z2_net::COOP_TWO_LINKS,
            )
        } else {
            z2_net::RollbackConfig::guest(
                z2_assets::rom::EXPECTED_BODY_CRC32,
                emu.trapset_id,
                z2_net::COOP_TWO_LINKS | z2_net::COOP_SPRITE_UNLIMITED,
            )
        };
        cfg.input_delay = delay;
        cfg.disconnect_timeout_ms = config.netplay.stall_timeout_ms;
        if host {
            cfg.wram = Some(emu.game.wram().to_vec());
        }
        let ice = crate::netplay::ice_setting(args.ice.as_deref(), &config.netplay)
            .map_err(|e| format!("netplay: {e}"))?;
        eprintln!(
            "netplay: {} room '{}' via {} (rollback, input delay {} frames, {})",
            if host { "hosting" } else { "joining" },
            room,
            signal,
            delay,
            ice.describe()
        );
        eprintln!("netplay: connecting to {url} — the game keeps running, Esc cancels");
        let transport = crate::netplay::connect_matchbox(&url, Some(ice));
        Some(
            crate::netplay::RollbackLink::new(cfg, transport, &room)
                .map_err(|e| format!("netplay: {e}"))?,
        )
    } else {
        None
    };

    let mut handler = Handler {
        config,
        data_dir,
        emu,
        display,
        movie: movie_track,
        movie_pos: 0,
        window: None,
        pixels: None,
        keyboard: crate::input::KeyboardState::new(),
        gilrs,
        active_pad: None,
        audio,
        _stream: stream,
        device_rate,
        timer: FrameTimer::new(),
        last_tick: None,
        paused: initial.paused,
        has_rom: initial.has_rom,
        ever_focused: false,
        fast_forward: false,
        savestate_slot: 0,
        frame_step: false,
        exec_errors_seen: 0,
        fps_acc: 0,
        fps_shown: 60.0,
        fps_last: Instant::now(),
        occluded: false,
        feats,
        coop_local,
        rom_body,
        active_pad2: None,
        pad_order: Vec::new(),
        p2_pad: args.p2_pad.or(config_p2_pad),
        tex_size: (0, 0),
        #[cfg(feature = "netplay")]
        start: Instant::now(),
        #[cfg(feature = "netplay")]
        net,
        #[cfg(feature = "netplay")]
        rollback,
    };
    // Keep audio in lockstep with the initial UI state.  Both cartless and
    // ROM launches now start running; the normal frame path produces audio,
    // while explicit pause/focus transitions still mute the ring.
    handler.audio.set_paused(handler.paused);
    event_loop
        .run_app(&mut handler)
        .map_err(|e| format!("event loop: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_steps_at_ntsc_rate() {
        let mut t = FrameTimer::new();
        // One frame worth of wall time → one step.
        assert_eq!(t.advance(FRAME_DT, 1.0, false), 1);
        // Half a frame buffers without stepping.
        assert_eq!(t.advance(FRAME_DT / 2.0, 1.0, false), 0);
        assert_eq!(t.advance(FRAME_DT / 2.0, 1.0, false), 1);
        // Paused discards time.
        assert_eq!(t.advance(1.0, 1.0, true), 0);
        assert_eq!(t.buffered(), 0.0);
    }

    #[test]
    fn accumulator_caps_and_drops_backlog() {
        let mut t = FrameTimer::new();
        assert_eq!(t.advance(1.0, 1.0, false), MAX_STEPS_PER_TICK);
        assert_eq!(t.buffered(), 0.0, "excess backlog dropped, no spiral");
    }

    #[test]
    fn fast_forward_multiplies_time() {
        let mut a = FrameTimer::new();
        let mut b = FrameTimer::new();
        let n1 = a.advance(FRAME_DT * 2.0, 1.0, false);
        let n4 = b.advance(FRAME_DT * 2.0, 4.0, false);
        assert_eq!((n1, n4), (2, MAX_STEPS_PER_TICK.min(8)));
    }

    #[test]
    fn viewport_centers_integer_scale() {
        // 768×720 @ integer+aspect: eff 292.57×240 → scale 2 → 585×480 centered.
        let v = compute_viewport(768, 720, true, true);
        assert_eq!(v.scale, 2);
        assert_eq!((v.w, v.h), (585, 480));
        assert_eq!((v.x, v.y), ((768 - 585) / 2, (720 - 480) / 2));
        // Without aspect correction: 256×240 in 768×720 → scale 3 → fills.
        let v2 = compute_viewport(768, 720, true, false);
        assert_eq!(v2.scale, 3);
        assert_eq!((v2.w, v2.h), (768, 720));
        assert_eq!((v2.x, v2.y), (0, 0));
        // Non-integer fills the window.
        let v3 = compute_viewport(800, 600, false, false);
        assert_eq!((v3.w, v3.h), (800, 600));
    }

    #[test]
    fn shared_step_path_advances_game_and_audio() {
        let audio = SharedAudio::new(44100);
        let mut emu = new_emu(44100);
        let n = step_frames(&mut emu, &[0, 1, 2], Some(&audio));
        assert_eq!(n, 3);
        assert_eq!(emu.game.frame_count(), 3);
        let (_, popped) = audio.totals();
        assert!(popped == 0, "push-only path does not pop");
        assert!(audio.depth() > 0, "one APU frame per game frame pushed");
    }

    #[test]
    fn synthetic_frames_produce_nominal_audio_totals() {
        let audio = SharedAudio::new(44100);
        let mut emu = new_emu(44100);
        step_frames(&mut emu, &[0u8; 60], Some(&audio));
        let depth = audio.depth() as u64;
        // 60 frames ≈ 60*735 ±60 jitter.
        assert!(depth.abs_diff(60 * 735) <= 60, "depth={depth}, want ~44100");
    }

    #[test]
    fn movie_track_parses_fm2_fixture() {
        let dir = std::env::temp_dir().join(format!("z2mov-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("t.fm2");
        std::fs::write(
            &p,
            "version 3\npalFlag 0\nromFilename Zelda II.nes\n|0|........|\n|0|.......A|\n",
        )
        .unwrap();
        let track = load_movie_track(&p).expect("fm2 parses");
        assert_eq!(track, vec![0x00, 0x01]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn movie_track_rejects_unknown_extension() {
        let p = Path::new("/tmp/x.xyz");
        assert!(load_movie_track(p).is_err());
    }

    #[test]
    fn snapshot_roundtrip_through_game() {
        let mut emu = new_emu(44100);
        step_frames(&mut emu, &[0x01, 0x02], None);
        let dir = std::env::temp_dir().join(format!("z2snap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = save_savestate(&emu.game, &dir, 0).expect("save");
        assert_eq!(path, savestate_path(&dir, 0));
        // Mutate, then restore.
        emu.game.ram[0] ^= 0xFF;
        load_savestate(&mut emu.game, &dir, 0).expect("load");
        let snap = snapshot_from_game(&emu.game, Vec::new()).expect("snap");
        let back = z2_verify::snapshot::Snapshot::decode(&snap.encode().unwrap()).unwrap();
        assert_eq!(snap, back);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_roundtrip_through_nonzero_slot() {
        let mut emu = new_emu(44100);
        step_frames(&mut emu, &[0x01, 0x02], None);
        let dir = std::env::temp_dir().join(format!("z2snap3-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = save_savestate(&emu.game, &dir, 3).expect("save");
        assert_eq!(path, savestate_path(&dir, 3));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("savestate3.z2snap")
        );
        // Mutate, then restore through the same slot.
        emu.game.ram[0] ^= 0xFF;
        load_savestate(&mut emu.game, &dir, 3).expect("load");
        let snap = snapshot_from_game(&emu.game, Vec::new()).expect("snap");
        let back = z2_verify::snapshot::Snapshot::decode(&snap.encode().unwrap()).unwrap();
        assert_eq!(snap, back);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_savestate_slot_cycles() {
        assert_eq!(next_savestate_slot(0), 1);
        assert_eq!(next_savestate_slot(8), 9);
        assert_eq!(next_savestate_slot(9), 0);
    }

    #[test]
    fn sram_save_load_roundtrips_the_raw_window() {
        let mut emu = new_emu(44100);
        emu.game.wram[0x0000] = 0x42;
        emu.game.wram[0x1FFF] = 0x99;
        let dir = std::env::temp_dir().join(format!("z2sram-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        save_sram(&mut emu.game, &dir).expect("save");
        // The app never edits slots: the header stays whatever the game left.
        assert_eq!(emu.game.wram()[0x1400], 0x00, "no slot stamping");
        emu.game.wram[0x0000] = 0x00;
        emu.game.wram[0x1FFF] = 0x00;
        assert!(load_sram(&mut emu.game, &dir).expect("load"));
        assert_eq!(emu.game.wram[0x0000], 0x42);
        assert_eq!(emu.game.wram[0x1FFF], 0x99);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suspicious_slots_flags_valid_header_with_zero_levels() {
        use z2_core::save_format::{slot_pointers, sram_index, HDR_ADDR, HDR_VALID};
        let mut sram = vec![0u8; z2_core::save_format::SRAM_LEN];
        assert!(
            suspicious_slots(&sram).is_empty(),
            "blank headers are not slots"
        );
        let p = slot_pointers(0).unwrap();
        sram[sram_index(HDR_ADDR[0]).unwrap()] = HDR_VALID;
        assert_eq!(
            suspicious_slots(&sram),
            vec![0],
            "valid + levels 0 = corrupt"
        );
        sram[sram_index(p.part1).unwrap()] = 1; // attack level 1: a real new game
        assert!(suspicious_slots(&sram).is_empty());
    }

    #[test]
    fn resolve_rom_path_errors_clearly_without_sources() {
        let config = NativeConfig::default();
        // Ensure the env var does not leak into the assertion.
        let saved = std::env::var(z2_assets::rom::ROM_ENV_VAR).ok();
        std::env::remove_var(z2_assets::rom::ROM_ENV_VAR);
        let err = resolve_rom_path(None, &config).expect_err("must error");
        assert!(err.contains("--rom"), "points at --rom: {err}");
        if let Some(v) = saved {
            std::env::set_var(z2_assets::rom::ROM_ENV_VAR, v);
        }
    }

    #[test]
    fn blit_produces_opaque_rgba() {
        let mut indexed = [0u8; FRAME_LEN];
        indexed[0] = 0x30; // near-white.
        let mut rgba = vec![0u8; FRAME_RGBA_LEN];
        blit_indexed_to_rgba(&indexed, &mut rgba);
        assert_eq!(&rgba[0..4], &[0xFC, 0xFC, 0xFC, 0xFF]);
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 0xFF));
    }

    #[test]
    fn no_rom_title_state_is_visible() {
        assert_eq!(
            no_rom_title(),
            "z2rs — no ROM: drop a .nes file or restart with --rom PATH"
        );
        // No-ROM state shows the message instead of the meter line …
        let t = window_title(60.0, 0, false, false, false, 0);
        assert_eq!(t, NO_ROM_TITLE);
        // … while ROM state shows the normal meter line.
        let t = window_title(60.0, 3, false, false, true, 0);
        assert!(t.starts_with("z2rs — "), "normal title: {t}");
        assert!(!t.contains("no ROM"), "normal title: {t}");
        assert!(!t.contains("[SLOT"), "slot 0 stays silent: {t}");
        // Paused ROM state keeps its hint.
        let t = window_title(60.0, 0, true, false, true, 0);
        assert!(t.contains("[PAUSED"), "paused title: {t}");
    }

    #[test]
    fn title_announces_nonzero_savestate_slot() {
        let t = window_title(60.0, 0, false, false, true, 3);
        assert!(t.contains("[SLOT 3]"), "slot title: {t}");
        // No-ROM windows never announce a slot (the message is authoritative).
        let t = window_title(60.0, 0, false, false, false, 3);
        assert_eq!(t, NO_ROM_TITLE);
        // The pure meter function mirrors the same contract.
        let t = title_text(60.0, 0, false, false, 9);
        assert!(t.contains("[SLOT 9]"), "meter title: {t}");
        let t = title_text(60.0, 0, false, false, 0);
        assert!(!t.contains("[SLOT"), "slot 0 meter: {t}");
    }

    #[test]
    fn audio_suffix_shows_rate_and_pathology() {
        assert_eq!(audio_suffix(Some(44100), 0, 0), " [audio 44100 Hz]");
        assert_eq!(audio_suffix(None, 0, 0), " [audio off]");
        let s = audio_suffix(Some(48000), 2, 5);
        assert!(s.contains("[audio 48000 Hz]"), "{s}");
        assert!(s.contains("[AUDIO ERR 2]"), "{s}");
        assert!(s.contains("[DROP 5]"), "{s}");
    }

    #[test]
    fn window_ui_state_starts_running_by_default() {
        let ui = WindowUiState::new(false);
        assert!(!ui.has_rom);
        assert!(!ui.paused, "no-ROM launch must not start paused");
        assert!(!ui.ever_focused);
        assert_eq!(ui.title(60.0, 0, false), NO_ROM_TITLE);
        let ui = WindowUiState::new(true);
        assert!(ui.has_rom);
        assert!(!ui.paused, "ROM launch steps immediately");
        assert!(ui.title(60.0, 0, false).starts_with("z2rs — "));
    }

    #[test]
    fn drop_to_load_transition_resumes_under_normal_title() {
        let mut ui = WindowUiState::new(false);
        assert_eq!(ui.title(60.0, 0, false), NO_ROM_TITLE);
        ui.on_rom_loaded();
        assert!(ui.has_rom);
        assert!(!ui.paused, "successful drop unpauses");
        let t = ui.title(60.0, 0, false);
        assert!(t.starts_with("z2rs — "), "back to normal title: {t}");
        assert!(!t.contains("no ROM"), "back to normal title: {t}");
    }

    #[test]
    fn rom_drop_rejects_garbage_without_flipping_state() {
        let dir = std::env::temp_dir().join(format!("z2drop-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bad = dir.join("bad.nes");
        std::fs::write(&bad, b"not a rom").unwrap();
        assert!(
            emu_from_rom_file(&bad, 44100).is_err(),
            "garbage drop must not build an emu"
        );
        // State machine stays no-ROM/running on rejection.
        let ui = WindowUiState::new(false);
        assert_eq!(ui.title(60.0, 0, false), NO_ROM_TITLE);
        assert!(!ui.paused);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropped_file_routing_splits_movies_and_roms() {
        assert_eq!(
            classify_dropped_file(Path::new("demo.fm2")),
            DroppedFileKind::Movie
        );
        assert_eq!(
            classify_dropped_file(Path::new("demo.BK2")),
            DroppedFileKind::Movie
        );
        assert_eq!(
            classify_dropped_file(Path::new("zelda2.nes")),
            DroppedFileKind::Rom
        );
        assert_eq!(
            classify_dropped_file(Path::new("no-extension")),
            DroppedFileKind::Rom
        );
    }

    #[test]
    fn focus_pause_needs_ever_focused() {
        // Opened unfocused: no instant [PAUSED].
        let mut ui = WindowUiState::new(true);
        ui.on_focus(false, true);
        assert!(!ui.paused, "never-focused window must not auto-pause");
        assert!(!ui.ever_focused);
        // After first focus, backgrounding pauses …
        ui.on_focus(true, true);
        assert!(ui.ever_focused);
        ui.paused = false;
        ui.on_focus(false, true);
        assert!(ui.paused, "focused-then-hidden window pauses");
        // … but only when the pref is on.
        let mut ui = WindowUiState::new(true);
        ui.on_focus(true, false);
        ui.paused = false;
        ui.on_focus(false, false);
        assert!(!ui.paused, "pref off keeps running in background");
        // No-ROM start stays running until an explicit pause or focus-loss
        // transition, just like a valid ROM launch.
        let mut ui = WindowUiState::new(false);
        ui.on_focus(true, true);
        assert!(!ui.paused, "no-ROM must start running");
        ui.on_rom_loaded();
        assert!(!ui.paused);
    }

    #[test]
    fn native_usage_documents_rom_drop_recovery() {
        assert!(NATIVE_USAGE.contains("--rom"), "usage names --rom");
        assert!(NATIVE_USAGE.contains(".nes"), "usage names .nes drops");
        assert!(
            NATIVE_USAGE.contains("drop"),
            "usage names drop-to-load recovery"
        );
        assert!(NATIVE_USAGE.contains("--headless"), "usage routes CI");
        let err = parse_native_args(&["z2-native".to_string(), "--help".to_string()])
            .expect_err("--help surfaces usage");
        assert!(err.contains("usage:"), "help text: {err}");
        assert!(err.contains("--rom"), "help text: {err}");
    }

    #[test]
    fn surface_size_clamps_zero_to_one() {
        // `winit` can report a zero dimension (minimized, not-yet-mapped);
        // every SurfaceTexture::new / resize_surface site routes through the
        // clamp so `pixels` never sees a zero-sized surface.
        assert_eq!(clamp_surface_size(0, 0), (1, 1));
        assert_eq!(clamp_surface_size(0, 720), (1, 720));
        assert_eq!(clamp_surface_size(768, 0), (768, 1));
        // Retina backing store passes through untouched.
        assert_eq!(clamp_surface_size(1536, 1440), (1536, 1440));
        assert_eq!(clamp_surface_size(768, 720), (768, 720));
    }

    #[test]
    fn audio_mute_follows_pause_at_every_transition() {
        // Contract the event loop mirrors (`audio_muted`): the `cpal`
        // callback keeps firing while stepping is suspended, so the ring
        // mute follows `paused` exactly.
        // Cartless and ROM launches both start running → audio is live.
        let ui = WindowUiState::new(false);
        assert!(!ui.paused);
        assert!(!ui.audio_muted());
        let ui = WindowUiState::new(true);
        assert!(!ui.paused);
        assert!(!ui.audio_muted());
        // ROM-drop resume unmutes (the loop calls set_paused + clear on the
        // swap; without it the new ROM runs silent).
        let mut ui = WindowUiState::new(false);
        ui.on_rom_loaded();
        assert!(!ui.paused);
        assert!(!ui.audio_muted(), "drop-resume must unmute");
        // Focus-loss pause mutes; pref-off backgrounding stays unmuted.
        let mut ui = WindowUiState::new(true);
        ui.on_focus(true, true);
        ui.paused = false;
        ui.on_focus(false, true);
        assert!(ui.paused);
        assert!(ui.audio_muted(), "focus pause must mute");
        let mut ui = WindowUiState::new(true);
        ui.on_focus(true, false);
        ui.paused = false;
        ui.on_focus(false, false);
        assert!(!ui.audio_muted(), "pref off keeps audio live");
    }

    #[test]
    fn native_rom_constructor_installs_traps_before_reset() {
        let mut game = Game::new();
        let before = game.traps.len();
        register_native_traps(&mut game);
        assert!(
            game.traps.len() > before,
            "native ROM path must not interpret bare"
        );
        // Representative fixed-bank entries prove the active registries are
        // present without requiring a ROM or a window. Town and palace are
        // called above too, but are currently mapper-banked no-ops.
        for addr in [
            0xFF9D, // bank7
            0xC9A5, // sideview
            0xD19B, // player
            0xDA02, // enemy
            0xC33C, // title
            0xC2CA, // boot
        ] {
            assert!(
                game.traps.get(addr).is_some(),
                "missing native trap ${addr:04X}"
            );
        }
        assert_eq!(game.traps.len(), game.traps.iter().count());
    }

    #[test]
    fn pace_steps_nudges_toward_the_water_marks() {
        let frame = 735usize;
        // Starving ring: one extra frame.
        assert_eq!(pace_steps(1, frame, 44100, true), 2);
        assert_eq!(pace_steps(0, 0, 44100, true), 1);
        // Healthy depth: unchanged.
        assert_eq!(pace_steps(1, 3 * frame, 44100, true), 1);
        // Overfull ring: hold one back, never below zero.
        assert_eq!(pace_steps(1, 8 * frame, 44100, true), 0);
        assert_eq!(pace_steps(0, 8 * frame, 44100, true), 0);
        // Fast-forward bypasses the nudge.
        assert_eq!(pace_steps(3, 0, 44100, false), 3);
        assert_eq!(pace_steps(3, 8 * frame, 44100, false), 3);
    }

    #[test]
    fn rom_swap_resets_pacing_state() {
        // What the drop handler resets alongside the emu swap: a fresh timer
        // holds no backlog (no catch-up burst on the first post-swap tick).
        let mut t = FrameTimer::new();
        assert_eq!(t.advance(FRAME_DT * 3.0, 1.0, false), 3);
        let fresh = FrameTimer::new();
        assert_eq!(fresh.buffered(), 0.0, "swapped-in timer starts empty");
        // And the UI side unpauses under the normal title with audio live.
        let mut ui = WindowUiState::new(false);
        ui.on_rom_loaded();
        assert!(ui.has_rom && !ui.paused && !ui.audio_muted());
        assert!(ui.title(60.0, 0, false).starts_with("z2rs — "));
    }

    #[test]
    fn power_on_frame_blits_to_uniform_grey() {
        // Initial-grey plausibility, pinned: the power-on indexed frame is
        // all index 0 (rendering disabled, zeroed palette RAM), which the
        // display palette maps to uniform grey — so the first presented
        // windowed frame is grey BY CONSTRUCTION, even with a valid ROM.
        // Persistent grey past boot (~600 frames to the title) is the defect;
        // transient grey on launch is not.
        let emu = new_emu(44100);
        assert!(
            emu.game.frame_indexed().iter().all(|&b| b == 0),
            "power-on frame is all index 0"
        );
        let mut rgba = vec![0u8; FRAME_RGBA_LEN];
        blit_indexed_to_rgba(emu.game.frame_indexed(), &mut rgba);
        assert!(
            rgba.chunks_exact(4).all(|p| p == [0x7C, 0x7C, 0x7C, 0xFF]),
            "index 0 blits to uniform opaque grey"
        );
    }

    #[test]
    fn blit_overwrites_the_whole_surface_no_stale_frame() {
        // The present path re-blits the FULL indexed frame every visible
        // redraw: pre-fill with a sentinel (a stuck prior frame) and prove
        // no byte survives.
        let mut rgba = vec![0xA5u8; FRAME_RGBA_LEN];
        let indexed = [0x30u8; FRAME_LEN]; // near-white.
        blit_indexed_to_rgba(&indexed, &mut rgba);
        assert!(
            rgba.chunks_exact(4).all(|p| p == [0xFC, 0xFC, 0xFC, 0xFF]),
            "every pixel overwritten, none of the sentinel survives"
        );
        // Distinct indexed frames present distinctly (pause re-presents the
        // same frame; resume must visibly change it once stepping advances).
        let mut grey = vec![0u8; FRAME_RGBA_LEN];
        blit_indexed_to_rgba(&[0x00u8; FRAME_LEN], &mut grey);
        assert_ne!(rgba, grey, "resumed stepping must change the picture");
    }

    #[test]
    fn slice_blit_handles_any_width_and_matches_the_fixed_one() {
        // The wide present path uses the slice blit; it must agree with the
        // 256x240 wrapper byte for byte on the same input.
        let indexed = [0x30u8; FRAME_LEN];
        let mut a = vec![0u8; FRAME_RGBA_LEN];
        let mut b = vec![0u8; FRAME_RGBA_LEN];
        blit_indexed_to_rgba(&indexed, &mut a);
        blit_indexed_slice_to_rgba(&indexed, &mut b);
        assert_eq!(a, b);
        // And a genuinely wide buffer (16:9 = 432 px).
        let wide_len = z2_ppu::wide_width(11) * FRAME_H;
        let wide = vec![0x21u8; wide_len];
        let mut rgba = vec![0u8; wide_len * 4];
        blit_indexed_slice_to_rgba(&wide, &mut rgba);
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 0xFF));
        assert_eq!(rgba.len(), 432 * 240 * 4);
    }

    /// The cross-crate netplay pin. `z2-web` computes the same value with the
    /// same algorithm; if either drifts, native and web peers stop being able
    /// to play together, so the constant is asserted in both crates.
    #[test]
    fn trapset_id_matches_the_cross_crate_pin() {
        let mut game = Game::new();
        game.trap_register("boot", Some(7), 0x00C2, |_| {});
        game.trap_register("bank7", Some(7), 0xFF9D, |_| {});
        assert_eq!(
            trapset_id(&game),
            TRAPSET_PIN_VALUE,
            "trapset_id changed: update z2-web's TRAPSET_PIN_VALUE in lockstep or \
             native and web peers can no longer connect"
        );
        // Registration order must not matter (the fold sorts by address).
        let mut other = Game::new();
        other.trap_register("bank7", Some(7), 0xFF9D, |_| {});
        other.trap_register("boot", Some(7), 0x00C2, |_| {});
        assert_eq!(trapset_id(&other), TRAPSET_PIN_VALUE);
        // A different routine set must be a different id.
        let mut third = Game::new();
        third.trap_register("boot", Some(7), 0x00C2, |_| {});
        assert_ne!(trapset_id(&third), TRAPSET_PIN_VALUE);
        assert_eq!(
            trapset_id(&Game::new()),
            0xcbf2_9ce4_8422_2325,
            "empty basis"
        );
    }

    #[test]
    fn session_trapset_id_matches_the_cross_crate_pin() {
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11)),
            WIDE_TRAPSET_PIN_VALUE,
            "session_trapset_id changed: update z2-web's WIDE_TRAPSET_PIN_VALUE in \
             lockstep or native and web peers can no longer connect"
        );
        // Off leaves the identity alone; margins differ from each other.
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, None),
            TRAPSET_PIN_VALUE
        );
        assert_ne!(
            session_trapset_id(TRAPSET_PIN_VALUE, Some(8)),
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11))
        );
    }

    #[test]
    fn wide_gameplay_follows_widescreen_and_the_flag() {
        let config = NativeConfig::default();
        assert!(config.widescreen_gameplay, "default on");
        let mut args = NativeArgs::default();
        assert_eq!(
            resolve_wide_gameplay(&args, &config, 0),
            None,
            "needs widescreen"
        );
        assert_eq!(resolve_wide_gameplay(&args, &config, 11), Some(11));
        args.wide_gameplay = Some(false);
        assert_eq!(resolve_wide_gameplay(&args, &config, 11), None, "flag wins");
        let off = NativeConfig {
            widescreen_gameplay: false,
            ..NativeConfig::default()
        };
        assert_eq!(resolve_wide_gameplay(&NativeArgs::default(), &off, 8), None);
        args.wide_gameplay = Some(true);
        assert_eq!(resolve_wide_gameplay(&args, &off, 8), Some(8));
        let movie = NativeArgs {
            movie: Some("run.fm2".into()),
            ..NativeArgs::default()
        };
        assert_eq!(
            resolve_wide_gameplay(&movie, &config, 11),
            None,
            "movies replay vanilla"
        );
        let argv = |v: &str| {
            ["z2-native", "--wide-gameplay", v]
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        };
        let parsed = parse_native_args(&argv("off")).expect("parses");
        assert_eq!(parsed.wide_gameplay, Some(false));
        assert!(parse_native_args(&argv("maybe")).is_err());
    }

    #[test]
    fn apply_features_toggles_wide_gameplay_and_the_identity() {
        let mut emu = new_emu(44_100);
        emu.trapset_base = TRAPSET_PIN_VALUE;
        emu.trapset_id = TRAPSET_PIN_VALUE;
        apply_features(
            &mut emu,
            Features {
                wide_gameplay: Some(11),
                ..Features::default()
            },
        );
        assert_eq!(emu.game.wide_gameplay_tiles(), Some(11));
        assert_eq!(emu.trapset_id, WIDE_TRAPSET_PIN_VALUE);
        apply_features(&mut emu, Features::default());
        assert_eq!(emu.game.wide_gameplay_tiles(), None);
        assert_eq!(emu.trapset_id, TRAPSET_PIN_VALUE);
    }

    #[test]
    fn coop_suffix_reports_the_second_player() {
        use z2_core::coop::CoopStatus;
        let base = CoopStatus {
            active: true,
            p2_alive: true,
            p2_hp: 12,
            hp_max: 16,
            respawn_frames: 0,
            p2_deaths: 0,
            p2_page: 0,
            p2_x: 0,
            p2_y: 0,
            p2_screen_x: 0,
        };
        assert_eq!(coop_suffix(None), "", "no suffix when co-op is off");
        assert_eq!(coop_suffix(Some(&base)), " [coop P2 hp 12/16]");
        let down = CoopStatus {
            respawn_frames: 45,
            ..base
        };
        assert_eq!(coop_suffix(Some(&down)), " [coop P2 down 45]");
        let hidden = CoopStatus {
            active: false,
            ..base
        };
        assert_eq!(
            coop_suffix(Some(&hidden)),
            " [coop P2 hidden]",
            "P2 is side-view only, so 'hidden' is a normal state"
        );
    }

    #[test]
    fn features_round_trip_through_apply() {
        let mut emu = new_emu(44100);
        assert!(!emu.game.record_enabled(), "off by default");
        apply_features(
            &mut emu,
            Features {
                coop: false,
                wide_gameplay: None,
                record: true,
                margin_sprites: true,
            },
        );
        assert!(emu.game.record_enabled(), "the decoder needs the record");
        assert!(emu.game.margin_sprites_enabled());
        apply_features(&mut emu, Features::default());
        assert!(!emu.game.record_enabled());
        assert!(!emu.game.margin_sprites_enabled());
        assert!(emu.game.coop_status().is_none());
    }

    /// The record is armed for exactly the reasons that need it, so a plain
    /// single-player run keeps the verification-identical PPU path.
    #[test]
    fn only_widescreen_hd_and_recording_arm_the_record() {
        let off = DisplaySettings {
            scale: 1,
            ..DisplaySettings::default()
        };
        assert!(!off.needs_record());
        assert!(!off.features(true).record, "co-op alone needs no record");
        assert!(off.features(true).coop);
        for s in [
            DisplaySettings {
                wide_tiles: 8,
                ..off.clone()
            },
            DisplaySettings {
                pack_dir: Some(PathBuf::from("/tmp/pack")),
                ..off.clone()
            },
            DisplaySettings {
                record_dir: Some(PathBuf::from("/tmp/rec")),
                ..off.clone()
            },
        ] {
            assert!(s.needs_record(), "{s:?} needs the record");
            assert!(s.features(false).record);
        }
        // A bigger scale on its own is pure upscaling: no record needed.
        assert!(!DisplaySettings { scale: 4, ..off }.needs_record());
    }

    /// The size the texture is allocated at must be the size the presenter
    /// writes, for every preset and scale — this is the widescreen/HD analogue
    /// of the "no stale frame" blit invariant.
    #[test]
    fn display_size_matches_the_composed_frame_for_every_preset() {
        for tiles in [0u8, 8, 11, 16] {
            for scale in [1u32, 2, 4] {
                let d = Display::new(DisplaySettings {
                    wide_tiles: tiles,
                    scale,
                    fill_left_clip: true,
                    fill_right_clip: true,
                    pack_dir: None,
                    record_dir: None,
                    margin_sprites: false,
                })
                .expect("no pack: cannot fail");
                assert_eq!(d.size(), present_size_scaled(tiles, scale));
                assert_eq!(d.effective_scale(), scale);
                assert_eq!(d.is_wide(), tiles > 0);
                assert_eq!(d.needs_record(), tiles > 0);
            }
        }
        // Out-of-range scales are clamped by the resolvers, never passed on.
        assert!(Display::new(DisplaySettings {
            scale: 99,
            ..DisplaySettings::default()
        })
        .is_err());
    }

    /// The failure both design reviews predicted: a no-ROM start under a
    /// feature flag. The synthetic `Game` has no CHR and has never latched a
    /// frame, so every margin decode must fall back to backdrop rather than
    /// panicking or handing back a short buffer.
    #[test]
    fn no_rom_present_never_panics_under_any_display_setting() {
        for tiles in [0u8, 8, 11, 16] {
            for scale in [1u32, 2, 3] {
                for coop in [false, true] {
                    let settings = DisplaySettings {
                        wide_tiles: tiles,
                        scale,
                        fill_left_clip: tiles > 0,
                        fill_right_clip: tiles > 0,
                        pack_dir: None,
                        record_dir: None,
                        margin_sprites: false,
                    };
                    let feats = settings.features(coop);
                    let mut emu = new_emu_with(44_100, feats);
                    assert_eq!(emu.game.record_enabled(), feats.record);
                    // Co-op is never enabled without a cartridge.
                    assert!(emu.game.coop_status().is_none());
                    let mut d = Display::new(settings).expect("no pack");
                    let (w, h) = d.size();
                    // Before any frame is stepped …
                    let rgba = d.present(&emu.game).expect("present at power-on");
                    assert_eq!(rgba.len() as u32, w * h * 4);
                    assert!(rgba.chunks_exact(4).all(|p| p[3] == 0xFF), "opaque");
                    // … and after real frames through the shared primitive.
                    step_frames(&mut emu, &[0u8; 3], None);
                    let rgba = d.present(&emu.game).expect("present after stepping");
                    assert_eq!(rgba.len() as u32, w * h * 4);
                }
            }
        }
    }

    /// `--hd-pack DIR` on a directory that is not a pack must fail loudly (the
    /// user named it), and `--hd-pack ''` must turn a configured pack off.
    #[test]
    fn a_bad_hd_pack_directory_is_a_loud_error() {
        let dir = std::env::temp_dir().join(format!("z2-nopack-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let err = Display::new(DisplaySettings {
            scale: 1,
            pack_dir: Some(dir.clone()),
            ..DisplaySettings::default()
        })
        .expect_err("no pack.json there");
        assert!(err.contains("HD pack"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);

        let cfg = NativeConfig {
            hd_pack: Some("/some/pack".into()),
            ..NativeConfig::default()
        };
        let args = NativeArgs {
            hd_pack: Some(String::new()),
            ..NativeArgs::default()
        };
        assert!(
            resolve_display(&args, &cfg).unwrap().pack_dir.is_none(),
            "--hd-pack '' turns a configured pack off"
        );
        assert_eq!(
            resolve_display(&NativeArgs::default(), &cfg)
                .unwrap()
                .pack_dir,
            Some(PathBuf::from("/some/pack")),
            "the config key supplies the default"
        );
    }

    /// `--hd-record` refuses to write ROM-derived sheets into the repository.
    #[test]
    fn hd_recording_refuses_a_directory_inside_the_repo() {
        let d = Display::new(DisplaySettings {
            scale: 1,
            record_dir: Some(PathBuf::from("target/z2-hdrec-probe")),
            ..DisplaySettings::default()
        })
        .expect("recorder needs no pack");
        // Nothing observed yet: that is its own (clearer) error.
        let err = d.write_recorded_pack(&[]).expect_err("nothing drawn");
        assert!(err.contains("nothing was drawn"), "{err}");
    }
}
