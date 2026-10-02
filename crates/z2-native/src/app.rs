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
//! # Lifecycle (desktop and Android)
//!
//! [`run_windowed`] builds its own winit event loop; [`run_windowed_with`]
//! takes one pre-built by the host (Android builds it with
//! `EventLoopBuilderExtAndroid::with_android_app`). The window, the `pixels`
//! surface and the viewport renderer exist only between `resumed` and
//! `suspended`: Android destroys the native window on every `suspended`
//! (app backgrounded, screen off) and hands a new one to the next `resumed`,
//! so the loop drops all three there, saves SRAM (the process may be killed
//! any time after `onStop`), stops stepping and silences audio, and rebuilds
//! them on the next `resumed`. Desktop platforms never send `suspended`, so
//! the desktop path is the old single `resumed` → run-until-quit.
//!
//! # Input latency note
//!
//! Keyboard + gamepad are polled once per emulated frame, immediately before
//! `Game::step` (see [`crate::input`]). Worst-case added latency is one
//! frame (~16.64 ms) plus OS/display queue depth.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use z2_core::enh::{DisplayEnh, Enhancements};
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

/// Audio-depth water-mark decision (pure, not rate limited).
///
/// Below [`PACE_LOW_FRAMES`] nominal frames of buffered game audio, one extra
/// frame is proposed; above [`PACE_HIGH_FRAMES`], one is held back. Only at
/// normal speed (fast-forward and pause bypass it). The loop never applies
/// this directly: a ring that drains faster than the emulator fills it would
/// then set the game speed. [`AudioPacer`] rate limits it.
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
/// Minimum wall time between two [`AudioPacer`] nudges (seconds). One frame
/// per second caps the audio's pull on game speed at about 1.7% (1 of ~60
/// frames/s), however fast or slow the device drains.
pub const PACE_NUDGE_INTERVAL_SECS: f64 = 1.0;

/// Audio as a *soft* sync on top of the wall-clock [`FrameTimer`].
///
/// The timer alone keeps emulation at [`NTSC_HZ`]. After a main-thread stall
/// the ring can sit low (or high), so the pacer lets [`pace_steps`] add or
/// hold back one frame, but at most once per [`PACE_NUDGE_INTERVAL_SECS`] of
/// wall time.
///
/// The old loop applied the nudge on every OS tick. With `ControlFlow::Poll`
/// that is hundreds to thousands of ticks per second, so a ring that drained
/// faster than it filled (a Windows device at 96/192 kHz, or 48 kHz under a
/// 44.1 kHz game, before the callback resampled) got a frame added on every
/// tick: the game ran as fast as the device drained, 2-4x at 192 kHz. The
/// rate limit keeps that from coming back whatever the audio path does.
#[derive(Debug, Clone, Default)]
pub struct AudioPacer {
    /// Wall time left before the next nudge is allowed.
    cooldown: f64,
}

impl AudioPacer {
    /// Pacer ready to nudge immediately (startup refill).
    #[must_use]
    pub fn new() -> Self {
        Self { cooldown: 0.0 }
    }

    /// Apply the rate-limited nudge to this tick's `steps` after `dt_secs` of
    /// wall time. Returns the adjusted step count.
    pub fn apply(
        &mut self,
        steps: usize,
        dt_secs: f64,
        depth_samples: usize,
        rate: u32,
        normal_speed: bool,
    ) -> usize {
        self.cooldown = (self.cooldown - dt_secs.max(0.0)).max(0.0);
        if self.cooldown > 0.0 {
            return steps;
        }
        let nudged = pace_steps(steps, depth_samples, rate, normal_speed);
        if nudged != steps {
            self.cooldown = PACE_NUDGE_INTERVAL_SECS;
        }
        nudged
    }
}

// ---------------------------------------------------------------------------
// Viewport / scaling
// ---------------------------------------------------------------------------

/// How the presented texture is scaled onto the window / fullscreen surface.
///
/// `pixels` 0.15's own renderer only ever scales by a whole number (and never
/// below 1x), which on the 2560x1600 and 1920x1080 screens players actually
/// have left thick black borders on every side and cropped any texture larger
/// than the window. The windowed loop therefore draws the texture itself
/// ([`crate::gpu_present`]) into the rectangle [`compute_viewport`] returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScaleMode {
    /// Largest aspect-preserving size that fits (fractional allowed), so the
    /// picture fills the screen height. The default.
    #[default]
    Fit,
    /// Whole-number multiples only (sharpest pixels, may leave borders).
    /// Still shrinks — never crops — when the surface is smaller than 1x.
    Integer,
}

impl ScaleMode {
    /// Parse `fit` / `integer` (the config key and `--scale-mode` values).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fit" | "fill" => Some(Self::Fit),
            "integer" | "int" => Some(Self::Integer),
            _ => None,
        }
    }

    /// The flag / config spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fit => "fit",
            Self::Integer => "integer",
        }
    }
}

/// How much of a widescreen frame's outer edge, per side and in unscaled
/// frame pixels, [`ScaleMode::Fit`] may trim so the picture fills the screen
/// height instead of leaving thin bars. One tile: enough for the 16:9 preset
/// (432 px wide, 1.8:1) to cover a 16:9 display (needs 2.67 px per side), and
/// only ever margin art — the original 256-px NES picture is never trimmed.
pub const FILL_CROP_MAX_PX: u32 = 8;

/// Side trim allowance in **texture** texels for [`compute_viewport`]:
/// [`FILL_CROP_MAX_PX`] times the HD output scale when widescreen margins are
/// on, 0 (never trim) for the plain 4:3 picture.
#[must_use]
pub fn fill_crop_texels(wide_tiles: u8, hd_scale: u32) -> u32 {
    if wide_tiles == 0 {
        0
    } else {
        FILL_CROP_MAX_PX.min(u32::from(wide_tiles) * 8) * hd_scale.max(1)
    }
}

/// Where the texture lands on the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Left edge of the drawn rectangle (surface pixels).
    pub x: u32,
    /// Top edge of the drawn rectangle (surface pixels).
    pub y: u32,
    /// Drawn width (surface pixels, never more than the surface).
    pub w: u32,
    /// Drawn height (surface pixels, never more than the surface).
    pub h: u32,
    /// First visible texture column (texels; > 0 only when the sides are
    /// trimmed to fill the height).
    pub src_x: f64,
    /// Visible texture width (texels). The full texture height is always
    /// shown: the HUD at the top is never cropped.
    pub src_w: f64,
    /// Surface pixels per texel.
    pub scale: f64,
}

impl Viewport {
    /// True when the drawn rectangle covers the whole surface.
    #[must_use]
    pub fn fills(&self, surface: (u32, u32)) -> bool {
        self.x == 0 && self.y == 0 && self.w == surface.0 && self.h == surface.1
    }
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

/// Compute where a `tex` texture is drawn on a `surface` (both in physical
/// pixels), centered and aspect-preserving.
///
/// * [`ScaleMode::Fit`]: `scale = min(sw/tw, sh/th)`, fractional. When the
///   texture is only slightly wider than the screen's aspect and trimming at
///   most `side_crop_texels` per side would fill the height, the sides are
///   trimmed instead (16:9 widescreen on a 16:9 display fills it fully).
/// * [`ScaleMode::Integer`]: the largest whole-number scale that fits, or the
///   fractional fit when even 1x does not (shrink, never crop).
///
/// The rectangle never exceeds the surface and the full texture height is
/// always visible.
#[must_use]
pub fn compute_viewport(
    surface: (u32, u32),
    tex: (u32, u32),
    mode: ScaleMode,
    side_crop_texels: u32,
) -> Viewport {
    let (sw, sh) = surface;
    let (tw, th) = tex;
    if sw == 0 || sh == 0 || tw == 0 || th == 0 {
        return Viewport {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            src_x: 0.0,
            src_w: f64::from(tw),
            scale: 1.0,
        };
    }
    let (swf, shf, twf, thf) = (f64::from(sw), f64::from(sh), f64::from(tw), f64::from(th));
    let fit = (swf / twf).min(shf / thf);
    let (scale, trim) = match mode {
        ScaleMode::Integer => (if fit >= 1.0 { fit.floor() } else { fit }, false),
        ScaleMode::Fit => {
            let by_height = shf / thf;
            let overflow = twf * by_height - swf;
            // Trim only when it is the height that is short, and only by the
            // allowance (in surface pixels at the fill scale).
            if overflow > 0.5 && overflow / 2.0 <= f64::from(side_crop_texels) * by_height + 1e-6 {
                (by_height, true)
            } else {
                (fit, false)
            }
        }
    };
    let h = ((thf * scale).round() as u32).clamp(1, sh);
    let (w, src_x, src_w) = if trim {
        let src_w = swf / scale;
        (sw, (twf - src_w) / 2.0, src_w)
    } else {
        (((twf * scale).round() as u32).clamp(1, sw), 0.0, twf)
    };
    Viewport {
        x: (sw - w) / 2,
        y: (sh - h) / 2,
        w,
        h,
        src_x,
        src_w,
        scale,
    }
}

/// CPU reference of the windowed blit: draw `rgba` (`tex` sized) into a black
/// `surface`-sized RGBA buffer through `vp`, nearest-sampled. The GPU path
/// ([`crate::gpu_present`]) samples the same texel for every pixel centre
/// (it only smooths the seams between texels at fractional scales), so a
/// headless `--dump-present --surface WxH` PNG shows exactly the borders,
/// size and trim a player gets.
#[must_use]
pub fn blit_viewport(rgba: &[u8], tex: (u32, u32), surface: (u32, u32), vp: &Viewport) -> Vec<u8> {
    let (sw, sh) = (surface.0 as usize, surface.1 as usize);
    let (tw, th) = (tex.0 as usize, tex.1 as usize);
    let mut out = vec![0u8; sw * sh * 4];
    for px in out.as_chunks_mut::<4>().0.iter_mut() {
        px[3] = 0xFF;
    }
    if tw == 0 || th == 0 || rgba.len() < tw * th * 4 || vp.w == 0 || vp.h == 0 {
        return out;
    }
    let (vx, vy, vw, vh) = (vp.x as usize, vp.y as usize, vp.w as usize, vp.h as usize);
    let tpx_x = vp.src_w / vw as f64;
    let tpx_y = th as f64 / vh as f64;
    for dy in 0..vh.min(sh.saturating_sub(vy)) {
        let ty = (((dy as f64 + 0.5) * tpx_y) as usize).min(th - 1);
        for dx in 0..vw.min(sw.saturating_sub(vx)) {
            let tx = ((vp.src_x + (dx as f64 + 0.5) * tpx_x) as usize).min(tw - 1);
            let s = (ty * tw + tx) * 4;
            let d = ((vy + dy) * sw + vx + dx) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    out
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
    /// What cartridge image this emulator runs (vanilla or a randomized
    /// seed) and which traps that image forced off.
    pub rom: RomIdentity,
}

/// Identity of the cartridge image an [`Emu`] was built from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RomIdentity {
    /// CRC32 of the body actually running (the netplay `Hello` value).
    /// `0` for the synthetic no-ROM emulator.
    pub body_crc32: u32,
    /// Randomizer hash code when the body is a randomized seed.
    pub hash_code: Option<String>,
    /// Fixed-bank traps disabled because the randomizer patched their code
    /// ([`z2_rando::trap_policy::untrap_list`]).
    pub untrapped: Vec<u16>,
    /// `--no-traps`: the whole trap table is off (pure interpretation).
    pub no_traps: bool,
    /// The running body differs from the verified vanilla one (a ROM
    /// randomizer seed that changed something). Gameplay enhancements that
    /// assume the original world are dropped ([`Enhancements::for_rom`]).
    pub randomized: bool,
    /// `(vanilla, running)` bodies of a randomized image, kept so the
    /// untrap list can be recomputed when the enhancements (and so the
    /// registered hooks) change at run time. `None` for the vanilla game.
    pub bodies: Option<Arc<(Vec<u8>, Vec<u8>)>>,
}

/// Disable every fixed-bank trap whose code the ROM randomizer changed
/// ([`z2_rando::trap_policy::untrap_list`] over the traps registered now,
/// enhancement hooks included), first re-enabling the ones `rom` disabled
/// before. Returns the new list (empty for the vanilla game).
pub fn apply_rom_untraps(game: &mut Game, rom: &RomIdentity) -> Vec<u16> {
    for &a in &rom.untrapped {
        game.set_untrapped(a, false);
    }
    let Some(bodies) = &rom.bodies else {
        return Vec::new();
    };
    let (vanilla, body) = (&bodies.0, &bodies.1);
    let addrs: Vec<u16> = game.traps.iter().map(|t| t.addr).collect();
    let list = z2_rando::trap_policy::untrap_list(&addrs, vanilla, body);
    for &a in &list {
        game.set_untrapped(a, true);
    }
    list
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
    /// Randomize the verified vanilla body with this spec before building
    /// (`--seed` / `--rando-flags`). Shared by every rebuild path (ROM drop,
    /// netplay restart), so the same seed is regenerated each time.
    pub rando: Option<&'static crate::rando::RandoSpec>,
    /// `--no-traps`: run every routine as interpreted ROM code (slow; for
    /// checking ported routines against patched ROMs).
    pub no_traps: bool,
    /// ZALiA-inspired gameplay enhancements (all off by default;
    /// [`Game::set_enhancements`]). Changes gameplay, so it is part of the
    /// netplay identity ([`session_trapset_id`]) and forced off for movies.
    pub enhancements: Enhancements,
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
    let enhancements = feats.enhancements.for_rom(emu.rom.randomized);
    if emu.game.wide_gameplay_tiles() != feats.wide_gameplay
        || *emu.game.enhancements() != enhancements
    {
        if emu.game.wide_gameplay_tiles() != feats.wide_gameplay {
            emu.game.set_wide_gameplay(feats.wide_gameplay);
        }
        if *emu.game.enhancements() != enhancements {
            for line in feats.enhancements.rom_conflicts(emu.rom.randomized) {
                eprintln!("{line}");
            }
            emu.game.set_enhancements(enhancements);
            // Enhancement hooks may sit on randomized fixed-bank code.
            emu.rom.untrapped = apply_rom_untraps(&mut emu.game, &emu.rom);
        }
        emu.trapset_id = fold_rom_identity(
            session_trapset_id(
                emu.trapset_base,
                feats.wide_gameplay,
                &enhancements.identity_bytes(),
            ),
            &emu.rom.untrapped,
            emu.rom.no_traps,
        );
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
/// (the same FNV-1a continued over `"wide_gameplay"` and the tile count),
/// then the enhancement identity
/// ([`z2_core::enh::Enhancements::identity_bytes`], continued over `"enh"`
/// and the bytes). `None` / empty bytes leave the id untouched, so a peer
/// without wide gameplay or enhancements keeps the identity it always had.
///
/// **Duplicated in `z2-web` (`session_trapset_id` there)**; both are pinned to
/// [`WIDE_TRAPSET_PIN_VALUE`] and [`ENH_TRAPSET_PIN_VALUE`].
#[must_use]
pub fn session_trapset_id(base: u64, wide_tiles: Option<u8>, enh_identity: &[u8]) -> u64 {
    let fold = |mut h: u64, bytes: &mut dyn Iterator<Item = u8>| {
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let mut h = base;
    if let Some(tiles) = wide_tiles {
        h = fold(
            h,
            &mut b"wide_gameplay"
                .iter()
                .copied()
                .chain(core::iter::once(tiles)),
        );
    }
    if !enh_identity.is_empty() {
        h = fold(h, &mut b"enh".iter().chain(enh_identity).copied());
    }
    h
}

/// Cross-crate pin for the enhancement fold of [`session_trapset_id`]:
/// `session_trapset_id(TRAPSET_PIN_VALUE, None, &[1, b'C', 1, 0, 0, 0])`.
pub const ENH_TRAPSET_PIN_VALUE: u64 = 0x8781_1390_773e_5234;

/// Cross-crate pin for [`session_trapset_id`]:
/// `session_trapset_id(TRAPSET_PIN_VALUE, Some(11))`.
pub const WIDE_TRAPSET_PIN_VALUE: u64 = 0x1094_c616_9aeb_24b9;

/// Fold the traps a randomized ROM forced off (and `--no-traps`) into a
/// session identity, continuing the same FNV-1a over `"untrap"` and each
/// address, then `"no_traps"`. Nothing to fold leaves the id untouched, so
/// vanilla peers keep the identity they always had.
///
/// **Duplicated in `z2-web` (`fold_rom_identity` there)**; both are pinned
/// to [`UNTRAP_PIN_VALUE`].
#[must_use]
pub fn fold_rom_identity(id: u64, untrapped: &[u16], no_traps: bool) -> u64 {
    let mut h = id;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    if !untrapped.is_empty() {
        eat(b"untrap");
        for a in untrapped {
            eat(&a.to_le_bytes());
        }
    }
    if no_traps {
        eat(b"no_traps");
    }
    h
}

/// Cross-crate pin for [`fold_rom_identity`]:
/// `fold_rom_identity(TRAPSET_PIN_VALUE, &[0xC358, 0xDF79], false)`.
pub const UNTRAP_PIN_VALUE: u64 = 0xb2a6_b81c_e697_bbc5;

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

/// Logical pixels kept free beside the window when sizing it to a monitor
/// (window borders).
pub const WINDOW_CHROME_W: u32 = 16;
/// Logical pixels kept free above/below the window when sizing it to a
/// monitor (title bar plus a taskbar or dock).
pub const WINDOW_CHROME_H: u32 = 120;

/// Initial window size in **logical** pixels for a `base_w x base_h` frame
/// (256x240, or the widescreen width x 240 — before any HD output scale).
///
/// Picks the largest of 3x, 2x, 1x that fits `monitor` (its **logical** size,
/// i.e. physical / DPI scale, with room kept for the title bar and taskbar),
/// falling back to a 1536-logical-px budget when the monitor size is unknown.
/// When even 1x does not fit, the window is shrunk to fit: the present path
/// scales the texture to any surface ([`compute_viewport`]), so a window
/// smaller than the texture no longer crops it.
#[must_use]
pub fn initial_window_size(base_w: u32, base_h: u32, monitor: Option<(u32, u32)>) -> (f64, f64) {
    match monitor {
        Some(_) => window_size_for_scale((base_w, base_h), 3, monitor),
        None => {
            let (bw, bh) = (base_w.max(1), base_h.max(1));
            let mut s = 3u32;
            while s > 1 && bw * s > 1536 {
                s -= 1;
            }
            (f64::from(bw * s), f64::from(bh * s))
        }
    }
}

/// Initial window size in **logical** pixels for an explicit `--scale` /
/// `window_scale` multiplier.
///
/// `base` is the frame before any HD output scale (256x240, or the widescreen
/// width x 240). The window is `base * scale`, stepped down while it does not
/// fit `monitor` (logical size, chrome room kept as in
/// [`initial_window_size`]), and shrunk below 1x to fit a monitor too small
/// even for that. The HD texture size plays no part: an HD pack used to force
/// the window up to its full texture (1728x960 logical for a 4x 16:9 pack,
/// 3456x1920 physical at 200% DPI), past the edge of a 2560x1600 screen.
#[must_use]
pub fn window_size_for_scale(
    base: (u32, u32),
    scale: u32,
    monitor: Option<(u32, u32)>,
) -> (f64, f64) {
    let (base_w, base_h) = (base.0.max(1), base.1.max(1));
    let mut s = scale.clamp(1, crate::config::MAX_WINDOW_SCALE);
    let Some((mw, mh)) = monitor else {
        return (f64::from(base_w * s), f64::from(base_h * s));
    };
    let room_w = mw.saturating_sub(WINDOW_CHROME_W).max(1);
    let room_h = mh.saturating_sub(WINDOW_CHROME_H).max(1);
    while s > 1 && (base_w * s > room_w || base_h * s > room_h) {
        s -= 1;
    }
    if base_w * s <= room_w && base_h * s <= room_h {
        return (f64::from(base_w * s), f64::from(base_h * s));
    }
    // Not even 1x fits: shrink, keeping the frame's aspect.
    let f = (f64::from(room_w) / f64::from(base_w)).min(f64::from(room_h) / f64::from(base_h));
    (
        (f64::from(base_w) * f).floor().max(1.0),
        (f64::from(base_h) * f).floor().max(1.0),
    )
}

/// What `Esc` does: leave fullscreen when fullscreen (so a player cannot quit
/// by accident while reaching for the menu), quit when windowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscAction {
    /// Return to a window; the game keeps running.
    LeaveFullscreen,
    /// Save SRAM and exit (the old behaviour, windowed only).
    Quit,
}

/// [`EscAction`] for the current fullscreen state.
#[must_use]
pub fn esc_action(fullscreen: bool) -> EscAction {
    if fullscreen {
        EscAction::LeaveFullscreen
    } else {
        EscAction::Quit
    }
}

/// Whether a key press toggles fullscreen: `F11` (no modifiers needed),
/// `Alt+Enter`, or `Cmd+Ctrl+F` (the macOS convention; `logo` is Cmd there).
/// None of these collide with a hotkey; `Enter` and `F` are pad bindings, so
/// the caller keeps the chord press away from the game.
#[must_use]
pub fn is_fullscreen_toggle(
    code: winit::keyboard::KeyCode,
    alt: bool,
    ctrl: bool,
    logo: bool,
) -> bool {
    use winit::keyboard::KeyCode as KC;
    match code {
        KC::F11 => true,
        KC::Enter | KC::NumpadEnter => alt,
        KC::KeyF => ctrl && logo,
        _ => false,
    }
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
        rom: RomIdentity::default(),
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

/// Build an emulator from a header-stripped vanilla ROM body (262144 bytes).
///
/// The body is verified with the `z2-assets` hash gate, then wrapped in a
/// synthetic MMC1 iNES header for [`Game::from_ines`]. The caller's ROM
/// bytes are never stored — only the parsed banks live in the returned
/// `Game`.
///
/// The CPU is `reset()` to the reset vector: without this the interpreter
/// would execute from address 0 (BRK soup) and the game would never boot.
pub fn emu_from_rom_body(body: &[u8], audio_rate: u32) -> Result<Emu, String> {
    emu_from_rom_body_with(body, audio_rate, Features::default())
}

/// [`emu_from_rom_body`] with optional features applied at the right moments.
///
/// This is the hash-gate seam: the vanilla body is verified here, then (with
/// [`Features::rando`]) randomized, and the result is built by
/// [`emu_from_trusted_body_with`] without a second hash check. Callers keep
/// passing the **vanilla** body, so every rebuild regenerates the same seed.
pub fn emu_from_rom_body_with(
    body: &[u8],
    audio_rate: u32,
    feats: Features,
) -> Result<Emu, String> {
    z2_assets::rom::verify_body(body).map_err(|e| format!("ROM gate: {e}"))?;
    match feats.rando {
        None => emu_from_trusted_body_with(body, None, None, audio_rate, feats),
        Some(spec) => {
            let out = spec.run(body)?;
            emu_from_trusted_body_with(
                &out.body,
                Some(body),
                Some(out.hash_code),
                audio_rate,
                feats,
            )
        }
    }
}

/// Build an emulator from a body that is already trusted: the verified
/// vanilla body, or the randomizer's output for it. Accepts the vanilla
/// layout (128 KiB PRG) and the expanded one (256 KiB PRG); CHR is 128 KiB
/// either way.
///
/// `vanilla` is the verified original when `body` is a patched image: the
/// fixed-bank traps whose code the patch changed are disabled
/// ([`z2_rando::trap_policy::untrap_list`]) before reset, and folded into the
/// session identity ([`fold_rom_identity`]).
///
/// Ordering matters and is the whole reason this function exists: the co-op
/// trap group must be registered **after** the nine default groups (it
/// overrides three of them and inherits their cycle costs) and **before**
/// `Game::reset`, exactly like the default groups. The trap-set identity is
/// captured before co-op registration so it describes the parity-relevant
/// table only.
pub fn emu_from_trusted_body_with(
    body: &[u8],
    vanilla: Option<&[u8]>,
    hash_code: Option<String>,
    audio_rate: u32,
    feats: Features,
) -> Result<Emu, String> {
    const CHR_LEN: usize = 128 * 1024;
    let Some(prg_units) = z2_rando::rom::prg_units_for_body_len(body.len()) else {
        return Err(format!(
            "ROM body is {} bytes, want {} (vanilla) or {} (expanded)",
            body.len(),
            z2_rando::rom::VANILLA_BODY_LEN,
            z2_rando::rom::EXPANDED_BODY_LEN
        ));
    };
    let prg_len = usize::from(prg_units) * 0x4000;
    let mut ines = Vec::with_capacity(16 + body.len());
    ines.extend_from_slice(&z2_rando::rom::ines_header(
        prg_units,
        (CHR_LEN / 0x2000) as u8,
    ));
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
    // Enhancements last (they may wrap any trap above), before `reset`. All
    // off registers nothing and leaves the identity untouched. They run on
    // top of the randomized image (their PRG patches see its tables); the
    // options that assume the original world are dropped first
    // ([`Enhancements::for_rom`]).
    let randomized = vanilla.is_some_and(|v| v != body);
    let enhancements = feats.enhancements.for_rom(randomized);
    for line in feats.enhancements.rom_conflicts(randomized) {
        eprintln!("{line}");
    }
    game.set_enhancements(enhancements);
    let mut rom = RomIdentity {
        body_crc32: z2_assets::rom::crc32_ieee(body),
        hash_code,
        untrapped: Vec::new(),
        no_traps: feats.no_traps,
        randomized,
        bodies: vanilla.map(|v| Arc::new((v.to_vec(), body.to_vec()))),
    };
    // Randomized code inside a trapped fixed-bank routine only runs when
    // that trap is off. Computed over every registered trap (enhancement
    // hooks included) against the randomizer's own output, never against
    // the enhancement-patched PRG.
    rom.untrapped = apply_rom_untraps(&mut game, &rom);
    if feats.no_traps {
        game.traps.enabled = false;
    }
    let trapset_base = trapset_id;
    let trapset_id = fold_rom_identity(
        session_trapset_id(
            trapset_base,
            feats.wide_gameplay,
            &enhancements.identity_bytes(),
        ),
        &rom.untrapped,
        feats.no_traps,
    );
    game.reset();
    let mut apu = z2_apu::Apu::new(crate::audio::clamp_rate(audio_rate));
    apu.install_dmc_source(Box::new(z2_apu::PrgSource::new(body[..prg_len].to_vec())));
    let mut emu = Emu {
        game,
        apu,
        trapset_id,
        trapset_base,
        rom,
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
    let log = emu.game.apu.drain_log();
    pcm.clear();
    // Display enhancements (volumes, M/N mutes, low-HP beep): only frames
    // headed for an audio ring, and neutral settings take the original path
    // untouched (`display_enh::with_audio_fx`).
    let fx_done = audio.is_some()
        && crate::display_enh::with_audio_fx(|fx| {
            let active = !fx.is_neutral();
            if active {
                fx.render_frame(&log, emu.game.ram(), &mut emu.apu, pcm);
            }
            active
        });
    if !fx_done {
        for (addr, val) in log {
            emu.apu.write_reg(addr, val);
        }
        emu.apu.audio(pcm);
    }
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

/// Largest `--p2-follow` delay (10 s of frames).
pub const MAX_P2_FOLLOW: usize = 600;

/// Pad 2 for `--p2-follow`: the movie's pad-1 byte `delay` frames before
/// frame `frame`, without Start/Select (pad 2 must not pause the game or open
/// menus). Idle (`0`) until the delay has elapsed.
#[must_use]
pub fn follow_pad(movie: &[u8], frame: usize, delay: usize) -> u8 {
    frame
        .checked_sub(delay)
        .and_then(|i| movie.get(i))
        .map_or(0, |&b| {
            b & !(z2_core::game::BTN_START | z2_core::game::BTN_SELECT)
        })
}

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
/// Returns the ROM path or a plain-language error telling the user exactly
/// what to do (no file picker — `--rom`, the launcher, or winit drag-drop onto
/// the running window). A config `rom_path` that no longer points at a file
/// is skipped like a stale `$Z2_ROM` (the file was moved or deleted since it
/// was remembered), and the error then names that path specifically. An
/// explicit `--rom` is never filtered: a bad path there stays a loud error.
pub fn resolve_rom_path(cli_rom: Option<&str>, config: &NativeConfig) -> Result<PathBuf, String> {
    if let Some(p) = cli_rom {
        return Ok(PathBuf::from(p));
    }
    let stale = match &config.rom_path {
        Some(p) if Path::new(p).is_file() => return Ok(PathBuf::from(p)),
        Some(p) if !p.trim().is_empty() => Some(p.as_str()),
        _ => None,
    };
    env_path_if_usable(z2_assets::rom::ROM_ENV_VAR).ok_or_else(|| no_rom_message(stale))
}

/// The plain-language "no ROM" note printed at startup (the window title,
/// [`NO_ROM_TITLE`], carries the short form). `stale_config_rom` is a config
/// `rom_path` that no longer exists, which gets its own first line.
#[must_use]
pub fn no_rom_message(stale_config_rom: Option<&str>) -> String {
    let head = match stale_config_rom {
        Some(p) => format!(
            "The Zelda II ROM remembered in {} is no longer at {p} (moved or deleted?).",
            crate::config::config_path().display()
        ),
        None => "No Zelda II ROM found yet.".to_string(),
    };
    format!(
        "{head}\n\
         To play, do one of these:\n\
         \x20 - drop your Zelda II (USA) .nes file onto the game window,\n\
         \x20 - start z2rs from the z2rs launcher and pick the file there, or\n\
         \x20 - run z2-native --rom PATH/TO/zelda2.nes (or set ${}).\n\
         z2rs remembers the file for next time. The ROM itself is never copied — \
         only assets.bin is extracted into the data dir.",
        z2_assets::rom::ROM_ENV_VAR
    )
}

/// Record `rom` (made absolute) as the config's `rom_path`, so the next plain
/// launch boots straight into the game. Returns whether the value changed —
/// the caller saves the config only then. Only the path is stored; the ROM
/// itself is never copied anywhere.
pub fn remember_rom_path(config: &mut NativeConfig, rom: &Path) -> bool {
    let abs = std::path::absolute(rom).unwrap_or_else(|_| rom.to_path_buf());
    let abs = abs.to_string_lossy().into_owned();
    if config.rom_path.as_deref() == Some(abs.as_str()) {
        return false;
    }
    config.rom_path = Some(abs);
    true
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

/// Load a `.z2snap` file into `game` (what `--load-state` and `F7` do).
pub fn load_state_file(game: &mut Game, path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let snap = z2_verify::snapshot::Snapshot::decode(&bytes)
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    apply_snapshot_to_game(game, &snap)
}

/// Blank frames run before `--load-state` applies its file: the game's reset
/// code clears RAM on its first frames, which would wipe a state loaded at
/// power-on.
pub const LOAD_STATE_WARMUP: usize = 30;

/// `--load-state`: run [`LOAD_STATE_WARMUP`] blank frames past reset, then
/// load `path` as `F7` would.
pub fn load_state_at_boot(emu: &mut Emu, path: &Path) -> Result<(), String> {
    step_frames(emu, &[0; LOAD_STATE_WARMUP], None);
    load_state_file(&mut emu.game, path)
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
    load_state_file(game, &savestate_path(data_dir, slot))
}

/// Save-state path for `slot`, kept apart per randomizer seed (`hash` is
/// [`RomIdentity::hash_code`]; `None` = the vanilla game, same file as
/// [`savestate_path`]).
pub fn savestate_path_for(data_dir: &Path, slot: u8, hash: Option<&str>) -> PathBuf {
    data_dir.join(crate::rando::savestate_file_name(hash, slot))
}

/// [`save_savestate`] into the per-seed file ([`savestate_path_for`]).
pub fn save_savestate_for(
    game: &Game,
    data_dir: &Path,
    slot: u8,
    hash: Option<&str>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    let snap = snapshot_from_game(game, Vec::new())?;
    let bytes = snap.encode().map_err(|e| format!("encode: {e}"))?;
    let path = savestate_path_for(data_dir, slot, hash);
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// [`load_savestate`] from the per-seed file ([`savestate_path_for`]).
pub fn load_savestate_for(
    game: &mut Game,
    data_dir: &Path,
    slot: u8,
    hash: Option<&str>,
) -> Result<(), String> {
    load_state_file(game, &savestate_path_for(data_dir, slot, hash))
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
    load_sram_named(game, data_dir, SRAM_FILE)
}

/// [`load_sram`] from a named file in `data_dir` (per-seed saves use
/// [`crate::rando::sram_file_name`]).
pub fn load_sram_named(game: &mut Game, data_dir: &Path, name: &str) -> Result<bool, String> {
    let path = data_dir.join(name);
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

/// Window-title suffix naming the randomizer seed by its hash code.
#[must_use]
pub fn rando_suffix(hash_code: &str) -> String {
    format!(" [rando {hash_code}]")
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
    for (dst, &px) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(indexed.iter()) {
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
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Display-only enhancements (screen shake, flash colour, post effects,
    /// volumes, dev overlays; see [`crate::display_enh`]). Never part of the
    /// netplay identity.
    pub display_enh: DisplayEnh,
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
            rando: None,
            no_traps: false,
            enhancements: Enhancements::default(),
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
    /// Display-enhancement state ([`crate::display_enh`]); untouched while
    /// `settings.display_enh` is the default.
    fx: crate::display_enh::DisplayFx,
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
            fx: crate::display_enh::DisplayFx::default(),
        })
    }

    /// The settings in force.
    #[must_use]
    pub fn settings(&self) -> &DisplaySettings {
        &self.settings
    }

    /// Replace the display-only enhancements (the options overlay; takes
    /// effect on the next presented frame).
    pub fn set_display_enh(&mut self, d: DisplayEnh) {
        self.settings.display_enh = d;
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
        let enh = self.settings.display_enh;
        if enh.is_default() {
            return self
                .presenter
                .present(game.frame_indexed(), record, &game.chr)
                .map_err(|e| format!("present: {e}"));
        }
        // Display enhancements: observe, maybe recolour the flash, compose,
        // then post effects / shake / overlays on a copy.
        self.fx.observe(game, &enh);
        let (w, h) = (
            self.presenter.width() as u32,
            self.presenter.height() as u32,
        );
        let scale = self.presenter.effective_scale();
        let (frame, record) = self.fx.frame_inputs(game, record);
        let rgba = self
            .presenter
            .present(frame, record, &game.chr)
            .map_err(|e| format!("present: {e}"))?;
        Ok(self
            .fx
            .finish(rgba, w, h, scale, u32::from(tiles) * 8, game, &enh))
    }

    /// Scanline strength for the window blit ([`crate::display_enh`]).
    #[must_use]
    pub fn scanline_strength(&self) -> f32 {
        crate::display_enh::scanline_strength(&self.settings.display_enh)
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
    /// `--widescreen off|16:10|16:9|21:9|N` (overrides the config key).
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
    /// `--p2-follow N`: during movie playback in local co-op, pad 2 replays
    /// pad 1's input N frames late (see [`follow_pad`]).
    pub p2_follow: Option<usize>,
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
    /// `--load-state PATH.z2snap`: loaded at startup exactly as `F7` does,
    /// before the first frame (and before `--movie` frame 0).
    pub load_state: Option<String>,
    /// `--scale N` initial window multiplier 1-8 (overrides `window_scale`).
    /// Display only.
    pub scale: Option<u32>,
    /// `--fullscreen`: start in borderless fullscreen (ORed with the
    /// `fullscreen` config key). Display only.
    pub fullscreen: bool,
    /// `--scale-mode fit|integer` (overrides the `scale_mode` config key).
    /// Display only.
    pub scale_mode: Option<ScaleMode>,
    /// `--seed TEXT`: randomizer seed (turns the randomizer on).
    pub seed: Option<String>,
    /// `--rando-flags STRING`: randomizer flag string (turns it on).
    pub rando_flags: Option<String>,
    /// `--rando-spoiler PATH`: write the spoiler log here.
    pub rando_spoiler: Option<String>,
    /// `--sprite-ips PATH`: bring-your-own player sprite patch.
    pub sprite_ips: Option<String>,
    /// `--no-traps`: interpret every routine (no Rust ports).
    pub no_traps: bool,
    /// `--enh-json JSON|@PATH`: gameplay enhancements (overrides the
    /// `enhancements` config key, and applies even with `--movie`).
    pub enhancements: Option<Enhancements>,
    /// `--display-enh-json JSON|@PATH`: display-only enhancements (overrides
    /// the `display_enh` config key).
    pub display_enh: Option<DisplayEnh>,
}

impl NativeArgs {
    /// The randomizer spec these flags ask for (`None` = vanilla game).
    ///
    /// # Errors
    /// A bad flag string or an unreadable sprite IPS file.
    pub fn rando_spec(&self) -> Result<Option<crate::rando::RandoSpec>, String> {
        crate::rando::RandoSpec::from_cli(
            self.seed.as_deref(),
            self.rando_flags.as_deref(),
            self.rando_spoiler.as_deref(),
            self.sprite_ips.as_deref(),
        )
    }
}

/// Text of a `--enh-json` / `--display-enh-json` value: inline JSON, or
/// `@PATH` to read it from a file.
///
/// # Errors
/// An unreadable `@PATH`.
pub fn json_arg_text(flag: &str, v: &str) -> Result<String, String> {
    match v.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{flag} {path}: {e}")),
        None => Ok(v.to_string()),
    }
}

/// Parse a `--enh-json` value (inline JSON or `@PATH`).
///
/// # Errors
/// An unreadable file or JSON that does not describe [`Enhancements`].
pub fn parse_enh_arg(v: &str) -> Result<Enhancements, String> {
    Enhancements::from_json(&json_arg_text("--enh-json", v)?)
        .map_err(|e| format!("--enh-json: {e}"))
}

/// Parse a `--display-enh-json` value (inline JSON or `@PATH`); values are
/// clamped into range.
///
/// # Errors
/// An unreadable file or JSON that does not describe [`DisplayEnh`].
pub fn parse_display_enh_arg(v: &str) -> Result<DisplayEnh, String> {
    DisplayEnh::from_json(&json_arg_text("--display-enh-json", v)?)
        .map(DisplayEnh::clamped)
        .map_err(|e| format!("--display-enh-json: {e}"))
}

pub const NATIVE_USAGE: &str = "\
usage: z2-native [--rom PATH] [--movie M.fm2|.bk2] [--config PATH]
                 [--widescreen off|16:10|16:9|21:9|N]
                 [--fill-left-clip on|off] [--fill-right-clip on|off]
                 [--margin-sprites on|off]
                 [--wide-gameplay on|off]
                 [--hd-pack DIR] [--hd-scale N] [--hd-record DIR]
                 [--scale N] [--fullscreen] [--scale-mode fit|integer]
                 [--coop-local] [--coop-host ROOM | --coop-join ROOM]
                 [--signal URL] [--net-mode rollback|lockstep] [--net-delay N]
                 [--ice SPEC] [--p2-pad INDEX] [--p2-follow N]
                 [--seed TEXT] [--rando-flags STRING] [--rando-spoiler PATH]
                 [--sprite-ips PATH] [--no-traps]
                 [--enh-json JSON|@PATH] [--display-enh-json JSON|@PATH] [--headless ...]
  --rom PATH       Zelda II .nes ROM (else config rom_path / $Z2_ROM). Never stored.
  --movie PATH     .fm2/.bk2 demo playback (oracle-free: steps Game, no verify).
  --config PATH    JSON config override (default: <data-dir>/z2-native.json).
  --widescreen P   widescreen margins: off | 16:10 (8 tiles/side) | 16:9 (11) | 21:9 (19) | N (0-20).
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
  --scale N        initial window size: N x the frame (1-8; default: largest of
                   3x/2x/1x that fits the screen). Config key window_scale.
  --fullscreen     start in borderless fullscreen (config key fullscreen).
                   F11 or Alt+Enter (Cmd+Ctrl+F on macOS) toggles it.
  --scale-mode M   fit (default: fill the window/screen height, any size) or
                   integer (whole multiples only; sharper, may leave borders).
                   Config key scale_mode.
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
  --p2-follow N    with --movie and --coop-local: player 2 replays player 1's
                   movie input N frames late (0-600; 0 = exact mirror; Start
                   and Select are dropped). For demo and trailer captures.
  --load-state P   load this .z2snap save state before the first frame (as F7
                   does), so a --movie plays from there.
  --seed TEXT      randomizer seed (any text). With --rando-flags, plays a randomized
                   game generated from your ROM; the window title shows its hash code.
  --rando-flags S  randomizer flag string (see README.md); '1' = no changes.
  --rando-spoiler P
                   write the randomizer spoiler log to P.
  --sprite-ips P   your own player-sprite IPS patch, applied by the randomizer.
  --no-traps       run every routine as interpreted ROM code (slow; for testing).
  --enh-json J     ZALiA-inspired gameplay enhancements as JSON (inline, or @PATH for a
                   file); missing keys stay off. Overrides the config key enhancements;
                   the config value is ignored with --movie. Changes gameplay, so both
                   netplay peers must agree. See README.md.
  --display-enh-json J
                   display-only enhancements (screen shake, flash colour, effects,
                   volumes, dev overlays) as JSON or @PATH. Config key display_enh.
  --headless ...   windowless CI surface (see --headless --help).
keys P1: Z=A X=B Enter=Start RightShift=Select arrows=dpad
keys P2: G=A F=B T=Start R=Select W/A/S/D=dpad (local co-op only; see keys_p2)
Tab=fast-forward F5=save F7=load F6/digits=slot (1-9,0; shown in title) P=pause .=step
M=mute music N=mute sound O=options menu (or LB+RB+Y on a gamepad)
F11/Alt+Enter=fullscreen Esc=leave fullscreen, or quit when windowed; drop a .nes ROM/movie \
file onto the window (a dropped or --rom ROM is remembered in the config for next time). Save states, movies, pause and fast-forward are disabled \
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
                        "--widescreen: expected off | 16:10 | 16:9 | 21:9 | a number 0-20, got '{v}'\n{NATIVE_USAGE}"
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
            "--p2-follow" => {
                let raw = native_arg_value(&mut it, "--p2-follow")?;
                let n: usize = raw.parse().map_err(|_| {
                    format!("--p2-follow expects a frame delay, got '{raw}'\n{NATIVE_USAGE}")
                })?;
                if n > MAX_P2_FOLLOW {
                    return Err(format!(
                        "--p2-follow {n} is out of range (0-{MAX_P2_FOLLOW})\n{NATIVE_USAGE}"
                    ));
                }
                out.p2_follow = Some(n);
            }
            "--hd-pack" => out.hd_pack = Some(native_arg_value(&mut it, "--hd-pack")?),
            "--load-state" => out.load_state = Some(native_arg_value(&mut it, "--load-state")?),
            "--hd-record" => out.hd_record = Some(native_arg_value(&mut it, "--hd-record")?),
            "--scale" => {
                let raw = native_arg_value(&mut it, "--scale")?;
                let n: u32 = raw.parse().unwrap_or(0);
                if n == 0 || n > crate::config::MAX_WINDOW_SCALE {
                    return Err(format!(
                        "--scale expects 1-{}, got '{raw}'\n{NATIVE_USAGE}",
                        crate::config::MAX_WINDOW_SCALE
                    ));
                }
                out.scale = Some(n);
            }
            "--fullscreen" => out.fullscreen = true,
            "--scale-mode" => {
                let raw = native_arg_value(&mut it, "--scale-mode")?;
                out.scale_mode = Some(ScaleMode::parse(&raw).ok_or_else(|| {
                    format!("--scale-mode expects fit | integer, got '{raw}'\n{NATIVE_USAGE}")
                })?);
            }
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
            "--seed" => out.seed = Some(native_arg_value(&mut it, "--seed")?),
            "--rando-flags" => {
                let v = native_arg_value(&mut it, "--rando-flags")?;
                if let Err(e) = z2_rando::flags::Flags::from_flag_string(&v) {
                    return Err(format!("--rando-flags: {e}\n{NATIVE_USAGE}"));
                }
                out.rando_flags = Some(v);
            }
            "--rando-spoiler" => {
                out.rando_spoiler = Some(native_arg_value(&mut it, "--rando-spoiler")?);
            }
            "--sprite-ips" => out.sprite_ips = Some(native_arg_value(&mut it, "--sprite-ips")?),
            "--no-traps" => out.no_traps = true,
            "--enh-json" => {
                let v = native_arg_value(&mut it, "--enh-json")?;
                out.enhancements =
                    Some(parse_enh_arg(&v).map_err(|e| format!("{e}\n{NATIVE_USAGE}"))?);
            }
            "--display-enh-json" => {
                let v = native_arg_value(&mut it, "--display-enh-json")?;
                out.display_enh =
                    Some(parse_display_enh_arg(&v).map_err(|e| format!("{e}\n{NATIVE_USAGE}"))?);
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
    if out.rando_spoiler.is_some()
        && out.seed.is_none()
        && out.rando_flags.is_none()
        && out.sprite_ips.is_none()
    {
        return Err(format!(
            "--rando-spoiler needs --seed and/or --rando-flags\n{NATIVE_USAGE}"
        ));
    }
    Ok(out)
}

/// Start the gamepad library, never fatally: disabled by config, an error
/// or a panic inside it all leave the game keyboard-only. Logs the backend
/// and every pad already connected.
#[cfg(not(target_os = "android"))]
fn open_gamepads(enabled: bool) -> Option<gilrs::Gilrs> {
    use crate::diag::breadcrumb;
    if !enabled {
        breadcrumb("gamepad: disabled (\"gamepads_enabled\": false); keyboard only");
        return None;
    }
    let backend = if cfg!(target_os = "windows") {
        "XInput"
    } else if cfg!(target_os = "macos") {
        "IOKit"
    } else if cfg!(target_os = "linux") {
        "evdev"
    } else {
        "default"
    };
    breadcrumb(format_args!("gamepad: starting gilrs ({backend})"));
    let built =
        std::panic::catch_unwind(|| crate::input::new_gilrs(true).map_err(|e| e.to_string()));
    let g = match built {
        Ok(Ok(g)) => g,
        Ok(Err(e)) => {
            breadcrumb(format_args!("gamepad: unavailable ({e}); keyboard only"));
            return None;
        }
        Err(_) => {
            breadcrumb("gamepad: the gamepad library panicked (see above); keyboard only");
            return None;
        }
    };
    let pads: Vec<String> = g
        .gamepads()
        .map(|(id, gp)| format!("{id}: '{}' ({:?})", gp.name(), gp.power_info()))
        .collect();
    breadcrumb(format_args!(
        "gamepad: ready, {} connected {pads:?}",
        pads.len()
    ));
    Some(g)
}

/// Run the windowed frontend. Opens a window + audio device; never call in
/// tests. Returns a human-readable error (the `main` wrapper maps it to an
/// exit code).
pub fn run_windowed(args: &NativeArgs) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    run_windowed_with(args, event_loop)
}

/// [`run_windowed`] on an event loop the host already built.
///
/// For hosts that must configure the loop themselves: Android builds it with
/// `EventLoopBuilderExtAndroid::with_android_app` (see `crates/z2-android`),
/// and winit allows one event loop per process, so it cannot be made here.
/// Everything else — config, ROM, audio, the loop body — is shared with the
/// desktop path.
pub fn run_windowed_with(
    args: &NativeArgs,
    event_loop: winit::event_loop::EventLoop<()>,
) -> Result<(), String> {
    // -- config + data dir -------------------------------------------------
    let mut config: NativeConfig = match &args.config {
        Some(p) => NativeConfig::load_from(Path::new(p)),
        None => NativeConfig::load(),
    };
    let data_dir = config.effective_data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("create {}: {e}", data_dir.display()))?;
    // Crash breadcrumbs from here on (`<data-dir>/z2-native.log`).
    crate::diag::init(&data_dir);
    crate::diag::breadcrumb(format_args!(
        "args: {:?}",
        std::env::args().skip(1).collect::<Vec<_>>()
    ));
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
    let rando = args.rando_spec()?.map(crate::rando::RandoSpec::leak);
    let feats = Features {
        coop,
        wide_gameplay: resolve_wide_gameplay(args, &config, display_settings.wide_tiles),
        record: display.needs_record(),
        margin_sprites: display_settings.features(coop).margin_sprites,
        rando,
        no_traps: args.no_traps,
        enhancements: resolve_enhancements(args, &config),
    };
    let coop_local = feats.coop && !args.coop.is_online();
    if args.p2_follow.is_some() && !coop_local {
        return Err(format!("--p2-follow needs --coop-local\n{NATIVE_USAGE}"));
    }

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
            Ok((e, body)) => {
                // Remember a `--rom` that loaded, so the next plain launch
                // (a double-click, no flags) boots straight into the game.
                // Never with an explicit `--config`: that file is the
                // caller's, not ours to rewrite.
                if args.rom.is_some()
                    && args.config.is_none()
                    && remember_rom_path(&mut config, &rom_path)
                {
                    match config.save() {
                        Ok(()) => eprintln!(
                            "remembered ROM location {} for next launch",
                            rom_path.display()
                        ),
                        Err(e) => eprintln!("could not remember the ROM location: {e}"),
                    }
                }
                (e, true, Some(body))
            }
            Err(err) => {
                eprintln!("ROM load failed ({err}); showing no-ROM window.");
                (new_emu_with(rate, feats), false, None)
            }
        },
        Err(note) => {
            eprintln!("{note}\nOpening the game window without a ROM.");
            (new_emu_with(rate, feats), false, None)
        }
    };
    let _ = load_sram_named(
        &mut emu.game,
        &data_dir,
        &crate::rando::sram_file_name(emu.rom.hash_code.as_deref(), false),
    );
    if let Some(h) = &emu.rom.hash_code {
        eprintln!(
            "randomizer: seed {:?}, hash code {h}{}",
            rando.map(|r| r.seed.as_str()).unwrap_or_default(),
            if emu.rom.untrapped.is_empty() {
                String::new()
            } else {
                format!(
                    ", {} ported routines disabled for patched code",
                    emu.rom.untrapped.len()
                )
            }
        );
    }
    // `load_sram` replaced WRAM under a possibly live co-op state.
    emu.game.coop_reset_area();
    if let Some(p) = &args.load_state {
        load_state_at_boot(&mut emu, Path::new(p))?;
        eprintln!("loaded save state {p}");
    }

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

    run_event_loop(
        RunConfig {
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
        },
        event_loop,
    )
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
            format!("--widescreen: expected off | 16:10 | 16:9 | 21:9 | a number 0-20, got '{p}'")
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
        display_enh: args.display_enh.unwrap_or(config.display_enh).clamped(),
    })
}

/// The gameplay enhancements for this run: `--enh-json` wins; otherwise the
/// `enhancements` config key, except for `--movie` playback, where the
/// config value is ignored (it changes gameplay, so the movie would desync).
#[must_use]
pub fn resolve_enhancements(args: &NativeArgs, config: &NativeConfig) -> Enhancements {
    match args.enhancements {
        Some(e) => e,
        None if args.movie.is_some() => Enhancements::default(),
        None => config.enhancements,
    }
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
    feats.enhancements = resolve_enhancements(args, config);
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
fn run_event_loop(
    run: RunConfig,
    event_loop: winit::event_loop::EventLoop<()>,
) -> Result<(), String> {
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
    use winit::event_loop::{ActiveEventLoop, ControlFlow};
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
        /// The window, shared with the `pixels` surface (which owns a clone,
        /// hence `Pixels<'static>` without leaking). `None` before the first
        /// `resumed` and between `suspended` and the next `resumed`.
        window: Option<Arc<Window>>,
        pixels: Option<pixels::Pixels<'static>>,
        keyboard: crate::input::KeyboardState,
        /// Per-player opposing-direction filters for live input (issue #7);
        /// bypassed when `config.allow_opposing_directions` is set.
        socd: [crate::input::OpposingFilter; 2],
        #[cfg(not(target_os = "android"))]
        gilrs: Option<gilrs::Gilrs>,
        /// Pads whose first input has been logged (crash breadcrumbs).
        #[cfg(not(target_os = "android"))]
        pads_heard: Vec<gilrs::GamepadId>,
        /// The first presented frame has been logged (crash breadcrumbs).
        presented_once: bool,
        /// Gamepad that most recently pressed a button (drives player 1).
        #[cfg(not(target_os = "android"))]
        active_pad: Option<gilrs::GamepadId>,
        audio: SharedAudio,
        _stream: Option<cpal::Stream>,
        /// Actual output-device rate (None = audio disabled). Shown in the
        /// title so rate fallback/silence is always visible.
        device_rate: Option<u32>,
        timer: FrameTimer,
        pacer: AudioPacer,
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
        #[cfg(not(target_os = "android"))]
        active_pad2: Option<gilrs::GamepadId>,
        /// Gamepads in connection order, for `--p2-pad INDEX`.
        #[cfg(not(target_os = "android"))]
        pad_order: Vec<gilrs::GamepadId>,
        /// `--p2-pad INDEX`: pin player 2 to this entry of `pad_order`.
        /// (Unused on Android, where the host numbers its pads itself.)
        #[cfg_attr(target_os = "android", allow(dead_code))]
        p2_pad: Option<usize>,
        /// `--p2-follow N`: pad 2 replays the movie's pad 1 N frames late.
        p2_follow: Option<usize>,
        /// Size the `pixels` texture currently has, so the present path only
        /// calls `resize_buffer` when it actually changes. `(0, 0)` forces a
        /// re-check on the next redraw (used after a ROM drop or a netplay
        /// session start rebuilds the emulator).
        tex_size: (u32, u32),
        /// `--scale` / `window_scale`: explicit initial window multiplier
        /// (`None` = the automatic 3x/2x/1x fit). Display only.
        window_scale: Option<u32>,
        /// `--fullscreen` / `fullscreen`: open borderless fullscreen.
        start_fullscreen: bool,
        /// Save a successfully dropped ROM's path into the config (off when
        /// the config was given explicitly with `--config`).
        remember_rom: bool,
        /// Current keyboard modifiers (for the Alt+Enter / Cmd+Ctrl+F chords).
        modifiers: winit::keyboard::ModifiersState,
        /// `--scale-mode` / `scale_mode`: how the texture fills the surface.
        scale_mode: ScaleMode,
        /// Size the `pixels` surface was last configured to (physical px);
        /// the viewport is computed against it.
        surface_size: (u32, u32),
        /// The viewport blit (`None` while there is no window).
        renderer: Option<crate::gpu_present::ViewportRenderer>,
        /// Between `suspended` and the next `resumed` (Android lifecycle).
        lifecycle_suspended: bool,
        /// Monotonic base for the millisecond clock netplay timers use.
        #[cfg(feature = "netplay")]
        start: Instant,
        /// Live netplay session, if any.
        #[cfg(feature = "netplay")]
        net: Option<crate::netplay::NetLink<z2_net::MatchboxTransport>>,
        /// Live rollback netplay session, if any (the default online mode).
        #[cfg(feature = "netplay")]
        rollback: Option<crate::netplay::RollbackLink<z2_net::MatchboxTransport>>,
        /// In-game options menu (`O` / LB+RB+Y; see `crate::overlay`).
        overlay: crate::overlay::Overlay,
        /// Config file the overlay saves to (`--config`, else the default).
        config_file: PathBuf,
    }

    impl Handler {
        /// Fold one gamepad's pressed buttons through a binding table.
        ///
        /// This is where `config.gamepad` / `config.gamepad_p2` finally take
        /// effect (they were parsed but ignored before). The default tables
        /// reproduce the previous hard-coded layout exactly — pinned by
        /// `input::config_gamepad_table_reproduces_the_default_layout` — so
        /// nothing changes for a user who never edited the config.
        #[cfg(not(target_os = "android"))]
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
        #[cfg(not(target_os = "android"))]
        fn pinned_p2(&self) -> Option<gilrs::GamepadId> {
            self.p2_pad.and_then(|i| self.pad_order.get(i).copied())
        }

        /// Gamepad currently driving player 2.
        #[cfg(not(target_os = "android"))]
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
        #[cfg(not(target_os = "android"))]
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
                    // Crash breadcrumb: the first input event from each pad.
                    let is_input = !matches!(
                        ev.event,
                        gilrs::EventType::Connected
                            | gilrs::EventType::Disconnected
                            | gilrs::EventType::Dropped
                    );
                    if is_input && !self.pads_heard.contains(&ev.id) {
                        self.pads_heard.push(ev.id);
                        let name = g
                            .connected_gamepad(ev.id)
                            .map(|gp| gp.name().to_string())
                            .unwrap_or_default();
                        crate::diag::breadcrumb(format_args!(
                            "gamepad: first input from '{name}' ({:?})",
                            ev.event
                        ));
                    }
                    match ev.event {
                        gilrs::EventType::Connected => {
                            if let Some(gp) = g.connected_gamepad(ev.id) {
                                names.push((ev.id, gp.name().to_string()));
                            }
                            evs.push(PadEv::Conn(ev.id));
                        }
                        gilrs::EventType::ButtonPressed(..) => evs.push(PadEv::Press(ev.id)),
                        gilrs::EventType::Disconnected => {
                            crate::diag::breadcrumb(format_args!(
                                "gamepad: {} disconnected",
                                ev.id
                            ));
                            self.pads_heard.retain(|&p| p != ev.id);
                            evs.push(PadEv::Disc(ev.id));
                        }
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
                    crate::diag::breadcrumb(format_args!(
                        "gamepad connected: '{name}' (index {idx})"
                    ));
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

        /// No gilrs on Android: hardware pads arrive through
        /// [`crate::external_pad`] instead (read in [`Self::current_inputs`]).
        #[cfg(target_os = "android")]
        #[allow(clippy::unused_self)]
        fn poll_gamepads(&mut self) -> (u8, u8) {
            (0, 0)
        }

        /// Both players' pads for one frame.
        ///
        /// A demo movie overrides player 1 while frames remain; movie files
        /// carry one port, so player 2 stays idle during playback unless
        /// `--p2-follow` replays player 1's track late.
        fn current_inputs(&mut self) -> (u8, u8) {
            if self.movie_pos < self.movie.len() {
                let b = self.movie[self.movie_pos];
                let p2 = match self.p2_follow {
                    Some(delay) if self.coop_local => {
                        follow_pad(&self.movie, self.movie_pos, delay)
                    }
                    _ => 0,
                };
                self.movie_pos += 1;
                return (b, p2);
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
            // Host-reported pads (the Android touch pad and hardware pads over
            // JNI; always 0 on desktop). Player 2's follows the same co-op gate
            // as its keyboard set, so a single-player run never feeds port 2.
            let gp1 = gp1 | crate::external_pad::p1_mask();
            let gp2 = if self.coop_local {
                gp2 | crate::external_pad::p2_mask()
            } else {
                gp2
            };
            let raw = (
                crate::input::combine_inputs(kb1, gp1),
                crate::input::combine_inputs(kb2, gp2),
            );
            // Live input only (movie playback returned above): resolve
            // keyboard-rollover Left+Right / Up+Down to the newest press
            // before the pad reaches the game or a netplay session, so every
            // peer and any recording sees the same filtered byte.
            if self.config.allow_opposing_directions {
                raw
            } else {
                (self.socd[0].apply(raw.0), self.socd[1].apply(raw.1))
            }
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
                        ..self.feats
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
                    let _ = save_sram_named(&mut self.emu.game, &self.data_dir, &name);
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
                    match crate::netplay::session_emu_with(
                        body, rate, self.feats, coop_flags, &wram,
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
                    let _ = save_sram_named(&mut self.emu.game, &self.data_dir, &name);
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
                let name = self.sram_file();
                let _ = save_sram_named(&mut self.emu.game, &self.data_dir, &name);
            }
        }

        fn redraw(&mut self) {
            let Some(window) = self.window.clone() else {
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
            // Options overlay UI (no-op while closed). First, because it can
            // change the presented size (widescreen).
            self.run_overlay(&window);
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
                    // `resize_buffer` replaced the texture the blit samples.
                    if let Some(r) = self.renderer.as_mut() {
                        r.rebind(p);
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
                renderer,
                surface_size,
                scale_mode,
                tex_size,
                overlay,
                presented_once,
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
            // Our own blit instead of `pixels.render()`: pixels' scaler only
            // does whole-number scales >= 1 (thick fullscreen borders, and a
            // texture larger than the window was cropped). See
            // `compute_viewport` / `gpu_present`.
            let tex = *tex_size;
            let vp = compute_viewport(
                *surface_size,
                tex,
                *scale_mode,
                fill_crop_texels(display.settings().wide_tiles, display.effective_scale()),
            );
            if let Some(r) = renderer.as_ref() {
                r.set_scanlines(display.scanline_strength());
            }
            let result = match renderer.as_ref() {
                Some(r) => pixels.render_with(|encoder, target, ctx| {
                    r.render(encoder, target, &ctx.queue, &vp, tex);
                    overlay.paint(&ctx.device, &ctx.queue, encoder, target, *surface_size);
                    Ok(())
                }),
                None => pixels.render(),
            };
            if !*presented_once {
                *presented_once = true;
                crate::diag::breadcrumb(format_args!("gpu: first frame presented ({result:?})"));
                // The backend survived start-up and its first present: clear
                // the crash-loop guard (`gpu_guard`).
                crate::gpu_guard::confirm();
            }
            if let Err(e) = result {
                // A swallowed render error leaves the last good (or the initial
                // grey) frame up while the title keeps moving - exactly the
                // reported symptom. Log it and re-establish the surface.
                eprintln!("present render: {e:?}");
                let size = window.inner_size();
                let (w, h) = clamp_surface_size(size.width, size.height);
                if let Err(e2) = pixels.resize_surface(w, h) {
                    eprintln!("present resize {w}x{h}: {e2:?}");
                } else {
                    *surface_size = (w, h);
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
            self.save_overlay_config();
            let name = self.sram_file();
            let _ = save_sram_named(&mut self.emu.game, &self.data_dir, &name);
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

        /// Whether the window is currently fullscreen (any kind).
        fn is_fullscreen(&self) -> bool {
            self.window
                .as_ref()
                .is_some_and(|w| w.fullscreen().is_some())
        }

        /// Enter or leave borderless fullscreen. winit answers with a
        /// `Resized` event, which is what resizes the `pixels` surface.
        fn set_fullscreen(&self, on: bool) {
            if let Some(w) = self.window.as_ref() {
                w.set_fullscreen(on.then_some(winit::window::Fullscreen::Borderless(None)));
            }
        }

        /// F11 / Alt+Enter / Cmd+Ctrl+F.
        fn toggle_fullscreen(&self) {
            self.set_fullscreen(!self.is_fullscreen());
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
        fn sram_file(&self) -> String {
            let hash = self.emu.rom.hash_code.as_deref();
            #[cfg(feature = "netplay")]
            {
                if self.net.as_ref().is_some_and(|n| !n.is_host())
                    || self.rollback.as_ref().is_some_and(|r| !r.is_host())
                {
                    return crate::rando::sram_file_name(hash, true);
                }
            }
            crate::rando::sram_file_name(hash, false)
        }

        /// Apply the pause / save-state requests a non-winit host posted to
        /// [`crate::external_pad`] (the Android app's buttons over JNI; never
        /// set on desktop). Same rules as the `P` / `F5` / `F7` hotkeys,
        /// including the netplay refusals.
        fn apply_external_requests(&mut self) {
            if let Some(paused) = crate::external_pad::take_pause_request() {
                if self.net_active() {
                    self.refuse_during_netplay("pause");
                } else if paused != self.paused {
                    eprintln!("{}", if paused { "paused" } else { "resumed" });
                    self.paused = paused;
                    self.audio.set_paused(paused);
                    if !paused {
                        self.audio.clear();
                    }
                }
            }
            let save = crate::external_pad::take_save_request();
            let load = crate::external_pad::take_load_request();
            if save.is_none() && load.is_none() {
                return;
            }
            if let Some(why) = crate::netplay::savestate_blocked(self.net_active()) {
                eprintln!("{why}");
                return;
            }
            // Save before load, so "save then load" in one iteration is a
            // round trip rather than a load of the previous contents.
            let hash = self.emu.rom.hash_code.clone();
            if let Some(slot) = save {
                match save_savestate_for(&self.emu.game, &self.data_dir, slot, hash.as_deref()) {
                    Ok(p) => eprintln!("saved {}", p.display()),
                    Err(e) => eprintln!("save failed: {e}"),
                }
            }
            if let Some(slot) = load {
                match load_savestate_for(&mut self.emu.game, &self.data_dir, slot, hash.as_deref())
                {
                    Ok(()) => eprintln!("loaded savestate{slot}"),
                    Err(e) => eprintln!("load failed: {e}"),
                }
            }
        }

        /// Start or stop the output stream itself (not just the ring mute):
        /// a backgrounded Android app must release the audio device. A
        /// failure is logged and otherwise harmless — the ring stays muted.
        fn set_stream_running(&self, running: bool) {
            use cpal::traits::StreamTrait;
            if let Some(stream) = self._stream.as_ref() {
                let result = if running {
                    stream.play().map_err(|e| e.to_string())
                } else {
                    stream.pause().map_err(|e| e.to_string())
                };
                if let Err(e) = result {
                    eprintln!(
                        "audio: could not {} the output stream: {e}",
                        if running { "restart" } else { "pause" }
                    );
                }
            }
        }

        /// The options overlay opened or closed: mute and stop the game
        /// while it is open offline, and save its edits when it closes.
        fn on_overlay_toggled(&mut self, open: bool) {
            self.keyboard.clear();
            self.fast_forward = false;
            let hold = self.overlay.pauses(self.net_active());
            self.audio.set_paused(self.paused || hold);
            if !open {
                if !self.paused {
                    self.audio.clear();
                }
                self.save_overlay_config();
            }
        }

        /// Write the overlay's edits to the config file, if any.
        fn save_overlay_config(&mut self) {
            if self.overlay.take_dirty() {
                if let Err(e) = self.config.save_to(&self.config_file) {
                    eprintln!("options: could not save the config: {e}");
                }
            }
        }

        /// One overlay UI frame, applying whatever it changed.
        fn run_overlay(&mut self, window: &Window) {
            use crate::overlay::{apply, Lock, Model, Targets};
            if !self.overlay.is_open() {
                return;
            }
            let lock = if self.net_active() {
                Lock::Netplay
            } else if self.movie_pos < self.movie.len() {
                Lock::Movie
            } else {
                Lock::None
            };
            let before = Model::capture(
                &self.config,
                &self.feats,
                &self.display,
                self.scale_mode,
                self.is_fullscreen(),
                lock,
            );
            let mut after = before.clone();
            let info = self.compose_title();
            let closed = self.overlay.run(window, &mut after, &info);
            if after != before {
                let applied = apply(
                    &before,
                    &after,
                    Targets {
                        config: &mut self.config,
                        emu: &mut self.emu,
                        feats: &mut self.feats,
                        display: &mut self.display,
                        scale_mode: &mut self.scale_mode,
                    },
                );
                if applied.config_changed {
                    self.overlay.mark_dirty();
                }
                if let Some(on) = applied.fullscreen {
                    self.set_fullscreen(on);
                }
            }
            if closed {
                self.on_overlay_toggled(false);
            }
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
            if let Some(h) = &self.emu.rom.hash_code {
                t.push_str(&rando_suffix(h));
            }
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
            // The window is sized from the frame BEFORE any HD output scale,
            // against the monitor's LOGICAL size (physical / DPI scale), so a
            // 125-200% Windows display or a handheld never gets a window
            // taller than its screen. The texture no longer dictates a
            // minimum window size: the viewport blit shrinks it to fit.
            let (tex_w, tex_h) = self.display.size();
            let hd = self.display.effective_scale().max(1);
            let base = (tex_w / hd, tex_h / hd);
            let primary = event_loop.primary_monitor();
            let monitor = primary.as_ref().map(|m| {
                let s: winit::dpi::LogicalSize<u32> = m.size().to_logical(m.scale_factor());
                (s.width, s.height)
            });
            let (lw, lh) = match self.window_scale {
                Some(n) => window_size_for_scale(base, n, monitor),
                None => initial_window_size(base.0, base.1, monitor),
            };
            let attrs = Window::default_attributes()
                .with_title(if self.has_rom { "z2rs" } else { NO_ROM_TITLE })
                .with_inner_size(winit::dpi::LogicalSize::new(lw, lh))
                .with_fullscreen(
                    self.start_fullscreen
                        .then_some(winit::window::Fullscreen::Borderless(None)),
                );
            crate::diag::breadcrumb(format_args!(
                "window: creating {lw}x{lh} logical (monitor {monitor:?}, fullscreen {})",
                self.start_fullscreen
            ));
            let window = match event_loop.create_window(attrs) {
                Ok(w) => w,
                Err(e) => {
                    crate::diag::breadcrumb(format_args!("create window failed: {e}"));
                    event_loop.exit();
                    return;
                }
            };
            // Shared, not leaked: the `pixels` surface owns an `Arc` clone
            // (raw-window-handle implements the handle traits for `Arc<W>`),
            // so it is `Pixels<'static>` and `suspended` can drop both and
            // free the window for real before the next `resumed` makes another.
            let window = Arc::new(window);
            let size = window.inner_size();
            let (sw, sh) = clamp_surface_size(size.width, size.height);
            crate::diag::breadcrumb(format_args!(
                "window: created, {}x{} physical, scale factor {}",
                size.width,
                size.height,
                window.scale_factor()
            ));
            match crate::gpu_present::create_pixels(
                (tex_w, tex_h),
                (sw, sh),
                Arc::clone(&window),
                &self.config.gpu_backend,
            ) {
                Ok(p) => {
                    crate::diag::breadcrumb("gpu: building the viewport pipeline");
                    self.renderer = Some(crate::gpu_present::ViewportRenderer::new(&p));
                    self.overlay.attach(&window, &p);
                    self.pixels = Some(p);
                    crate::diag::breadcrumb("gpu: ready");
                }
                Err(e) => {
                    crate::diag::breadcrumb(format_args!("create pixels surface: {e}"));
                    event_loop.exit();
                    return;
                }
            }
            self.surface_size = (sw, sh);
            self.tex_size = (tex_w, tex_h);
            self.window = Some(window);
            self.last_tick = Some(Instant::now());
            self.occluded = false;
            if self.lifecycle_suspended {
                // Back from `suspended` (Android only): run again, restart the
                // device and follow the pause state exactly as before the
                // suspend; drop whatever was queued so no stale burst plays.
                self.lifecycle_suspended = false;
                event_loop.set_control_flow(ControlFlow::Poll);
                self.audio.clear();
                self.audio.set_paused(self.paused);
                self.set_stream_running(true);
            }
        }

        /// The native window is going away (Android: app backgrounded or the
        /// screen turned off; never sent on desktop). Everything that holds
        /// the window is dropped here — surface first, then the window — and
        /// rebuilt by the next `resumed`. The emulator stops stepping (see
        /// `about_to_wait`) with its `paused` flag untouched, so it resumes in
        /// whatever state the player left it; audio is muted and the device
        /// released. SRAM is saved now because Android may kill the process
        /// at any point after `onStop` without another callback.
        fn suspended(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_none() {
                return;
            }
            self.renderer = None;
            self.overlay.detach();
            self.pixels = None;
            self.window = None;
            self.tex_size = (0, 0);
            self.lifecycle_suspended = true;
            self.keyboard.clear();
            self.fast_forward = false;
            crate::external_pad::clear_masks();
            self.audio.set_paused(true);
            self.set_stream_running(false);
            // Same wedge guard as the periodic autosave: never write faulted
            // RAM over a good save.
            if self.exec_errors_seen == 0 {
                let name = self.sram_file();
                if let Err(e) = save_sram_named(&mut self.emu.game, &self.data_dir, &name) {
                    eprintln!("suspend: SRAM save failed: {e}");
                }
            }
            // Nothing to present or step until the next `resumed`: sleep
            // instead of spinning the CPU in the background.
            event_loop.set_control_flow(ControlFlow::Wait);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            // The options overlay sees events first: `O` toggles it, and
            // while it is open the keyboard, mouse and touch are its own.
            if let Some(w) = self.window.clone() {
                match self.overlay.on_window_event(&w, &event) {
                    crate::overlay::EventUse::Pass => {}
                    crate::overlay::EventUse::Consumed => return,
                    crate::overlay::EventUse::Toggled(open) => {
                        self.on_overlay_toggled(open);
                        return;
                    }
                }
            }
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
                    //
                    // On Android the Kotlin activity owns pausing (its pause
                    // menu calls `setPaused`), and backgrounding arrives as
                    // `suspended`, which stops stepping by itself. Focus
                    // events there race the JNI unpause and could leave the
                    // game paused with no visible way out, so they never
                    // pause.
                    let pause_pref = self.config.pause_on_focus_loss
                        && !self.net_active()
                        && !cfg!(target_os = "android");
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
                                let name = self.sram_file();
                                let _ = load_sram_named(&mut self.emu.game, &self.data_dir, &name);
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
                                // Remember it so the next plain launch boots
                                // straight in (path only; saved only when it
                                // changed, never over an explicit --config).
                                if self.remember_rom && remember_rom_path(&mut self.config, &path) {
                                    match self.config.save() {
                                        Ok(()) => {
                                            eprintln!("remembered ROM location for next launch")
                                        }
                                        Err(e) => {
                                            eprintln!("could not remember the ROM location: {e}")
                                        }
                                    }
                                }
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
                        } else {
                            self.surface_size = (w, h);
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    // macOS Retina transitions (window moved across displays,
                    // display-scale change): the backing store size changed
                    // without a `Resized`. Without this arm the surface keeps
                    // the old physical size (stale/blurry presentation). The
                    // viewport follows `surface_size` on the next redraw.
                    if let (Some(p), Some(w)) = (self.pixels.as_mut(), self.window.as_ref()) {
                        let size = w.inner_size();
                        let (sw, sh) = clamp_surface_size(size.width, size.height);
                        if let Err(e) = p.resize_surface(sw, sh) {
                            eprintln!("present scale-factor resize {sw}x{sh}: {e:?}");
                        } else {
                            self.surface_size = (sw, sh);
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
                            } else {
                                self.surface_size = (sw, sh);
                            }
                        }
                    }
                }
                WindowEvent::ModifiersChanged(mods) => {
                    self.modifiers = mods.state();
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    use winit::keyboard::KeyCode as KC;
                    let pressed = event.state == ElementState::Pressed;
                    let repeat = event.repeat;
                    if let PhysicalKey::Code(code) = event.physical_key {
                        let m = self.modifiers;
                        // A fullscreen chord (Alt+Enter, Cmd+Ctrl+F) must not
                        // also press Start / P2 B in the game; releases always
                        // pass through so nothing sticks.
                        let fs_chord = pressed
                            && is_fullscreen_toggle(
                                code,
                                m.alt_key(),
                                m.control_key(),
                                m.super_key(),
                            );
                        if let Some(name) = Self::key_name(code) {
                            if !fs_chord {
                                self.keyboard.set(name, pressed);
                            }
                        }
                        if fs_chord && !repeat {
                            self.toggle_fullscreen();
                        } else if pressed && !repeat {
                            match code {
                                KC::Escape
                                    if esc_action(self.is_fullscreen())
                                        == EscAction::LeaveFullscreen =>
                                {
                                    // Leaving fullscreen instead of quitting:
                                    // a reach for a menu key must not end the
                                    // session. Esc again (windowed) quits.
                                    self.set_fullscreen(false);
                                }
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
                                KC::KeyM | KC::KeyN => {
                                    // Display-only audio mutes (music / sound
                                    // effects); never part of netplay.
                                    let (what, on) = crate::display_enh::with_audio_fx(|fx| {
                                        if code == KC::KeyM {
                                            ("music", fx.toggle_music_mute())
                                        } else {
                                            ("sound effects", fx.toggle_sfx_mute())
                                        }
                                    });
                                    eprintln!("{what} {}", if on { "muted" } else { "on" });
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
                                        let hash = self.emu.rom.hash_code.clone();
                                        match save_savestate_for(
                                            &self.emu.game,
                                            &self.data_dir,
                                            slot,
                                            hash.as_deref(),
                                        ) {
                                            Ok(p) => eprintln!("saved {}", p.display()),
                                            Err(e) => eprintln!("save failed: {e}"),
                                        }
                                    } else {
                                        let slot = self.savestate_slot;
                                        let hash = self.emu.rom.hash_code.clone();
                                        match load_savestate_for(
                                            &mut self.emu.game,
                                            &self.data_dir,
                                            slot,
                                            hash.as_deref(),
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
            if self.window.is_none() {
                // Suspended (or not yet resumed): no stepping, so a
                // backgrounded app neither advances the game nor fills the
                // audio ring. The clock restarts cleanly on resume.
                self.last_tick = None;
                return;
            }
            self.apply_external_requests();
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
            // LB + RB + Y on any gamepad toggles the options overlay. While
            // it holds the game still nothing else drains gilrs, so drain
            // here to keep the button state current.
            #[cfg(not(target_os = "android"))]
            {
                let holding = self.overlay.pauses(self.net_active());
                let chord = self.gilrs.as_mut().is_some_and(|g| {
                    if holding {
                        while g.next_event().is_some() {}
                    }
                    g.gamepads().any(|(_, gp)| {
                        gp.is_pressed(gilrs::Button::LeftTrigger)
                            && gp.is_pressed(gilrs::Button::RightTrigger)
                            && gp.is_pressed(gilrs::Button::North)
                    })
                });
                if self.overlay.chord(chord) {
                    let open = !self.overlay.is_open();
                    self.overlay.set_open(open);
                    self.on_overlay_toggled(open);
                }
            }
            // The open overlay holds the game still (except during netplay).
            let paused = self.paused || self.overlay.pauses(self.net_active());
            let mut steps = self.timer.advance(dt.as_secs_f64(), speed, paused);
            // Only with a live output stream draining the ring: without a
            // device the ring never empties and the nudge would hold the
            // emulator back. Rate limited: audio is a soft sync, never the
            // clock (see `AudioPacer`).
            if !paused && self.device_rate.is_some() {
                steps = self.pacer.apply(
                    steps,
                    dt.as_secs_f64(),
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
    // Display-only window settings (never part of any netplay identity).
    let window_scale = config.effective_window_scale();
    let config_scale_mode = config.effective_scale_mode();
    let start_fullscreen = config.fullscreen;
    crate::diag::breadcrumb("creating the event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let audio = SharedAudio::new(config.effective_audio_rate());
    crate::display_enh::with_audio_fx(|fx| fx.configure(&display.settings().display_enh));
    crate::diag::breadcrumb(format_args!(
        "audio: opening the default output device (game rate {} Hz)",
        audio.rate()
    ));
    // A panic inside the audio library (cpal has a few `expect`s on COM
    // calls) must not stop the game: it runs silent instead.
    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::audio::open_output_stream(&audio)
    }))
    .unwrap_or_else(|_| Err("the audio library panicked (see the log)".to_string()));
    let (stream, device_rate) = match opened {
        Ok((s, r)) => {
            if r != audio.rate() {
                eprintln!(
                    "audio: device runs {r} Hz vs game {} Hz (resampled; speed and pitch unaffected)",
                    audio.rate()
                );
            }
            // Prime one frame of silence so the meter doesn't open red.
            audio.prime_silence();
            (Some(s), Some(r))
        }
        Err(e) => {
            crate::diag::breadcrumb(format_args!("audio disabled: {e}"));
            (None, None)
        }
    };
    #[cfg(not(target_os = "android"))]
    let gilrs = open_gamepads(config.gamepads_enabled);

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
            z2_net::SessionConfig::host(emu.rom.body_crc32, emu.trapset_id, z2_net::COOP_TWO_LINKS)
        } else {
            z2_net::SessionConfig::guest(
                emu.rom.body_crc32,
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
            z2_net::RollbackConfig::host(emu.rom.body_crc32, emu.trapset_id, z2_net::COOP_TWO_LINKS)
        } else {
            z2_net::RollbackConfig::guest(
                emu.rom.body_crc32,
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
        socd: Default::default(),
        #[cfg(not(target_os = "android"))]
        gilrs,
        #[cfg(not(target_os = "android"))]
        active_pad: None,
        audio,
        _stream: stream,
        device_rate,
        timer: FrameTimer::new(),
        pacer: AudioPacer::new(),
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
        #[cfg(not(target_os = "android"))]
        active_pad2: None,
        #[cfg(not(target_os = "android"))]
        pad_order: Vec::new(),
        p2_pad: args.p2_pad.or(config_p2_pad),
        p2_follow: args.p2_follow,
        tex_size: (0, 0),
        window_scale: args.scale.or(window_scale),
        start_fullscreen: args.fullscreen || start_fullscreen,
        scale_mode: args.scale_mode.unwrap_or(config_scale_mode),
        surface_size: (1, 1),
        renderer: None,
        lifecycle_suspended: false,
        #[cfg(not(target_os = "android"))]
        pads_heard: Vec::new(),
        presented_once: false,
        remember_rom: args.config.is_none(),
        modifiers: winit::keyboard::ModifiersState::empty(),
        #[cfg(feature = "netplay")]
        start: Instant::now(),
        #[cfg(feature = "netplay")]
        net,
        #[cfg(feature = "netplay")]
        rollback,
        overlay: crate::overlay::Overlay::new(),
        config_file: args
            .config
            .as_ref()
            .map_or_else(crate::config::config_path, PathBuf::from),
    };
    // Keep audio in lockstep with the initial UI state.  Both cartless and
    // ROM launches now start running; the normal frame path produces audio,
    // while explicit pause/focus transitions still mute the ring.
    handler.audio.set_paused(handler.paused);
    crate::diag::breadcrumb("starting the event loop");
    event_loop
        .run_app(&mut handler)
        .map_err(|e| format!("event loop: {e}"))?;
    crate::diag::breadcrumb("event loop ended; exiting normally");
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

    /// Frame sizes the launcher's widescreen options produce (scale 1).
    const OFF: (u32, u32) = (256, 240);
    const W16X10: (u32, u32) = (384, 240);
    const W16X9: (u32, u32) = (432, 240);

    fn fit(surface: (u32, u32), tex: (u32, u32)) -> Viewport {
        let wide = u8::from(tex.0 > 256);
        compute_viewport(surface, tex, ScaleMode::Fit, fill_crop_texels(wide, 1))
    }

    fn rect(v: &Viewport) -> (u32, u32, u32, u32) {
        (v.x, v.y, v.w, v.h)
    }

    /// Fullscreen always fills the screen height for 4:3 (fractional scale,
    /// pillarboxed sides) — the Legion Go report showed 6x = 1440 of 1600.
    #[test]
    fn viewport_fit_4x3_fills_height_on_common_screens() {
        for (surface, want) in [
            ((1920, 1080), (384, 0, 1152, 1080)),
            ((2560, 1600), (426, 0, 1707, 1600)),
            ((1280, 800), (213, 0, 853, 800)),
            ((3840, 2160), (768, 0, 2304, 2160)),
        ] {
            let v = fit(surface, OFF);
            assert_eq!(rect(&v), want, "{surface:?}");
            assert_eq!(v.h, surface.1, "fills the height on {surface:?}");
            assert_eq!((v.src_x, v.src_w), (0.0, 256.0), "4:3 is never trimmed");
        }
        assert!((fit((2560, 1600), OFF).scale - 6.6667).abs() < 1e-3);
    }

    /// "16:9 doesn't cover my 16:9 screen": 432x240 is 1.8:1, so it fills a
    /// 16:9 display by trimming under 3 px of margin per side.
    #[test]
    fn viewport_fit_16x9_covers_16x9_displays() {
        for surface in [(1920, 1080), (3840, 2160), (1280, 720), (2560, 1440)] {
            let v = fit(surface, W16X9);
            assert!(v.fills(surface), "{surface:?}: {v:?}");
            let trim = v.src_x;
            assert!(
                trim > 0.0 && trim < 3.0,
                "{surface:?} trims {trim} px per side"
            );
            assert!((v.src_w - (432.0 - 2.0 * trim)).abs() < 1e-9);
        }
        let v = fit((1920, 1080), W16X9);
        assert!((v.scale - 4.5).abs() < 1e-9);
        assert!((v.src_w - 426.6667).abs() < 1e-3);
    }

    /// 16:10 screens (Legion Go 2560x1600, 1280x800 laptops/Steam Deck):
    /// the 16:10 preset fills them exactly; 16:9 letterboxes minimally
    /// (would need 24 px per side of trim, more than the one-tile allowance)
    /// and never exceeds the surface, so nothing is clipped.
    #[test]
    fn viewport_fit_on_16x10_displays() {
        assert_eq!(rect(&fit((2560, 1600), W16X10)), (0, 0, 2560, 1600));
        assert_eq!(rect(&fit((1280, 800), W16X10)), (0, 0, 1280, 800));
        assert_eq!(rect(&fit((2560, 1600), W16X9)), (0, 89, 2560, 1422));
        assert_eq!(rect(&fit((1280, 800), W16X9)), (0, 44, 1280, 711));
        for s in [(2560, 1600), (1280, 800)] {
            let v = fit(s, W16X9);
            assert_eq!((v.src_x, v.src_w), (0.0, 432.0), "no trim, whole picture");
        }
        // 16:10 on 16:9 displays: pillarboxed, height filled.
        assert_eq!(rect(&fit((1920, 1080), W16X10)), (96, 0, 1728, 1080));
        assert_eq!(rect(&fit((3840, 2160), W16X10)), (192, 0, 3456, 2160));
    }

    /// Integer mode keeps whole multiples (the old look) for those who want it.
    #[test]
    fn viewport_integer_mode() {
        let int = |s, t| compute_viewport(s, t, ScaleMode::Integer, 64);
        assert_eq!(rect(&int((1920, 1080), OFF)), (448, 60, 1024, 960));
        assert_eq!(rect(&int((2560, 1600), OFF)), (512, 80, 1536, 1440));
        assert_eq!(rect(&int((1280, 800), OFF)), (256, 40, 768, 720));
        assert_eq!(rect(&int((3840, 2160), OFF)), (768, 0, 2304, 2160));
        assert_eq!(rect(&int((1920, 1080), W16X9)), (96, 60, 1728, 960));
        assert_eq!(rect(&int((2560, 1600), W16X9)), (200, 200, 2160, 1200));
        assert_eq!(rect(&int((3840, 2160), W16X9)), (192, 120, 3456, 1920));
        assert_eq!(rect(&int((2560, 1600), W16X10)), (128, 80, 2304, 1440));
        assert_eq!(int((1920, 1080), W16X9).src_w, 432.0, "never trims");
        // Smaller than 1x: shrinks instead of cropping.
        let v = int((200, 200), OFF);
        assert!(v.w <= 200 && v.h <= 200 && v.scale < 1.0, "{v:?}");
    }

    /// An HD texture (any output scale) lands on exactly the same rectangle
    /// as the 1x frame, and a texture larger than the window is shrunk into
    /// it — the old pixels scaler cropped it (the Windows 16:10 clipping).
    #[test]
    fn viewport_hd_texture_matches_1x_and_shrinks_to_fit() {
        for surface in [(1920, 1080), (2560, 1600), (1280, 800), (3840, 2160)] {
            for base in [OFF, W16X10, W16X9] {
                let wide = u8::from(base.0 > 256) * 11;
                let v1 = compute_viewport(surface, base, ScaleMode::Fit, fill_crop_texels(wide, 1));
                let hd = (base.0 * 4, base.1 * 4);
                let v4 = compute_viewport(surface, hd, ScaleMode::Fit, fill_crop_texels(wide, 4));
                assert_eq!(rect(&v1), rect(&v4), "{surface:?} {base:?}");
                assert!((v4.src_x - 4.0 * v1.src_x).abs() < 1e-6);
            }
        }
        // 4x 16:9 pack (1728x960) in a 1280x800 window.
        let v = compute_viewport((1280, 800), (1728, 960), ScaleMode::Fit, 32);
        assert!(v.w <= 1280 && v.h <= 800, "{v:?}");
        assert!(v.scale < 1.0);
        assert_eq!(v.src_w, 1728.0, "whole picture, shrunk");
    }

    #[test]
    fn viewport_never_exceeds_surface_and_is_centered() {
        for surface in [
            (1, 1),
            (7, 900),
            (1920, 1080),
            (2560, 1600),
            (1280, 800),
            (3840, 2160),
            (2560, 1080),
        ] {
            for tex in [OFF, W16X10, W16X9, (560, 240), (1728, 960)] {
                for mode in [ScaleMode::Fit, ScaleMode::Integer] {
                    let v =
                        compute_viewport(surface, tex, mode, fill_crop_texels(11, tex.0 / 432 + 1));
                    assert!(
                        v.x + v.w <= surface.0 && v.y + v.h <= surface.1,
                        "{surface:?} {tex:?} {v:?}"
                    );
                    assert!(v.w >= 1 && v.h >= 1);
                    assert!(v.x.abs_diff(surface.0 - v.x - v.w) <= 1, "centered x {v:?}");
                    assert!(v.y.abs_diff(surface.1 - v.y - v.h) <= 1, "centered y {v:?}");
                    assert!(v.src_x >= 0.0 && v.src_x + v.src_w <= f64::from(tex.0) + 1e-9);
                }
            }
        }
        let z = compute_viewport((0, 0), OFF, ScaleMode::Fit, 0);
        assert_eq!((z.w, z.h), (0, 0));
    }

    #[test]
    fn fill_crop_only_for_widescreen() {
        assert_eq!(fill_crop_texels(0, 1), 0);
        assert_eq!(fill_crop_texels(0, 4), 0);
        assert_eq!(fill_crop_texels(11, 1), 8);
        assert_eq!(fill_crop_texels(11, 4), 32);
        assert_eq!(fill_crop_texels(8, 2), 16);
    }

    #[test]
    fn scale_mode_parses_and_defaults_to_fit() {
        assert_eq!(ScaleMode::default(), ScaleMode::Fit);
        assert_eq!(ScaleMode::parse("fit"), Some(ScaleMode::Fit));
        assert_eq!(ScaleMode::parse(" Integer "), Some(ScaleMode::Integer));
        assert_eq!(ScaleMode::parse("stretch"), None);
        for m in [ScaleMode::Fit, ScaleMode::Integer] {
            assert_eq!(ScaleMode::parse(m.as_str()), Some(m));
        }
        let a = parse_native_args(&sv(&["--scale-mode", "integer"])).unwrap();
        assert_eq!(a.scale_mode, Some(ScaleMode::Integer));
        assert_eq!(parse_native_args(&sv(&[])).unwrap().scale_mode, None);
        let err = parse_native_args(&sv(&["--scale-mode", "zoom"])).expect_err("bad");
        assert!(err.contains("--scale-mode expects fit | integer"), "{err}");
        assert!(NATIVE_USAGE.contains("--scale-mode"));
    }

    #[test]
    fn blit_viewport_letterboxes_and_trims() {
        // 4x2 texture: left half red, right half blue.
        let mut tex = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                tex.extend_from_slice(if x < 2 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let vp = compute_viewport((16, 4), (4, 2), ScaleMode::Fit, 0);
        assert_eq!(rect(&vp), (4, 0, 8, 4));
        let out = blit_viewport(&tex, (4, 2), (16, 4), &vp);
        let px = |x: usize, y: usize| &out[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
        assert_eq!(px(0, 0), &[0, 0, 0, 255], "pillarbox is black");
        assert_eq!(px(15, 3), &[0, 0, 0, 255]);
        assert_eq!(px(4, 0), &[255, 0, 0, 255]);
        assert_eq!(px(11, 3), &[0, 0, 255, 255]);
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
        // 60 frames at 60.0988 Hz: 60 * 44100 / 60.0988 ≈ 44027.6 samples.
        assert!(depth.abs_diff(44_028) <= 2, "depth={depth}, want ~44028");
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
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF));
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

    /// Simulate `secs` of the windowed loop at `tick_hz` OS ticks per second
    /// with a device draining `drain_hz` game samples/s from a ring fed one
    /// nominal frame per step; returns emulated frames per second.
    fn simulated_fps(tick_hz: f64, drain_hz: f64, secs: f64) -> f64 {
        let rate = 44100u32;
        let frame = f64::from(rate) / NTSC_HZ;
        let mut timer = FrameTimer::new();
        let mut pacer = AudioPacer::new();
        let mut depth = frame; // startup prime
        let dt = 1.0 / tick_hz;
        let ticks = (secs * tick_hz) as usize;
        let mut frames = 0usize;
        for _ in 0..ticks {
            depth = (depth - drain_hz * dt).max(0.0);
            let steps = timer.advance(dt, 1.0, false);
            let steps = pacer.apply(steps, dt, depth as usize, rate, true);
            depth += steps as f64 * frame;
            frames += steps;
        }
        frames as f64 / secs
    }

    #[test]
    fn audio_pacer_is_a_soft_sync_not_the_clock() {
        // Matched drain: the NES rate at any display/tick rate.
        for tick_hz in [60.0, 144.0, 240.0, 1000.0, 5000.0] {
            let fps = simulated_fps(tick_hz, 44100.0, 60.0);
            assert!((fps - NTSC_HZ).abs() < 0.2, "tick {tick_hz}: {fps} fps");
        }
        // The Windows bug: raw 48/96/192 kHz drain against a 44.1 kHz ring.
        // The old per-tick nudge ran the game at the drain rate; the pacer
        // caps the pull at one frame per second.
        for tick_hz in [60.0, 144.0, 1000.0, 5000.0] {
            for drain in [48000.0, 96000.0, 192000.0] {
                let fps = simulated_fps(tick_hz, drain, 60.0);
                assert!(
                    fps <= NTSC_HZ + 1.0 / PACE_NUDGE_INTERVAL_SECS + 0.1,
                    "tick {tick_hz} drain {drain}: {fps} fps"
                );
            }
        }
        // A slow drain can hold back at most one frame per second too.
        let fps = simulated_fps(1000.0, 22050.0, 60.0);
        assert!(
            fps >= NTSC_HZ - 1.0 / PACE_NUDGE_INTERVAL_SECS - 0.1,
            "{fps}"
        );
    }

    #[test]
    fn audio_pacer_rate_limits_nudges() {
        let mut p = AudioPacer::new();
        // Starving ring: the first tick nudges, the next ones within 1 s do not.
        assert_eq!(p.apply(1, 0.001, 0, 44100, true), 2);
        for _ in 0..900 {
            assert_eq!(p.apply(1, 0.001, 0, 44100, true), 1);
        }
        // After the interval, one more nudge is allowed.
        assert_eq!(p.apply(1, 0.2, 0, 44100, true), 2);
        // A decision that changes nothing (cannot hold back 0) spends nothing.
        let mut q = AudioPacer::new();
        assert_eq!(q.apply(0, 0.001, 8 * 735, 44100, true), 0);
        assert_eq!(q.apply(1, 0.001, 8 * 735, 44100, true), 0);
        // Fast-forward bypasses entirely.
        let mut f = AudioPacer::new();
        assert_eq!(f.apply(4, 0.001, 0, 44100, false), 4);
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
            rgba.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0x7C, 0x7C, 0x7C, 0xFF]),
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
            rgba.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0xFC, 0xFC, 0xFC, 0xFF]),
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
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF));
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
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11), &[]),
            WIDE_TRAPSET_PIN_VALUE,
            "session_trapset_id changed: update z2-web's WIDE_TRAPSET_PIN_VALUE in \
             lockstep or native and web peers can no longer connect"
        );
        // Off leaves the identity alone; margins differ from each other.
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, None, &[]),
            TRAPSET_PIN_VALUE
        );
        assert_ne!(
            session_trapset_id(TRAPSET_PIN_VALUE, Some(8), &[]),
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11), &[])
        );
        // Enhancements: all off is empty identity bytes, so nothing moves.
        let off = Enhancements::default().identity_bytes();
        assert!(off.is_empty());
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11), &off),
            WIDE_TRAPSET_PIN_VALUE
        );
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, None, &[1, b'C', 1, 0, 0, 0]),
            ENH_TRAPSET_PIN_VALUE,
            "enhancement fold changed: update z2-web's ENH_TRAPSET_PIN_VALUE in lockstep"
        );
        let mut cheat = Enhancements::default();
        cheat.cheats.invincible = true;
        assert_eq!(cheat.identity_bytes(), [1, b'C', 1, 0, 0, 0]);
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
    fn enhancement_flags_parse_and_resolve() {
        let argv = |words: &[&str]| {
            std::iter::once("z2-native")
                .chain(words.iter().copied())
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let p = parse_native_args(&argv(&[])).expect("parses");
        assert_eq!(p.enhancements, None);
        assert_eq!(p.display_enh, None);
        let p = parse_native_args(&argv(&[
            "--enh-json",
            r#"{"cheats":{"invincible":true}}"#,
            "--display-enh-json",
            r#"{"screen_shake":true,"music_volume":99}"#,
        ]))
        .expect("parses");
        assert!(p.enhancements.unwrap().cheats.invincible);
        let d = p.display_enh.unwrap();
        assert!(d.screen_shake);
        assert_eq!(d.music_volume, 10, "clamped");
        assert!(parse_native_args(&argv(&["--enh-json", "{nope"])).is_err());
        assert!(parse_native_args(&argv(&["--enh-json", "@/no/such/enh.json"])).is_err());
        let dir = std::env::temp_dir().join(format!("z2-enh-arg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("enh.json");
        std::fs::write(&f, Enhancements::zalia_preset().to_json()).unwrap();
        let at = format!("@{}", f.display());
        let p = parse_native_args(&argv(&["--enh-json", &at])).expect("@path parses");
        assert_eq!(p.enhancements, Some(Enhancements::zalia_preset()));
        let _ = std::fs::remove_dir_all(&dir);

        // Config value applies interactively, not under --movie; the flag
        // always wins.
        let config = NativeConfig {
            enhancements: Enhancements::zalia_preset(),
            display_enh: DisplayEnh::zalia_preset(),
            ..NativeConfig::default()
        };
        let none = NativeArgs::default();
        assert_eq!(
            resolve_enhancements(&none, &config),
            Enhancements::zalia_preset()
        );
        assert_eq!(
            resolve_enhancements(&none, &NativeConfig::default()),
            Enhancements::default()
        );
        let movie = NativeArgs {
            movie: Some("run.fm2".into()),
            ..NativeArgs::default()
        };
        assert_eq!(
            resolve_enhancements(&movie, &config),
            Enhancements::default()
        );
        let explicit = NativeArgs {
            enhancements: Some(Enhancements::zalia_preset()),
            ..movie.clone()
        };
        assert_eq!(
            resolve_enhancements(&explicit, &config),
            Enhancements::zalia_preset()
        );
        let feats = resolve_features(&movie, &config).unwrap();
        assert_eq!(feats.enhancements, Enhancements::default());
        // Display enhancements are cosmetic: they follow the config even for movies.
        let d = resolve_display(&movie, &config).unwrap();
        assert_eq!(d.display_enh, DisplayEnh::zalia_preset());
    }

    #[test]
    fn p2_follow_parses_and_bounds_the_delay() {
        let argv = |v: &str| {
            ["z2-native", "--p2-follow", v]
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_native_args(&argv("0")).expect("parses").p2_follow,
            Some(0)
        );
        assert_eq!(
            parse_native_args(&argv("24")).expect("parses").p2_follow,
            Some(24)
        );
        assert!(parse_native_args(&argv("601")).is_err());
        assert!(parse_native_args(&argv("-1")).is_err());
        assert_eq!(
            parse_native_args(&argv("600")).expect("parses").p2_follow,
            Some(600)
        );
    }

    #[test]
    fn follow_pad_replays_player_one_late_without_start_or_select() {
        use z2_core::game::{BTN_A, BTN_RIGHT, BTN_SELECT, BTN_START};
        let movie = [BTN_RIGHT, BTN_RIGHT | BTN_A, BTN_START, BTN_SELECT | BTN_A];
        // Idle until the delay has elapsed.
        assert_eq!(follow_pad(&movie, 0, 2), 0);
        assert_eq!(follow_pad(&movie, 1, 2), 0);
        assert_eq!(follow_pad(&movie, 2, 2), BTN_RIGHT);
        assert_eq!(follow_pad(&movie, 3, 2), BTN_RIGHT | BTN_A);
        // Delay 0 mirrors the current frame; Start/Select never reach pad 2.
        assert_eq!(follow_pad(&movie, 1, 0), BTN_RIGHT | BTN_A);
        assert_eq!(follow_pad(&movie, 2, 0), 0);
        assert_eq!(follow_pad(&movie, 3, 0), BTN_A);
    }

    #[test]
    fn rom_identity_fold_is_pinned_and_neutral_when_empty() {
        assert_eq!(
            fold_rom_identity(TRAPSET_PIN_VALUE, &[], false),
            TRAPSET_PIN_VALUE
        );
        assert_eq!(
            fold_rom_identity(TRAPSET_PIN_VALUE, &[0xC358, 0xDF79], false),
            UNTRAP_PIN_VALUE
        );
        assert_ne!(
            fold_rom_identity(TRAPSET_PIN_VALUE, &[], true),
            TRAPSET_PIN_VALUE,
            "--no-traps splits peers"
        );
    }

    #[test]
    fn randomizer_flags_parse_and_validate() {
        let argv = |v: &[&str]| -> Vec<String> {
            std::iter::once("z2-native")
                .chain(v.iter().copied())
                .map(String::from)
                .collect()
        };
        let std_flags = z2_rando::flags::Preset::Standard.flags().to_flag_string();
        let a = parse_native_args(&argv(&[
            "--seed",
            "race 1",
            "--rando-flags",
            &std_flags,
            "--rando-spoiler",
            "/tmp/s.txt",
            "--no-traps",
        ]))
        .unwrap();
        assert_eq!(a.seed.as_deref(), Some("race 1"));
        assert_eq!(a.rando_flags.as_deref(), Some(std_flags.as_str()));
        assert!(a.no_traps);
        let spec = a.rando_spec().unwrap().unwrap();
        assert_eq!(spec.flags, z2_rando::flags::Preset::Standard.flags());
        assert!(parse_native_args(&argv(&["--rando-flags", "zzz"])).is_err());
        assert!(parse_native_args(&argv(&["--rando-spoiler", "x"])).is_err());
        let plain = parse_native_args(&argv(&[])).unwrap();
        assert_eq!(plain.rando_spec().unwrap(), None);
        assert!(NATIVE_USAGE.contains("--rando-flags"));
        assert_eq!(rando_suffix("AB12CD"), " [rando AB12CD]");
    }

    #[test]
    fn trusted_body_builder_accepts_both_layouts() {
        // Synthetic cartridges (no ROM): an RTS at the reset target, in the
        // vanilla 8-bank layout and the expanded 16-bank one.
        for units in [8usize, 16] {
            let prg = units * 0x4000;
            let mut body = vec![0u8; prg + 128 * 1024];
            body[prg - 0x4000] = 0x60;
            body[prg - 4] = 0x00;
            body[prg - 3] = 0xC0;
            let emu = emu_from_trusted_body_with(&body, None, None, 44_100, Features::default())
                .expect("builds");
            assert_eq!(emu.game.prg.len(), prg);
            assert_eq!(emu.rom.body_crc32, z2_assets::rom::crc32_ieee(&body));
            assert!(emu.rom.untrapped.is_empty());
        }
        assert!(
            emu_from_trusted_body_with(&[0u8; 1000], None, None, 44_100, Features::default())
                .is_err()
        );
        let no_traps = Features {
            no_traps: true,
            ..Features::default()
        };
        let mut body = vec![0u8; 8 * 0x4000 + 128 * 1024];
        body[8 * 0x4000 - 0x4000] = 0x60;
        body[8 * 0x4000 - 3] = 0xC0;
        let emu = emu_from_trusted_body_with(&body, None, None, 44_100, no_traps).unwrap();
        assert!(!emu.game.traps.enabled);
        assert_eq!(
            emu.trapset_id,
            fold_rom_identity(emu.trapset_base, &[], true)
        );
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
                rando: None,
                no_traps: false,
                enhancements: Default::default(),
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
                    display_enh: Default::default(),
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
                        display_enh: Default::default(),
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
                    assert!(
                        rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0xFF),
                        "opaque"
                    );
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

    fn sv(args: &[&str]) -> Vec<String> {
        std::iter::once("z2-native")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn scale_flag_parses_and_bounds() {
        assert_eq!(
            parse_native_args(&sv(&["--scale", "1"])).unwrap().scale,
            Some(1)
        );
        assert_eq!(
            parse_native_args(&sv(&["--scale", "8"])).unwrap().scale,
            Some(8)
        );
        assert_eq!(parse_native_args(&sv(&[])).unwrap().scale, None);
        for bad in ["0", "9", "x", "-1", ""] {
            let err = parse_native_args(&sv(&["--scale", bad])).expect_err(bad);
            assert!(err.contains("--scale expects 1-8"), "{bad}: {err}");
            assert!(err.contains("usage:"), "usage printed for {bad}");
        }
        assert!(
            parse_native_args(&sv(&["--scale"])).is_err(),
            "missing value"
        );
    }

    #[test]
    fn fullscreen_flag_parses() {
        assert!(
            parse_native_args(&sv(&["--fullscreen"]))
                .unwrap()
                .fullscreen
        );
        assert!(!parse_native_args(&sv(&[])).unwrap().fullscreen);
        let a = parse_native_args(&sv(&["--fullscreen", "--scale", "2"])).unwrap();
        assert!(a.fullscreen);
        assert_eq!(a.scale, Some(2));
        assert!(NATIVE_USAGE.contains("--scale N"));
        assert!(NATIVE_USAGE.contains("--fullscreen"));
        assert!(NATIVE_USAGE.contains("F11"));
        assert!(NATIVE_USAGE.contains("Alt+Enter"));
    }

    /// Scale and fullscreen are display-only: they must never reach the
    /// game features (and so never the netplay trap-set identity).
    #[test]
    fn window_settings_never_touch_features() {
        let config = NativeConfig::default();
        let plain = resolve_features(&NativeArgs::default(), &config).unwrap();
        let args = NativeArgs {
            scale: Some(5),
            fullscreen: true,
            ..NativeArgs::default()
        };
        let cfg = NativeConfig {
            window_scale: Some(7),
            fullscreen: true,
            ..NativeConfig::default()
        };
        assert_eq!(resolve_features(&args, &cfg).unwrap(), plain);
        assert_eq!(
            resolve_display(&args, &cfg).unwrap(),
            resolve_display(&NativeArgs::default(), &config).unwrap()
        );
    }

    #[test]
    fn esc_leaves_fullscreen_and_quits_windowed() {
        assert_eq!(esc_action(true), EscAction::LeaveFullscreen);
        assert_eq!(esc_action(false), EscAction::Quit);
    }

    #[test]
    fn fullscreen_chords() {
        use winit::keyboard::KeyCode as KC;
        assert!(is_fullscreen_toggle(KC::F11, false, false, false));
        assert!(is_fullscreen_toggle(KC::Enter, true, false, false));
        assert!(is_fullscreen_toggle(KC::NumpadEnter, true, false, false));
        assert!(is_fullscreen_toggle(KC::KeyF, false, true, true));
        // Plain Enter is Start and plain F is P2's B: never a toggle.
        assert!(!is_fullscreen_toggle(KC::Enter, false, false, false));
        assert!(!is_fullscreen_toggle(KC::KeyF, false, false, false));
        assert!(!is_fullscreen_toggle(KC::KeyF, false, true, false));
        // Existing hotkeys stay theirs.
        for k in [
            KC::F5,
            KC::F6,
            KC::F7,
            KC::Tab,
            KC::KeyP,
            KC::Period,
            KC::Escape,
        ] {
            assert!(!is_fullscreen_toggle(k, true, true, true), "{k:?}");
        }
    }

    #[test]
    fn window_size_for_scale_multiplies_and_fits_the_logical_monitor() {
        // Plain 256x240 at 3x with no monitor info.
        assert_eq!(window_size_for_scale((256, 240), 3, None), (768.0, 720.0));
        // Widescreen 16:9 (11 tiles/side = 432 wide) at 2x.
        assert_eq!(window_size_for_scale((432, 240), 2, None), (864.0, 480.0));
        // Too big for a 1280x800 screen: steps down to the largest that fits
        // (240*3 + 120 > 800, so 2x).
        assert_eq!(
            window_size_for_scale((256, 240), 8, Some((1280, 800))),
            (512.0, 480.0)
        );
        // Legion Go / 16:10 laptop at 200%: 2560x1600 physical is 1280x800
        // logical. The launcher's default 3x of 16:9 (1296x720) no longer
        // opens past the screen edge.
        assert_eq!(
            window_size_for_scale((432, 240), 3, Some((1280, 800))),
            (864.0, 480.0)
        );
        // 1920x1200 at 150% = 1280x800 logical, 6x asked: 2x fits.
        assert_eq!(
            window_size_for_scale((384, 240), 6, Some((1280, 800))),
            (768.0, 480.0)
        );
        // Smaller than 1x on a tiny screen: shrinks to fit (the viewport
        // blit scales the texture down; it no longer crops).
        let (w, h) = window_size_for_scale((256, 240), 4, Some((200, 300)));
        assert!(w <= 184.0 && h <= 180.0, "{w}x{h}");
        assert!((w / h - 256.0 / 240.0).abs() < 0.02, "keeps the aspect");
        // Out-of-range scales are clamped, not trusted.
        assert_eq!(window_size_for_scale((256, 240), 0, None), (256.0, 240.0));
        assert_eq!(
            window_size_for_scale((256, 240), 99, None),
            (2048.0, 1920.0)
        );
    }

    /// Every launcher scale (1-6) and widescreen option on the logical sizes
    /// of common displays: the window (plus chrome) always fits the monitor.
    #[test]
    fn window_always_fits_common_monitors() {
        // Logical sizes: 1080p @100/125/150%, 1440p, 2560x1600 @100/150/200%,
        // 4K @200%.
        for monitor in [
            (1920, 1080),
            (1536, 864),
            (1280, 720),
            (2560, 1440),
            (2560, 1600),
            (1706, 1066),
            (1280, 800),
        ] {
            for base in [(256, 240), (384, 240), (432, 240), (560, 240)] {
                for scale in 1..=6 {
                    let (w, h) = window_size_for_scale(base, scale, Some(monitor));
                    assert!(
                        w + f64::from(WINDOW_CHROME_W) <= f64::from(monitor.0)
                            && h + f64::from(WINDOW_CHROME_H) <= f64::from(monitor.1),
                        "{base:?} x{scale} on {monitor:?} -> {w}x{h}"
                    );
                }
                let (w, h) = initial_window_size(base.0, base.1, Some(monitor));
                assert!(w <= f64::from(monitor.0) && h + 120.0 <= f64::from(monitor.1));
            }
        }
    }

    #[test]
    fn remember_rom_path_stores_absolute_and_reports_change() {
        let mut c = NativeConfig::default();
        let abs = std::env::temp_dir().join("z2rs-remember-test.nes");
        assert!(remember_rom_path(&mut c, &abs), "first time changes");
        assert_eq!(c.rom_path.as_deref(), Some(abs.to_string_lossy().as_ref()));
        assert!(!remember_rom_path(&mut c, &abs), "same path: no save");
        // A relative path is stored absolute.
        let mut c = NativeConfig::default();
        assert!(remember_rom_path(&mut c, Path::new("some/rel.nes")));
        let stored = c.rom_path.expect("stored");
        assert!(Path::new(&stored).is_absolute(), "{stored}");
        assert!(stored.ends_with("rel.nes"), "{stored}");
    }

    #[test]
    fn no_rom_message_is_plain_and_names_a_stale_path() {
        let generic = no_rom_message(None);
        assert!(
            generic.contains("drop your Zelda II (USA) .nes file"),
            "{generic}"
        );
        assert!(generic.contains("launcher"), "{generic}");
        assert!(generic.contains("--rom"), "{generic}");
        assert!(!generic.contains("no longer"), "{generic}");
        let stale = no_rom_message(Some("/gone/zelda2.nes"));
        assert!(stale.contains("no longer at /gone/zelda2.nes"), "{stale}");
        assert!(stale.contains("--rom"), "{stale}");
    }

    /// A config `rom_path` whose file has gone is skipped (not handed to the
    /// loader), and the error names it — unless `$Z2_ROM` supplies a ROM.
    #[test]
    fn stale_config_rom_path_is_named() {
        let missing = std::env::temp_dir().join("z2rs-definitely-missing-rom.nes");
        let config = NativeConfig {
            rom_path: Some(missing.to_string_lossy().into_owned()),
            ..NativeConfig::default()
        };
        match resolve_rom_path(None, &config) {
            Err(e) => assert!(e.contains("no longer at"), "{e}"),
            // Only reachable with a real $Z2_ROM in the environment.
            Ok(p) => assert_ne!(p, missing, "a missing config path is never returned"),
        }
        // An explicit --rom is never filtered.
        assert_eq!(
            resolve_rom_path(Some("/nope.nes"), &config).unwrap(),
            PathBuf::from("/nope.nes")
        );
    }
}
