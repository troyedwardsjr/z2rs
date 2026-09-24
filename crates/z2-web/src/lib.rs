//! `z2-web`: browser frontend for the Zelda II reconstruction.
//!
//! The tab runs the same hybrid-execution [`z2_core::game::Game`] as the
//! native frontend and `xtask verify --dut game`: ROM bytes dropped into
//! the page pass the ROM hash gate ([`z2_assets::rom`]), are extracted
//! in-tab ([`z2_assets::extract`]), and boot through `Game::from_ines` +
//! `reset` with the full ported-routine trap table registered (same order
//! as `tools/xtask/src/verify.rs`).
//!
//! # Architecture
//!
//! * [`WebEmu`] is the whole emulator state as one opaque handle. All logic
//!   is plain Rust (host-testable under `cargo test -p z2-web`); the
//!   `#[wasm_bindgen]` impl block only adapts it to JS values. No
//!   `js-sys`/`web-sys` dependency: the JS side (`site/app.js`) owns the
//!   DOM, canvas, IndexedDB, keyboard/Gamepad and AudioContext, and talks
//!   to this module through bytes, strings and numbers only.
//! * Framebuffer: the engine renders indexed pixels (`Game::frame_indexed`);
//!   [`WebEmu::render_frame`] expands them to RGBA with
//!   [`z2_ppu::palette::indexed_to_rgba`] (read-only — verification never
//!   touches the display palette). JS blits via `putImageData`, either by
//!   copying [`WebEmu::frame_rgba`] or zero-copy through
//!   [`WebEmu::frame_ptr`]/[`WebEmu::frame_len`]. **The framebuffer is not
//!   fixed at 256x240**: widescreen widens it and an HD pack (or a scale
//!   above 1) multiplies it, so JS sizes the canvas from
//!   [`WebEmu::frame_width`]/[`WebEmu::frame_height`] every frame and uses
//!   [`WebEmu::logical_width`]/[`WebEmu::output_scale`] for the CSS size.
//! * Widescreen: [`WebEmu::set_widescreen_preset`] (`"off"`, `"16:10"`,
//!   `"16:9"`, or tiles per side) arms the PPU render record and composes
//!   through `Game::compose_wide`. Display only — the 256x240 frame the
//!   engine computes is untouched, and margins carry scenery, never sprites.
//! * Co-op: [`WebEmu::coop_enable`] + [`WebEmu::step_frames2`] drive
//!   `Game::step2`. The flag is remembered across `load_rom` (which builds a
//!   new `Game`) and `restore` calls `Game::coop_reset_area`.
//!
//! # Cargo features
//!
//! Default is empty, so the default wasm bundle is exactly what it was
//! before these features existed; widescreen and local co-op need nothing
//! extra. Every exported method exists in every build — a bundle without a
//! feature answers [`WebEmu::hd_supported`] / [`WebEmu::net_supported`] with
//! `false` and refuses the operation with a message, so `site/app.js` needs
//! no build-time branching.
//!
//! * `hd` — HD graphics packs through `z2-render` (`png`, serde). A browser
//!   directory pick is staged with [`WebEmu::hd_pack_add_file`] and built by
//!   [`WebEmu::hd_pack_commit`]; sheets are decoded in Rust, never in JS.
//! * `net` — the `z2-net` lockstep and rollback protocol cores (no dependencies,
//!   host-testable over the loopback transport).
//! * `netplay` — `net` plus the matchbox WebRTC transport. On wasm the
//!   message-loop future is `!Send` and `z2-net` drives it with
//!   `wasm_bindgen_futures::spawn_local`.
//! * Input contract (shared with native + oracle): bits 0..7 are
//!   A,B,Select,Start,Up,Down,Left,Right. See [`pack_input`].
//! * Timing: NTSC NES runs at [`NTSC_HZ`] Hz; `site/app.js` runs a
//!   `requestAnimationFrame` accumulator on that cadence.
//! * Audio: the game's own bank-6 sound engine runs on the interpreter and
//!   writes `$4000-$4017` into the null-APU façade's per-frame log. Each
//!   audible frame drains that log into the [`z2_apu::Apu`] synth (DMC bytes
//!   read from the loaded PRG) and renders 44.1 kHz PCM, which
//!   [`WebEmu::take_audio_f32`] hands to the `AudioWorklet` ring buffer
//!   (`site/worklet.js`). This is the same path the native frontend plays.
//! * Saves: [`WebEmu::snapshot`]/[`WebEmu::restore`] move `Z2WEB01` blobs
//!   (see [`snapshot`]); [`WebEmu::sram_bytes`]/[`WebEmu::load_sram`] move
//!   the raw 8 KiB battery WRAM. IndexedDB lives in JS (`site/app.js`).
//! * QA: `site/app.js` publishes `window.z2` on top of these exports — the
//!   nine stable members `{ facts(), state(), input(mask, frames), step(n),
//!   snapshot(), restore(bytes), loadMovie(text), screenshot(), version }`,
//!   backed by [`z2_core::facts`], [`snapshot`] and [`movie`], plus the
//!   unstable `ext` namespace for everything added since. The member count
//!   is asserted by `site/smoke.mjs` and documented in `site/README`.
//!
//! ROM bytes are never embedded anywhere: tests synthesize their own iNES
//! images, and the page only ever holds the user's dropped file in memory.

pub mod movie;
#[cfg(feature = "net")]
pub mod netroll;
#[cfg(feature = "net")]
pub mod netsim;
pub mod perf;
pub mod snapshot;

use wasm_bindgen::prelude::*;
use z2_apu::{Apu, PrgSource};
use z2_assets::{extract, rom};
use z2_core::facts::Game as FactsGame;
use z2_core::game::{self, Game, FRAME_H, FRAME_LEN};
use z2_core::ram::Ram;
use z2_ppu::palette::indexed_to_rgba;

use movie::MovieKind;
use snapshot::{import_corpus_snapshot, SnapshotError, WebSnapshot, WRAM_SIZE};

// ---------------------------------------------------------------------------
// Constants (also mirrored in site/app.js — keep in sync).
// ---------------------------------------------------------------------------

/// Crate version (also the `window.z2.version` string).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// NTSC NES frame rate: `39375000 / 655171 ≈ 60.0988` Hz.
pub const NTSC_HZ: f64 = 60.0988;

/// Audio PCM rate produced per frame (Hz).
pub const SAMPLE_RATE: u32 = 44_100;

/// Max logic frames advanced by one exported call (hang guard for the tab).
pub const MAX_STEPS_PER_CALL: u32 = 3600;

/// Max accepted ROM file bytes (headered file is ~256 KiB; generous bound).
pub const MAX_ROM_BYTES: usize = 512 * 1024;

/// Max accepted snapshot/SRAM upload bytes.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

/// RGBA bytes per frame (`256 * 240 * 4`).
pub const FRAME_RGBA_LEN: usize = FRAME_LEN * 4;

/// iNES PRG/CHR units of the pinned Zelda II USA image: 8x16 KiB PRG plus
/// 16x8 KiB CHR (128 KiB + 128 KiB). The sum is pinned to the verified body
/// length so the header can never again cover only part of the CHR.
const INES_PRG_UNITS: u8 = 8;
const INES_CHR_UNITS: u8 = 16;
const _: () = assert!(
    INES_PRG_UNITS as usize * 0x4000 + INES_CHR_UNITS as usize * 0x2000 == rom::EXPECTED_BODY_LEN,
    "the canonical iNES header must cover the whole verified ROM body"
);

/// The verified ROM body behind the canonical Zelda II (USA) iNES header
/// (MMC1, horizontal-mirror seed). Every `Game` the page builds comes from
/// this image, whether the dropped file had a header or not, so a headered
/// dump, a bare body and a netplay restart from the stored body all boot
/// byte-identical games.
fn canonical_ines(body: &[u8]) -> Vec<u8> {
    let mut ines = Vec::with_capacity(rom::INES_HEADER_LEN + body.len());
    ines.extend_from_slice(&rom::INES_MAGIC);
    ines.extend_from_slice(&[INES_PRG_UNITS, INES_CHR_UNITS, 0x10]);
    ines.resize(rom::INES_HEADER_LEN, 0);
    ines.extend_from_slice(body);
    ines
}

/// Audio FIFO cap in samples (~5 s at 44.1 kHz, ~4.6 s at 48 kHz): the page
/// drains this every rAF tick whether or not audio is on, so it is a memory
/// guard for a stopped tab, not a latency budget — the worklet's queue is
/// what the player hears as delay (see `site/worklet.js`).
const AUDIO_FIFO_CAP: usize = 44_100 * 5;

// ---------------------------------------------------------------------------
// Errors (internal) — the wasm boundary converts these to JS strings.
// ---------------------------------------------------------------------------

/// Failure of a [`WebEmu`] operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebError {
    /// No ROM loaded yet.
    NoRom,
    /// ROM rejected (hash gate, extractor, or `Game::from_ines`).
    Rom(String),
    /// Snapshot blob rejected.
    Snapshot(String),
    /// SRAM image rejected.
    Sram(String),
    /// Movie text rejected.
    Movie(String),
}

impl std::fmt::Display for WebError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebError::NoRom => write!(f, "no ROM loaded: drop a Zelda II (USA) .nes file first"),
            WebError::Rom(m) => write!(f, "ROM rejected: {m}"),
            WebError::Snapshot(m) => write!(f, "snapshot rejected: {m}"),
            WebError::Sram(m) => write!(f, "SRAM rejected: {m}"),
            WebError::Movie(m) => write!(f, "movie rejected: {m}"),
        }
    }
}

impl std::error::Error for WebError {}

impl From<SnapshotError> for WebError {
    fn from(e: SnapshotError) -> Self {
        WebError::Snapshot(e.0)
    }
}

fn need_game(game: &Option<Game>) -> Result<&Game, WebError> {
    game.as_ref().ok_or(WebError::NoRom)
}

fn need_game_mut(game: &mut Option<Game>) -> Result<&mut Game, WebError> {
    game.as_mut().ok_or(WebError::NoRom)
}

// ---------------------------------------------------------------------------
// Input contract: bits 0..7 = A,B,Select,Start,Up,Down,Left,Right.
// ---------------------------------------------------------------------------

/// Pack eight button states into the shared input byte (bit 0 = A … bit 7
/// = Right). The JS glue maps keyboard keys and Gamepad buttons onto these
/// booleans (see `site/app.js` `pollInput`); the engine consumes the byte
/// in [`Game::step`].
#[allow(clippy::too_many_arguments)] // one bool per button reads best at the JS boundary
pub fn pack_input(
    a: bool,
    b: bool,
    select: bool,
    start: bool,
    up: bool,
    down: bool,
    left: bool,
    right: bool,
) -> u8 {
    (u8::from(a))
        | (u8::from(b) << game::INPUT_B)
        | (u8::from(select) << game::INPUT_SELECT)
        | (u8::from(start) << game::INPUT_START)
        | (u8::from(up) << game::INPUT_UP)
        | (u8::from(down) << game::INPUT_DOWN)
        | (u8::from(left) << game::INPUT_LEFT)
        | (u8::from(right) << game::INPUT_RIGHT)
}

// ---------------------------------------------------------------------------
// ROM report.
// ---------------------------------------------------------------------------

/// One extracted asset section (offsets are into the verified ROM body).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssetEntry {
    id: u16,
    off: usize,
    len: usize,
}

/// Provenance of a loaded ROM (also serialized into [`WebEmu::state`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomReport {
    /// Body CRC32 (hex, 8 chars).
    pub crc32_hex: String,
    /// Body SHA1 (hex, 40 chars).
    pub sha1_hex: String,
    /// Sections resolved from the extractor table.
    pub sections: usize,
}

// ---------------------------------------------------------------------------
// WebEmu.
// ---------------------------------------------------------------------------

/// Whole tab emulator state: live [`Game`] plus the QA/movie/audio helpers
/// the JS glue needs. Construct with [`WebEmu::new`], boot with
/// [`WebEmu::load_rom`], then step.
#[wasm_bindgen]
pub struct WebEmu {
    game: Option<Game>,
    rom_report: Option<RomReport>,
    rom_body: Vec<u8>,
    assets: Vec<AssetEntry>,
    rgba: Vec<u8>,
    movie: Vec<u8>,
    movie_cursor: usize,
    movie_kind: Option<MovieKind>,
    apu: Apu,
    audio_fifo: Vec<i16>,
    /// PCM rate the synth renders at. Set from the page's `AudioContext`
    /// rate (see [`WebEmu::set_audio_rate`]) so no resampling — and no
    /// production/consumption drift — sits between the two.
    audio_rate: u32,
    last_input: u8,
    /// Widescreen margin tiles per side (0 = off).
    wide_tiles: u8,
    /// Decoded margins, allocated only while widescreen is on.
    margins: Option<Box<z2_ppu::Margins>>,
    /// Composed wide indexed frame, allocated only while widescreen is on.
    wide: Option<Box<z2_ppu::WideFrame>>,
    /// Paint the NES window's clipped left 8 columns from the fetched tiles
    /// (widescreen only). The engine default is off; the page turns it on
    /// because the overworld blanks those columns (`PPUMASK $18`), which would
    /// otherwise leave a black bar between the left margin and the play field.
    fill_left_clip: bool,
    /// Paint the NES window's right 8 columns (x 248-255) from the line's own
    /// tiles wherever the game masked them with an opaque edge sprite
    /// (widescreen only). The engine default is off; the page turns it on
    /// because the overworld parks a column of black sprites there, which
    /// would otherwise leave a black bar between the play field and the right
    /// margin.
    fill_right_clip: bool,
    /// Draw side-view enemies, NPCs and items outside the window into the
    /// margins (widescreen only; `Game::set_margin_sprites`). Display only:
    /// the observer stays out of [`trapset_id`]. Default on.
    margin_sprites: bool,
    /// Two-Link co-op requested. Re-applied after every ROM load, so the flag
    /// cannot go stale when a second ROM is dropped into the page.
    coop: bool,
    /// Wide gameplay requested: while widescreen is on, enemies spawn and
    /// live in the margins ([`Game::set_wide_gameplay`] with the widescreen
    /// margin). Default on; it changes gameplay, so it is part of the netplay
    /// identity. Re-applied after every ROM load.
    wide_gameplay: bool,
    /// Identity of the registered trap set (see [`trapset_id`]) with the
    /// wide-gameplay margin folded in ([`session_trapset_id`]).
    trapset_id: u64,
    /// [`trapset_id`] of the default groups alone.
    trapset_base: u64,
    /// HD-pack presenter and the staged directory upload (feature `hd`).
    #[cfg(feature = "hd")]
    hd: HdSlot,
    /// Live netplay session.
    #[cfg(feature = "net")]
    net: Option<NetSlot>,
    /// Netplay mode for the next session: rollback (default) or lockstep.
    net_rollback: bool,
    /// Rollback prediction window for the next session.
    net_max_prediction: u8,
    /// QA-only simulated network conditions (`z2.ext.net.simulate`).
    #[cfg(feature = "net")]
    net_sim: netsim::NetSim,
    /// Rollback cost probe ring (`perf_begin` .. `perf_end`).
    perf: Option<Box<perf::PerfRing>>,
}

/// Rollback prediction window the page uses unless told otherwise. Chosen
/// from in-browser timings of a worst-case rollback tick.
pub const WEB_DEFAULT_MAX_PREDICTION: u8 = 8;

/// Largest prediction window the page accepts (the protocol allows 30, but a
/// deeper rollback cannot be re-simulated inside one browser frame).
pub const WEB_MAX_PREDICTION: u8 = 12;

/// Identity of a registered trap set: FNV-1a over every `(addr, name)` pair,
/// address-sorted so registration order cannot change it. Display-only
/// observers ([`z2_core::wide_sprites::is_display_only_trap`]) are left out,
/// so peers with different widescreen settings still connect.
///
/// **This algorithm is duplicated in `z2-native` (`app::trapset_id`) and the
/// two must stay byte-identical**, or a native host and a web guest reject
/// each other's handshake even though they run the same routines. Both crates
/// pin it to [`TRAPSET_PIN_VALUE`] for the same input.
#[must_use]
pub fn trapset_id(game: &Game) -> u64 {
    let mut entries: Vec<(u16, &'static str)> = game
        .traps
        .iter()
        .filter(|t| !z2_core::wide_sprites::is_display_only_trap(t.name))
        .map(|t| (t.addr, t.name))
        .collect();
    entries.sort_unstable();
    fold_trapset(&entries)
}

/// The fold behind [`trapset_id`], over an already-sorted list.
fn fold_trapset(entries: &[(u16, &str)]) -> u64 {
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
/// (the same FNV-1a continued over `"wide_gameplay"` and the tile count);
/// `None` leaves the id untouched.
///
/// **Duplicated in `z2-native` (`app::session_trapset_id`)**; both are pinned
/// to [`WIDE_TRAPSET_PIN_VALUE`].
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

/// Cross-crate pin for [`trapset_id`]: the value both `z2-web` and
/// `z2-native` must produce for the entries
/// `[(0xFF9D, "bank7"), (0x00C2, "boot")]`.
pub const TRAPSET_PIN_VALUE: u64 = 0xad1b_d206_40e7_ea04;

/// Co-op status as JSON, hand-rolled.
///
/// `serde_json` is a dev-dependency only: pulling it into the default wasm
/// bundle to serialise ten scalars would be a poor trade.
fn coop_status_json(st: &z2_core::coop::CoopStatus) -> String {
    format!(
        "{{\"active\":{},\"p2Alive\":{},\"p2Hp\":{},\"hpMax\":{},\
         \"respawnFrames\":{},\"p2Deaths\":{},\"p2Page\":{},\"p2X\":{},\
         \"p2Y\":{},\"p2ScreenX\":{}}}",
        st.active,
        st.p2_alive,
        st.p2_hp,
        st.hp_max,
        st.respawn_frames,
        st.p2_deaths,
        st.p2_page,
        st.p2_x,
        st.p2_y,
        st.p2_screen_x
    )
}

/// The ICE setting a new page starts with (the panel's ICE field).
#[cfg(feature = "net")]
const NET_DEFAULT_ICE: &str = z2_net::DEFAULT_ICE_SPEC;
#[cfg(not(feature = "net"))]
const NET_DEFAULT_ICE: &str = "";

/// Status JSON for "no session" (also the whole reply in a build without the
/// `net` feature, so the JS status line works either way).
#[cfg(not(feature = "net"))]
const NET_INERT_JSON: &str = "{\"active\":false,\"supported\":false,\"state\":\"idle\",\
     \"closeReason\":null,\"role\":null,\"frame\":0,\"ready\":0,\"remoteAhead\":0,\
     \"stallMs\":0,\"delay\":0,\"requestedDelay\":0,\"started\":false,\"hashesOk\":0,\
     \"error\":null,\"link\":null,\"linkMs\":0,\"elapsedMs\":0,\"mode\":\"rollback\",\
     \"rttMs\":null}";

/// HD-pack state (feature `hd`): the presenter that owns the scaled RGBA
/// output plus the directory upload being assembled.
///
/// The presenter is built lazily, because a page with no pack at scale 1 never
/// needs it: `render_frame` then takes the light `z2-ppu`-only path that the
/// default bundle uses, and the two produce the same pixels.
#[cfg(feature = "hd")]
struct HdSlot {
    /// Built only while a pack is loaded or the scale is above 1.
    presenter: Option<z2_render::presenter::Presenter>,
    /// Requested output multiplier (`1..=MAX_HD_SCALE`).
    scale: u32,
    /// `scale` was raised by a pack rather than chosen by the user, so
    /// clearing the pack puts it back to 1.
    scale_auto: bool,
    /// `(path, bytes)` staged by `hd_pack_add_file`, consumed by
    /// `hd_pack_commit`. A directory picker hands over `pack.json` plus the
    /// PNG sheets; `HdPack::from_files` strips the picker's directory prefix.
    staged: Vec<(String, Vec<u8>)>,
    /// Total staged bytes (guards the tab against a mis-picked folder).
    staged_bytes: usize,
    /// Summary of the loaded pack, as JSON (`None` = no pack).
    info: Option<String>,
}

#[cfg(feature = "hd")]
impl HdSlot {
    fn new() -> Self {
        Self {
            presenter: None,
            scale: 1,
            scale_auto: false,
            staged: Vec::new(),
            staged_bytes: 0,
            info: None,
        }
    }
}

/// Largest output multiplier the page offers. `z2-render` accepts 1..=8, but
/// 8x widescreen is 3456x1920 RGBA (~26 MiB per frame), which is not a
/// browser-friendly default to leave one click away.
pub const MAX_HD_SCALE: u32 = 4;

/// Largest accepted total size of a staged HD-pack upload (decoded sheets cost
/// roughly `scale^2 * 4 KiB` per CHR page on top of this).
pub const MAX_HD_PACK_BYTES: usize = 96 * 1024 * 1024;

/// Largest accepted single file in a staged HD-pack upload.
pub const MAX_HD_FILE_BYTES: usize = 32 * 1024 * 1024;

/// Largest accepted file count in a staged HD-pack upload (a Zelda II pack has
/// one sheet per CHR page plus the manifest).
pub const MAX_HD_PACK_FILES: usize = 256;

/// Pack summary for builds without the `hd` feature, so the JS status line and
/// `hd_pack_info()` have the same shape either way.
#[cfg(not(feature = "hd"))]
const HD_INERT_JSON: &str = "null";

/// What a build without the `hd` feature says when asked to load a pack.
#[cfg(not(feature = "hd"))]
const HD_UNSUPPORTED: &str = "this build has no HD-pack support (build z2-web with --features hd)";

/// Pack summary as JSON, hand-rolled for the same reason as
/// [`coop_status_json`] (`serde_json` reaches the bundle through `z2-render`
/// only in an `hd` build, and the field names here are the JS contract).
#[cfg(feature = "hd")]
fn hd_pack_json(pack: &z2_render::pack::HdPack) -> String {
    format!(
        "{{\"name\":\"{}\",\"author\":{},\"scale\":{},\"cellSize\":{},\
         \"tiles\":{},\"variants\":{},\"sheets\":{}}}",
        json_escape(pack.name()),
        match pack.author() {
            Some(a) => format!("\"{}\"", json_escape(a)),
            None => "null".to_string(),
        },
        pack.scale(),
        pack.cell_size(),
        pack.tile_count(),
        pack.variant_count(),
        pack.sheets().len(),
    )
}

/// The protocol a live session runs.
#[cfg(feature = "net")]
enum NetKind {
    /// Input-delay lockstep; the session borrows the transport per update.
    Lockstep {
        session: Box<z2_net::Session>,
        transport: netsim::SimTransport,
    },
    /// Rollback; the session owns the transport.
    Rollback(Box<netroll::RollbackSlot>),
}

/// A live netplay session (feature `net`).
#[cfg(feature = "net")]
struct NetSlot {
    kind: NetKind,
    role: z2_net::Role,
    started: bool,
    delay: u8,
    requested_delay: u8,
    last_error: Option<String>,
    /// Wall clock accumulated from `net_poll(dt_ms)`; drives stall timers only.
    now_ms: u64,
    /// Last connect stage seen, and `now_ms` when it began (status line).
    stage: z2_net::ConnectStage,
    stage_since_ms: u64,
}

#[cfg(feature = "net")]
impl NetSlot {
    fn rollback_mut(&mut self) -> Option<&mut netroll::RollbackSlot> {
        match &mut self.kind {
            NetKind::Rollback(rb) => Some(rb),
            NetKind::Lockstep { .. } => None,
        }
    }

    fn transport(&self) -> &netsim::SimTransport {
        match &self.kind {
            NetKind::Lockstep { transport, .. } => transport,
            NetKind::Rollback(rb) => rb.session.transport(),
        }
    }

    fn transport_mut(&mut self) -> &mut netsim::SimTransport {
        match &mut self.kind {
            NetKind::Lockstep { transport, .. } => transport,
            NetKind::Rollback(rb) => rb.session.transport_mut(),
        }
    }
}

// The default `#[wasm_bindgen]` constructor path needs `Default`-free
// explicit construction; keep `new` in the bindgen impl so JS gets it.
#[wasm_bindgen]
impl WebEmu {
    /// New unbooted instance (no ROM, silence, empty RGBA buffer).
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            game: None,
            rom_report: None,
            rom_body: Vec::new(),
            assets: Vec::new(),
            rgba: vec![0; FRAME_RGBA_LEN],
            movie: Vec::new(),
            movie_cursor: 0,
            movie_kind: None,
            apu: Apu::new(SAMPLE_RATE),
            audio_fifo: Vec::new(),
            audio_rate: SAMPLE_RATE,
            last_input: 0,
            wide_tiles: 0,
            margins: None,
            wide: None,
            fill_left_clip: true,
            fill_right_clip: true,
            margin_sprites: true,
            coop: false,
            wide_gameplay: true,
            trapset_id: 0,
            trapset_base: 0,
            #[cfg(feature = "hd")]
            hd: HdSlot::new(),
            #[cfg(feature = "net")]
            net: None,
            net_rollback: true,
            net_max_prediction: WEB_DEFAULT_MAX_PREDICTION,
            #[cfg(feature = "net")]
            net_sim: netsim::NetSim::default(),
            perf: None,
        }
    }

    /// Crate version string (`window.z2.version`).
    pub fn version(&self) -> String {
        VERSION.to_string()
    }

    /// True once [`WebEmu::load_rom`] has accepted a ROM.
    pub fn rom_loaded(&self) -> bool {
        self.game.is_some()
    }

    /// Verified body CRC32 as 8 hex chars (`""` when no ROM).
    pub fn rom_crc32_hex(&self) -> String {
        self.rom_report
            .as_ref()
            .map(|r| r.crc32_hex.clone())
            .unwrap_or_default()
    }

    /// Verified body SHA1 as 40 hex chars (`""` when no ROM).
    pub fn rom_sha1_hex(&self) -> String {
        self.rom_report
            .as_ref()
            .map(|r| r.sha1_hex.clone())
            .unwrap_or_default()
    }

    /// Extracted asset section count (0 when no ROM).
    pub fn asset_section_count(&self) -> usize {
        self.assets.len()
    }

    /// Copy one extracted asset section by id (see
    /// `z2_assets::extract_tables::section_id`).
    pub fn asset_section(&self, id: u16) -> Result<Vec<u8>, String> {
        let e =
            self.assets.iter().find(|e| e.id == id).ok_or_else(|| {
                WebError::Rom(format!("unknown asset section ${id:04X}")).to_string()
            })?;
        Ok(self.rom_body[e.off..e.off + e.len].to_vec())
    }

    /// Load a dropped ROM file: hash-gate → in-tab extract → boot.
    ///
    /// Accepts a headered iNES file (16-byte `NES\x1A` header + 256 KiB
    /// body) or the bare 256 KiB body; both are verified against the
    /// pinned No-Intro USA hashes before a single byte executes.
    pub fn load_rom(&mut self, file: &[u8]) -> Result<(), String> {
        self.load_rom_impl(file).map_err(|e| e.to_string())
    }

    /// Reset the CPU to the RESET vector (keeps the loaded ROM + traps).
    pub fn reset_cpu(&mut self) -> Result<(), String> {
        need_game_mut(&mut self.game)
            .map_err(|e| e.to_string())?
            .reset();
        Ok(())
    }

    /// Run one logic frame on `input` (bits 0..7 = A,B,Select,Start,
    /// Up,Down,Left,Right).
    pub fn step_frame(&mut self, input: u8) -> Result<(), String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        self.step_one(input);
        Ok(())
    }

    /// Run up to `n` frames on a held `input` (capped at
    /// [`MAX_STEPS_PER_CALL`]; returns frames actually stepped).
    pub fn step_frames(&mut self, input: u8, n: u32) -> Result<u32, String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        let n = n.min(MAX_STEPS_PER_CALL);
        for _ in 0..n {
            self.step_one(input);
        }
        Ok(n)
    }

    /// QA `step(n)`: advance `n` frames, consuming the loaded movie's pad
    /// track when one is armed (cursor advances), else neutral input.
    /// Returns frames actually stepped.
    pub fn step_movie_or_idle(&mut self, n: u32) -> Result<u32, String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        let n = n.min(MAX_STEPS_PER_CALL);
        for _ in 0..n {
            let input = if self.movie_cursor < self.movie.len() {
                let b = self.movie[self.movie_cursor];
                self.movie_cursor += 1;
                b
            } else {
                0
            };
            self.step_one(input);
        }
        Ok(n)
    }

    /// QA `input(mask, frames)`: hold `mask` for `frames` frames (direct
    /// pad override; does not consume the movie track). Returns frames
    /// actually stepped.
    pub fn hold_input(&mut self, mask: u8, frames: u32) -> Result<u32, String> {
        self.step_frames(mask, frames)
    }

    /// Frames executed since boot (exact to 2^53 frames as a JS number).
    pub fn frame_count(&self) -> f64 {
        self.frame_count_u64() as f64
    }

    /// Last input byte stepped.
    pub fn last_input(&self) -> u8 {
        self.last_input
    }

    /// Expand the emulator's frame into the RGBA staging buffer. Call before
    /// reading [`WebEmu::frame_ptr`]/[`WebEmu::frame_rgba`].
    ///
    /// Three paths, all producing the same pixels for the same settings:
    /// plain 256x240 ([`z2_ppu::palette::indexed_to_rgba`], read-only), the
    /// widescreen composite ([`Game::compose_wide`]), and — with an HD pack or
    /// a scale above 1 in a `hd` build — the `z2-render` presenter.
    pub fn render_frame(&mut self) -> Result<(), String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        #[cfg(feature = "hd")]
        if self.hd_active() {
            return self.render_hd();
        }
        if self.wide_tiles > 0 {
            return self.render_wide();
        }
        let game = need_game(&self.game).map_err(|e| e.to_string())?;
        self.rgba.resize(FRAME_RGBA_LEN, 0);
        for (dst, &px) in self
            .rgba
            .chunks_exact_mut(4)
            .zip(game.frame_indexed().iter())
        {
            dst.copy_from_slice(&indexed_to_rgba(px));
        }
        Ok(())
    }

    /// Pointer to the RGBA staging buffer (pairs with
    /// [`WebEmu::frame_len`]; the buffer is stable until the next
    /// [`WebEmu::render_frame`] or method call — copy it out with
    /// `new Uint8ClampedArray(memory.buffer, ptr, len)`).
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// RGBA staging buffer length in bytes
    /// (`frame_width() * frame_height() * 4`).
    pub fn frame_len(&self) -> usize {
        self.rgba.len()
    }

    /// Presented width in pixels: `256 + 16 * margin_tiles`, times the HD
    /// output scale.
    ///
    /// JS reads this (and [`WebEmu::frame_height`]) to size the canvas after
    /// `load_rom` and after every widescreen / pack / scale change — the
    /// canvas is never assumed to be 256x240.
    pub fn frame_width(&self) -> usize {
        #[cfg(feature = "hd")]
        if let Some(p) = self.hd.presenter.as_ref() {
            return p.width();
        }
        z2_ppu::wide_width(self.wide_tiles)
    }

    /// Presented height in pixels: 240 times the HD output scale.
    pub fn frame_height(&self) -> usize {
        #[cfg(feature = "hd")]
        if let Some(p) = self.hd.presenter.as_ref() {
            return p.height();
        }
        FRAME_H
    }

    /// NES-pixel width of the presented frame, ignoring the HD scale
    /// (`wide_width(tiles)`). The page multiplies this — not
    /// [`WebEmu::frame_width`] — to pick a CSS width, so a 4x pack does not
    /// make the picture four times bigger on screen.
    pub fn logical_width(&self) -> usize {
        z2_ppu::wide_width(self.wide_tiles)
    }

    /// NES-pixel height of the presented frame, ignoring the HD scale.
    pub fn logical_height(&self) -> usize {
        FRAME_H
    }

    /// Output multiplier actually in force (1 without an HD pack, and always
    /// 1 in a build without the `hd` feature).
    pub fn output_scale(&self) -> u32 {
        #[cfg(feature = "hd")]
        if let Some(p) = self.hd.presenter.as_ref() {
            return p.effective_scale();
        }
        1
    }

    /// Widescreen margin tiles per side (0 = off).
    pub fn widescreen_tiles(&self) -> u8 {
        self.wide_tiles
    }

    /// Set the widescreen margin in tiles per side (0 = off, max 16).
    ///
    /// Applies immediately when a ROM is loaded and is re-applied to every
    /// later `load_rom`, so the UI toggle cannot go stale. Display only: the
    /// 256x240 NES frame the engine computes is never touched.
    pub fn set_widescreen(&mut self, tiles: u8) -> Result<(), String> {
        if usize::from(tiles) > z2_ppu::MAX_MARGIN_TILES {
            return Err(format!(
                "widescreen margin is {tiles} tiles, max {}",
                z2_ppu::MAX_MARGIN_TILES
            ));
        }
        self.wide_tiles = tiles;
        self.apply_wide_gameplay();
        if tiles == 0 {
            self.margins = None;
            self.wide = None;
        } else {
            let mut m = z2_ppu::Margins::new(tiles);
            m.fill_left_clip = self.fill_left_clip;
            m.fill_right_clip = self.fill_right_clip;
            m.fill_left_sprites = self.margin_sprites;
            self.margins = Some(Box::new(m));
            self.wide = Some(Box::new(z2_ppu::WideFrame::new(tiles)));
        }
        self.sync_present()
    }

    /// Whether the window's clipped left 8 columns are painted from the
    /// fetched tiles (widescreen only).
    pub fn fill_left_clip(&self) -> bool {
        self.fill_left_clip
    }

    /// Set [`WebEmu::fill_left_clip`]. On the overworld the ROM blanks those
    /// columns (`PPUMASK $18`); painting them removes the black bar between
    /// the left margin and the play field, at the cost of showing 8 columns
    /// the original hardware hid.
    pub fn set_fill_left_clip(&mut self, on: bool) -> Result<(), String> {
        self.fill_left_clip = on;
        if let Some(m) = self.margins.as_mut() {
            m.fill_left_clip = on;
        }
        self.sync_present()
    }

    /// Whether the window's right 8 columns are painted from the line's own
    /// tiles where the game masked them (widescreen only).
    pub fn fill_right_clip(&self) -> bool {
        self.fill_right_clip
    }

    /// Set [`WebEmu::fill_right_clip`]. The NES has no right-edge clip bit, so
    /// the overworld hides x 248-255 behind a column of opaque black 8x16
    /// sprites; painting those columns from the background tiles the line
    /// already fetched removes the black bar between the play field and the
    /// right margin, at the cost of showing 8 columns the original hid.
    pub fn set_fill_right_clip(&mut self, on: bool) -> Result<(), String> {
        self.fill_right_clip = on;
        if let Some(m) = self.margins.as_mut() {
            m.fill_right_clip = on;
        }
        self.sync_present()
    }

    /// Whether side-view objects outside the window are drawn into the
    /// margins (widescreen only).
    pub fn margin_sprites(&self) -> bool {
        self.margin_sprites
    }

    /// Set [`WebEmu::margin_sprites`]. The game keeps enemies, NPCs and items
    /// alive well past the window edge; with this on they are drawn into the
    /// margins (and the part of one just left of the window that the game
    /// hides). The game runs byte-identically either way.
    pub fn set_margin_sprites(&mut self, on: bool) -> Result<(), String> {
        self.margin_sprites = on;
        if let Some(m) = self.margins.as_mut() {
            m.fill_left_sprites = on;
        }
        self.sync_present()
    }

    // ------------------------------------------------------------- HD packs

    /// True when this bundle can load HD graphics packs (feature `hd`).
    ///
    /// The exported pack surface exists in both builds so `site/app.js` needs
    /// no `cfg` of its own: it asks this first and hides the picker.
    pub fn hd_supported(&self) -> bool {
        cfg!(feature = "hd")
    }

    /// Start (or restart) a pack upload, discarding anything already staged.
    pub fn hd_pack_begin(&mut self) -> Result<(), String> {
        #[cfg(feature = "hd")]
        {
            self.hd.staged.clear();
            self.hd.staged_bytes = 0;
        }
        Ok(())
    }

    /// Stage one file of a pack upload. `name` is the path as the directory
    /// picker reported it (`webkitRelativePath`); the shallowest `pack.json`
    /// defines the pack root and its prefix is stripped, so a whole picked
    /// folder can be handed over verbatim.
    ///
    /// The PNG sheets are decoded in Rust ([`z2_render::pack`]) — JS never
    /// touches image data.
    pub fn hd_pack_add_file(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        #[cfg(feature = "hd")]
        {
            if bytes.len() > MAX_HD_FILE_BYTES {
                return Err(format!(
                    "'{name}' is {} bytes, max {MAX_HD_FILE_BYTES} per file",
                    bytes.len()
                ));
            }
            if self.hd.staged.len() >= MAX_HD_PACK_FILES {
                return Err(format!(
                    "too many files in the picked folder (max {MAX_HD_PACK_FILES}) — \
                     pick the pack directory itself, not a parent"
                ));
            }
            let total = self.hd.staged_bytes.saturating_add(bytes.len());
            if total > MAX_HD_PACK_BYTES {
                return Err(format!(
                    "staged pack is {total} bytes, max {MAX_HD_PACK_BYTES}"
                ));
            }
            self.hd.staged_bytes = total;
            self.hd.staged.push((name.to_string(), bytes.to_vec()));
            Ok(())
        }
        #[cfg(not(feature = "hd"))]
        {
            let _ = (name, bytes);
            Err(HD_UNSUPPORTED.to_string())
        }
    }

    /// Build the staged files into a pack and present through it.
    ///
    /// On success returns the pack summary JSON (also
    /// [`WebEmu::hd_pack_info`]). On failure the previous presentation is left
    /// untouched — a rejected pack never stops the game, it just keeps the
    /// original art — and the staged files are dropped.
    pub fn hd_pack_commit(&mut self) -> Result<String, String> {
        #[cfg(feature = "hd")]
        {
            let staged = std::mem::take(&mut self.hd.staged);
            self.hd.staged_bytes = 0;
            if staged.is_empty() {
                return Err("no files staged: pick a folder containing pack.json".to_string());
            }
            let pack = z2_render::pack::HdPack::from_files(&staged).map_err(|e| e.to_string())?;
            let info = hd_pack_json(&pack);
            // A pack is drawn at its own scale unless the user picked one: at
            // the default scale 1 a 2x pack would be point-sampled straight
            // back down to NES resolution, which is not what loading it means.
            if self.hd.scale == 1 && !self.hd.scale_auto {
                self.hd.scale = pack.scale().clamp(1, MAX_HD_SCALE);
                self.hd.scale_auto = self.hd.scale > 1;
            }
            // Build (or reuse) the presenter, then hand it the pack: a pack
            // whose scale does not divide the requested one wins, so re-read
            // width()/height() afterwards (the JS side does, per frame).
            self.ensure_presenter()?;
            let p = self
                .hd
                .presenter
                .as_mut()
                .ok_or_else(|| "presenter unavailable".to_string())?;
            p.set_pack(Some(pack)).map_err(|e| e.to_string())?;
            self.hd.info = Some(info.clone());
            self.sync_present()?;
            Ok(info)
        }
        #[cfg(not(feature = "hd"))]
        {
            Err(HD_UNSUPPORTED.to_string())
        }
    }

    /// Drop the loaded pack and go back to the original art (the output scale
    /// is kept).
    pub fn hd_pack_clear(&mut self) -> Result<(), String> {
        #[cfg(feature = "hd")]
        {
            self.hd.staged.clear();
            self.hd.staged_bytes = 0;
            self.hd.info = None;
            if self.hd.scale_auto {
                self.hd.scale = 1;
                self.hd.scale_auto = false;
            }
            if let Some(p) = self.hd.presenter.as_mut() {
                p.set_pack(None).map_err(|e| e.to_string())?;
            }
        }
        self.sync_present()
    }

    /// Summary of the loaded pack as JSON
    /// (`{name, author, scale, cellSize, tiles, variants, sheets}`), or
    /// `"null"` when no pack is loaded.
    pub fn hd_pack_info(&self) -> String {
        #[cfg(feature = "hd")]
        {
            self.hd.info.clone().unwrap_or_else(|| "null".to_string())
        }
        #[cfg(not(feature = "hd"))]
        {
            HD_INERT_JSON.to_string()
        }
    }

    /// Requested output multiplier (what the selector shows). The value
    /// actually used is [`WebEmu::output_scale`].
    pub fn requested_scale(&self) -> u32 {
        #[cfg(feature = "hd")]
        {
            self.hd.scale
        }
        #[cfg(not(feature = "hd"))]
        {
            1
        }
    }

    /// Set the output multiplier, `1..=`[`MAX_HD_SCALE`].
    ///
    /// Scale 1 with no pack is the default and costs nothing: the presenter is
    /// not even built. Larger scales are a deliberate choice — 4x widescreen
    /// composes 1728x960 RGBA per frame (~1.9 ms) and a 4x pack keeps about
    /// 1 MiB of decoded sheet per CHR page — so the page warns before it
    /// leaves the cheap path.
    pub fn set_output_scale(&mut self, scale: u32) -> Result<(), String> {
        #[cfg(feature = "hd")]
        {
            if !(1..=MAX_HD_SCALE).contains(&scale) {
                return Err(format!("output scale is {scale}, want 1..={MAX_HD_SCALE}"));
            }
            self.hd.scale = scale;
            self.hd.scale_auto = false;
            if scale > 1 {
                self.ensure_presenter()?;
            }
            self.sync_present()
        }
        #[cfg(not(feature = "hd"))]
        {
            if scale == 1 {
                return Ok(());
            }
            Err(HD_UNSUPPORTED.to_string())
        }
    }

    /// [`WebEmu::set_widescreen`] from a preset name: `"off"`, `"16:10"`,
    /// `"16:9"`, or a number of tiles.
    pub fn set_widescreen_preset(&mut self, name: &str) -> Result<(), String> {
        let tiles = z2_ppu::preset_tiles(name).ok_or_else(|| {
            format!("unknown widescreen preset '{name}' (off | 16:10 | 16:9 | N)")
        })?;
        self.set_widescreen(tiles)
    }

    /// Whether wide gameplay is requested (it acts only while widescreen is
    /// on).
    pub fn wide_gameplay(&self) -> bool {
        self.wide_gameplay
    }

    /// Wide-gameplay margin in effect, tiles per side (0 = off).
    pub fn wide_gameplay_tiles(&self) -> u8 {
        self.wide_gameplay_margin().unwrap_or(0)
    }

    /// Request wide gameplay on or off: with widescreen on, overworld blobs,
    /// side-view enemies and townsfolk spawn and live in the margins instead
    /// of popping in at the original screen edge. Changes gameplay (and the
    /// netplay identity); ignored for the running game while a netplay
    /// session is live, and re-applied after every ROM load.
    pub fn set_wide_gameplay(&mut self, on: bool) -> Result<(), String> {
        self.wide_gameplay = on;
        self.apply_wide_gameplay();
        Ok(())
    }

    /// Enable/disable two-Link co-op (pad 2 drives a second Link in
    /// side-view). Re-applied after every ROM load.
    pub fn coop_enable(&mut self, on: bool) -> Result<(), String> {
        self.coop = on;
        match self.game.as_mut() {
            Some(g) => {
                g.set_coop(on);
                Ok(())
            }
            // Remembered for the next load_rom rather than refused, so the UI
            // checkbox works before a ROM is dropped.
            None => Ok(()),
        }
    }

    /// Whether co-op is requested.
    pub fn coop_enabled(&self) -> bool {
        self.coop
    }

    /// Co-op status as JSON (`null` when co-op is off).
    pub fn coop_status(&self) -> Result<String, String> {
        let game = need_game(&self.game).map_err(|e| e.to_string())?;
        match game.coop_status() {
            Some(st) => Ok(coop_status_json(&st)),
            None => Ok("null".to_string()),
        }
    }

    /// True when this build can open an online session (wasm32 + `netplay`).
    pub fn net_supported(&self) -> bool {
        cfg!(all(feature = "netplay", target_arch = "wasm32"))
    }

    /// Start a session. Needs a loaded ROM; `host` = player 1 (camera owner).
    ///
    /// `ice` is the shared ICE text form (`z2_net::IceConfig::parse`): space-
    /// separated `stun:`/`turn:` URLs with optional `username=`/`credential=`,
    /// `default`, or empty / `none` for no ICE servers (same machine or LAN).
    pub fn net_connect(
        &mut self,
        signal_url: &str,
        room: &str,
        host: bool,
        delay: u8,
        ice: &str,
    ) -> Result<(), String> {
        self.net_connect_impl(signal_url, room, host, delay, ice)
    }

    /// The default ICE setting, in the text form `net_connect` takes.
    pub fn net_default_ice(&self) -> String {
        NET_DEFAULT_ICE.to_string()
    }

    /// One tick of transport + session work (`dt_ms` = elapsed wall ms, which
    /// only drives stall timers). Returns the same JSON as
    /// [`WebEmu::net_state`].
    pub fn net_poll(&mut self, dt_ms: u32) -> Result<String, String> {
        self.net_poll_impl(dt_ms)
    }

    /// Step up to `max` confirmed lockstep frames, latching `local_pad` for
    /// each. Returns frames actually stepped (0 = waiting for the peer).
    pub fn net_step(&mut self, local_pad: u8, max: u32) -> Result<u32, String> {
        self.net_step_impl(local_pad, max)
    }

    /// Session status as JSON.
    pub fn net_state(&self) -> String {
        self.net_state_impl()
    }

    /// The desync hash the two peers actually compare, as hex.
    ///
    /// This is the *same* value `net_step` reports through
    /// `Session::report_hash`, so two peers that agree on it at the same
    /// session frame are in lockstep by the protocol's own definition — which
    /// is what `site/netplay-e2e.mjs` asserts. Errs without a ROM, and in a
    /// bundle built without the `net` feature (where no such hash exists).
    pub fn net_state_hash_hex(&self) -> Result<String, String> {
        self.net_state_hash_hex_impl()
    }

    /// Announce the quit and drop the session.
    pub fn net_disconnect(&mut self) {
        self.net_disconnect_impl();
    }

    /// Choose the protocol for the next session: `"rollback"` (default) or
    /// `"lockstep"`, and the rollback prediction window (clamped to
    /// `0..=WEB_MAX_PREDICTION`). A live session keeps the mode it began with.
    pub fn net_set_mode(&mut self, mode: &str, max_prediction: u8) -> Result<(), String> {
        self.net_rollback = match mode {
            "rollback" => true,
            "lockstep" => false,
            other => {
                return Err(format!(
                    "unknown netplay mode '{other}' (rollback or lockstep)"
                ))
            }
        };
        self.net_max_prediction = max_prediction.min(WEB_MAX_PREDICTION);
        Ok(())
    }

    /// The mode the next session uses, as JSON
    /// (`{"mode","maxPrediction","defaultMaxPrediction","maxAllowed"}`).
    pub fn net_mode(&self) -> String {
        format!(
            "{{\"mode\":\"{}\",\"maxPrediction\":{},\"defaultMaxPrediction\":{},\"maxAllowed\":{}}}",
            if self.net_rollback { "rollback" } else { "lockstep" },
            self.net_max_prediction,
            WEB_DEFAULT_MAX_PREDICTION,
            WEB_MAX_PREDICTION,
        )
    }

    /// QA only: simulate latency, jitter and loss on packets this page sends
    /// (see the `netsim` module). Applies to the live session and later ones; all zero
    /// turns it off. Returns the applied setting as JSON.
    pub fn net_simulate(&mut self, latency_ms: u32, jitter_ms: u32, loss_pct: u8) -> String {
        self.net_simulate_impl(latency_ms, jitter_ms, loss_pct)
    }

    /// Rollback only: `[[frame,"hash"],...]` for every 15th saved state that no
    /// rollback can change any more (`[]` otherwise).
    pub fn net_confirmed_hashes(&self) -> String {
        self.net_confirmed_hashes_impl()
    }

    /// Co-op state hash as hex (netplay/QA). Hex rather than a `u64`, which
    /// would cross the wasm boundary as a BigInt.
    pub fn coop_hash_hex(&self) -> Result<String, String> {
        let game = need_game(&self.game).map_err(|e| e.to_string())?;
        Ok(format!("{:016x}", game.coop_hash()))
    }

    /// Registered trap-set identity as hex (netplay handshake / QA).
    pub fn trapset_id_hex(&self) -> String {
        format!("{:016x}", self.trapset_id)
    }

    /// Run up to `n` frames with both pads held (co-op). Returns frames
    /// stepped (capped at [`MAX_STEPS_PER_CALL`]).
    pub fn step_frames2(&mut self, p1: u8, p2: u8, n: u32) -> Result<u32, String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        let n = n.min(MAX_STEPS_PER_CALL);
        for _ in 0..n {
            self.step_one2(p1, p2);
        }
        Ok(n)
    }

    /// Copying read of the RGBA staging buffer (render with
    /// [`WebEmu::render_frame`] first).
    pub fn frame_rgba(&self) -> Vec<u8> {
        self.rgba.clone()
    }

    /// QA `screenshot()`: render + copy the RGBA framebuffer.
    pub fn screenshot(&mut self) -> Result<Vec<u8>, String> {
        self.render_frame()?;
        Ok(self.rgba.clone())
    }

    /// QA `facts()`: typed automation snapshot as compact JSON
    /// ([`z2_core::facts::GameFacts`], identical schema on native/web).
    pub fn facts(&self) -> Result<String, String> {
        self.facts_impl().map_err(|e| e.to_string())
    }

    /// QA `state()`: tab status as compact JSON
    /// (`{version, romLoaded, crc32, sha1, sections, frame, lastInput,
    /// movieKind, movieLen, movieCursor, audioQueued}`).
    pub fn state(&self) -> String {
        let (crc, sha, sections) = match &self.rom_report {
            Some(r) => (r.crc32_hex.clone(), r.sha1_hex.clone(), r.sections),
            None => (String::new(), String::new(), 0),
        };
        let kind = match self.movie_kind {
            Some(MovieKind::Fm2) => "fm2",
            Some(MovieKind::Bk2Log) => "bk2log",
            None => "none",
        };
        format!(
            "{{\"version\":\"{VERSION}\",\"romLoaded\":{},\"crc32\":\"{crc}\",\
             \"sha1\":\"{sha}\",\"sections\":{sections},\"frame\":{},\
             \"lastInput\":{},\"movieKind\":\"{kind}\",\"movieLen\":{},\
             \"movieCursor\":{},\"audioQueued\":{}}}",
            self.game.is_some(),
            self.frame_count_u64(),
            self.last_input,
            self.movie.len(),
            self.movie_cursor,
            self.audio_fifo.len(),
        )
    }

    /// QA `snapshot()`: encode the `Z2WEB01` save-state blob (persist it
    /// in IndexedDB on the JS side).
    pub fn snapshot(&self) -> Result<Vec<u8>, String> {
        let game = need_game(&self.game).map_err(|e| e.to_string())?;
        Ok(
            WebSnapshot::new(game.frame_count(), game.ram(), game.wram())
                .map_err(WebError::from)
                .map_err(|e| e.to_string())?
                .encode(),
        )
    }

    /// QA `restore(bytes)`: accept a `Z2WEB01` blob (exact) or a `Z2SNAP01`
    /// corpus snapshot (ram/wram import; see [`snapshot`]). Returns a JSON
    /// status object. Note: the engine frame counter is not rewound (the
    /// core exposes no state import); memory images are restored exactly.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.restore_impl(bytes).map_err(|e| e.to_string())
    }

    /// QA `loadMovie(text)`: parse `.fm2` / `Input Log.txt` text, arm the
    /// pad track (cursor reset). Returns a JSON report
    /// (`{kind, frames, warnings[]}`). Binary `.bk2` (ZIP) is not
    /// accepted in-tab — drop the extracted `Input Log.txt` instead.
    pub fn load_movie(&mut self, text: &str) -> Result<String, String> {
        let track =
            movie::parse_movie(text).map_err(|e| WebError::Movie(e.to_string()).to_string())?;
        let kind = match track.kind {
            MovieKind::Fm2 => "fm2",
            MovieKind::Bk2Log => "bk2log",
        };
        let report = format!(
            "{{\"kind\":\"{kind}\",\"frames\":{},\"warnings\":[{}]}}",
            track.pads.len(),
            track
                .warnings
                .iter()
                .map(|w| format!("\"{}\"", json_escape(w)))
                .collect::<Vec<_>>()
                .join(","),
        );
        self.movie = track.pads;
        self.movie_cursor = 0;
        self.movie_kind = Some(track.kind);
        Ok(report)
    }

    /// Loaded movie length in frames (0 when none).
    pub fn movie_len(&self) -> usize {
        self.movie.len()
    }

    /// Movie cursor (frames consumed by [`WebEmu::step_movie_or_idle`]).
    pub fn movie_cursor(&self) -> usize {
        self.movie_cursor
    }

    /// Rewind the armed movie to frame 0.
    pub fn movie_rewind(&mut self) {
        self.movie_cursor = 0;
    }

    /// Copy out the 8 KiB battery WRAM (`$6000-$7FFF`) for IndexedDB slots.
    pub fn sram_bytes(&self) -> Result<Vec<u8>, String> {
        Ok(need_game(&self.game)
            .map_err(|e| e.to_string())?
            .wram()
            .to_vec())
    }

    /// Replace the 8 KiB battery WRAM (exactly 8192 bytes).
    pub fn load_sram(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != WRAM_SIZE {
            return Err(WebError::Sram(format!(
                "SRAM image is {} bytes, want {WRAM_SIZE}",
                bytes.len()
            ))
            .to_string());
        }
        let game = need_game_mut(&mut self.game).map_err(|e| e.to_string())?;
        game.wram.copy_from_slice(bytes);
        Ok(())
    }

    /// Queued PCM samples (mono, at [`WebEmu::audio_rate`]) awaiting the
    /// AudioWorklet.
    pub fn audio_queued(&self) -> usize {
        self.audio_fifo.len()
    }

    /// PCM rate the synth currently renders at, in Hz.
    pub fn audio_rate(&self) -> u32 {
        self.audio_rate
    }

    /// Render PCM at `rate` from now on; returns the rate actually applied.
    ///
    /// The page calls this with its `AudioContext` sample rate once the
    /// context exists. Matching the two matters for latency, not just pitch:
    /// the page steps 60.0988 NES frames per wall-clock second and each frame
    /// renders `rate / 60.0988` samples, so production is exactly `rate`
    /// samples per second. The audio thread consumes exactly the context's
    /// rate. Render 44100 into a 48000 Hz context and the worklet runs 3900
    /// samples/s short (a permanent underrun, ~8% sharp); render 48000 into a
    /// 44100 Hz context and it gains 3900 samples/s, i.e. a second of extra
    /// delay every 11 s until the worklet's cap starts dropping audio.
    ///
    /// Only the two shipped game rates are synthesisable; anything else
    /// falls back to 44100 and is reported by the return value so the caller
    /// can rebuild its context at a rate the synth can feed exactly. The
    /// queue is dropped because its contents are at the old rate.
    pub fn set_audio_rate(&mut self, rate: u32) -> u32 {
        let want = if z2_apu::audio::sample_rate_supported(rate) {
            rate
        } else {
            SAMPLE_RATE
        };
        if want != self.audio_rate {
            self.audio_rate = self.apu.set_sample_rate(want);
            self.audio_fifo.clear();
        }
        self.audio_rate
    }

    /// Drain queued PCM as `f32` mono in `[-1, 1]` (empties the queue).
    pub fn take_audio_f32(&mut self) -> Vec<f32> {
        self.audio_fifo
            .drain(..)
            .map(|s| (f32::from(s)) / 32768.0)
            .collect()
    }

    /// Pack eight button states into the shared input byte (see
    /// [`pack_input`]).
    #[allow(clippy::too_many_arguments)] // mirrors pack_input for direct JS calls
    pub fn pack_buttons(
        &self,
        a: bool,
        b: bool,
        select: bool,
        start: bool,
        up: bool,
        down: bool,
        left: bool,
        right: bool,
    ) -> u8 {
        pack_input(a, b, select, start, up, down, left, right)
    }
}

// Non-exported logic (plain Rust, directly unit-tested on host).
impl WebEmu {
    /// Whether tile identities are needed this frame: widescreen margins are
    /// decoded from the render record, and HD art is looked up by tile.
    fn needs_record(&self) -> bool {
        if self.wide_tiles > 0 {
            return true;
        }
        #[cfg(feature = "hd")]
        if self.hd.info.is_some() {
            return true;
        }
        false
    }

    /// Arm/disarm the PPU render record and resize the RGBA buffer to the
    /// current presentation. Called after every setting change and after every
    /// `load_rom` (which builds a brand-new `Game`).
    fn sync_present(&mut self) -> Result<(), String> {
        let want_record = self.needs_record();
        let want_sprites = self.wide_tiles > 0 && self.margin_sprites;
        if let Some(g) = self.game.as_mut() {
            g.set_record(want_record);
            g.set_margin_sprites(want_sprites);
        }
        #[cfg(feature = "hd")]
        {
            let (tiles, clip) = (self.wide_tiles, self.fill_left_clip);
            let rclip = self.fill_right_clip;
            let scale = self.hd.scale;
            if let Some(p) = self.hd.presenter.as_mut() {
                p.set_config(z2_render::presenter::PresentConfig {
                    scale,
                    margin_tiles: tiles,
                    fill_left_clip: clip,
                    fill_right_clip: rclip,
                })
                .map_err(|e| e.to_string())?;
            }
        }
        let want = self.frame_width() * self.frame_height() * 4;
        self.rgba.resize(want, 0);
        Ok(())
    }

    /// Widescreen without an HD pack: compose the wide indexed frame and
    /// expand it through the shared palette.
    fn render_wide(&mut self) -> Result<(), String> {
        let Self {
            game,
            margins,
            wide,
            rgba,
            wide_tiles,
            ..
        } = self;
        let game = game.as_ref().ok_or_else(|| WebError::NoRom.to_string())?;
        let m = margins
            .as_mut()
            .ok_or_else(|| "widescreen margins not allocated".to_string())?;
        let w = wide
            .as_mut()
            .ok_or_else(|| "widescreen frame not allocated".to_string())?;
        // False only when the record is off; the centre is still copied, so a
        // frame rendered before the first step is backdrop + centre.
        let _armed = game.compose_wide(*wide_tiles, m, w);
        rgba.resize(w.pixels.len() * 4, 0);
        for (dst, &px) in rgba.chunks_exact_mut(4).zip(w.pixels.iter()) {
            dst.copy_from_slice(&indexed_to_rgba(px));
        }
        Ok(())
    }

    fn frame_count_u64(&self) -> u64 {
        self.game.as_ref().map(Game::frame_count).unwrap_or(0)
    }

    fn load_rom_impl(&mut self, file: &[u8]) -> Result<(), WebError> {
        if file.len() > MAX_ROM_BYTES {
            return Err(WebError::Rom(format!(
                "file is {} bytes, max {MAX_ROM_BYTES}",
                file.len()
            )));
        }
        // Hash gate first: nothing executes unless the body verifies.
        let body = rom::strip_ines_header(file);
        rom::verify_body(body).map_err(|e| WebError::Rom(e.to_string()))?;
        let extracted = extract::extract(body).map_err(|e| WebError::Rom(e.to_string()))?;

        // `Game::from_ines` wants the full iNES image (header + PRG + CHR).
        // The body just verified is the pinned 256 KiB image, so its layout
        // is known: boot it behind the canonical header rather than the
        // dropped file's, and a netplay restart (`restart_for_session`),
        // which rebuilds from the stored bare body, boots the same image.
        // That restart header used to declare half the CHR (8x8 KiB): every
        // bank the mapper selected past 64 KiB then failed to load while the
        // mapper cache marked it served, and the overworld drew with the
        // previous area's tiles — black except the road.
        let ines = canonical_ines(body);

        let mut game = Game::from_ines(&ines).map_err(|e| WebError::Rom(e.to_string()))?;
        game.reset();
        // Same trap wiring as `xtask verify --dut game` (see
        // tools/xtask/src/verify.rs): ported routines replace interpretation.
        z2_core::bank7_traps::register_bank7_traps(&mut game);
        z2_core::sideview_traps::register_sideview_traps(&mut game);
        z2_core::sideview_traps::register_overworld_traps(&mut game);
        z2_core::player_traps::register_player_traps(&mut game);
        z2_core::enemy_traps::register_enemy_traps(&mut game);
        z2_core::town_traps::register_town_traps(&mut game);
        z2_core::palace_traps::register_palace_traps(&mut game);
        z2_core::title_traps::register_title_traps(&mut game);
        // The ninth group, which the web build used to omit: native and
        // `xtask verify` both register it, and netplay compares the trap-set
        // identity between peers, so a web guest could never have joined a
        // native host without it.
        z2_core::boot_traps::register_boot_traps(&mut game);
        self.trapset_base = trapset_id(&game);
        // Re-apply the opt-in features to the NEW game. Without this a second
        // ROM load would silently drop widescreen and co-op while the UI still
        // showed them as on.
        if self.coop {
            game.set_coop(true);
        }
        let wide = self.wide_gameplay_margin();
        game.set_wide_gameplay(wide);
        self.trapset_id = session_trapset_id(self.trapset_base, wide);

        let mut assets = Vec::with_capacity(extracted.sections.len());
        for s in &extracted.sections {
            let off = (s.def.file_off as usize).saturating_sub(rom::INES_HEADER_LEN);
            assets.push(AssetEntry {
                id: s.def.id,
                off,
                len: s.bytes.len(),
            });
        }

        self.apu = Apu::new(self.audio_rate);
        // DMC sample bytes come from the cartridge PRG the game just parsed.
        self.apu
            .install_dmc_source(Box::new(PrgSource::new(game.prg.to_vec())));
        self.audio_fifo.clear();
        self.movie.clear();
        self.movie_cursor = 0;
        self.movie_kind = None;
        self.last_input = 0;
        self.rom_body = body.to_vec();
        self.assets = assets;
        self.rom_report = Some(RomReport {
            crc32_hex: format!("{:08X}", extracted.body_crc32),
            sha1_hex: hex20(&extracted.body_sha1),
            sections: extracted.sections.len(),
        });
        self.game = Some(game);
        // Arm the render record and size the RGBA buffer for whatever the UI
        // already has selected: `load_rom` builds a brand-new `Game`, so
        // without this a second ROM drop would run with widescreen and HD off
        // while the controls still showed them on.
        self.sync_present().map_err(WebError::Rom)?;
        Ok(())
    }

    /// Wide-gameplay margin the settings ask for.
    fn wide_gameplay_margin(&self) -> Option<u8> {
        (self.wide_gameplay && self.wide_tiles > 0).then_some(self.wide_tiles)
    }

    /// Push the wide-gameplay setting into the running game (not while a
    /// netplay session is live: both peers started with the margin the
    /// handshake matched, and changing it would desync them).
    fn apply_wide_gameplay(&mut self) {
        #[cfg(feature = "net")]
        if self.net.is_some() {
            return;
        }
        let want = self.wide_gameplay_margin();
        if let Some(g) = self.game.as_mut() {
            if g.wide_gameplay_tiles() != want {
                g.set_wide_gameplay(want);
            }
            self.trapset_id = session_trapset_id(self.trapset_base, want);
        }
    }

    /// Two-pad sibling of [`WebEmu::step_one`] (co-op / netplay).
    fn step_one2(&mut self, p1: u8, p2: u8) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        self.last_input = p1;
        game.step2(p1, p2);
        self.audio_after_step();
    }

    fn step_one(&mut self, input: u8) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        self.last_input = input;
        game.step(input);
        self.audio_after_step();
    }

    /// Per-frame audio tail shared by every stepping path.
    fn audio_after_step(&mut self) {
        let Some(game) = self.game.as_mut() else {
            return;
        };
        // The game's own sound engine ran on the interpreter this frame and
        // sank its `$4000-$4017` writes into the null-APU façade; drain them
        // into the synth before rendering PCM (same as the native frontend's
        // `drain_audio_frame`), so what you hear is the game's music and SFX.
        for (addr, val) in game.apu.drain_log() {
            self.apu.write_reg(addr, val);
        }
        let mut out = Vec::new();
        self.apu.audio(&mut out);
        self.audio_fifo.extend_from_slice(&out);
        if self.audio_fifo.len() > AUDIO_FIFO_CAP {
            let drop = self.audio_fifo.len() - AUDIO_FIFO_CAP;
            self.audio_fifo.drain(..drop);
        }
    }

    fn facts_impl(&self) -> Result<String, WebError> {
        let game = need_game(&self.game)?;
        let ram = Ram::from_slice(game.ram()).map_err(|e| WebError::Rom(e.to_string()))?;
        Ok(FactsGame::new(ram).facts().to_json())
    }

    fn restore_impl(&mut self, bytes: &[u8]) -> Result<String, WebError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(WebError::Snapshot(format!(
                "blob is {} bytes, max {MAX_STATE_BYTES}",
                bytes.len()
            )));
        }
        // Own format first, corpus interop second.
        if let Ok(snap) = WebSnapshot::decode(bytes) {
            let game = need_game_mut(&mut self.game)?;
            game.ram.copy_from_slice(&snap.ram);
            game.wram.copy_from_slice(&snap.wram);
            // The second Link's parked block describes the RAM we just
            // replaced; drop it so it re-anchors beside player 1.
            game.coop_reset_area();
            return Ok(format!(
                "{{\"format\":\"z2web01\",\"frame\":{}}}",
                snap.frame_count
            ));
        }
        let cs = import_corpus_snapshot(bytes).map_err(WebError::from)?;
        let game = need_game_mut(&mut self.game)?;
        game.ram.copy_from_slice(&cs.ram);
        game.wram.copy_from_slice(&cs.wram);
        game.coop_reset_area();
        Ok(format!(
            "{{\"format\":\"z2snap01\",\"frameLogic\":{}}}",
            cs.frame_logic
        ))
    }
}

// ---------------------------------------------------------------------------
// HD packs (feature `hd`): the `z2-render` presenter path.
// ---------------------------------------------------------------------------

#[cfg(feature = "hd")]
impl WebEmu {
    /// Whether rendering goes through the presenter. Scale 1 with no pack does
    /// not, so the cheap path stays identical to the default bundle's.
    fn hd_active(&self) -> bool {
        self.hd.presenter.is_some() && (self.hd.info.is_some() || self.hd.scale > 1)
    }

    /// Build the presenter if it does not exist yet.
    fn ensure_presenter(&mut self) -> Result<(), String> {
        if self.hd.presenter.is_some() {
            return Ok(());
        }
        let cfg = z2_render::presenter::PresentConfig {
            scale: self.hd.scale,
            margin_tiles: self.wide_tiles,
            fill_left_clip: self.fill_left_clip,
            fill_right_clip: self.fill_right_clip,
        };
        self.hd.presenter =
            Some(z2_render::presenter::Presenter::new(cfg).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// Compose one frame through the presenter (HD pack and/or upscale).
    fn render_hd(&mut self) -> Result<(), String> {
        let Self {
            game,
            hd,
            rgba,
            wide_tiles,
            fill_right_clip,
            margin_sprites,
            ..
        } = self;
        let tiles = *wide_tiles;
        let right_clip = *fill_right_clip;
        let left_sprites = *margin_sprites;
        let game = game.as_ref().ok_or_else(|| WebError::NoRom.to_string())?;
        let p = hd
            .presenter
            .as_mut()
            .ok_or_else(|| "HD presenter not built".to_string())?;
        // Widescreen margins are decoded by z2-core (which owns the level
        // decoder) straight into the presenter's own Margins.
        if tiles > 0 {
            // `PresentConfig` carries both edge flags, but the toggles can
            // change between `set_config` calls, so re-apply here each frame.
            p.margins_mut().fill_right_clip = right_clip;
            p.margins_mut().fill_left_sprites = left_sprites;
            game.wide_margins(tiles, p.margins_mut());
        }
        if p.needs_scene() {
            p.set_scene(game.sideview_scene().map(|s| z2_render::SceneView {
                world: s.world,
                region: s.region,
                scene: s.scene,
                camera_x: s.camera_x,
            }));
        }
        // An unarmed record still composes: `FrameRecord::new()` yields the
        // centre frame with backdrop margins and no HD substitutions.
        let empty = z2_ppu::FrameRecord::new();
        let record = game.frame_record().unwrap_or(&empty);
        let out = p
            .present(game.frame_indexed(), record, &game.chr)
            .map_err(|e| e.to_string())?;
        rgba.resize(out.len(), 0);
        rgba.copy_from_slice(out);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Netplay: inert stubs without the `net` feature, real session with it. The
// exported methods exist in both builds so `site/app.js` needs no cfg of its
// own — it asks `net_supported()` and disables the panel.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "net"))]
impl WebEmu {
    fn net_connect_impl(
        &mut self,
        _s: &str,
        _r: &str,
        _h: bool,
        _d: u8,
        _i: &str,
    ) -> Result<(), String> {
        Err("this build has no netplay support (build z2-web with --features netplay)".to_string())
    }
    fn net_poll_impl(&mut self, _dt_ms: u32) -> Result<String, String> {
        Ok(self.net_state_impl())
    }
    fn net_step_impl(&mut self, _local_pad: u8, _max: u32) -> Result<u32, String> {
        Ok(0)
    }
    fn net_state_impl(&self) -> String {
        NET_INERT_JSON.to_string()
    }
    fn net_state_hash_hex_impl(&self) -> Result<String, String> {
        Err(
            "this build has no netplay support, so there is no lockstep hash \
             (build z2-web with --features netplay)"
                .to_string(),
        )
    }
    fn net_disconnect_impl(&mut self) {}
    fn net_simulate_impl(&mut self, _l: u32, _j: u32, _p: u8) -> String {
        "{\"latencyMs\":0,\"jitterMs\":0,\"lossPct\":0}".to_string()
    }
    fn net_confirmed_hashes_impl(&self) -> String {
        "[]".to_string()
    }
    pub(crate) fn net_active(&self) -> bool {
        false
    }
}

#[cfg(feature = "net")]
impl WebEmu {
    /// The desync hash lockstep peers compare. Must stay identical to
    /// `z2_native::netplay::state_hash`.
    fn net_state_hash(&self) -> u64 {
        self.game.as_ref().map_or(0, netroll::state_hash)
    }

    fn net_state_hash_hex_impl(&self) -> Result<String, String> {
        need_game(&self.game).map_err(|e| e.to_string())?;
        Ok(format!("{:016x}", self.net_state_hash()))
    }

    pub(crate) fn net_active(&self) -> bool {
        self.net.is_some()
    }

    fn net_simulate_impl(&mut self, latency_ms: u32, jitter_ms: u32, loss_pct: u8) -> String {
        self.net_sim = netsim::NetSim {
            latency_ms,
            jitter_ms,
            loss_pct,
        }
        .clamped();
        let sim = self.net_sim;
        if let Some(slot) = self.net.as_mut() {
            slot.transport_mut().set_sim(sim);
        }
        sim.json()
    }

    fn net_confirmed_hashes_impl(&self) -> String {
        match self.net.as_ref().map(|s| &s.kind) {
            Some(NetKind::Rollback(rb)) => rb.confirmed_hashes_json(),
            _ => "[]".to_string(),
        }
    }

    #[cfg(all(feature = "netplay", target_arch = "wasm32"))]
    fn open_transport(
        url: &str,
        ice: z2_net::IceConfig,
    ) -> Result<Box<dyn z2_net::Transport>, String> {
        // Never blocks: an unreachable server, a full room or a peer link that
        // cannot open surfaces later through the session state, and the
        // connect stage is readable at every tick in between.
        Ok(Box::new(z2_net::MatchboxTransport::connect(url, Some(ice))))
    }

    #[cfg(not(all(feature = "netplay", target_arch = "wasm32")))]
    fn open_transport(
        _url: &str,
        _ice: z2_net::IceConfig,
    ) -> Result<Box<dyn z2_net::Transport>, String> {
        Err("netplay needs the wasm32 build with --features netplay".to_string())
    }

    fn net_connect_impl(
        &mut self,
        signal_url: &str,
        room: &str,
        host: bool,
        delay: u8,
        ice: &str,
    ) -> Result<(), String> {
        if self.game.is_none() {
            return Err(WebError::NoRom.to_string());
        }
        // A `ws://` URL from an `https` page is blocked by the browser with an
        // opaque failure. The page checks `location.protocol` and says so
        // before it ever calls this (see `netConnect` in site/app.js); here we
        // only validate the URL shape.
        let url = z2_net::room_url(signal_url, room).map_err(|e| e.to_string())?;
        let ice = z2_net::IceConfig::parse(ice).map_err(|e| e.to_string())?;
        let inner = Self::open_transport(&url, ice)?;
        self.net_open(inner, host, delay)
    }

    /// Create the session over an already opened transport (the page's
    /// matchbox link, or a loopback link in tests).
    fn net_open(
        &mut self,
        inner: Box<dyn z2_net::Transport>,
        host: bool,
        delay: u8,
    ) -> Result<(), String> {
        if self.game.is_none() {
            return Err(WebError::NoRom.to_string());
        }
        let seed = if host { 0x5eed_0001 } else { 0x5eed_0002 };
        let transport = netsim::SimTransport::new(inner, self.net_sim, seed);
        let wram = self.game.as_ref().expect("checked above").wram().to_vec();
        // TWO_LINKS is mandatory: with zero co-op flags the guest's pad would
        // reach a ROM that never reads it, i.e. no gameplay at all.
        let host_flags = z2_net::COOP_TWO_LINKS;
        let guest_flags = z2_net::COOP_TWO_LINKS | z2_net::COOP_SPRITE_UNLIMITED;
        let (kind, role, delay) = if self.net_rollback {
            let delay = delay.min(z2_net::MAX_ROLLBACK_DELAY);
            let mut cfg = if host {
                z2_net::RollbackConfig::host(rom::EXPECTED_BODY_CRC32, self.trapset_id, host_flags)
            } else {
                z2_net::RollbackConfig::guest(
                    rom::EXPECTED_BODY_CRC32,
                    self.trapset_id,
                    guest_flags,
                )
            };
            cfg.input_delay = delay;
            cfg.max_prediction = self.net_max_prediction;
            if host {
                cfg.wram = Some(wram);
            }
            let role = cfg.role;
            let session =
                z2_net::RollbackSession::new(cfg, transport).map_err(|e| e.to_string())?;
            let slot = netroll::RollbackSlot::new(session, self.net_max_prediction);
            (NetKind::Rollback(Box::new(slot)), role, delay)
        } else {
            let delay = delay.min(z2_net::MAX_DELAY);
            let mut cfg = if host {
                z2_net::SessionConfig::host(rom::EXPECTED_BODY_CRC32, self.trapset_id, host_flags)
            } else {
                z2_net::SessionConfig::guest(rom::EXPECTED_BODY_CRC32, self.trapset_id, guest_flags)
            };
            cfg.delay = delay;
            if host {
                cfg.wram = Some(wram);
            }
            let role = cfg.role;
            let session = z2_net::Session::new(cfg).map_err(|e| e.to_string())?;
            (
                NetKind::Lockstep {
                    session: Box::new(session),
                    transport,
                },
                role,
                delay,
            )
        };
        self.net = Some(NetSlot {
            kind,
            role,
            started: false,
            delay,
            requested_delay: delay,
            last_error: None,
            now_ms: 0,
            stage: z2_net::ConnectStage::Signalling,
            stage_since_ms: 0,
        });
        Ok(())
    }

    fn net_poll_impl(&mut self, dt_ms: u32) -> Result<String, String> {
        if self.net.is_none() {
            return Ok(self.net_state_impl());
        }
        let lockstep_events = {
            let slot = self.net.as_mut().expect("checked");
            slot.now_ms = slot.now_ms.saturating_add(u64::from(dt_ms));
            let now = slot.now_ms;
            let events = match &mut slot.kind {
                NetKind::Lockstep { session, transport } => {
                    transport.set_now(now);
                    session.update(transport, now);
                    session.take_events()
                }
                NetKind::Rollback(rb) => {
                    rb.session.transport_mut().set_now(now);
                    rb.session.poll(now);
                    Vec::new()
                }
            };
            let stage = z2_net::Transport::stage(slot.transport());
            if stage != slot.stage {
                slot.stage = stage;
                slot.stage_since_ms = now;
            }
            events
        };
        for ev in lockstep_events {
            self.on_net_event(ev)?;
        }
        self.drain_rollback_events()?;
        Ok(self.net_state_impl())
    }

    /// Both modes: restart from power-on with the host's save and co-op
    /// flags, so the two games are byte-identical before frame 0.
    pub(crate) fn restart_for_session(
        &mut self,
        coop_flags: u32,
        wram: &[u8],
    ) -> Result<(), String> {
        self.coop = coop_flags & z2_net::COOP_TWO_LINKS != 0;
        let body = self.rom_body.clone();
        if body.is_empty() {
            return Err("no ROM body available to start a session".to_string());
        }
        // Rebuilds the game and re-applies widescreen + co-op.
        self.load_rom_impl(&body).map_err(|e| e.to_string())?;
        if let Some(g) = self.game.as_mut() {
            if wram.len() == g.wram.len() {
                g.wram.copy_from_slice(wram);
                g.coop_reset_area();
            }
        }
        self.audio_fifo.clear();
        Ok(())
    }

    fn on_net_event(&mut self, ev: z2_net::SessionEvent) -> Result<(), String> {
        use z2_net::SessionEvent as Ev;
        match ev {
            Ev::Started {
                delay,
                coop_flags,
                wram,
                ..
            } => {
                self.restart_for_session(coop_flags, &wram)?;
                if let Some(slot) = self.net.as_mut() {
                    slot.delay = delay;
                    slot.started = true;
                }
            }
            Ev::Desync { frame, .. } => {
                if let Some(slot) = self.net.as_mut() {
                    slot.last_error = Some(format!("desync at frame {frame}"));
                }
            }
            Ev::Closed(reason) => {
                if let Some(slot) = self.net.as_mut() {
                    slot.last_error = Some(reason.to_string());
                }
                self.audio_fifo.clear();
            }
            Ev::PeerHello { .. } | Ev::Stalled { .. } | Ev::Resumed { .. } => {}
        }
        Ok(())
    }

    fn net_step_impl(&mut self, local_pad: u8, max: u32) -> Result<u32, String> {
        let max = max.min(MAX_STEPS_PER_CALL);
        if matches!(
            self.net.as_ref().map(|s| &s.kind),
            Some(NetKind::Rollback(_))
        ) {
            // One rollback tick per due frame. Returns simulated frames
            // (replays included), so 0 means nothing new to present.
            let mut simulated = 0u32;
            for _ in 0..max {
                simulated += self.rollback_tick(local_pad)?.simulated;
            }
            return Ok(simulated);
        }
        let mut stepped = 0u32;
        for _ in 0..max {
            // The session borrow is released before the game steps, so the two
            // disjoint fields are never borrowed at once.
            let next = {
                let Some(slot) = self.net.as_mut() else { break };
                if !slot.started {
                    break;
                }
                let NetKind::Lockstep { session, .. } = &mut slot.kind else {
                    break;
                };
                if !session.is_live() {
                    break;
                }
                session.latch_local(local_pad);
                session
                    .poll()
                    .map(|(f, p1, p2)| (f, p1, p2, session.needs_hash(f)))
            };
            let Some((frame, p1, p2, want)) = next else {
                break;
            };
            self.step_one2(p1, p2);
            if want {
                let h = self.net_state_hash();
                if let Some(NetKind::Lockstep { session, .. }) =
                    self.net.as_mut().map(|s| &mut s.kind)
                {
                    session.report_hash(frame, h);
                }
            }
            stepped += 1;
        }
        // Flush the pads just latched so the peer is not kept waiting.
        if let Some(slot) = self.net.as_mut() {
            let now = slot.now_ms;
            if let NetKind::Lockstep { session, transport } = &mut slot.kind {
                transport.set_now(now);
                session.update(transport, now);
            }
        }
        Ok(stepped)
    }

    fn net_state_impl(&self) -> String {
        let Some(slot) = self.net.as_ref() else {
            return format!(
                "{{\"active\":false,\"supported\":{},\"state\":\"idle\",\"closeReason\":null,\
                  \"role\":null,\"frame\":0,\"ready\":0,\"remoteAhead\":0,\"stallMs\":0,\
                  \"delay\":0,\"requestedDelay\":0,\"started\":false,\"hashesOk\":0,\
                  \"error\":null,\"link\":null,\"linkMs\":0,\"elapsedMs\":0,\"mode\":\"{}\",\
                  \"rttMs\":null}}",
                self.net_supported(),
                if self.net_rollback {
                    "rollback"
                } else {
                    "lockstep"
                },
            );
        };
        let err = match &slot.last_error {
            Some(e) => format!("\"{}\"", json_escape(e)),
            None => "null".to_string(),
        };
        let role = match slot.role {
            z2_net::Role::Host => "host",
            z2_net::Role::Guest => "guest",
        };
        let (name, reason, frame, ready, ahead, stall_ms, hashes_ok, mode, extra) = match &slot.kind
        {
            NetKind::Lockstep { session, .. } => {
                let name = match session.state() {
                    z2_net::SessionState::Idle => "idle",
                    z2_net::SessionState::Handshake => "handshake",
                    z2_net::SessionState::Running => "running",
                    z2_net::SessionState::Stalled => "stalled",
                    z2_net::SessionState::Desynced => "desynced",
                    z2_net::SessionState::Closed => "closed",
                };
                let stats = session.stats();
                let reason = match session.close_reason() {
                    Some(r) => format!("\"{}\"", json_escape(&r.to_string())),
                    None => "null".to_string(),
                };
                (
                    name,
                    reason,
                    stats.frame,
                    session.frames_ready(),
                    i64::from(stats.remote_known) - i64::from(stats.frame),
                    stats.stall_ms,
                    stats.hashes_ok,
                    "lockstep",
                    format!(
                        ",\"rttMs\":{}",
                        stats.rtt_ms.map_or("null".to_string(), |r| r.to_string())
                    ),
                )
            }
            NetKind::Rollback(rb) => {
                let st = rb.session.stats();
                (
                    netroll::rollback_state_name(rb),
                    netroll::rollback_close_json(rb),
                    st.frame,
                    0,
                    i64::from(st.confirmed_frame) - i64::from(st.frame),
                    0,
                    st.checksums_ok,
                    "rollback",
                    netroll::rollback_status_fields(rb),
                )
            }
        };
        format!(
            "{{\"active\":true,\"supported\":{},\"state\":\"{name}\",\"closeReason\":{reason},\
              \"role\":\"{role}\",\"frame\":{frame},\"ready\":{ready},\"remoteAhead\":{ahead},\
              \"stallMs\":{stall_ms},\"delay\":{},\"requestedDelay\":{},\"started\":{},\
              \"hashesOk\":{hashes_ok},\"error\":{err},\"link\":\"{}\",\"linkMs\":{},\
              \"elapsedMs\":{},\"mode\":\"{mode}\"{extra}}}",
            self.net_supported(),
            slot.delay,
            slot.requested_delay,
            slot.started,
            slot.stage.as_str(),
            slot.now_ms.saturating_sub(slot.stage_since_ms),
            slot.now_ms,
        )
    }

    fn net_disconnect_impl(&mut self) {
        if let Some(slot) = self.net.as_mut() {
            let now = slot.now_ms;
            // One pump hands the goodbye to the transport; the browser flushes
            // the data channel as the page continues to run. Simulated delay is
            // dropped so the goodbye is not held back.
            slot.transport_mut().set_sim(netsim::NetSim::default());
            slot.transport_mut().set_now(u64::MAX / 2);
            match &mut slot.kind {
                NetKind::Lockstep { session, transport } => {
                    session.close();
                    session.update(transport, now);
                }
                NetKind::Rollback(rb) => {
                    rb.session.close();
                    rb.session.poll(now);
                }
            }
        }
        self.net = None;
        self.audio_fifo.clear();
    }
}

impl Default for WebEmu {
    fn default() -> Self {
        Self::new()
    }
}

fn hex20(digest: &[u8; 20]) -> String {
    let mut s = String::with_capacity(40);
    for b in digest {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('?'));
        s.push(char::from_digit(u32::from(b & 0xF), 16).unwrap_or('?'));
    }
    s
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Re-exported movie-track type for tests and embedders.
pub use movie::MovieTrack as WebMovieTrack;

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests need the narrow width now that `frame_width()` reports the
    // widescreen-aware size.
    use z2_core::game::FRAME_W;

    /// Synthetic body of the pinned length (no ROM bytes) behind the
    /// canonical header: zeroed PRG with the RESET vector parked on an `RTS`
    /// so `step` stays alive, plus zeroed CHR. Never touches the hash gate
    /// (that path is ROM-gated and covered by `load_rom` rejection tests
    /// below).
    fn synthetic_ines() -> Vec<u8> {
        let mut body = vec![0u8; rom::EXPECTED_BODY_LEN];
        // PRG offset of $FFFC (last bank): point RESET at $C000, and put an
        // RTS ($60) there so interpreted boot runs one benign routine.
        let prg_len = usize::from(INES_PRG_UNITS) * 0x4000;
        body[0] = 0x60; // $C000 in fixed-last-bank view
        let v = prg_len - 4;
        body[v] = 0x00;
        body[v + 1] = 0xC0;
        canonical_ines(&body)
    }

    /// `Z2_ROM` file bytes, or `None` (the test self-skips).
    fn rom_file(test: &str) -> Option<Vec<u8>> {
        let file = std::env::var("Z2_ROM")
            .ok()
            .filter(|p| std::path::Path::new(p).is_file())
            .map(|p| std::fs::read(p).expect("read Z2_ROM"));
        if file.is_none() {
            eprintln!("skipping {test}: Z2_ROM not set");
        }
        file
    }

    /// Text of a corpus `.fm2` movie (`$Z2_CORPUS/movies/NAME`,
    /// `$Z2_CORPUS_MOVIES/NAME`, or the checked-out corpus next to the
    /// repository), or `None`.
    fn movie_text(name: &str, test: &str) -> Option<String> {
        let dir = match std::env::var("Z2_CORPUS") {
            Ok(c) => std::path::Path::new(&c).join("movies"),
            Err(_) => std::path::PathBuf::from(
                std::env::var("Z2_CORPUS_MOVIES")
                    .unwrap_or_else(|_| "/Volumes/Holy Drive/dev/z2-corpus/movies".to_string()),
            ),
        };
        let text = std::fs::read_to_string(dir.join(name)).ok();
        if text.is_none() {
            eprintln!("skipping {test}: no movie {name} under {}", dir.display());
        }
        text
    }

    fn booted() -> WebEmu {
        let mut emu = WebEmu::new();
        let mut game = Game::from_ines(&synthetic_ines()).expect("synthetic boots");
        game.reset();
        emu.game = Some(game);
        emu
    }

    /// The canonical header must describe the whole 256 KiB body: PRG and
    /// CHR together are exactly the verified body, so no CHR bank the mapper
    /// can select lies past the image end.
    #[test]
    fn canonical_header_covers_the_whole_body() {
        let game =
            Game::from_ines(&canonical_ines(&vec![0u8; rom::EXPECTED_BODY_LEN])).expect("image");
        assert_eq!(game.prg.len(), usize::from(INES_PRG_UNITS) * 0x4000);
        assert_eq!(game.chr.len(), usize::from(INES_CHR_UNITS) * 0x2000);
        assert_eq!(game.prg.len() + game.chr.len(), rom::EXPECTED_BODY_LEN);
    }

    /// A headered drop and the bare body boot the same game with the full
    /// CHR, and on the overworld (the warpless track at frame 800) both CHR
    /// slots hold the bank the MMC1 selects, so the map is drawn with its
    /// own tiles. Regression for the netplay restart that rebuilt the game
    /// with a half-size CHR: the road stayed, everything else went black.
    /// Self-skips without `Z2_ROM`; the overworld half also needs the corpus.
    #[test]
    fn every_load_path_keeps_the_full_chr() {
        const TEST: &str = "every_load_path_keeps_the_full_chr";
        let Some(file) = rom_file(TEST) else {
            return;
        };
        let body = rom::strip_ines_header(&file).to_vec();
        let mut headered = WebEmu::new();
        headered.load_rom(&file).expect("headered file");
        let mut bare = WebEmu::new();
        bare.load_rom(&body).expect("bare body");
        for emu in [&headered, &bare] {
            let g = emu.game.as_ref().expect("game");
            assert_eq!(g.chr.len(), usize::from(INES_CHR_UNITS) * 0x2000);
        }
        let Some(movie) = movie_text("warpless.fm2", TEST) else {
            return;
        };
        for (name, emu) in [("headered", &mut headered), ("bare", &mut bare)] {
            emu.load_movie(&movie).expect("movie");
            emu.step_movie_or_idle(800).expect("step");
            let g = emu.game.as_ref().expect("game");
            assert_eq!(g.ram()[0x0736], 0x05, "{name}: not on the overworld");
            for slot in 0..2u8 {
                let want = (g.mmc1.map_chr4(slot) & 0xFF) as u8;
                assert_eq!(
                    g.ppu.model().chr_page_no(usize::from(slot)),
                    want,
                    "{name}: CHR slot {slot} does not hold the selected bank"
                );
            }
            emu.render_frame().expect("render");
            let black = emu
                .frame_rgba()
                .chunks_exact(4)
                .filter(|px| px[0] == 0 && px[1] == 0 && px[2] == 0)
                .count();
            assert!(
                black < FRAME_LEN / 4,
                "{name}: overworld frame is mostly black ({black} of {FRAME_LEN} px)"
            );
        }
        assert_eq!(headered.frame_rgba(), bare.frame_rgba());
    }

    /// A netplay session restarts the game from the stored bare body; that
    /// rebuild must carry the same full CHR image as the original drop.
    #[cfg(feature = "net")]
    #[test]
    fn session_restart_rebuilds_the_game_with_the_full_chr() {
        let Some(file) = rom_file("session_restart_rebuilds_the_game_with_the_full_chr") else {
            return;
        };
        let mut emu = WebEmu::new();
        emu.load_rom(&file).expect("ROM");
        let chr = emu.game.as_ref().expect("game").chr.clone();
        emu.step_frames(0, 3).expect("solo");
        emu.restart_for_session(z2_net::COOP_TWO_LINKS, &[])
            .expect("restart");
        let g = emu.game.as_ref().expect("game");
        assert_eq!(g.chr, chr, "restart changed the CHR image");
        assert_eq!(g.frame_count(), 0, "restart did not go back to power-on");
        assert!(emu.coop, "restart dropped the session's co-op flag");
    }

    #[test]
    fn pack_input_matches_shared_bit_contract() {
        assert_eq!(
            pack_input(true, false, false, false, false, false, false, false),
            game::BTN_A
        );
        assert_eq!(
            pack_input(false, false, false, false, false, false, false, true),
            game::BTN_RIGHT
        );
        assert_eq!(
            pack_input(true, true, true, true, true, true, true, true),
            0xFF
        );
        assert_eq!(
            pack_input(false, false, false, false, false, false, false, false),
            0
        );
        assert_eq!(game::INPUT_A, 0);
        assert_eq!(game::INPUT_RIGHT, 7);
    }

    #[test]
    fn load_rom_rejects_junk_without_panicking() {
        let mut emu = WebEmu::new();
        assert!(!emu.rom_loaded());
        assert!(emu.load_rom(b"not a rom").is_err());
        assert!(emu.load_rom(&vec![0u8; 262144]).is_err());
        assert!(!emu.rom_loaded());
        assert!(emu.facts().is_err());
    }

    #[test]
    fn step_and_facts_run_without_a_browser() {
        let mut emu = booted();
        assert!(emu.facts().is_ok());
        let f0 = emu.frame_count_u64();
        emu.step_frame(game::BTN_A).unwrap();
        emu.step_frames(0, 10).unwrap();
        assert_eq!(emu.frame_count_u64(), f0 + 11);
        assert_eq!(emu.last_input(), 0);
        // facts() stays valid JSON after stepping.
        let facts: serde_json::Value =
            serde_json::from_str(&emu.facts().unwrap()).expect("facts is JSON");
        assert!(facts.get("link").is_some());
        assert!(facts.get("enemies").is_some());
    }

    #[test]
    fn step_cap_bounds_runaway_calls() {
        let mut emu = booted();
        let stepped = emu.step_frames(0, u32::MAX).unwrap();
        assert_eq!(stepped, MAX_STEPS_PER_CALL);
    }

    #[test]
    fn render_frame_expands_indexed_to_opaque_rgba() {
        let mut emu = booted();
        emu.render_frame().unwrap();
        assert_eq!(emu.frame_len(), FRAME_RGBA_LEN);
        assert_eq!((emu.frame_width(), emu.frame_height()), (FRAME_W, FRAME_H));
        assert!(!emu.frame_ptr().is_null());
        let rgba = emu.frame_rgba();
        assert_eq!(rgba.len(), FRAME_RGBA_LEN);
        for px in rgba.chunks_exact(4) {
            assert_eq!(px[3], 0xFF);
        }
        // Indexed 0 maps through the shared palette fn (read-only reuse).
        assert_eq!(&rgba[..4], &indexed_to_rgba(0));
    }

    #[test]
    fn snapshot_roundtrip_restores_memory_images() {
        let mut emu = booted();
        emu.step_frames(game::BTN_START, 3).unwrap();
        let blob = emu.snapshot().unwrap();
        assert_eq!(&blob[..8], snapshot::WEB_MAGIC);
        // Poke RAM, restore, compare.
        emu.game.as_mut().unwrap().ram[0x42] ^= 0xFF;
        let status: serde_json::Value =
            serde_json::from_str(&emu.restore(&blob).unwrap()).expect("status JSON");
        assert_eq!(status["format"], "z2web01");
        let blob2 = emu.snapshot().unwrap();
        assert_eq!(blob, blob2);
        emu.restore(b"junk").unwrap_err();
    }

    #[test]
    fn sram_export_import_is_exact_8kib() {
        let mut emu = booted();
        emu.game.as_mut().unwrap().wram[0x123] = 0x5A;
        let sram = emu.sram_bytes().unwrap();
        assert_eq!(sram.len(), WRAM_SIZE);
        emu.game.as_mut().unwrap().wram[0x123] = 0x00;
        emu.load_sram(&sram).unwrap();
        assert_eq!(emu.game.as_ref().unwrap().wram[0x123], 0x5A);
        emu.load_sram(&[0u8; 100]).unwrap_err();
    }

    #[test]
    fn movie_arm_and_step_consume_track_then_idle() {
        let mut emu = booted();
        let report = emu.load_movie("|0|.......A|\n|0|...U....|\n").unwrap();
        let report: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["frames"], 2);
        assert_eq!(emu.movie_len(), 2);
        emu.step_movie_or_idle(5).unwrap();
        assert_eq!(emu.movie_cursor(), 2);
        assert_eq!(emu.frame_count_u64(), 5);
        emu.movie_rewind();
        assert_eq!(emu.movie_cursor(), 0);
        emu.load_movie("not a movie at all").unwrap(); // header-only: 0 frames
        assert_eq!(emu.movie_len(), 0);
    }

    #[test]
    fn state_and_screenshot_are_well_formed() {
        let mut emu = booted();
        emu.step_frame(0).unwrap();
        let state: serde_json::Value = serde_json::from_str(&emu.state()).unwrap();
        // Synthetic boot bypasses the hash gate: live game, but no ROM report.
        assert_eq!(state["romLoaded"], true);
        assert_eq!(state["crc32"], "");
        assert_eq!(state["sections"], 0);
        assert_eq!(state["frame"], 1);
        let shot = emu.screenshot().unwrap();
        assert_eq!(shot.len(), FRAME_RGBA_LEN);
    }

    #[test]
    fn audio_rate_follows_the_context_and_paces_production() {
        let mut emu = booted();
        assert_eq!(
            emu.audio_rate(),
            44_100,
            "44.1 kHz until the page says otherwise"
        );
        // A 48 kHz AudioContext: the synth must render 48000 samples per
        // wall-clock second too, or the worklet queue drifts by 3900
        // samples/s and the delay grows about a second every 11 seconds.
        assert_eq!(emu.set_audio_rate(48_000), 48_000);
        emu.take_audio_f32(); // the switch drops samples rendered at the old rate
        emu.step_frames(0, 60).unwrap();
        let queued = emu.audio_queued();
        // 60 frames at 60.0988 Hz is 0.998 s, so 48000 Hz gives ~47921
        // samples (and 44100 would give only ~44027 — well outside this).
        assert!((47_800..=48_050).contains(&queued), "queued={queued}");
        // A rate the synth cannot render falls back and reports the fallback
        // so the page can rebuild its context to match.
        assert_eq!(emu.set_audio_rate(22_050), 44_100);
        assert_eq!(emu.audio_rate(), 44_100);
    }

    #[test]
    fn audio_pipeline_produces_frame_rate_samples() {
        let mut emu = booted();
        emu.step_frames(0, 4).unwrap();
        // 44100 Hz / 60.0988 Hz ≈ 733.9 samples per frame.
        let queued = emu.audio_queued();
        assert!((4 * 700..=4 * 800).contains(&queued), "queued={queued}");
        let floats = emu.take_audio_f32();
        assert_eq!(floats.len(), queued);
        assert_eq!(emu.audio_queued(), 0);
        assert!(floats.iter().all(|s| s.is_finite()));
    }

    /// Peak-to-peak swing of a PCM block (a DC offset alone reads as 0).
    fn swing(pcm: &[f32]) -> f32 {
        let hi = pcm.iter().copied().fold(f32::MIN, f32::max);
        let lo = pcm.iter().copied().fold(f32::MAX, f32::min);
        hi - lo
    }

    /// The game's APU register writes are what the page hears: a frame's
    /// write log is drained into the synth, and a tone comes out.
    #[test]
    fn apu_register_writes_reach_the_pcm_queue() {
        use z2_core::cpu::ApuBus;
        let mut emu = booted();
        emu.step_frames(0, 2).unwrap();
        assert!(swing(&emu.take_audio_f32()) < 1e-3, "idle game is silent");

        // Pulse 1: enable, 50% duty at constant volume 15, ~440 Hz.
        let game = emu.game.as_mut().unwrap();
        for (addr, val) in [
            (0x4015, 0x01),
            (0x4000, 0xBF),
            (0x4002, 0xFD),
            (0x4003, 0x00),
        ] {
            game.apu.apu_write(addr, val);
        }
        emu.step_frames(0, 4).unwrap();
        assert!(
            emu.game.as_ref().unwrap().apu.log.is_empty(),
            "APU log not drained"
        );
        let pcm = emu.take_audio_f32();
        assert!(swing(&pcm) > 0.05, "tone swing {}", swing(&pcm));
    }

    /// With the real ROM the title theme is audible within ten seconds of
    /// power-on. Self-skips without `Z2_ROM`.
    #[test]
    fn real_rom_title_music_is_audible() {
        let Some(file) = rom_file("real_rom_title_music_is_audible") else {
            return;
        };
        let mut emu = WebEmu::new();
        emu.load_rom(&file).expect("ROM loads");
        let mut loudest = 0.0f32;
        for _ in 0..10 {
            emu.step_frames(0, 60).unwrap();
            loudest = loudest.max(swing(&emu.take_audio_f32()));
        }
        assert!(loudest > 0.05, "title music swing {loudest}");
    }

    #[test]
    fn widescreen_resizes_the_reported_frame_and_buffer() {
        let mut emu = booted();
        assert_eq!((emu.frame_width(), emu.frame_height()), (FRAME_W, FRAME_H));
        assert_eq!(emu.widescreen_tiles(), 0);

        emu.set_widescreen_preset("16:9").expect("16:9 accepted");
        assert_eq!(emu.widescreen_tiles(), 11);
        assert_eq!(emu.frame_width(), 432, "256 + 16*11");
        assert_eq!(emu.frame_height(), 240, "height never changes");
        emu.render_frame().expect("renders wide");
        assert_eq!(emu.frame_len(), 432 * 240 * 4);
        assert_eq!(emu.frame_rgba().len(), 432 * 240 * 4);

        emu.set_widescreen_preset("16:10").expect("16:10 accepted");
        assert_eq!((emu.frame_width(), emu.widescreen_tiles()), (384, 8));

        // Off restores the original geometry exactly, so the QA contract
        // (frame_len == 256*240*4) holds again.
        emu.set_widescreen_preset("off").expect("off accepted");
        emu.render_frame().expect("renders narrow");
        assert_eq!((emu.frame_width(), emu.frame_height()), (FRAME_W, FRAME_H));
        assert_eq!(emu.frame_len(), FRAME_RGBA_LEN);
    }

    #[test]
    fn widescreen_rejects_bad_presets_without_changing_state() {
        let mut emu = booted();
        emu.set_widescreen_preset("16:9").unwrap();
        for bad in ["21:9", "17", "", "garbage"] {
            assert!(
                emu.set_widescreen_preset(bad).is_err(),
                "must reject '{bad}'"
            );
        }
        assert!(emu.set_widescreen(17).is_err(), "17 tiles is out of range");
        assert_eq!(
            emu.widescreen_tiles(),
            11,
            "a rejected preset changes nothing"
        );
    }

    #[test]
    fn coop_toggle_survives_before_and_after_a_rom_exists() {
        let mut emu = WebEmu::new();
        // Before a ROM: remembered rather than refused, so the checkbox works.
        emu.coop_enable(true).expect("remembered");
        assert!(emu.coop_enabled());
        assert!(emu.coop_status().is_err(), "no ROM yet");

        let mut emu = booted();
        emu.coop_enable(true).unwrap();
        assert!(emu.coop_enabled());
        let status = emu.coop_status().expect("status available");
        assert!(status.starts_with('{'), "co-op status is JSON: {status}");
        assert!(status.contains("\"active\""), "{status}");
        // Two-pad stepping advances the frame counter like the one-pad path.
        let f0 = emu.frame_count_u64();
        emu.step_frames2(game::BTN_A, game::BTN_B, 3).unwrap();
        assert_eq!(emu.frame_count_u64(), f0 + 3);
        assert_eq!(emu.last_input(), game::BTN_A, "last_input tracks player 1");
        assert_eq!(emu.coop_hash_hex().unwrap().len(), 16);

        emu.coop_enable(false).unwrap();
        assert_eq!(emu.coop_status().unwrap(), "null", "off reports null");
    }

    /// The wide-gameplay half of the cross-crate netplay pin (see below).
    #[test]
    fn session_trapset_id_matches_the_cross_crate_pin() {
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, Some(11)),
            WIDE_TRAPSET_PIN_VALUE,
            "session_trapset_id changed: update z2-native's WIDE_TRAPSET_PIN_VALUE in lockstep"
        );
        assert_eq!(
            session_trapset_id(TRAPSET_PIN_VALUE, None),
            TRAPSET_PIN_VALUE
        );
    }

    #[test]
    fn wide_gameplay_defaults_on_and_follows_widescreen() {
        let mut emu = WebEmu::new();
        assert!(emu.wide_gameplay(), "the page defaults it on");
        assert_eq!(emu.wide_gameplay_tiles(), 0, "inactive without widescreen");
        emu.set_widescreen(11).unwrap();
        assert_eq!(emu.wide_gameplay_tiles(), 11);
        emu.set_wide_gameplay(false).unwrap();
        assert_eq!(emu.wide_gameplay_tiles(), 0);
        emu.set_wide_gameplay(true).unwrap();
        emu.set_widescreen(8).unwrap();
        assert_eq!(emu.wide_gameplay_tiles(), 8);
    }

    /// The cross-crate netplay pin, asserted here as well as in `z2-native`:
    /// the two implementations must agree or a native host and a web guest
    /// reject each other's handshake.
    #[test]
    fn trapset_id_matches_the_cross_crate_pin() {
        assert_eq!(
            fold_trapset(&[(0x00C2, "boot"), (0xFF9D, "bank7")]),
            TRAPSET_PIN_VALUE,
            "trapset_id changed: update z2-native's TRAPSET_PIN_VALUE in lockstep"
        );
        // Sorted by address, so input order cannot matter.
        let mut entries = [(0xFF9D_u16, "bank7"), (0x00C2, "boot")];
        entries.sort_unstable();
        assert_eq!(fold_trapset(&entries), TRAPSET_PIN_VALUE);
        assert_ne!(fold_trapset(&[(0x00C2, "boot")]), TRAPSET_PIN_VALUE);
    }

    #[test]
    fn netplay_surface_is_inert_but_present_without_the_feature() {
        // JS calls these unconditionally and asks `net_supported()` first, so
        // they must never panic in a bundle built without the feature.
        let mut emu = booted();
        let st: serde_json::Value = serde_json::from_str(&emu.net_state()).expect("state is JSON");
        assert_eq!(st["active"], false);
        assert_eq!(st["supported"], emu.net_supported());
        // `hashesOk` is what `site/netplay-e2e.mjs` reads to prove the two
        // peers actually compared state hashes; it must be present (0) in
        // every build so the status JSON has one shape.
        assert_eq!(st["hashesOk"], 0, "hashesOk is part of the status shape");
        assert!(st["link"].is_null(), "no session, no link stage");
        assert_eq!(emu.net_step(0, 5).unwrap(), 0, "no session steps nothing");
        let polled: serde_json::Value =
            serde_json::from_str(&emu.net_poll(16).unwrap()).expect("poll is JSON");
        assert_eq!(polled["active"], false);
        emu.net_disconnect(); // no-op, must not panic

        // The lockstep hash exists exactly when the protocol does.
        if cfg!(feature = "net") {
            let h = emu
                .net_state_hash_hex()
                .expect("a booted `net` build hashes");
            assert_eq!(h.len(), 16, "16 hex chars: {h}");
            assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "hex only: {h}");
        } else {
            assert!(
                emu.net_state_hash_hex().is_err(),
                "no protocol means no lockstep hash to report"
            );
        }
        if !emu.net_supported() {
            assert!(
                emu.net_connect("ws://127.0.0.1:3536", "room", true, 2, "")
                    .is_err(),
                "a build without the transport must refuse, not pretend"
            );
        }
    }

    /// `state()` is the stable QA contract: widescreen and co-op must not
    /// change its shape (they are reported through their own accessors).
    #[test]
    fn state_json_shape_is_unchanged_by_the_new_features() {
        let mut emu = booted();
        let before: serde_json::Value = serde_json::from_str(&emu.state()).unwrap();
        emu.set_widescreen_preset("16:9").unwrap();
        emu.coop_enable(true).unwrap();
        let after: serde_json::Value = serde_json::from_str(&emu.state()).unwrap();
        let mut ka: Vec<_> = before.as_object().unwrap().keys().cloned().collect();
        let mut kb: Vec<_> = after.as_object().unwrap().keys().cloned().collect();
        ka.sort();
        kb.sort();
        assert_eq!(ka, kb, "state() keys are part of the QA contract");
    }

    #[test]
    fn fill_right_clip_defaults_on_and_reaches_the_margins() {
        let mut emu = WebEmu::new();
        assert!(emu.fill_right_clip(), "the page paints the masked columns");
        emu.set_widescreen_preset("16:9").unwrap();
        assert!(emu.margins.as_ref().unwrap().fill_right_clip);
        emu.set_fill_right_clip(false).unwrap();
        assert!(!emu.fill_right_clip());
        assert!(!emu.margins.as_ref().unwrap().fill_right_clip);
        // …and the flag survives a preset change, like its left-edge twin.
        emu.set_widescreen_preset("16:10").unwrap();
        assert!(!emu.margins.as_ref().unwrap().fill_right_clip);
        emu.set_fill_right_clip(true).unwrap();
        assert!(emu.margins.as_ref().unwrap().fill_right_clip);
    }

    #[test]
    fn fill_left_clip_defaults_on_and_reaches_the_margins() {
        let mut emu = booted();
        assert!(emu.fill_left_clip(), "the page paints the clipped columns");
        emu.set_widescreen_preset("16:10").unwrap();
        assert!(emu.margins.as_ref().unwrap().fill_left_clip);
        emu.set_fill_left_clip(false).unwrap();
        assert!(!emu.fill_left_clip());
        assert!(!emu.margins.as_ref().unwrap().fill_left_clip);
        // Order the other way round: the flag must survive a later resize.
        emu.set_fill_left_clip(true).unwrap();
        emu.set_widescreen_preset("16:9").unwrap();
        assert!(emu.margins.as_ref().unwrap().fill_left_clip);
    }

    /// Margin sprites: on by default, registered only while widescreen is
    /// on, and never part of the netplay trap-set identity.
    #[test]
    fn margin_sprites_follow_widescreen_and_stay_out_of_the_trapset() {
        let mut emu = booted();
        assert!(
            emu.margin_sprites(),
            "the page draws objects in the margins"
        );
        let game = |emu: &WebEmu| emu.game.as_ref().unwrap().margin_sprites_enabled();
        let id_off = trapset_id(emu.game.as_ref().unwrap());
        assert!(!game(&emu), "no observer without widescreen");
        emu.set_widescreen_preset("16:9").unwrap();
        assert!(game(&emu));
        assert!(emu.margins.as_ref().unwrap().fill_left_sprites);
        assert_eq!(
            trapset_id(emu.game.as_ref().unwrap()),
            id_off,
            "display-only observer must not split netplay peers"
        );
        emu.set_margin_sprites(false).unwrap();
        assert!(!game(&emu));
        assert!(!emu.margins.as_ref().unwrap().fill_left_sprites);
        emu.set_margin_sprites(true).unwrap();
        emu.set_widescreen_preset("off").unwrap();
        assert!(!game(&emu));
    }

    /// The canvas is sized from these four numbers, so they must stay
    /// consistent: `frame_*` is the pixel buffer, `logical_*` the NES-pixel
    /// geometry, and `output_scale` the ratio between them.
    #[test]
    fn reported_sizes_agree_with_the_rgba_buffer() {
        let mut emu = booted();
        for preset in ["off", "16:10", "16:9", "4"] {
            emu.set_widescreen_preset(preset).unwrap();
            emu.render_frame().unwrap();
            let (w, h, s) = (emu.frame_width(), emu.frame_height(), emu.output_scale());
            assert_eq!(emu.frame_len(), w * h * 4, "{preset}: buffer matches size");
            assert_eq!(emu.frame_rgba().len(), w * h * 4);
            assert_eq!(w, emu.logical_width() * s as usize, "{preset}: width/scale");
            assert_eq!(
                h,
                emu.logical_height() * s as usize,
                "{preset}: height/scale"
            );
            assert_eq!(emu.logical_height(), FRAME_H);
        }
    }

    /// Without the `hd` feature the pack surface must still answer (JS calls it
    /// unconditionally after asking `hd_supported()`), and scale 1 must be
    /// accepted so the selector's default never throws.
    #[test]
    fn hd_surface_answers_in_every_build() {
        let mut emu = booted();
        assert_eq!(emu.hd_supported(), cfg!(feature = "hd"));
        emu.hd_pack_begin().unwrap();
        emu.set_output_scale(1).expect("scale 1 always works");
        assert_eq!(emu.requested_scale(), 1);
        assert_eq!(emu.output_scale(), 1, "no pack, no upscale");
        emu.hd_pack_clear().unwrap();
        if !emu.hd_supported() {
            assert_eq!(emu.hd_pack_info(), "null");
            assert!(emu.hd_pack_add_file("pack.json", b"{}").is_err());
            assert!(emu.hd_pack_commit().is_err());
            assert!(emu.set_output_scale(2).is_err());
        }
    }

    /// A pack built in memory, handed over exactly the way the directory
    /// picker does: `pack.json` + one PNG per CHR page, with the picker's
    /// folder prefix still attached.
    #[cfg(feature = "hd")]
    fn synthetic_pack_files(scale: u32, prefix: &str) -> Vec<(String, Vec<u8>)> {
        // "page" layout: 16x16 cells of `8 * scale` pixels each.
        let dim = 16 * 8 * scale;
        let mut rgba = vec![0u8; (dim * dim * 4) as usize];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, 0x40, 0x80, 0xFF]);
        }
        let png = z2_render::encode_png_rgba(dim, dim, &rgba, &[]).expect("encodes");
        let manifest = format!(
            "{{\"format\":\"z2rs-hdpack\",\"version\":1,\"name\":\"unit test pack\",\
              \"author\":\"z2-web tests\",\"scale\":{scale},\
              \"sheets\":[{{\"file\":\"sheets/page00.png\",\"layout\":\"page\",\"page\":0}}]}}"
        );
        vec![
            (format!("{prefix}pack.json"), manifest.into_bytes()),
            (format!("{prefix}sheets/page00.png"), png),
        ]
    }

    #[cfg(feature = "hd")]
    fn stage(emu: &mut WebEmu, files: &[(String, Vec<u8>)]) {
        emu.hd_pack_begin().unwrap();
        for (name, bytes) in files {
            emu.hd_pack_add_file(name, bytes).expect("stages");
        }
    }

    /// Pack loading through the one browser code path: raw bytes from a
    /// `<input webkitdirectory>` pick, decoded in Rust.
    #[cfg(feature = "hd")]
    #[test]
    fn hd_pack_loads_from_picked_files_and_resizes_the_output() {
        let mut emu = booted();
        assert!(emu.hd_supported());
        assert_eq!(emu.hd_pack_info(), "null");

        // The picker prefixes every path with the chosen folder's name; the
        // loader finds the shallowest pack.json and strips that prefix.
        stage(&mut emu, &synthetic_pack_files(2, "my-pack/"));
        let info: serde_json::Value =
            serde_json::from_str(&emu.hd_pack_commit().expect("pack loads")).expect("info JSON");
        assert_eq!(info["name"], "unit test pack");
        assert_eq!(info["author"], "z2-web tests");
        assert_eq!(info["scale"], 2);
        assert_eq!(info["sheets"], 1);
        let cached: serde_json::Value =
            serde_json::from_str(&emu.hd_pack_info()).expect("cached info JSON");
        assert_eq!(cached, info, "hd_pack_info() replays the commit report");
        assert_eq!(info["cellSize"], 16, "8 px tile at 2x");
        assert_eq!(info["tiles"], 256, "one full CHR page of cells");

        // A 2x pack doubles the pixel buffer but not the NES geometry.
        assert_eq!(emu.output_scale(), 2);
        assert_eq!((emu.frame_width(), emu.frame_height()), (512, 480));
        assert_eq!((emu.logical_width(), emu.logical_height()), (256, 240));
        emu.render_frame().expect("presents through the pack");
        assert_eq!(emu.frame_len(), 512 * 480 * 4);

        // Widescreen composes on top of the pack.
        emu.set_widescreen_preset("16:9").unwrap();
        assert_eq!(emu.frame_width(), 432 * 2);
        emu.render_frame().expect("presents wide + HD");
        assert_eq!(emu.frame_len(), 432 * 2 * 480 * 4);

        // Clearing the pack returns to the original art at scale 1.
        emu.hd_pack_clear().unwrap();
        assert_eq!(emu.hd_pack_info(), "null");
        assert_eq!(emu.output_scale(), 1);
        assert_eq!(emu.frame_width(), 432);
        emu.render_frame().expect("renders without the pack");
        assert_eq!(emu.frame_len(), 432 * 240 * 4);
    }

    /// A bad pack must be reported and change nothing: the tab keeps playing
    /// with the original art.
    #[cfg(feature = "hd")]
    #[test]
    fn hd_pack_failures_keep_the_previous_presentation() {
        let mut emu = booted();
        assert!(
            emu.hd_pack_commit().is_err(),
            "committing nothing is an error, not an empty pack"
        );

        // No manifest among the picked files.
        emu.hd_pack_begin().unwrap();
        emu.hd_pack_add_file("notes.txt", b"hello").unwrap();
        let err = emu.hd_pack_commit().expect_err("no pack.json");
        assert!(err.contains("pack.json"), "{err}");
        assert_eq!(emu.hd_pack_info(), "null");
        assert_eq!(emu.frame_width(), 256, "a rejected pack changes nothing");

        // Malformed manifest.
        emu.hd_pack_begin().unwrap();
        emu.hd_pack_add_file("pack.json", b"{ not json").unwrap();
        assert!(emu.hd_pack_commit().is_err());

        // Load a good pack, then fail: the good one must survive.
        stage(&mut emu, &synthetic_pack_files(2, ""));
        emu.hd_pack_commit().expect("good pack");
        assert_eq!(emu.output_scale(), 2);
        emu.hd_pack_begin().unwrap();
        emu.hd_pack_add_file("pack.json", b"{}").unwrap();
        assert!(emu.hd_pack_commit().is_err());
        assert_eq!(emu.output_scale(), 2, "the loaded pack is still in force");
        emu.render_frame().expect("still renders");

        // Oversized single file and oversized folder are refused up front.
        emu.hd_pack_begin().unwrap();
        assert!(emu
            .hd_pack_add_file("big.png", &vec![0u8; MAX_HD_FILE_BYTES + 1])
            .is_err());
    }

    /// Scale selection: the range is the page's, and a pack whose own scale
    /// does not divide the request wins (`Presenter::set_pack`).
    #[cfg(feature = "hd")]
    #[test]
    fn output_scale_range_and_pack_override() {
        let mut emu = booted();
        emu.set_output_scale(2).unwrap();
        assert_eq!((emu.requested_scale(), emu.output_scale()), (2, 2));
        assert_eq!(emu.frame_width(), 512, "upscaling with no pack at all");
        emu.render_frame().expect("renders upscaled");
        assert_eq!(emu.frame_len(), 512 * 480 * 4);

        assert!(emu.set_output_scale(0).is_err());
        assert!(emu.set_output_scale(MAX_HD_SCALE + 1).is_err());
        assert_eq!(emu.output_scale(), 2, "a rejected scale changes nothing");

        // Requested 2 with a 3x pack: 2 does not divide 3, so the pack wins.
        stage(&mut emu, &synthetic_pack_files(3, ""));
        emu.hd_pack_commit().expect("3x pack loads");
        assert_eq!(emu.requested_scale(), 2, "the user's pick is kept");
        assert_eq!(emu.output_scale(), 3, "2 does not divide 3: the pack wins");
        assert_eq!(emu.frame_width(), 256 * 3);

        // Asking for 1 with a 3x pack is legal: 1 divides 3, so the HD cells
        // are point-sampled back down to NES resolution.
        emu.set_output_scale(1).unwrap();
        assert_eq!(emu.output_scale(), 1);
        assert_eq!(emu.frame_width(), 256);
    }

    /// Session gating: without a ROM (no CRC, no trap-set identity) a session
    /// must be refused, and the status JSON must stay well formed throughout.
    #[test]
    fn netplay_session_is_gated_on_a_loaded_rom() {
        let mut emu = WebEmu::new();
        let err = emu
            .net_connect("ws://127.0.0.1:3536", "room", true, 2, "")
            .expect_err("no ROM, no session");
        // With `net` the ROM gate refuses; without it the whole feature does.
        // Either way the message names the reason rather than throwing raw.
        let expected = if cfg!(feature = "net") {
            "no ROM"
        } else {
            "netplay support"
        };
        assert!(err.contains(expected), "want {expected:?}, got {err:?}");
        let st: serde_json::Value = serde_json::from_str(&emu.net_state()).expect("JSON");
        assert_eq!(st["active"], false);
        assert_eq!(st["state"], "idle");
    }

    /// Room and delay validation happens before any socket is opened, so a
    /// typo is a message rather than a silent failure. Needs `net` for the
    /// `z2_net` validators; the default build's refusal is covered above.
    #[cfg(feature = "net")]
    #[test]
    fn netplay_rejects_bad_rooms_and_signal_urls_before_connecting() {
        let mut emu = booted();
        emu.rom_body = vec![0u8; 16]; // non-empty: the ROM check is not what we test
        for (signal, room) in [
            ("http://127.0.0.1:3536", "room"),    // not a ws/wss URL
            ("ws://127.0.0.1:3536/path", "room"), // path is the room, not the base
            ("ws://127.0.0.1:3536", ""),          // empty room
            ("ws://127.0.0.1:3536", "bad room"),  // space is not allowed
        ] {
            assert!(
                emu.net_connect(signal, room, true, 2, "").is_err(),
                "must reject signal={signal:?} room={room:?}"
            );
        }
        // A well-formed request still cannot open a transport without the
        // `netplay` feature on wasm32, so either outcome is a clean result —
        // never a panic.
        assert!(
            emu.net_connect("ws://127.0.0.1:3536", "ok-room", true, 2, "http://x")
                .expect_err("bad ICE text is refused")
                .contains("ICE"),
            "the ICE setting is validated before connecting"
        );
        let _ = emu.net_connect("ws://127.0.0.1:3536", "ok-room", true, 2, "");
        let st: serde_json::Value = serde_json::from_str(&emu.net_state()).expect("JSON");
        assert!(st["supported"].is_boolean());
    }

    /// The mode selector and the test hook round-trip in every build.
    #[test]
    fn netplay_mode_defaults_to_rollback_and_round_trips() {
        let mut emu = WebEmu::new();
        let m: serde_json::Value = serde_json::from_str(&emu.net_mode()).expect("JSON");
        assert_eq!(m["mode"], "rollback");
        assert_eq!(m["maxPrediction"], u64::from(WEB_DEFAULT_MAX_PREDICTION));
        emu.net_set_mode("lockstep", 99).expect("lockstep");
        let m: serde_json::Value = serde_json::from_str(&emu.net_mode()).expect("JSON");
        assert_eq!(m["mode"], "lockstep");
        assert_eq!(m["maxPrediction"], u64::from(WEB_MAX_PREDICTION));
        assert!(emu.net_set_mode("turbo", 8).is_err());
        let st: serde_json::Value = serde_json::from_str(&emu.net_state()).expect("JSON");
        assert!(st["mode"].is_string());
        assert_eq!(emu.net_confirmed_hashes(), "[]");
        let _ = emu.net_simulate(10, 5, 1);
        let off: serde_json::Value =
            serde_json::from_str(&emu.net_simulate(0, 0, 0)).expect("JSON");
        assert_eq!(off["latencyMs"], 0);
    }

    /// Two real games in rollback mode over a lossy, late loopback link, with
    /// a network poll between every tick (the page's rAF order). Before the
    /// session starts both games have run a different number of solo frames,
    /// so if a tick ever advanced before `Running` rebuilt the game from
    /// power-on, the confirmed checksums would differ. Self-skips without
    /// `Z2_ROM`.
    #[cfg(feature = "net")]
    #[test]
    fn rollback_over_lossy_loopback_agrees_on_confirmed_state() {
        let Some(file) = std::env::var("Z2_ROM")
            .ok()
            .filter(|p| std::path::Path::new(p).is_file())
            .map(|p| std::fs::read(p).expect("read Z2_ROM"))
        else {
            eprintln!(
                "skipping rollback_over_lossy_loopback_agrees_on_confirmed_state: Z2_ROM not set"
            );
            return;
        };
        let cfg = z2_net::LoopbackConfig {
            latency_ticks: 3,
            jitter_ticks: 2,
            loss_per_mille: 80,
            connect_after_ticks: 5,
        };
        let (link, ta, tb) = z2_net::loopback_pair(cfg, 7);
        let mut a = WebEmu::new();
        let mut b = WebEmu::new();
        for emu in [&mut a, &mut b] {
            emu.load_rom(&file).expect("ROM");
        }
        // Different solo histories before the session.
        a.step_frames(0, 5).expect("solo");
        b.step_frames(0x08, 37).expect("solo");
        a.net_open(Box::new(ta), true, 2).expect("host");
        b.net_open(Box::new(tb), false, 2).expect("guest");
        const FRAMES: u32 = 240;
        let mut ticks = 0u32;
        let session_frame = |emu: &WebEmu| -> u64 {
            let st: serde_json::Value = serde_json::from_str(&emu.net_state()).expect("JSON");
            st["frame"].as_u64().unwrap_or(0)
        };
        while session_frame(&a) < u64::from(FRAMES) || session_frame(&b) < u64::from(FRAMES) {
            ticks += 1;
            assert!(ticks < 5_000, "session did not reach frame {FRAMES}");
            link.advance(1);
            for (i, emu) in [&mut a, &mut b].into_iter().enumerate() {
                let st: serde_json::Value =
                    serde_json::from_str(&emu.net_poll(16).expect("poll")).expect("JSON");
                assert!(
                    st["state"] != "closed" && st["state"] != "desynced",
                    "session ended: {st}"
                );
                if st["started"] == true {
                    // Pads change every 10 ticks and differ per side, so the
                    // other peer's predictions keep failing.
                    let pad = if (ticks / 10 + i as u32).is_multiple_of(2) {
                        0x80
                    } else {
                        0x01
                    };
                    emu.net_step(pad, 1).expect("tick");
                } else {
                    // Not started: the solo game keeps running, differently per side.
                    emu.step_frames(0, 1 + i as u32 * 2).expect("solo");
                }
            }
        }
        // A few more ticks so the last pads and checksums are delivered.
        for _ in 0..40 {
            link.advance(1);
            for emu in [&mut a, &mut b] {
                emu.net_poll(16).expect("poll");
                emu.net_step(0, 1).expect("tick");
            }
        }
        let sa: serde_json::Value = serde_json::from_str(&a.net_state()).expect("JSON");
        let sb: serde_json::Value = serde_json::from_str(&b.net_state()).expect("JSON");
        for s in [&sa, &sb] {
            assert_eq!(s["mode"], "rollback", "{s}");
            assert_eq!(s["state"], "running", "{s}");
            assert_eq!(s["error"], serde_json::Value::Null, "{s}");
            assert!(s["rollbacks"].as_u64().unwrap() > 0, "no rollback: {s}");
            assert!(
                s["hashesOk"].as_u64().unwrap() > 0,
                "no checksum compared: {s}"
            );
        }
        let la: Vec<(u64, String)> = serde_json::from_str(&a.net_confirmed_hashes()).expect("JSON");
        let lb: Vec<(u64, String)> = serde_json::from_str(&b.net_confirmed_hashes()).expect("JSON");
        let frames_b: Vec<u64> = lb.iter().map(|x| x.0).collect();
        let shared = la.iter().filter(|x| frames_b.contains(&x.0)).count();
        let equal = la.iter().filter(|x| lb.contains(x)).count();
        assert!(shared >= 10, "only {shared} confirmed frames in common");
        assert_eq!(equal, shared, "confirmed hashes differ: {la:?} vs {lb:?}");
    }
}
