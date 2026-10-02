//! Display-only enhancements for the native frontend (ZALiA-inspired; see
//! README.md).
//!
//! The settings type is [`DisplayEnh`] (defined in `z2_core::enh::display`
//! so the launcher and the web build share its JSON shape). It reaches the
//! present path through [`crate::app::DisplaySettings::display_enh`]
//! (config key `display_enh`, CLI `--display-enh-json`). Nothing here writes
//! to the game: every observation reads CPU RAM / palette RAM after the
//! frame, so none of it touches the netplay identity, and all of it is
//! allowed under `--movie`. With [`DisplayEnh::default`] the present and
//! audio paths skip this module entirely (byte-identical output).
//!
//! # Pieces
//!
//! * [`DisplayFx`]: per-[`crate::app::Display`] state, run once per
//!   **presented** frame: flash recolouring of the indexed frame (and of the
//!   render record for widescreen margins / HD packs), the CPU post effects,
//!   screen shake, the quest timer and the dev overlays.
//! * [`PostFx`] / [`apply_post_effects`]: brightness, saturation, bloom and
//!   blur over the composed RGBA frame. Scanlines are drawn by the window
//!   blit at output resolution (`gpu_present`, see [`scanline_strength`]);
//!   a texture-space scanline would alias at 1x.
//! * [`AudioFx`]: music / sound-effect volumes, the `M` / `N` mutes and the
//!   reduced low-HP beep, applied where each frame's APU register writes are
//!   synthesized ([`crate::app::drain_audio_frame`]).
//!
//! # RAM observables (all verified on a ROM run, see the tests)
//!
//! | Address | Use |
//! |---------|-----|
//! | `$074B` | spell / event flash counter (`$20`, `$A0` after a cast; `$E8` on the boss-death path `bank7_code29`; `$C0` wise men). While nonzero, the NMI cycles the backdrop colour (palette `$3F00`). |
//! | `$074A` | last-cast spell (`selector + 1`; `8` = Thunder) |
//! | `$0700`, `$076C` | lives, game state (`0`/`0` before the quest starts) |
//! | `$07FF`, `$07FE`, `$07FD` | id of the sound effect that owns pulse 1, pulse 2, noise (0 = music owns it); set by the bank 6 SFX buses `$EF`, `$EE`, `$ED` |
//! | `$07FF == $40` | the low-HP beep (every 48 frames while HP is low) |
//! | `$00CC`, `$0029`, `$004D`, `$003B`, `$009F`, `$0080` | Link screen X, Y, world X, page, facing, sprite |
//! | `$047E`, `$0480` | sword anchor X, Y (`>= $F8` retracted) |
//! | `$002A+`, `$003C+`, `$004E+`, `$00A1+`, `$00B6+`, `$00C2+` | enemy slots 0-5: Y, page, X, id, exists, HP |
//! | `$0736` | game mode (`$0B` = side view) |

use std::collections::VecDeque;

use z2_core::enh::display::MAX_VOLUME;
pub use z2_core::enh::{DisplayEnh, FlashColor};
use z2_core::game::{Game, FRAME_H, FRAME_LEN, FRAME_W};
use z2_core::player::{body_box, sword_box, BODY_DY, SWORD_RETRACTED};
use z2_ppu::FrameRecord;

// ------------------------------------------------------------ RAM addresses

/// Spell / event flash counter.
pub const RAM_FLASH: usize = 0x074B;
/// Last-cast spell (`selector + 1`, written with the cast flash by the
/// post-cast bookkeeping `$8DF5-$8E1D`, see `z2_core::player_magic`).
pub const RAM_LAST_CAST: usize = 0x074A;
/// Lives.
pub const RAM_LIVES: usize = 0x0700;
/// Game state.
pub const RAM_GAME_STATE: usize = 0x076C;
/// Game mode.
pub const RAM_MODE: usize = 0x0736;
/// Sound effect owning pulse 1 (bus `$EF`); the low-HP beep plays here.
pub const RAM_SFX_PULSE1: usize = 0x07FF;
/// Sound effect owning pulse 2 (bus `$EE`).
pub const RAM_SFX_PULSE2: usize = 0x07FE;
/// Sound effect owning the noise channel (bus `$ED`).
pub const RAM_SFX_NOISE: usize = 0x07FD;
/// `$07FF` value of the low-HP beep.
pub const LOW_HP_BEEP_ID: u8 = 0x40;
/// `$074A` value after casting Thunder (selector 7).
pub const THUNDER_CAST: u8 = 8;
/// `$074B` value the boss-death flash starts at (`bank7_code29`).
pub const BOSS_FLASH: u8 = 0xE8;
/// Game mode of side-view play.
pub const MODE_SIDEVIEW: u8 = 0x0B;

const RAM_LINK_SX: usize = 0x00CC;
const RAM_LINK_Y: usize = 0x0029;
const RAM_LINK_X: usize = 0x004D;
const RAM_LINK_PAGE: usize = 0x003B;
const RAM_LINK_FACING: usize = 0x009F;
const RAM_LINK_SPRITE: usize = 0x0080;
const RAM_SHIELD_POS: usize = 0x0017;
const RAM_SWORD_X: usize = 0x047E;
const RAM_SWORD_Y: usize = 0x0480;
const RAM_ENEMY_Y: usize = 0x002A;
const RAM_ENEMY_PAGE: usize = 0x003C;
const RAM_ENEMY_X: usize = 0x004E;
const RAM_ENEMY_EXISTS: usize = 0x00B6;
const RAM_ENEMY_HP: usize = 0x00C2;
/// Enemy slots.
pub const ENEMY_SLOTS: usize = 6;

// ------------------------------------------------------------ post effects

/// Scratch buffers for [`PostFx::apply`] (kept between frames so the hot
/// path does not allocate).
#[derive(Debug, Default, Clone)]
pub struct PostFx {
    tmp: Vec<u16>,
    blurred: Vec<u16>,
}

impl PostFx {
    /// Apply brightness, saturation, bloom and blur to `rgba`
    /// (`width x height`, RGBA8) in place. No-op unless
    /// `enh.effects_enabled` and at least one of them is nonzero. The blur
    /// radius is one game pixel (`height / 240` texels), so the look is the
    /// same at every HD scale. Alpha is left alone.
    pub fn apply(&mut self, rgba: &mut [u8], width: u32, height: u32, enh: &DisplayEnh) {
        let (w, h) = (width as usize, height as usize);
        if !post_effects_active(enh) || w == 0 || h == 0 || rgba.len() < w * h * 4 {
            return;
        }
        let rgba = &mut rgba[..w * h * 4];
        let need_blur = enh.blur > 0.0 || enh.bloom > 0.0;
        if need_blur {
            let r = (h / FRAME_H).max(1);
            box_blur(rgba, w, h, r, &mut self.tmp, &mut self.blurred);
        }
        let blur = enh.blur.clamp(0.0, 1.0);
        let bloom = enh.bloom.clamp(0.0, 1.0);
        let sat = 1.0 + enh.saturation.clamp(-1.0, 1.0);
        let bright = enh.brightness.clamp(-1.0, 1.0) * 255.0;
        let finish = |px: &mut [u8; 4], c: [f32; 3]| {
            let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
            for k in 0..3 {
                px[k] = (l + (c[k] - l) * sat + bright).clamp(0.0, 255.0) as u8;
            }
        };
        let pixels = rgba.as_chunks_mut::<4>().0.iter_mut();
        if need_blur {
            let glow = bloom * 1.5;
            for (px, b) in pixels.zip(self.blurred.as_chunks::<3>().0) {
                let mut c = [0.0f32; 3];
                for k in 0..3 {
                    let (v, bk) = (f32::from(px[k]), f32::from(b[k]));
                    // Mix toward the blurred picture, then let what of it is
                    // above mid-grey bleed back in (bright-pass glow).
                    c[k] = v + (bk - v) * blur + (bk - 128.0).max(0.0) * glow;
                }
                finish(px, c);
            }
        } else {
            for px in pixels {
                finish(px, [f32::from(px[0]), f32::from(px[1]), f32::from(px[2])]);
            }
        }
    }
}

/// Whether the CPU post effects change anything for these settings.
#[must_use]
pub fn post_effects_active(enh: &DisplayEnh) -> bool {
    enh.effects_enabled
        && (enh.brightness != 0.0 || enh.saturation != 0.0 || enh.bloom > 0.0 || enh.blur > 0.0)
}

/// Scanline strength for the window blit (`0` = none): `enh.scanlines`
/// while `effects_enabled`.
#[must_use]
pub fn scanline_strength(enh: &DisplayEnh) -> f32 {
    if enh.effects_enabled {
        enh.scanlines.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Apply the post-process chain to a composed RGBA8 frame of
/// `width x height` pixels, in place. No-op unless `enh.effects_enabled`.
/// (Allocating convenience over [`PostFx::apply`].)
pub fn apply_post_effects(rgba: &mut [u8], width: u32, height: u32, enh: &DisplayEnh) {
    PostFx::default().apply(rgba, width, height, enh);
}

/// Separable box blur of radius `r` over the RGB of `rgba` into `out`
/// (3 `u16` per pixel). Sliding sums: O(1) per pixel whatever the radius.
fn box_blur(rgba: &[u8], w: usize, h: usize, r: usize, tmp: &mut Vec<u16>, out: &mut Vec<u16>) {
    tmp.clear();
    tmp.resize(w * h * 3, 0);
    out.clear();
    out.resize(w * h * 3, 0);
    let n = (2 * r + 1) as u32;
    // Fixed-point 1/n (16.16), so the inner loops do not divide.
    let inv = (65_536 + n / 2) / n;
    let (wi, hi, ri) = (w as isize, h as isize, r as isize);
    // Horizontal, row by row (edges clamp).
    for y in 0..h {
        let src = &rgba[y * w * 4..(y + 1) * w * 4];
        let dst = &mut tmp[y * w * 3..(y + 1) * w * 3];
        let px = |x: isize| x.clamp(0, wi - 1) as usize * 4;
        let mut sum = [0u32; 3];
        for x in -ri..=ri {
            let i = px(x);
            for k in 0..3 {
                sum[k] += u32::from(src[i + k]);
            }
        }
        for x in 0..wi {
            let o = x as usize * 3;
            let (add, sub) = (px(x + ri + 1), px(x - ri));
            for k in 0..3 {
                dst[o + k] = ((sum[k] * inv + 0x8000) >> 16) as u16;
                sum[k] = sum[k] + u32::from(src[add + k]) - u32::from(src[sub + k]);
            }
        }
    }
    // Vertical with one running sum per column, still row by row so memory
    // is walked in order.
    let stride = w * 3;
    let row = |y: isize| y.clamp(0, hi - 1) as usize * stride;
    let mut sums = vec![0u32; stride];
    for y in -ri..=ri {
        let base = row(y);
        for (s, &v) in sums.iter_mut().zip(&tmp[base..base + stride]) {
            *s += u32::from(v);
        }
    }
    for y in 0..hi {
        let o = y as usize * stride;
        for (d, &s) in out[o..o + stride].iter_mut().zip(&sums) {
            *d = ((s * inv + 0x8000) >> 16) as u16;
        }
        let (add, sub) = (row(y + ri + 1), row(y - ri));
        for (i, s) in sums.iter_mut().enumerate() {
            *s = *s + u32::from(tmp[add + i]) - u32::from(tmp[sub + i]);
        }
    }
}

// ------------------------------------------------------------ screen shake

/// Frames a shake lasts.
pub const SHAKE_FRAMES: u8 = 32;

/// Shake trigger: a rising edge of the flash counter `$074B` that is the
/// boss-death flash ([`BOSS_FLASH`]) or a Thunder cast (the high "flash
/// decor" bit with `$074A ==` [`THUNDER_CAST`]). Other spell flashes, the
/// wise-man flash and other decor flashes do not shake.
#[must_use]
pub fn shake_trigger(prev_flash: u8, flash: u8, last_cast: u8) -> bool {
    prev_flash == 0
        && flash != 0
        && (flash == BOSS_FLASH || (flash & 0x80 != 0 && last_cast == THUNDER_CAST))
}

/// Picture offset in game pixels with `left` frames of shake remaining
/// (`(0, 0)` when none): 2 px for the first half, 1 px after, alternating
/// sides each frame.
#[must_use]
pub fn shake_offset(left: u8) -> (i32, i32) {
    if left == 0 {
        return (0, 0);
    }
    let a = if left > SHAKE_FRAMES / 2 { 2 } else { 1 };
    let dx = if left.is_multiple_of(2) { a } else { -a };
    let dy = match left % 4 {
        0 | 1 => a / 2,
        _ => -(a / 2),
    };
    (dx, dy)
}

/// Move the picture by `(dx, dy)` texels, filling what is uncovered with
/// opaque black.
pub fn shift_rgba(rgba: &mut [u8], width: usize, height: usize, dx: i32, dy: i32) {
    if (dx == 0 && dy == 0) || rgba.len() < width * height * 4 {
        return;
    }
    let stride = width * 4;
    let src = rgba[..height * stride].to_vec();
    for y in 0..height {
        let sy = y as i32 - dy;
        let row = &mut rgba[y * stride..(y + 1) * stride];
        if sy < 0 || sy >= height as i32 {
            for px in row.as_chunks_mut::<4>().0.iter_mut() {
                *px = [0, 0, 0, 0xFF];
            }
            continue;
        }
        let srow = &src[sy as usize * stride..(sy as usize + 1) * stride];
        for x in 0..width {
            let sx = x as i32 - dx;
            let dst = &mut row[x * 4..x * 4 + 4];
            if sx < 0 || sx >= width as i32 {
                dst.copy_from_slice(&[0, 0, 0, 0xFF]);
            } else {
                dst.copy_from_slice(&srow[sx as usize * 4..sx as usize * 4 + 4]);
            }
        }
    }
}

// ------------------------------------------------------------ flash colour

/// The NES colour a [`FlashColor`] paints the flashing backdrop with
/// (`None` for [`FlashColor::Og`] and [`FlashColor::None`], which keep the
/// game's colour / the steady backdrop).
#[must_use]
pub fn flash_nes_color(c: FlashColor) -> Option<u8> {
    match c {
        FlashColor::Og | FlashColor::None => None,
        FlashColor::Gray => Some(0x00),
        FlashColor::Red => Some(0x16),
        FlashColor::Violet => Some(0x13),
        FlashColor::Green => Some(0x1A),
    }
}

/// Frames per half period of a replaced flash: on for 10, off for 10
/// (3 Hz, the photosensitivity guideline limit), instead of the game's
/// colour change every frame.
pub const FLASH_HALF_PERIOD: u32 = 10;

/// Replacement backdrop for flash frame number `n` (0 = first flash frame):
/// `None` (no flashing) keeps the steady backdrop; a colour pulses at 3 Hz
/// between it and the steady backdrop.
#[must_use]
pub fn flash_replacement(c: FlashColor, steady: u8, n: u32) -> Option<u8> {
    match c {
        FlashColor::Og => None,
        FlashColor::None => Some(steady),
        _ => {
            let on = (n / FLASH_HALF_PERIOD).is_multiple_of(2);
            Some(if on {
                flash_nes_color(c).unwrap_or(steady)
            } else {
                steady
            })
        }
    }
}

/// Palette RAM slots that render as the backdrop (`$3F00` and its
/// mirrors).
const BACKDROP_SLOTS: [usize; 8] = [0, 4, 8, 12, 16, 20, 24, 28];

/// Recolour the flashing backdrop of an indexed frame in place.
///
/// `flash` is the backdrop colour this frame (palette RAM `$3F00`), `to`
/// the replacement. When `flash` is not used by any other visible palette
/// entry, every pixel of that colour is backdrop and is replaced exactly.
/// Otherwise a pixel is replaced only where the last steady frame showed
/// the steady backdrop (`steady_frame`, `steady`), so tiles that use the
/// same colour keep it (a pixel or two of drift while scrolling at worst).
pub fn recolor_flash(
    frame: &mut [u8; FRAME_LEN],
    palette: &[u8; 32],
    flash: u8,
    to: u8,
    steady_frame: Option<(&[u8; FRAME_LEN], u8)>,
) {
    let shared = palette
        .iter()
        .enumerate()
        .any(|(i, &c)| !BACKDROP_SLOTS.contains(&i) && c == flash);
    if !shared {
        for px in frame.iter_mut() {
            if *px == flash {
                *px = to;
            }
        }
    } else if let Some((prev, steady)) = steady_frame {
        for (px, &p) in frame.iter_mut().zip(prev.iter()) {
            if *px == flash && p == steady {
                *px = to;
            }
        }
    }
}

/// Recolour the flashing backdrop in a render record (per-line palettes,
/// read by the widescreen margins and HD packs).
pub fn recolor_record(rec: &mut FrameRecord, flash: u8, to: u8) {
    for line in rec.lines.iter_mut() {
        if line.backdrop == flash {
            line.backdrop = to;
        }
        for &i in &BACKDROP_SLOTS {
            if line.palette[i] == flash {
                line.palette[i] = to;
            }
        }
    }
}

// ------------------------------------------------------------ quest timer

/// NTSC frames per second, times 10 000 (`60.0988`).
const NTSC_FPS_X10K: u128 = 600_988;

/// `HH:MM:SS.cc` for `frames` NTSC frames (hours wrap at 100).
#[must_use]
pub fn format_quest_timer(frames: u64) -> String {
    let centis = u128::from(frames) * 1_000_000 / NTSC_FPS_X10K;
    let cs = centis % 100;
    let secs = centis / 100;
    format!(
        "{:02}:{:02}:{:02}.{:02}",
        (secs / 3600) % 100,
        (secs / 60) % 60,
        secs % 60,
        cs
    )
}

/// Quest-timer overlay text (`HH:MM:SS.cc`) for `frames` stepped since the
/// quest started, or `None` when the timer is hidden.
#[must_use]
pub fn quest_timer_text(enh: &DisplayEnh, frames: u64) -> Option<String> {
    enh.quest_timer.then(|| format_quest_timer(frames))
}

/// Before the quest: power-on, title and file select (no lives, game state
/// 0).
#[must_use]
pub fn before_quest(ram: &[u8; 0x800]) -> bool {
    ram[RAM_LIVES] == 0 && ram[RAM_GAME_STATE] == 0
}

// ------------------------------------------------------------ audio

/// Linear output gain for music (`music = true`) or sound effects, from the
/// `0..=10` volume settings: `(v / 10)^2` (a perceptual curve; `10` =
/// unchanged, `5` = a quarter, about -12 dB; `0` = silent).
#[must_use]
pub fn audio_gain(enh: &DisplayEnh, music: bool) -> f32 {
    let v = if music {
        enh.music_volume
    } else {
        enh.sfx_volume
    };
    let g = f32::from(v.min(MAX_VOLUME)) / f32::from(MAX_VOLUME);
    g * g
}

/// `$4015` bits: pulse 1, pulse 2, triangle, noise, DMC.
const CH_P1: u8 = 0x01;
const CH_P2: u8 = 0x02;
const CH_NOISE: u8 = 0x08;
const CH_ALL: u8 = 0x1F;

/// Channels a sound effect owns this frame (`$4015` bit layout), from the
/// sound engine's per-channel SFX ids. The triangle and the DMC are always
/// counted as music (the gameplay SFX buses never take the triangle; DMC
/// samples are rare and stay with the music volume).
#[must_use]
pub fn sfx_channels(ram: &[u8; 0x800]) -> u8 {
    let mut bits = 0;
    if ram[RAM_SFX_PULSE1] != 0 {
        bits |= CH_P1;
    }
    if ram[RAM_SFX_PULSE2] != 0 {
        bits |= CH_P2;
    }
    if ram[RAM_SFX_NOISE] != 0 {
        bits |= CH_NOISE;
    }
    bits
}

/// Low-HP beeps that still play before [`AudioFx`] silences the rest.
pub const LOW_HP_BEEPS_AUDIBLE: u8 = 3;
/// Frames without a beep after which the count starts over (two beep
/// periods).
const BEEP_RESET_FRAMES: u32 = 96;

/// Counts low-HP beeps (rising edges of `$07FF == $40`).
#[derive(Debug, Clone, Copy, Default)]
struct BeepTracker {
    prev: u8,
    count: u8,
    quiet: u32,
}

impl BeepTracker {
    /// Observe one frame; true when this frame's beep should be silent.
    fn observe(&mut self, ram: &[u8; 0x800], reduce: bool) -> bool {
        let cur = ram[RAM_SFX_PULSE1];
        let beeping = cur == LOW_HP_BEEP_ID;
        if beeping {
            if self.prev != LOW_HP_BEEP_ID {
                self.count = self.count.saturating_add(1);
            }
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
            if self.quiet > BEEP_RESET_FRAMES {
                self.count = 0;
            }
        }
        self.prev = cur;
        reduce && beeping && self.count > LOW_HP_BEEPS_AUDIBLE
    }
}

/// One APU's `$4015` routing: the enable mask applied to the game's
/// `$4015` writes, re-applied when the mask changes mid-note.
#[derive(Debug, Clone, Copy)]
struct Route {
    mask: u8,
}

impl Default for Route {
    fn default() -> Self {
        Self { mask: CH_ALL }
    }
}

impl Route {
    /// Feed one frame of register writes to `apu` through `mask`.
    fn feed(&mut self, apu: &mut z2_apu::Apu, log: &[(u16, u8)], last_4015: u8, mask: u8) {
        if mask != self.mask {
            // Keep a playing DMC sample alive (a `$4015` write with the DMC
            // bit clear would stop it; set while active never restarts it).
            let dmc = apu.read_status() & 0x10;
            apu.write_reg(0x4015, (last_4015 & mask & 0x0F) | (dmc & mask));
            self.mask = mask;
        }
        for &(addr, val) in log {
            if addr == 0x4015 {
                apu.write_reg(addr, val & mask);
            } else {
                apu.write_reg(addr, val);
            }
        }
    }
}

/// Music / SFX volume, mute hotkeys and the reduced low-HP beep, applied
/// where each frame's APU register writes are synthesized.
///
/// With equal music and SFX gains one APU renders as always (its PCM
/// scaled). With different gains a second APU renders the channels the
/// sound engine has lent to a sound effect ([`sfx_channels`]) while the
/// main APU renders the rest; the two are mixed with their gains. Ownership
/// is read after each frame, which is when the engine has set it for that
/// frame's writes. The low-HP beep is silenced by masking pulse 1 for the
/// frames it owns the channel ([`LOW_HP_BEEPS_AUDIBLE`] beeps still play).
pub struct AudioFx {
    music_gain: f32,
    sfx_gain: f32,
    mute_music: bool,
    mute_sfx: bool,
    low_hp_reduced: bool,
    beep: BeepTracker,
    last_4015: u8,
    main_route: Route,
    sfx_route: Route,
    sfx_apu: Option<z2_apu::Apu>,
    sfx_pcm: Vec<i16>,
    sfx_buf: VecDeque<i16>,
}

impl std::fmt::Debug for AudioFx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioFx")
            .field("gains", &self.gains())
            .field("low_hp_reduced", &self.low_hp_reduced)
            .field("split", &self.sfx_apu.is_some())
            .finish()
    }
}

impl Default for AudioFx {
    fn default() -> Self {
        Self {
            music_gain: 1.0,
            sfx_gain: 1.0,
            mute_music: false,
            mute_sfx: false,
            low_hp_reduced: false,
            beep: BeepTracker::default(),
            last_4015: 0x0F,
            main_route: Route::default(),
            sfx_route: Route { mask: 0 },
            sfx_apu: None,
            sfx_pcm: Vec::new(),
            sfx_buf: VecDeque::new(),
        }
    }
}

impl AudioFx {
    /// Take the volumes and the low-HP option from `enh` (mutes are kept).
    pub fn configure(&mut self, enh: &DisplayEnh) {
        self.music_gain = audio_gain(enh, true);
        self.sfx_gain = audio_gain(enh, false);
        self.low_hp_reduced = enh.low_hp_beep_reduced;
    }

    /// Toggle the music mute (`M`); returns the new state.
    pub fn toggle_music_mute(&mut self) -> bool {
        self.mute_music = !self.mute_music;
        self.mute_music
    }

    /// Toggle the sound-effect mute (`N`); returns the new state.
    pub fn toggle_sfx_mute(&mut self) -> bool {
        self.mute_sfx = !self.mute_sfx;
        self.mute_sfx
    }

    /// Effective `(music, sfx)` gains, mutes included.
    #[must_use]
    pub fn gains(&self) -> (f32, f32) {
        (
            if self.mute_music {
                0.0
            } else {
                self.music_gain
            },
            if self.mute_sfx { 0.0 } else { self.sfx_gain },
        )
    }

    /// True when this changes nothing (the caller then runs the original
    /// path untouched).
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        self.gains() == (1.0, 1.0) && !self.low_hp_reduced && self.main_route.mask == CH_ALL
    }

    /// Synthesize one frame: `log` is the frame's `$4000-$4017` writes,
    /// `ram` the CPU RAM after the frame. Appends the PCM to `pcm`.
    pub fn render_frame(
        &mut self,
        log: &[(u16, u8)],
        ram: &[u8; 0x800],
        apu: &mut z2_apu::Apu,
        pcm: &mut Vec<i16>,
    ) {
        if let Some(&(_, v)) = log.iter().rev().find(|(a, _)| *a == 0x4015) {
            // Remember the game's own enable bits for re-applying masks.
            self.last_4015 = v;
        }
        let (gm, gs) = self.gains();
        let mute = if self.beep.observe(ram, self.low_hp_reduced) {
            CH_P1
        } else {
            0
        };
        let split = gm != gs;
        let sfx = if split { sfx_channels(ram) } else { 0 };
        self.main_route
            .feed(apu, log, self.last_4015, CH_ALL & !sfx & !mute);
        let start = pcm.len();
        apu.audio(pcm);
        if split {
            let rate = apu.sample_rate();
            let sfx_apu = match &mut self.sfx_apu {
                Some(a) if a.sample_rate() == rate => a,
                slot => slot.insert(z2_apu::Apu::new(rate)),
            };
            self.sfx_route
                .feed(sfx_apu, log, self.last_4015, sfx & !mute & 0x0F);
            self.sfx_pcm.clear();
            sfx_apu.audio(&mut self.sfx_pcm);
            self.sfx_buf.extend(self.sfx_pcm.iter().copied());
            for s in &mut pcm[start..] {
                let e = self.sfx_buf.pop_front().unwrap_or(0);
                *s = mix(*s, gm, e, gs);
            }
            // Phase drift between the two resamplers is at most a sample;
            // never let the queue grow.
            while self.sfx_buf.len() > 4 {
                self.sfx_buf.pop_front();
            }
        } else {
            self.sfx_buf.clear();
            if gm != 1.0 {
                for s in &mut pcm[start..] {
                    *s = mix(*s, gm, 0, 0.0);
                }
            }
        }
    }
}

thread_local! {
    /// The [`AudioFx`] of the windowed loop. Thread-local rather than part of
    /// [`crate::audio::SharedAudio`] (whose clones run on the audio thread;
    /// the second APU is not `Send`) or of [`crate::app::Emu`]: the loop
    /// steps and handles keys on one thread, and only frames pushed to an
    /// audio ring pass through it, so headless runs never do.
    static AUDIO_FX: std::cell::RefCell<AudioFx> = std::cell::RefCell::new(AudioFx::default());
}

/// Run `f` on this thread's [`AudioFx`].
pub fn with_audio_fx<R>(f: impl FnOnce(&mut AudioFx) -> R) -> R {
    AUDIO_FX.with(|fx| f(&mut fx.borrow_mut()))
}

/// `a * ga + b * gb`, saturated to `i16`.
#[must_use]
pub fn mix(a: i16, ga: f32, b: i16, gb: f32) -> i16 {
    (f32::from(a) * ga + f32::from(b) * gb)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

// ------------------------------------------------------------ overlay drawing

/// 3x5 glyphs (one byte per row, bits 2..0 left to right).
fn glyph(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        ':' => [0, 2, 0, 2, 0],
        '.' => [0, 0, 0, 0, 2],
        '-' => [0, 0, 7, 0, 0],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [7, 4, 4, 4, 7],
        'D' => [6, 5, 5, 5, 6],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'H' => [5, 5, 7, 5, 5],
        'P' => [7, 5, 7, 4, 4],
        'F' => [7, 4, 6, 4, 4],
        'E' => [7, 4, 6, 4, 7],
        ' ' => [0; 5],
        _ => return None,
    })
}

/// Glyph advance in game pixels.
pub const GLYPH_ADVANCE: i32 = 4;

/// Width of `text` in game pixels.
#[must_use]
pub fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * GLYPH_ADVANCE - 1
}

/// A drawing surface: the composed RGBA frame, its texel scale and the
/// left margin (widescreen) in game pixels. Coordinates are game pixels
/// relative to the 256x240 window.
#[derive(Debug)]
pub struct Canvas<'a> {
    /// Pixels (RGBA8).
    pub rgba: &'a mut [u8],
    /// Width in texels.
    pub width: usize,
    /// Height in texels.
    pub height: usize,
    /// Texels per game pixel.
    pub scale: usize,
    /// Left margin in game pixels.
    pub margin: i32,
}

impl Canvas<'_> {
    /// Fill game pixel `(x, y)`.
    pub fn dot(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x < -self.margin || y < 0 {
            return;
        }
        let s = self.scale;
        let (tx, ty) = (((x + self.margin) as usize) * s, (y as usize) * s);
        if tx + s > self.width || ty + s > self.height {
            return;
        }
        for yy in ty..ty + s {
            let row = yy * self.width;
            for xx in tx..tx + s {
                let i = (row + xx) * 4;
                self.rgba[i..i + 3].copy_from_slice(&c);
                self.rgba[i + 3] = 0xFF;
            }
        }
    }

    /// 1-px rectangle outline.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
        if w <= 0 || h <= 0 {
            return;
        }
        for i in 0..w {
            self.dot(x + i, y, c);
            self.dot(x + i, y + h - 1, c);
        }
        for j in 0..h {
            self.dot(x, y + j, c);
            self.dot(x + w - 1, y + j, c);
        }
    }

    /// Small cross centred on `(x, y)`.
    pub fn cross(&mut self, x: i32, y: i32, c: [u8; 3]) {
        for d in -1..=1 {
            self.dot(x + d, y, c);
            self.dot(x, y + d, c);
        }
    }

    /// Text with a black drop shadow, top-left at `(x, y)`.
    pub fn text(&mut self, x: i32, y: i32, text: &str, c: [u8; 3]) {
        for (col, off) in [([0, 0, 0], 1), (c, 0)] {
            for (n, ch) in text.chars().enumerate() {
                let Some(g) = glyph(ch) else { continue };
                let gx = x + n as i32 * GLYPH_ADVANCE + off;
                for (row, bits) in g.iter().enumerate() {
                    for b in 0..3 {
                        if bits & (4 >> b) != 0 {
                            self.dot(gx + b, y + row as i32 + off, col);
                        }
                    }
                }
            }
        }
    }
}

const WHITE: [u8; 3] = [0xFC, 0xFC, 0xFC];
const GREEN: [u8; 3] = [0x40, 0xF0, 0x40];
const YELLOW: [u8; 3] = [0xF8, 0xD8, 0x40];
const RED: [u8; 3] = [0xF8, 0x38, 0x38];
const CYAN: [u8; 3] = [0x40, 0xD8, 0xF8];

/// One overlay box in window game pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevBox {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
    /// Enemy slot (`None` = Link's body or sword).
    pub slot: Option<u8>,
    /// Sword box.
    pub sword: bool,
}

/// Link and enemy boxes from side-view RAM (empty elsewhere). Link's body
/// and sword use the engine's own box builders (`z2_core::player`); enemy
/// boxes are 16x16 at the slot position (per-type sizes are not ported),
/// placed relative to Link: `screen = world - link_world + $CC`.
#[must_use]
pub fn dev_boxes(ram: &[u8; 0x800]) -> Vec<DevBox> {
    let mut out = Vec::new();
    if ram[RAM_MODE] != MODE_SIDEVIEW {
        return out;
    }
    let sx = ram[RAM_LINK_SX];
    let ly = ram[RAM_LINK_Y];
    let right = ram[RAM_LINK_FACING] != 2;
    let dy = BODY_DY[usize::from(ram[RAM_SHIELD_POS] != 0)];
    let b = body_box(sx, ly, right, dy);
    out.push(DevBox {
        x: i32::from(b.x),
        y: i32::from(b.y),
        w: i32::from(b.w),
        h: i32::from(b.h),
        slot: None,
        sword: false,
    });
    let swy = ram[RAM_SWORD_Y];
    if swy < SWORD_RETRACTED {
        let swx = ram[RAM_SWORD_X];
        let sprite = ram[RAM_LINK_SPRITE];
        let s = sword_box(swx, swy, swx < sx, sprite == 8 || sprite == 9);
        out.push(DevBox {
            x: i32::from(s.x),
            y: i32::from(s.y),
            w: i32::from(s.w),
            h: i32::from(s.h),
            slot: None,
            sword: true,
        });
    }
    let link_world = i32::from(ram[RAM_LINK_PAGE]) * 256 + i32::from(ram[RAM_LINK_X]);
    for s in 0..ENEMY_SLOTS {
        if ram[RAM_ENEMY_EXISTS + s] == 0 {
            continue;
        }
        let world = i32::from(ram[RAM_ENEMY_PAGE + s]) * 256 + i32::from(ram[RAM_ENEMY_X + s]);
        let x = world - link_world + i32::from(sx);
        if !(-16..FRAME_W as i32 + 16).contains(&x) {
            continue;
        }
        out.push(DevBox {
            x,
            y: i32::from(ram[RAM_ENEMY_Y + s]),
            w: 16,
            h: 16,
            slot: Some(s as u8),
            sword: false,
        });
    }
    out
}

/// Whether any overlay is drawn.
#[must_use]
pub fn overlays_active(enh: &DisplayEnh) -> bool {
    enh.quest_timer || enh.dev_hitboxes || enh.dev_xy || enh.dev_hp || enh.dev_framecount
}

/// Draw the quest timer and dev overlays for `ram` onto `cv`.
pub fn draw_overlays(
    cv: &mut Canvas<'_>,
    ram: &[u8; 0x800],
    frame_count: u64,
    quest_frames: Option<u64>,
    enh: &DisplayEnh,
) {
    if let Some(t) = quest_frames.and_then(|f| quest_timer_text(enh, f)) {
        let x = FRAME_W as i32 - 2 - text_width(&t);
        cv.text(x, 1, &t, WHITE);
    }
    if enh.dev_framecount {
        cv.text(2, 1, &format!("F {frame_count}"), WHITE);
    }
    if !(enh.dev_hitboxes || enh.dev_xy || enh.dev_hp) {
        return;
    }
    let boxes = dev_boxes(ram);
    for b in &boxes {
        if enh.dev_hitboxes {
            let c = match (b.slot, b.sword) {
                (Some(_), _) => RED,
                (None, true) => YELLOW,
                (None, false) => GREEN,
            };
            cv.rect(b.x, b.y, b.w, b.h, c);
        }
        if enh.dev_xy && !b.sword {
            cv.cross(b.x, b.y, CYAN);
        }
        if enh.dev_hp {
            if let Some(s) = b.slot {
                let hp = ram[RAM_ENEMY_HP + usize::from(s)];
                cv.text(b.x, (b.y - 7).max(0), &format!("{hp}"), RED);
            }
        }
    }
    if enh.dev_xy {
        let t = format!(
            "X {:04X} Y {:02X}",
            u16::from(ram[RAM_LINK_PAGE]) << 8 | u16::from(ram[RAM_LINK_X]),
            ram[RAM_LINK_Y]
        );
        cv.text(2, FRAME_H as i32 - 7, &t, CYAN);
    }
}

// ------------------------------------------------------------ per-display state

/// Per-display enhancement state, driven once per presented frame by
/// [`crate::app::Display::present`].
#[derive(Debug, Default)]
pub struct DisplayFx {
    post: PostFx,
    out: Vec<u8>,
    // Shake.
    prev_flash: u8,
    shake_left: u8,
    // Flash.
    steady_valid: bool,
    steady_backdrop: u8,
    steady_frame: Option<Box<[u8; FRAME_LEN]>>,
    flash_frames: u32,
    flash: Option<(u8, u8)>,
    flash_frame: Option<Box<[u8; FRAME_LEN]>>,
    flash_record: Option<Box<FrameRecord>>,
    // Quest timer: frame count the quest started at.
    quest_start: Option<u64>,
    seen_title: bool,
}

impl DisplayFx {
    /// Observe the game after a frame (call once per presented frame,
    /// before [`Self::frame_inputs`]).
    pub fn observe(&mut self, game: &Game, enh: &DisplayEnh) {
        let ram = game.ram();
        let pal = game.palette();
        let f = ram[RAM_FLASH];
        // Shake.
        if enh.screen_shake && shake_trigger(self.prev_flash, f, ram[RAM_LAST_CAST]) {
            self.shake_left = SHAKE_FRAMES;
        } else {
            self.shake_left = self.shake_left.saturating_sub(1);
        }
        if !enh.screen_shake {
            self.shake_left = 0;
        }
        self.prev_flash = f;
        // Flash.
        self.flash = None;
        if enh.flash_color != FlashColor::Og {
            if f == 0 {
                self.steady_valid = true;
                self.steady_backdrop = pal[0];
                self.flash_frames = 0;
                let s = self
                    .steady_frame
                    .get_or_insert_with(|| Box::new([0u8; FRAME_LEN]));
                s.copy_from_slice(game.frame_indexed());
            } else if self.steady_valid && pal[0] != self.steady_backdrop {
                if let Some(to) =
                    flash_replacement(enh.flash_color, self.steady_backdrop, self.flash_frames)
                {
                    self.flash = Some((pal[0], to));
                }
                self.flash_frames += 1;
            }
        }
        // Quest clock.
        if enh.quest_timer {
            if before_quest(ram) {
                self.seen_title = true;
                self.quest_start = None;
            } else if self.quest_start.is_none() {
                // Joined mid-quest (a loaded state, a headless dump): count
                // from power-on.
                self.quest_start = Some(if self.seen_title {
                    game.frame_count()
                } else {
                    0
                });
            }
        }
    }

    /// Frames since the quest started (`None` before it).
    #[must_use]
    pub fn quest_frames(&self, game: &Game) -> Option<u64> {
        self.quest_start
            .map(|s| game.frame_count().saturating_sub(s))
    }

    /// The indexed frame and record to compose: the game's own, or
    /// recoloured copies while a flash is being replaced.
    pub fn frame_inputs<'a>(
        &'a mut self,
        game: &'a Game,
        record: &'a FrameRecord,
    ) -> (&'a [u8; FRAME_LEN], &'a FrameRecord) {
        let Some((flash, to)) = self.flash else {
            return (game.frame_indexed(), record);
        };
        let frame = self
            .flash_frame
            .get_or_insert_with(|| Box::new([0u8; FRAME_LEN]));
        frame.copy_from_slice(game.frame_indexed());
        recolor_flash(
            frame,
            game.palette(),
            flash,
            to,
            self.steady_frame
                .as_deref()
                .map(|f| (f, self.steady_backdrop)),
        );
        let rec = match &mut self.flash_record {
            Some(r) => {
                r.as_mut().clone_from(record);
                r
            }
            slot => slot.insert(Box::new(record.clone())),
        };
        recolor_record(rec, flash, to);
        (frame, rec)
    }

    /// Post effects, shake and overlays over the composed frame `rgba`
    /// (`width x height`, `scale` texels per game pixel, `margin` game
    /// pixels of widescreen margin per side). Returns the finished frame.
    #[allow(clippy::too_many_arguments)]
    pub fn finish<'a>(
        &'a mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        scale: u32,
        margin: u32,
        game: &Game,
        enh: &DisplayEnh,
    ) -> &'a [u8] {
        self.out.clear();
        self.out.extend_from_slice(rgba);
        self.post.apply(&mut self.out, width, height, enh);
        let (w, h, s) = (width as usize, height as usize, scale.max(1) as usize);
        let (dx, dy) = shake_offset(self.shake_left);
        shift_rgba(&mut self.out, w, h, dx * s as i32, dy * s as i32);
        if overlays_active(enh) {
            let quest = self.quest_frames(game);
            let mut cv = Canvas {
                rgba: &mut self.out,
                width: w,
                height: h,
                scale: s,
                margin: margin as i32,
            };
            draw_overlays(&mut cv, game.ram(), game.frame_count(), quest, enh);
        }
        &self.out
    }

    /// Frames of shake left (for tests and the title bar).
    #[must_use]
    pub fn shake_left(&self) -> u8 {
        self.shake_left
    }

    /// `(flash colour, replacement)` while a flash is being replaced.
    #[must_use]
    pub fn active_flash(&self) -> Option<(u8, u8)> {
        self.flash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx_on() -> DisplayEnh {
        DisplayEnh {
            effects_enabled: true,
            brightness: 0.1,
            saturation: -0.5,
            bloom: 0.3,
            blur: 0.5,
            scanlines: 0.5,
            ..DisplayEnh::default()
        }
    }

    #[test]
    fn effects_are_neutral_when_off() {
        let px: Vec<u8> = (0..16 * 16 * 4).map(|i| (i * 37 % 251) as u8).collect();
        let mut a = px.clone();
        apply_post_effects(&mut a, 16, 16, &DisplayEnh::default());
        assert_eq!(a, px, "default");
        // Values set but the master switch off: still untouched.
        let mut off = fx_on();
        off.effects_enabled = false;
        let mut b = px.clone();
        apply_post_effects(&mut b, 16, 16, &off);
        assert_eq!(b, px, "effects_enabled off");
        assert!(scanline_strength(&off) == 0.0);
        // Switch on with all-zero strengths: untouched too.
        let zero = DisplayEnh {
            effects_enabled: true,
            ..DisplayEnh::default()
        };
        let mut c = px.clone();
        apply_post_effects(&mut c, 16, 16, &zero);
        assert_eq!(c, px, "all zero");
        // And on: changed, alpha kept.
        let mut d = px.clone();
        apply_post_effects(&mut d, 16, 16, &fx_on());
        assert_ne!(d, px);
        assert!(d
            .as_chunks::<4>()
            .0
            .iter()
            .zip(px.as_chunks::<4>().0)
            .all(|(a, b)| a[3] == b[3]));
        assert!((scanline_strength(&fx_on()) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn brightness_and_saturation_math() {
        let e = DisplayEnh {
            effects_enabled: true,
            brightness: 0.1,
            ..DisplayEnh::default()
        };
        let mut px = vec![100, 50, 0, 255];
        apply_post_effects(&mut px, 1, 1, &e);
        assert_eq!(px, [125, 75, 25, 255]);
        let grey = DisplayEnh {
            effects_enabled: true,
            saturation: -1.0,
            ..DisplayEnh::default()
        };
        let mut px = vec![200, 100, 0, 255];
        apply_post_effects(&mut px, 1, 1, &grey);
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
    }

    #[test]
    fn blur_spreads_and_keeps_flat_areas() {
        let e = DisplayEnh {
            effects_enabled: true,
            blur: 1.0,
            ..DisplayEnh::default()
        };
        // Flat image: unchanged by a box blur.
        let mut flat = vec![80u8; 8 * 8 * 4];
        apply_post_effects(&mut flat, 8, 8, &e);
        assert!(flat.iter().all(|&v| v == 80));
        // A single bright pixel spreads to its neighbours.
        let mut img = vec![0u8; 5 * 5 * 4];
        img[(2 * 5 + 2) * 4] = 255;
        apply_post_effects(&mut img, 5, 5, &e);
        assert!(img[(2 * 5 + 2) * 4] < 255);
        assert!(img[(2 * 5 + 3) * 4] > 0);
        assert!(img[6 * 4] > 0);
        assert_eq!(img[0], 0, "radius one game pixel");
    }

    #[test]
    fn quest_timer_formatting() {
        assert_eq!(format_quest_timer(0), "00:00:00.00");
        assert_eq!(format_quest_timer(1), "00:00:00.01");
        assert_eq!(format_quest_timer(60), "00:00:00.99");
        assert_eq!(format_quest_timer(61), "00:00:01.01");
        // One NTSC hour = 216 355.68 frames.
        assert_eq!(format_quest_timer(216_356), "01:00:00.00");
        assert_eq!(format_quest_timer(216_356 + 3_606), "01:01:00.00");
        let off = DisplayEnh::default();
        assert_eq!(quest_timer_text(&off, 100), None);
        let on = DisplayEnh {
            quest_timer: true,
            ..off
        };
        assert_eq!(quest_timer_text(&on, 0).as_deref(), Some("00:00:00.00"));
    }

    #[test]
    fn gain_math() {
        let d = DisplayEnh::default();
        assert!((audio_gain(&d, true) - 1.0).abs() < f32::EPSILON);
        assert!((audio_gain(&d, false) - 1.0).abs() < f32::EPSILON);
        let e = DisplayEnh {
            music_volume: 5,
            sfx_volume: 0,
            ..d
        };
        assert!((audio_gain(&e, true) - 0.25).abs() < 1e-6);
        assert_eq!(audio_gain(&e, false), 0.0);
        let over = DisplayEnh {
            music_volume: 200,
            ..d
        };
        assert!((audio_gain(&over, true) - 1.0).abs() < f32::EPSILON);
        assert_eq!(mix(1000, 0.5, 1000, 0.25), 750);
        assert_eq!(mix(i16::MAX, 1.0, i16::MAX, 1.0), i16::MAX);
        assert_eq!(mix(i16::MIN, 1.0, -5, 1.0), i16::MIN);
    }

    #[test]
    fn audio_fx_neutral_and_mutes() {
        let mut fx = AudioFx::default();
        assert!(fx.is_neutral());
        fx.configure(&DisplayEnh::default());
        assert!(fx.is_neutral());
        assert!(fx.toggle_music_mute());
        assert!(!fx.is_neutral());
        assert_eq!(fx.gains(), (0.0, 1.0));
        assert!(!fx.toggle_music_mute());
        assert!(fx.toggle_sfx_mute());
        assert_eq!(fx.gains(), (1.0, 0.0));
        fx.toggle_sfx_mute();
        fx.configure(&DisplayEnh {
            low_hp_beep_reduced: true,
            ..DisplayEnh::default()
        });
        assert!(!fx.is_neutral());
    }

    /// A pulse-1 tone and a pulse-2 tone; the SFX id byte decides which
    /// gain each gets.
    #[test]
    fn audio_fx_splits_music_from_sfx() {
        let tone = [
            (0x4015, 0x0F),
            (0x4000, 0xBF),
            (0x4001, 0x08),
            (0x4002, 0xFD),
            (0x4003, 0x08),
            (0x4004, 0xBF),
            (0x4005, 0x08),
            (0x4006, 0x7D),
            (0x4007, 0x08),
        ];
        // AC power (the mixer output carries a DC offset).
        let energy = |pcm: &[i16]| {
            let mean = pcm.iter().map(|&s| i64::from(s)).sum::<i64>() / pcm.len().max(1) as i64;
            pcm.iter()
                .map(|&s| (i64::from(s) - mean).pow(2))
                .sum::<i64>()
        };
        let run = |music: u8, sfx: u8, owner: u8| {
            let mut apu = z2_apu::Apu::new(44_100);
            let mut fx = AudioFx::default();
            fx.configure(&DisplayEnh {
                music_volume: music,
                sfx_volume: sfx,
                ..DisplayEnh::default()
            });
            let mut ram = [0u8; 0x800];
            ram[RAM_SFX_PULSE1] = owner;
            let mut pcm = Vec::new();
            for _ in 0..4 {
                fx.render_frame(&tone, &ram, &mut apu, &mut pcm);
            }
            energy(&pcm)
        };
        let both = run(10, 10, 0);
        assert!(both > 0);
        // Music silenced, pulse 1 owned by an effect: only half the tone.
        let sfx_only = run(0, 10, 0x08);
        assert!(
            sfx_only > both / 10 && sfx_only < both,
            "{sfx_only} vs {both}"
        );
        // Music silenced and nothing owned by an effect: silence.
        assert!(run(0, 10, 0) < both / 100, "{}", run(0, 10, 0));
        // Effects silenced, pulse 1 owned by an effect: the other half.
        let music_only = run(10, 0, 0x08);
        assert!(
            music_only > both / 10 && music_only < both,
            "{music_only} vs {both}"
        );
    }

    #[test]
    fn low_hp_beep_is_silenced_after_three() {
        let mut t = BeepTracker::default();
        let mut ram = [0u8; 0x800];
        let mut muted = Vec::new();
        for beep in 0..6 {
            // 12 frames of beep, 36 quiet (the game's 48-frame period).
            ram[RAM_SFX_PULSE1] = LOW_HP_BEEP_ID;
            let m = (0..12).map(|_| t.observe(&ram, true)).collect::<Vec<_>>();
            assert!(m.iter().all(|&x| x == m[0]), "whole beep alike ({beep})");
            muted.push(m[0]);
            ram[RAM_SFX_PULSE1] = 0;
            for _ in 0..36 {
                assert!(!t.observe(&ram, true));
            }
        }
        assert_eq!(muted, [false, false, false, true, true, true]);
        // A long quiet spell (HP restored) starts the count over.
        for _ in 0..200 {
            t.observe(&ram, true);
        }
        ram[RAM_SFX_PULSE1] = LOW_HP_BEEP_ID;
        assert!(!t.observe(&ram, true));
        // Option off: never muted.
        let mut t = BeepTracker {
            count: 10,
            ..BeepTracker::default()
        };
        assert!(!t.observe(&ram, false));
        // Another pulse-1 effect is never muted.
        ram[RAM_SFX_PULSE1] = 0x08;
        assert!(!t.observe(&ram, true));
    }

    #[test]
    fn sfx_channel_ownership() {
        let mut ram = [0u8; 0x800];
        assert_eq!(sfx_channels(&ram), 0);
        ram[RAM_SFX_PULSE1] = 0x40;
        ram[RAM_SFX_NOISE] = 0x20;
        assert_eq!(sfx_channels(&ram), CH_P1 | CH_NOISE);
        ram[RAM_SFX_PULSE2] = 0x04;
        assert_eq!(sfx_channels(&ram), CH_P1 | CH_P2 | CH_NOISE);
    }

    #[test]
    fn shake_triggers_and_decays() {
        assert!(shake_trigger(0, BOSS_FLASH, 0));
        assert!(shake_trigger(0, 0xA0, THUNDER_CAST));
        assert!(!shake_trigger(0, 0xA0, 7), "Spell flash");
        assert!(!shake_trigger(0, 0xA0, 0), "a decor flash without a cast");
        assert!(!shake_trigger(0, 0x20, THUNDER_CAST), "no decor bit");
        assert!(!shake_trigger(0, 0xC0, 0), "wise man");
        assert!(!shake_trigger(BOSS_FLASH, 0xE7, 0), "only the rising edge");
        assert_eq!(shake_offset(0), (0, 0));
        assert_eq!(shake_offset(SHAKE_FRAMES).0.abs(), 2);
        assert_eq!(shake_offset(3).0.abs(), 1);
        assert_ne!(shake_offset(4), shake_offset(3));
    }

    #[test]
    fn shift_moves_and_blackens() {
        let mut px = vec![0u8; 3 * 2 * 4];
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *p = [i as u8 + 1, 0, 0, 0xFF];
        }
        let orig = px.clone();
        shift_rgba(&mut px, 3, 2, 0, 0);
        assert_eq!(px, orig);
        shift_rgba(&mut px, 3, 2, 1, 0);
        assert_eq!(&px[0..4], &[0, 0, 0, 0xFF]);
        assert_eq!(&px[4..8], &orig[0..4]);
        let mut px = orig.clone();
        shift_rgba(&mut px, 3, 2, 0, -1);
        assert_eq!(&px[0..4], &orig[12..16]);
        assert_eq!(&px[12..16], &[0, 0, 0, 0xFF]);
    }

    #[test]
    fn flash_replacement_colours() {
        assert_eq!(flash_replacement(FlashColor::Og, 0x0F, 0), None);
        assert_eq!(flash_replacement(FlashColor::None, 0x0F, 3), Some(0x0F));
        assert_eq!(flash_replacement(FlashColor::Red, 0x0F, 0), Some(0x16));
        assert_eq!(
            flash_replacement(FlashColor::Red, 0x0F, FLASH_HALF_PERIOD),
            Some(0x0F)
        );
        assert_eq!(
            flash_replacement(FlashColor::Green, 0x0F, 2 * FLASH_HALF_PERIOD),
            Some(0x1A)
        );
        assert_eq!(flash_nes_color(FlashColor::Gray), Some(0x00));
        assert_eq!(flash_nes_color(FlashColor::Violet), Some(0x13));
    }

    #[test]
    fn recolor_flash_exact_and_shared() {
        let mut pal = [0x0Fu8; 32];
        pal[0] = 0x2A;
        pal[1] = 0x30;
        let mut frame = Box::new([0x30u8; FRAME_LEN]);
        frame[0] = 0x2A;
        frame[1] = 0x2A;
        recolor_flash(&mut frame, &pal, 0x2A, 0x0F, None);
        assert_eq!((frame[0], frame[1], frame[2]), (0x0F, 0x0F, 0x30));
        // 0x16 also used by a tile colour: only former-backdrop pixels move.
        pal[0] = 0x16;
        pal[3] = 0x16;
        let mut frame = Box::new([0x16u8; FRAME_LEN]);
        let mut steady = Box::new([0x16u8; FRAME_LEN]);
        steady[5] = 0x0F;
        recolor_flash(&mut frame, &pal, 0x16, 0x0F, Some((&steady, 0x0F)));
        assert_eq!(frame[5], 0x0F);
        assert_eq!(frame[4], 0x16);
        // Shared and no steady frame: leave it alone.
        let mut frame = Box::new([0x16u8; FRAME_LEN]);
        recolor_flash(&mut frame, &pal, 0x16, 0x0F, None);
        assert!(frame.iter().all(|&p| p == 0x16));
        // Record lines.
        let mut rec = FrameRecord::new();
        rec.lines[3].backdrop = 0x16;
        rec.lines[3].palette[0] = 0x16;
        rec.lines[3].palette[3] = 0x16;
        recolor_record(&mut rec, 0x16, 0x0F);
        assert_eq!(rec.lines[3].backdrop, 0x0F);
        assert_eq!(rec.lines[3].palette[0], 0x0F);
        assert_eq!(rec.lines[3].palette[3], 0x16, "tile colours untouched");
    }

    #[test]
    fn dev_boxes_from_ram() {
        let mut ram = [0u8; 0x800];
        assert!(dev_boxes(&ram).is_empty(), "not side view");
        ram[RAM_MODE] = MODE_SIDEVIEW;
        ram[RAM_LINK_SX] = 0x70;
        ram[RAM_LINK_Y] = 0x80;
        ram[RAM_LINK_PAGE] = 1;
        ram[RAM_LINK_X] = 0x10;
        ram[RAM_LINK_FACING] = 1;
        ram[RAM_SWORD_Y] = SWORD_RETRACTED;
        ram[RAM_ENEMY_EXISTS + 2] = 1;
        ram[RAM_ENEMY_PAGE + 2] = 1;
        ram[RAM_ENEMY_X + 2] = 0x40;
        ram[RAM_ENEMY_Y + 2] = 0x90;
        let b = dev_boxes(&ram);
        assert_eq!(b.len(), 2, "body + one enemy");
        assert_eq!(b[0].slot, None);
        assert_eq!(b[1].slot, Some(2));
        assert_eq!((b[1].x, b[1].y), (0x70 + 0x30, 0x90));
        ram[RAM_SWORD_Y] = 0x80;
        ram[RAM_SWORD_X] = 0x80;
        assert!(dev_boxes(&ram).iter().any(|b| b.sword));
    }

    #[test]
    fn overlay_text_draws_only_where_asked() {
        let (w, h) = (256usize, 240usize);
        let mut px = vec![0u8; w * h * 4];
        let ram = [0u8; 0x800];
        let none = DisplayEnh::default();
        assert!(!overlays_active(&none));
        let mut cv = Canvas {
            rgba: &mut px,
            width: w,
            height: h,
            scale: 1,
            margin: 0,
        };
        draw_overlays(&mut cv, &ram, 1234, Some(600), &none);
        assert!(px.iter().all(|&v| v == 0));
        let on = DisplayEnh {
            quest_timer: true,
            dev_framecount: true,
            ..none
        };
        let mut cv = Canvas {
            rgba: &mut px,
            width: w,
            height: h,
            scale: 1,
            margin: 0,
        };
        draw_overlays(&mut cv, &ram, 1234, Some(600), &on);
        let lit = |x0: usize, x1: usize| {
            (0..8).any(|y| (x0..x1).any(|x| px[(y * w + x) * 4] == WHITE[0]))
        };
        assert!(lit(0, 40), "frame counter top-left");
        assert!(lit(200, 256), "timer top-right");
        assert!(!lit(100, 150));
        assert!(text_width("00:00:00.00") < 60);
    }
}
