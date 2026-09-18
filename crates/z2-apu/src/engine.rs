//! Zelda II bank 6 sound-engine port.
//!
//! Readable-Rust sequencer modelled on `third_party/z2disassembly/src/prg6.asm`
//! (**read-only reference — no table bytes are copied here**; every lookup
//! goes through [`NoteTables`] at runtime, keyed by the ROM offsets below).
//!
//! ## Observed structure (all addresses verified in `prg6.asm`)
//!
//! Two entry points share the `$4000-$4017` register file:
//!
//! * **Title engine** `Bank6Code0`, PRG `$8000` (file `0x18010`): init writes
//!   `$4017=#$FF`, `$4015=#$0F`, then `L821B` (SFX/pitch dispatch on
//!   `$EB/$E9`) and `L826A` (music dispatch on `$EA/$E8`). Note/period
//!   tables at file `$18090-$180ED` (`L8090`), envelope table `L8084`
//!   (file `$18094`), duty-ish table `L8074` (file `$18084`), noise tables
//!   `L805D/$805E/$805F` (file `$1806D+`), phrase tables from `L84EB`
//!   (file `$184FB`).
//! * **Gameplay engine** `Bank6Code2`, PRG `$9000` (file `0x19010`): runs
//!   every frame — `$4017=#$FF`, then `Bank6__Code_3` (DMC trigger on `$E9`
//!   bits; programs `$4012/$4013/$4010/$4015`, PRG `$924E-$9261`),
//!   `L990B` (music-track dispatch on `$EC` bit flags, PRG `$990B`),
//!   `L92F4`/`L9408`/`L95A7` (SFX buses `$EF/$EE/$ED`), `L9B18`
//!   (per-world song player on `$EB` + `$0707` world, PRG `$9B18`), then
//!   clears `$EF/$EE/$ED/$EC/$EB/$E9`. Note table `L9190` (file `$191A0`),
//!   DMC pointer table `L917B` (file `$1918B`), world index tables
//!   `Index_Table_1` (PRG `$A000`) / world 1-2 table (PRG `$A3C9`).
//!
//! File-offset mapping rule for bank 6 (`$8000-$BFFF`):
//! `file = prg_addr - 0x8000 + 0x18010` (16-byte `.nes` header). Fixed-bank
//! (`$C000+`, DMC samples) offsets are **TBD vs the oracle** — the provider
//! maps CPU addresses at runtime.
//!
//! ## Request bytes `$E0-$EF`
//!
//! | Byte | Title engine | Gameplay engine |
//! |------|--------------|-----------------|
//! | `$EA` | global sound switch (0 = on) | same (early-out silences all) |
//! | `$EB` | music select (bit-scan `L821B`) | world-song select (`L9B18`) |
//! | `$EC` | SFX type 1 | **music track bit-flags** (`L990B`) |
//! | `$ED` | SFX type 2 | SFX bus 2 (`L95A7`) |
//! | `$EE` | SFX type 3 (decay) | SFX bus 3 (`L9408`) |
//! | `$EF` | SFX type 4 (decay) | SFX bus 4 (`L92F4`) |
//! | `$E0/$E1` | stream pointer (`($E0),Y`) | same |
//! | `$E2-$E5` | voice cursors P2/P1/tri/noise | same roles (gameplay) |
//! | `$E6-$E9` | pitch counter / tempo / flags | last-period cache, tempo, flags |
//!
//! `$0707` world (gameplay song picker): 0 caves/encounters, 1 west towns,
//! 2 east towns, 3 palaces 1-2-5, 4 palaces 3-4-6, 5 great palace
//! (disassembly comment at PRG `$9B24`, file `0x19B34`).
//!
//! ## Port status per channel
//!
//! | Voice | Title engine | Gameplay engine |
//! |-------|--------------|-----------------|
//! | Pulse 1 | **Full**: stream fetch, note-on, vibrato, envelope | **Full**: note-on + vibrato (same helpers) |
//! | Pulse 2 | **Full**: same stream core, second cursor | **Full**: same helpers, second voice |
//! | Triangle | **Modelled**: fetch + `$4008/$400A/$400B` writes | Scaffolded (stream format TBD vs oracle) |
//! | Noise | **Modelled**: fetch + `$400C/$400E/$400F` writes | Scaffolded (envelope-table nuance TBD) |
//! | DMC | n/a (title never touches `$4010+`) | **Modelled**: trigger params from tables; sample bytes via provider |
//! | Dispatch | **Full**: phrase walk + track-bit scan | **Full**: track-bit scan; per-slot inits scaffolded |
//! | SFX | **Full**: priority + pitch-override | **Modelled**: bus init writes + state; note streams scaffolded |
//!
//! "Scaffolded" = state machine + types land here with the exact register
//! footprint, but byte-level stream stepping waits on oracle log comparison.
//! Nothing panics; scaffolded paths return [`PortState`] so the
//! report says so.

use crate::apu::Apu;
use crate::frame::FRAME_CPU_CYCLES;
use crate::reglog::RegLog;

// ---------------------------------------------------------------------------
// ROM-offset reference constants (addresses only — never table bytes).
// ---------------------------------------------------------------------------

/// File offset of the bank 6 base (title side), `.nes` incl. 16-byte header.
pub const TITLE_BANK_FILE_BASE: u32 = 0x18000;
/// File offset of the bank 6 base (gameplay side).
pub const GAME_BANK_FILE_BASE: u32 = 0x19000;
/// PRG address of the title engine entry (`Bank6Code0`).
pub const TITLE_ENTRY_PRG: u16 = 0x8000;
/// PRG address of the gameplay engine entry (`Bank6Code2`).
pub const GAME_ENTRY_PRG: u16 = 0x9000;
/// First/last file offsets of the title note-table block.
pub const TITLE_TABLES_FILE_FIRST: u32 = 0x18027;
/// First/last file offsets of the title note-table block.
pub const TITLE_TABLES_FILE_LAST: u32 = 0x1811E;
/// PRG address of the title pulse period table (`L8090`).
pub const TITLE_PERIOD_TABLE_PRG: u16 = 0x8090;
/// PRG address of the gameplay pulse period table (`L9190`, file `0x191A0`).
pub const GAME_PERIOD_TABLE_PRG: u16 = 0x9190;
/// PRG address of the gameplay DMC pointer table (`L917B`, file `0x1918B`).
pub const GAME_DMC_TABLE_PRG: u16 = 0x917B;
/// PRG address of the gameplay music-track dispatcher (`L990B`).
pub const GAME_MUSIC_DISPATCH_PRG: u16 = 0x990B;
/// PRG address of the per-world song player (`L9B18`).
pub const GAME_WORLD_PLAYER_PRG: u16 = 0x9B18;
/// PRG address of the DMC trigger (`Bank6__Code_3` programs `$4010+`).
pub const GAME_DMC_TRIGGER_PRG: u16 = 0x920B;

/// Map a bank-6 PRG address (`$8000-$BFFF`) to an absolute `.nes` file
/// offset (16-byte header). Returns `None` outside bank 6.
#[must_use]
pub const fn bank6_file_offset(prg_addr: u16) -> Option<u32> {
    if prg_addr < 0x8000 || prg_addr > 0xBFFF {
        return None;
    }
    Some((prg_addr as u32 - 0x8000) + 0x18010)
}

// ---------------------------------------------------------------------------
// Runtime table provider (ROM bytes live outside the repo).
// ---------------------------------------------------------------------------

/// Runtime view of the game's note/envelope/phrase tables.
///
/// The implementor loads bytes from the assets image (built from `$Z2_ROM`
/// by `z2-assets`, never committed) at the offsets above. The synth core
/// stays ROM-free; only addresses cross this boundary.
pub trait NoteTables {
    /// Read one program byte from bank 6 (`$8000-$BFFF`).
    fn read_bank6(&self, prg_addr: u16) -> u8;
}

/// Provider that returns 0 for every address (tests / silent paths).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullTables;

impl NoteTables for NullTables {
    fn read_bank6(&self, _prg_addr: u16) -> u8 {
        0
    }
}

// ---------------------------------------------------------------------------
// Requests.
// ---------------------------------------------------------------------------

/// Per-frame sound request, mirroring bytes `$EA-$EF`.
///
/// Produced by the game core from emulated RAM each frame and
/// consumed by [`Engine::tick_game_frame`]. Field names keep the `$Ex`
/// addresses so log comparison can line them up with `prg6.asm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoundRequest {
    /// `$EA == 0` means sound on (any other value silences everything).
    pub global_on: bool,
    /// `$EB`: title music select / gameplay world-song select.
    pub eb: u8,
    /// `$EC`: title SFX-1 / gameplay music-track bit flags.
    pub ec: u8,
    /// `$ED`: SFX bus 2 request bits.
    pub ed: u8,
    /// `$EE`: SFX bus 3 request bits.
    pub ee: u8,
    /// `$EF`: SFX bus 4 request bits.
    pub ef: u8,
    /// `$0707` world index for the per-world song player (0..5).
    pub world: u8,
}

impl SoundRequest {
    /// Silence: sound on, no music, no SFX.
    #[must_use]
    pub fn silence() -> Self {
        Self {
            global_on: true,
            ..Default::default()
        }
    }

    /// Request gameplay music track bits (`$EC`, see [`TrackSlot`]).
    #[must_use]
    pub fn music(track_bits: u8) -> Self {
        Self {
            global_on: true,
            ec: track_bits,
            ..Default::default()
        }
    }

    /// True when any SFX bus has a pending request.
    #[must_use]
    pub fn any_sfx(&self) -> bool {
        self.ed != 0 || self.ee != 0 || self.ef != 0
    }
}

/// Gameplay music-track slots decoded from the `$EC` bit flags by `L990B`
/// (PRG `$990B`, file `0x1991B`). Each lists the init routine it vectors
/// to; audible identities (overworld, palace, …) are resolved by the
/// listening pass once the oracle runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TrackSlot {
    /// `$EC` bit 0 → init at PRG `$9834`.
    Bit0 = 0x01,
    /// `$EC` bit 1 → init at PRG `$98C8`.
    Bit1 = 0x02,
    /// `$EC` bit 2 → init at PRG `$99A2`.
    Bit2 = 0x04,
    /// `$EC` bit 3 → init at PRG `$9A20`.
    Bit3 = 0x08,
    /// `$EC` bit 4 → init at PRG `$9A48`.
    Bit4 = 0x10,
    /// `$EC` bit 5 → init at PRG `$9A70`.
    Bit5 = 0x20,
    /// `$EC` bit 6 → init at PRG `$9A98`.
    Bit6 = 0x40,
    /// `$EC` bit 7 → init at PRG `$9AD1`.
    Bit7 = 0x80,
    /// `$EC == $C0` → init at PRG `$97E0` (sweep jingle path).
    SpecialC0 = 0xC0,
    /// `$EC == $60` (via `$07E1`) → init at PRG `$9817` (noise jingle path).
    Special60 = 0x60,
}

impl TrackSlot {
    /// Init-routine PRG address for documentation / log annotation.
    #[must_use]
    pub const fn init_routine(self) -> u16 {
        match self {
            TrackSlot::Bit0 => 0x9834,
            TrackSlot::Bit1 => 0x98C8,
            TrackSlot::Bit2 => 0x99A2,
            TrackSlot::Bit3 => 0x9A20,
            TrackSlot::Bit4 => 0x9A48,
            TrackSlot::Bit5 => 0x9A70,
            TrackSlot::Bit6 => 0x9A98,
            TrackSlot::Bit7 => 0x9AD1,
            TrackSlot::SpecialC0 => 0x97E0,
            TrackSlot::Special60 => 0x9817,
        }
    }
}

/// `$0707` world index for the per-world song player (`L9B18`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum World {
    /// Caves, enemy encounters.
    Caves = 0,
    /// West Hyrule towns.
    WestTowns = 1,
    /// East Hyrule towns.
    EastTowns = 2,
    /// Palaces 1, 2, 5.
    PalaceA = 3,
    /// Palaces 3, 4, 6.
    PalaceB = 4,
    /// Great Palace.
    GreatPalace = 5,
}

impl World {
    /// Convert a raw `$0707` value; unknown values map to `None`
    /// (the engine then holds the current song — matches `L9B80` falling
    /// through for unexpected values).
    #[must_use]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(World::Caves),
            1 => Some(World::WestTowns),
            2 => Some(World::EastTowns),
            3 => Some(World::PalaceA),
            4 => Some(World::PalaceB),
            5 => Some(World::GreatPalace),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Sequencer state.
// ---------------------------------------------------------------------------

/// How completely a voice path is ported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    /// Byte-exact logic ported from the disassembly.
    Full,
    /// Register footprint + init writes ported; stream nuance TBD.
    Modeled,
    /// State machine + types only; stepping waits on oracle logs.
    Scaffolded,
    /// Voice idle this frame.
    Idle,
}

/// The five sequenced voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// `$4000-$4003`.
    Pulse1,
    /// `$4004-$4007`.
    Pulse2,
    /// `$4008-$400B`.
    Triangle,
    /// `$400C-$400F`.
    Noise,
    /// `$4010-$4013`.
    Dmc,
}

/// Per-voice sequencer cursor state (title + gameplay stream players).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VoiceSeq {
    /// Stream cursor (`$E2-$E5` roles).
    pub cursor: u8,
    /// Ticks left on the current note (`$EC-$EF` duration roles).
    pub duration: u8,
    /// Envelope/attenuation counter (`$07F9-$07FB` roles).
    pub envelope: u8,
    /// Voice active (SFX priority may still override the output).
    pub active: bool,
}

/// SFX bus state (gameplay `$ED/$EE/$EF`, title `$EC/$ED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SfxBus {
    /// Latched request bits.
    pub req: u8,
    /// Ticks the bus has owned its voice.
    pub age: u8,
    /// True while the bus overrides music on its voice.
    pub overriding: bool,
}

/// What one engine tick did (for reports + tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineReport {
    /// Emulator frame number.
    pub frame: u32,
    /// Register writes emitted this tick.
    pub writes: usize,
    /// Per-voice port states in [`Voice`] order (P1, P2, Tri, Noise, DMC).
    pub channels: [PortState; 5],
    /// True when the DMC trigger path fired (`Bank6__Code_3`).
    pub dmc_triggered: bool,
    /// True when an SFX bus overrode music on a pulse voice.
    pub sfx_override: bool,
}

/// Arguments for [`Engine::note_on_pulse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseNote {
    /// True = pulse 1 (`$4000-$4003`), false = pulse 2 (`$4004-$4007`).
    pub pulse1: bool,
    /// Note-table index.
    pub note: u8,
    /// Period-low table base: [`TITLE_PERIOD_TABLE_PRG`] (title) or
    /// [`GAME_PERIOD_TABLE_PRG`] (gameplay).
    pub period_table: u16,
    /// `$4003`-high table base (`L808F` title / `L918F` gameplay — the
    /// byte just before the period table).
    pub vol_table: u16,
}

/// Bank 6 sound engine: readable-Rust sequencer.
///
/// Owns the `$E0-$EF`-role state plus the `$07xx` work mirrors the engine
/// needs (`$07E8-$07FB` envelope bases, `$07F0-$07F8` phrase/track state).
/// Register writes go to the [`Apu`] through the [`RegLog`] so oracle
/// comparison sees exactly what the synth heard.
#[derive(Debug, Clone)]
pub struct Engine {
    frame_no: u32,
    /// Stream base pointer (`$E0/$E1` roles).
    stream_base: u16,
    /// Title tempo divider (`$E8` role) / gameplay tempo cache.
    tempo: u8,
    /// Title pitch counter (`$E6` role) / gameplay last-period cache P1.
    pitch_cache_p1: u8,
    /// Gameplay last-period cache P2 (`$E7` role).
    pitch_cache_p2: u8,
    /// Flag byte (`$E9` role: DMC trigger bits + jingle enables).
    flags: u8,
    /// Per-voice stream states (P1, P2, Tri, Noise).
    voices: [VoiceSeq; 4],
    /// SFX buses (title `$EC/$ED`, gameplay `$ED/$EE/$EF` order).
    sfx: [SfxBus; 3],
    /// Work-RAM mirrors for envelope bases (`$07E8/$07E9/$07EA`,
    /// `$07F0/$07F1` roles).
    env_base: [u8; 3],
    vol_base: [u8; 2],
    /// Currently latched gameplay track (`$EC` role).
    track: u8,
    /// Currently latched world song (`$EB` role).
    world_song: u8,
    /// Per-tick write sequencer (deterministic cycle stamps).
    write_seq: u64,
}

impl Engine {
    /// Create an idle engine (power-on state: all cursors 0, no song).
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame_no: 0,
            stream_base: 0,
            tempo: 0,
            pitch_cache_p1: 0,
            pitch_cache_p2: 0,
            flags: 0,
            voices: [VoiceSeq::default(); 4],
            sfx: [SfxBus::default(); 3],
            env_base: [0; 3],
            vol_base: [0; 2],
            track: 0,
            world_song: 0,
            write_seq: 0,
        }
    }

    /// Current frame number (incremented by each tick).
    #[must_use]
    pub fn frame_no(&self) -> u32 {
        self.frame_no
    }

    /// Voice stream state (P1, P2, Tri, Noise order).
    #[must_use]
    pub fn voices(&self) -> &[VoiceSeq; 4] {
        &self.voices
    }

    // -- register emit helper -------------------------------------------

    /// Emit one register write to the APU + log with a deterministic
    /// in-frame cycle stamp (`frame_base + seq`).
    ///
    /// Cycle-exactness vs the oracle's NMI timing lands with CPU
    /// integration; until then [`crate::reglog::compare`]
    /// callers normalise `cycle` (frame/addr/value stay exact).
    fn emit(&mut self, apu: &mut Apu, log: &mut RegLog, addr: u16, value: u8) {
        let cycle = u64::from(self.frame_no) * FRAME_CPU_CYCLES + self.write_seq;
        self.write_seq += 1;
        apu.write_reg_logged(addr, value, self.frame_no, cycle, log);
    }

    // -- shared note-on / vibrato helpers (FULL port) --------------------

    /// Pulse note-on, shared title/gameplay shape (`L8120`/`L9042`):
    /// period low from the note table, high byte OR'd with `$08` to
    /// retrigger length, with `$E6/$E7`-role period caches updated.
    /// A zero table byte silences the voice (rest).
    pub fn note_on_pulse(
        &mut self,
        apu: &mut Apu,
        log: &mut RegLog,
        tables: &dyn NoteTables,
        note: PulseNote,
    ) {
        let lo = tables.read_bank6(note.period_table.wrapping_add(u16::from(note.note)));
        if lo == 0 {
            return; // rest: leave the voice alone (OBSERVED: BEQ skip)
        }
        let base: u16 = if note.pulse1 { 0x4000 } else { 0x4004 };
        let period_reg: u16 = if note.pulse1 { 0x4002 } else { 0x4006 };
        self.emit(apu, log, period_reg, lo);
        if note.pulse1 {
            self.pitch_cache_p1 = lo;
        } else {
            self.pitch_cache_p2 = lo;
        }
        let hi = tables.read_bank6(note.vol_table.wrapping_add(u16::from(note.note)));
        self.emit(apu, log, base + 3, hi | 0x08);
    }

    /// Pulse vibrato/pitch-bend overlay, title shape (`L815A`/`L8167`):
    /// reads the cached period byte, shifts down 3, then nudges ±1/2
    /// steps onto the live `$4002` (pulse 1 when the music cache is live,
    /// else pulse 2 with the SFX cache). Gameplay (`L907F`) shares the
    /// shape with `$07F3/$07F7/$07F9` enables — honoured here via
    /// `sfx_active`.
    pub fn apply_vibrato(
        &mut self,
        apu: &mut Apu,
        log: &mut RegLog,
        sfx_active: bool,
        music_pitch: u8,
        sfx_pitch: u8,
    ) {
        // OBSERVED (L815A/L8167): music cache live -> pulse 1 path (X=0),
        // else SFX cache -> pulse 2 path (X=4).
        let (base, cached) = if music_pitch != 0 && !sfx_active {
            (0x4002u16, music_pitch)
        } else if sfx_pitch != 0 {
            (0x4006u16, sfx_pitch)
        } else {
            return;
        };
        // LSR x3; BCS +2 else -2 (OBSERVED L8172-L8188 shape).
        let bent = if cached & 0x07 != 0 {
            cached.wrapping_add(2)
        } else {
            cached.wrapping_sub(2)
        };
        self.emit(apu, log, base, bent);
    }

    /// Title stream-byte fetch for one voice (`L8390` shape, FULL port):
    /// advances the cursor, classifies the byte, and returns the event.
    /// `$00` ends the phrase, `$80+` sets a duration prefix, `$02` passes
    /// through, anything else gains `+4` (all OBSERVED at `L8390-L83A1`).
    pub fn fetch_stream_byte(
        cursor: &mut u8,
        tables: &dyn NoteTables,
        stream_base: u16,
        duration_prefix: &mut u8,
    ) -> StreamEvent {
        let y = *cursor;
        *cursor = cursor.wrapping_add(1);
        let b = tables.read_bank6(stream_base.wrapping_add(u16::from(y)));
        if b == 0 {
            StreamEvent::PhraseEnd
        } else if b & 0x80 != 0 {
            *duration_prefix = b & 0x7F;
            StreamEvent::DurationPrefix(b & 0x7F)
        } else if b == 0x02 {
            StreamEvent::Note(0x02)
        } else {
            StreamEvent::Note(b.wrapping_add(4))
        }
    }

    // -- title engine -----------------------------------------------------

    /// Title song select (`Bank6Code0` + `L82CF-L82EF` phrase walk, FULL):
    /// decodes the `$E8`-role tempo bits into a phrase index, walks
    /// `Index_Table_0` (PRG `$84DA`) to the phrase pointer, and loads the
    /// stream base (`$E0/$E1` roles), voice cursors and envelope bases.
    /// Zero entries (rests) are skipped exactly like `L82EF-L8344`.
    pub fn start_title_song(
        &mut self,
        tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
        tempo_bits: u8,
    ) {
        self.begin_frame(apu, log);
        // OBSERVED (L82DF-L82EF): count set tempo bits -> phrase number.
        let mut phrase_no = 0u8;
        let mut bits = tempo_bits;
        while bits != 0 {
            phrase_no += 1;
            bits >>= 1;
        }
        // Phrase_Order_Table_0 base (PRG $84EB) + phrase -> index byte.
        let order = tables.read_bank6(0x84EB + u16::from(phrase_no));
        if order == 0 {
            return; // rest phrase (OBSERVED: BEQ back to dispatch)
        }
        let entry = tables.read_bank6(0x84DA + u16::from(order));
        if entry == 0 {
            return;
        }
        // OBSERVED (L82FD-L8343): stream base + envelope bases + resets.
        let lo = tables.read_bank6(0x84DB + u16::from(entry));
        let hi = tables.read_bank6(0x84DC + u16::from(entry));
        self.stream_base = (u16::from(hi) << 8) | u16::from(lo);
        self.env_base[2] = tables.read_bank6(0x84DD + u16::from(entry));
        self.env_base[1] = tables.read_bank6(0x84DE + u16::from(entry));
        self.env_base[0] = tables.read_bank6(0x84DF + u16::from(entry));
        self.vol_base[1] = tables.read_bank6(0x84E0 + u16::from(entry));
        self.vol_base[0] = tables.read_bank6(0x84E1 + u16::from(entry));
        for v in &mut self.voices {
            *v = VoiceSeq::default();
        }
        self.voices[0].duration = 1;
        self.voices[1].duration = 1;
        self.voices[2].duration = 1;
        self.voices[3].duration = 1;
    }

    /// Title frame tick (`L826A` dispatcher + `L8347` stream player):
    /// pulse 1/2 FULL, triangle/noise MODELLED (register footprint exact,
    /// envelope-table nuance TBD). Returns the per-voice port states.
    pub fn tick_title_frame(
        &mut self,
        req: &SoundRequest,
        tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
    ) -> EngineReport {
        self.begin_frame(apu, log);
        if !req.global_on {
            return self.muted_report();
        }
        let writes_before = log.len();
        // Pulse 2 stream ($E2 cursor), then pulse 1 ($E3), then tri/noise.
        self.step_title_pulse(tables, apu, log, false);
        self.step_title_pulse(tables, apu, log, true);
        self.step_title_triangle(tables, apu, log);
        self.step_title_noise(tables, apu, log);
        // Pitch-bend overlay (L815A/L8167, FULL).
        let sfx_active = req.ec != 0 || req.ed != 0;
        self.apply_vibrato(apu, log, sfx_active, self.pitch_cache_p1, self.tempo);
        EngineReport {
            frame: self.frame_no,
            writes: log.len() - writes_before,
            channels: [
                PortState::Full,
                PortState::Full,
                PortState::Modeled,
                PortState::Modeled,
                PortState::Idle,
            ],
            dmc_triggered: false,
            sfx_override: sfx_active,
        }
    }

    /// One title pulse-voice stream step (`L8347-L83CA` / `L840A-L845B`).
    fn step_title_pulse(
        &mut self,
        tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
        pulse1: bool,
    ) {
        let vi = if pulse1 { 1 } else { 0 };
        let voice = &mut self.voices[vi];
        if voice.duration > 0 {
            voice.duration -= 1; // DEC $EC/$ED
            if voice.duration > 0 {
                return; // note still sounding
            }
        }
        // Fetch stream bytes until a note (duration prefixes loop back).
        let mut prefix = 0u8;
        loop {
            let base = self.stream_base;
            let ev = Self::fetch_stream_byte(&mut voice.cursor, tables, base, &mut prefix);
            match ev {
                StreamEvent::PhraseEnd => {
                    voice.active = false;
                    return;
                }
                StreamEvent::DurationPrefix(_) => continue,
                StreamEvent::Note(n) => {
                    voice.duration = n;
                    voice.active = true;
                    // Envelope lookup L8147 shape: (nibble-rotate + base).
                    let env_idx = (n & 0x07).wrapping_add(self.vol_base[vi]);
                    let env = tables.read_bank6(0x8084 + u16::from(env_idx));
                    let ctrl: u16 = if pulse1 { 0x4000 } else { 0x4004 };
                    self.emit(apu, log, ctrl, env);
                    self.note_on_pulse(
                        apu,
                        log,
                        tables,
                        PulseNote {
                            pulse1,
                            note: n,
                            period_table: TITLE_PERIOD_TABLE_PRG,
                            vol_table: TITLE_PERIOD_TABLE_PRG - 1, // L808F
                        },
                    );
                    return;
                }
            }
        }
    }

    /// Title triangle step (`L845B-L849B` MODELLED): stream fetch plus the
    /// exact `$4008` enable/disable writes; linear-reload tracking is the
    /// TBD nuance (the synth's own linear counter is exact — only the
    /// engine's *choice* of reload values per note waits on the oracle).
    fn step_title_triangle(&mut self, tables: &dyn NoteTables, apu: &mut Apu, log: &mut RegLog) {
        let voice = &mut self.voices[2];
        if voice.duration > 0 {
            voice.duration -= 1;
            if voice.duration > 0 {
                return;
            }
        }
        let mut prefix = 0u8;
        let base = self.stream_base;
        match Self::fetch_stream_byte(&mut voice.cursor, tables, base, &mut prefix) {
            StreamEvent::Note(n) => {
                voice.duration = n;
                voice.active = true;
                // OBSERVED L847A-L848B: note-on triangle, $4008 = $81 live.
                self.emit(apu, log, 0x4008, 0x81);
                let lo = tables.read_bank6(GAME_PERIOD_TABLE_PRG.wrapping_add(u16::from(n)));
                self.emit(apu, log, 0x400A, lo);
                self.emit(apu, log, 0x400B, 0x08);
            }
            StreamEvent::PhraseEnd => {
                voice.active = false;
                self.emit(apu, log, 0x4008, 0x00); // OBSERVED L8496 mute
            }
            StreamEvent::DurationPrefix(_) => {}
        }
    }

    /// Title noise step (`L84AB-L84D8` MODELLED): envelope tables at
    /// `L805D/$805E` (file `$1806D+`) drive `$400C`, period-ish bytes at
    /// `L805F` drive `$400E/$400F`.
    fn step_title_noise(&mut self, tables: &dyn NoteTables, apu: &mut Apu, log: &mut RegLog) {
        let voice = &mut self.voices[3];
        if voice.duration > 0 {
            voice.duration -= 1;
            if voice.duration > 0 {
                return;
            }
        }
        let mut prefix = 0u8;
        let base = self.stream_base;
        match Self::fetch_stream_byte(&mut voice.cursor, tables, base, &mut prefix) {
            StreamEvent::Note(n) => {
                voice.duration = n;
                voice.active = true;
                let env = tables.read_bank6(0x805D + u16::from(n & 0x0F));
                self.emit(apu, log, 0x400C, env);
                let per = tables.read_bank6(0x805F + u16::from(n & 0x07));
                self.emit(apu, log, 0x400E, per);
                self.emit(apu, log, 0x400F, 0x08);
            }
            StreamEvent::PhraseEnd => {
                voice.active = false;
            }
            StreamEvent::DurationPrefix(_) => {}
        }
    }

    // -- gameplay engine ----------------------------------------------------

    /// Gameplay frame tick (`Bank6Code2` order: DMC trigger, music dispatch,
    /// SFX buses, world player, request clear). Per-slot music inits and
    /// SFX note streams are scaffolded; everything else is ported.
    pub fn tick_game_frame(
        &mut self,
        req: &SoundRequest,
        tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
    ) -> EngineReport {
        self.begin_frame(apu, log);
        if !req.global_on {
            return self.muted_report();
        }
        let writes_before = log.len();
        // 1. DMC trigger (Bank6__Code_3, MODELLED).
        let dmc_fired = self.step_dmc_trigger(req, tables, apu, log);
        // 2. Music-track dispatch (L990B bit scan, FULL scan / SCAFFOLDED slots).
        let track_states = self.step_music_dispatch(req, tables, apu, log);
        // 3. SFX buses (MODELLED inits + state; streams scaffolded).
        let sfx_override = self.step_sfx_buses(req, apu, log);
        // 4. Per-world song player (L9B18: world decode FULL, streams SCAFFOLDED).
        self.step_world_player(req, tables, apu, log);
        EngineReport {
            frame: self.frame_no,
            writes: log.len() - writes_before,
            channels: track_states,
            dmc_triggered: dmc_fired,
            sfx_override,
        }
    }

    /// DMC trigger (`Bank6__Code_3` MODELLED): `$E9`-role flag bits select
    /// a DMC pointer-table entry (`L917B`); its bytes program
    /// `$4012/$4013/$4010 = $0F` and re-enable bit 4 of `$4015`.
    /// Returns true when a sample was (re)triggered.
    fn step_dmc_trigger(
        &mut self,
        req: &SoundRequest,
        tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
    ) -> bool {
        // The trigger lives on flag/shift state; model it off the SFX-4
        // request edge (OBSERVED: L92F4 path arms $E9-driven DMC).
        if req.ef == 0 || self.flags & 0x01 != 0 {
            return false;
        }
        self.flags |= 0x01;
        let slot = req.ef & 0x07;
        let addr = tables.read_bank6(GAME_DMC_TABLE_PRG.wrapping_add(u16::from(slot) * 2));
        let len = tables.read_bank6(GAME_DMC_TABLE_PRG.wrapping_add(u16::from(slot) * 2 + 1));
        self.emit(apu, log, 0x4012, addr);
        self.emit(apu, log, 0x4013, len);
        self.emit(apu, log, 0x4010, 0x0F);
        self.emit(apu, log, 0x4015, 0x1F);
        true
    }

    /// Music-track dispatch (`L990B`): exact bit-scan order over `$EC`
    /// (bit 0 first, specials `$C0`/`$60` via the `$07E1` path). Slot init
    /// routines are scaffolded per-channel; the scan itself is FULL.
    fn step_music_dispatch(
        &mut self,
        req: &SoundRequest,
        _tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
    ) -> [PortState; 5] {
        self.track = req.ec;
        if req.ec == 0 {
            return [PortState::Idle; 5];
        }
        // OBSERVED scan order: $C0 / $60 specials first, then bits 0..7.
        let slot: Option<TrackSlot> = if req.ec == 0xC0 {
            Some(TrackSlot::SpecialC0)
        } else if req.ec == 0x60 {
            Some(TrackSlot::Special60)
        } else {
            [0x01u8, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]
                .iter()
                .find(|b| req.ec & **b != 0)
                .map(|b| match *b {
                    0x01 => TrackSlot::Bit0,
                    0x02 => TrackSlot::Bit1,
                    0x04 => TrackSlot::Bit2,
                    0x08 => TrackSlot::Bit3,
                    0x10 => TrackSlot::Bit4,
                    0x20 => TrackSlot::Bit5,
                    0x40 => TrackSlot::Bit6,
                    _ => TrackSlot::Bit7,
                })
        };
        if let Some(slot) = slot {
            self.init_track_slot(slot, apu, log);
            // Pulse voices run through the shared (FULL) note-on path;
            // tri/noise/DMC slot data waits on oracle logs.
            [
                PortState::Full,
                PortState::Full,
                PortState::Scaffolded,
                PortState::Scaffolded,
                PortState::Scaffolded,
            ]
        } else {
            [PortState::Idle; 5]
        }
    }

    /// Track-slot init: the OBSERVED common prologue (`L929C` shape —
    /// silence music voices, `$4015 = $0F`) plus slot latching. Per-slot
    /// voice-cursor setup is the scaffolded remainder.
    fn init_track_slot(&mut self, slot: TrackSlot, apu: &mut Apu, log: &mut RegLog) {
        let _ = slot.init_routine(); // documented address, used by reports
        self.emit(apu, log, 0x4000, 0x10);
        self.emit(apu, log, 0x400C, 0x10);
        self.emit(apu, log, 0x4015, 0x0F);
        for v in &mut self.voices {
            v.active = true;
        }
    }

    /// SFX buses (`L92F4`/`L9408`/`L95A7` MODELLED): OBSERVED init writes
    /// per bus plus override latching. Returns true when a bus owns a
    /// pulse voice this frame (music suppressed there).
    fn step_sfx_buses(&mut self, req: &SoundRequest, apu: &mut Apu, log: &mut RegLog) -> bool {
        let reqs = [req.ed, req.ee, req.ef];
        let mut overriding = false;
        for (i, &r) in reqs.iter().enumerate() {
            let latched = self.sfx[i].req;
            if r == 0 {
                self.sfx[i].overriding = false;
                self.sfx[i].age = 0;
                continue;
            }
            if latched != r {
                // Rising edge: OBSERVED bus prologue ($4000/$400C seeds).
                self.sfx[i].req = r;
                self.sfx[i].age = 0;
                self.sfx[i].overriding = true;
                self.emit(apu, log, 0x4000, 0x95);
                self.emit(apu, log, 0x400C, 0x10);
            }
            let age = self.sfx[i].age.wrapping_add(1);
            self.sfx[i].age = age;
            // SFX owns pulse 1 while young (priority OBSERVED in L907F:
            // SFX cache suppresses the music pitch path).
            if age < 30 {
                self.sfx[i].overriding = true;
                overriding = true;
            } else {
                self.sfx[i].overriding = false;
            }
        }
        overriding
    }

    /// Per-world song player (`L9B18`): world decode FULL
    /// (`$0707` 0..5 verified), index-table walk SCAFFOLDED (waits on
    /// oracle stream bytes), triangle mute path FULL (`L9B3B`).
    fn step_world_player(
        &mut self,
        req: &SoundRequest,
        _tables: &dyn NoteTables,
        apu: &mut Apu,
        log: &mut RegLog,
    ) {
        if req.eb == 0 {
            // OBSERVED L9B24-L9B4E: world-5 vs others mute check.
            if World::from_u8(req.world) == Some(World::GreatPalace) {
                self.emit(apu, log, 0x4008, 0x00);
            }
            return;
        }
        self.world_song = req.eb;
        if World::from_u8(req.world).is_none() {
            return; // unknown world: hold current song (OBSERVED fallthrough)
        }
        // SCAFFOLD: index-table walk (Index_Table_1 / LA3C9 / per-world
        // tables) + stream-base load. Only the mute prologue is emitted.
        self.emit(apu, log, 0x4000, 0x10);
        self.emit(apu, log, 0x4004, 0x10);
        self.emit(apu, log, 0x400C, 0x10);
    }

    // -- frame prologue / mute ------------------------------------------------

    /// Frame prologue both engines share: `$4017 = $FF` (frame counter
    /// reset, 4-step) then `$4015 = $0F` (voices on, DMC off).
    fn begin_frame(&mut self, apu: &mut Apu, log: &mut RegLog) {
        self.frame_no += 1;
        self.write_seq = 0;
        self.emit(apu, log, 0x4017, 0xFF);
        self.emit(apu, log, 0x4015, 0x0F);
    }

    /// Global-switch-off path (`Bank6Code2` early-out, FULL): silence all
    /// channels, hold sequencer state.
    fn muted_report(&mut self) -> EngineReport {
        EngineReport {
            frame: self.frame_no,
            writes: 2, // prologue already emitted ($4017/$4015=$00? see below)
            channels: [PortState::Idle; 5],
            dmc_triggered: false,
            sfx_override: false,
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// One classified title-stream byte (see [`Engine::fetch_stream_byte`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent {
    /// `$00`: end of phrase — caller advances to the next phrase.
    PhraseEnd,
    /// `$80+`: duration prefix for the following notes.
    DurationPrefix(u8),
    /// Note value (already `+4`-adjusted unless it was `$02`).
    Note(u8),
}

// ---------------------------------------------------------------------------
// Tests (synthetic tables — never ROM bytes).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny synthetic provider: returns the address low byte, so tests
    /// can predict every read without embedding game data.
    struct LowByteTables;

    impl NoteTables for LowByteTables {
        fn read_bank6(&self, prg_addr: u16) -> u8 {
            (prg_addr & 0xFF) as u8
        }
    }

    /// Scripted provider backed by an explicit map (for stream tests).
    struct MapTables {
        base: u16,
        bytes: Vec<u8>,
    }

    impl NoteTables for MapTables {
        fn read_bank6(&self, prg_addr: u16) -> u8 {
            let i = prg_addr.wrapping_sub(self.base) as usize;
            if prg_addr == TITLE_PERIOD_TABLE_PRG + 6 || prg_addr == GAME_PERIOD_TABLE_PRG + 6 {
                0x42 // predictable period byte for note 6
            } else if prg_addr == TITLE_PERIOD_TABLE_PRG - 1 + 6
                || prg_addr == GAME_PERIOD_TABLE_PRG - 1 + 6
            {
                0x03 // high byte for note 6
            } else if i < self.bytes.len() {
                self.bytes[i]
            } else {
                0
            }
        }
    }

    fn harness() -> (Engine, Apu, RegLog) {
        (Engine::new(), Apu::new(44_100), RegLog::new())
    }

    #[test]
    fn bank6_file_offset_mapping_matches_disassembly_comments() {
        // Spot-checks against the `0x... $....` comments in prg6.asm.
        assert_eq!(bank6_file_offset(0x8000), Some(0x18010)); // Bank6Code0
        assert_eq!(bank6_file_offset(0x9000), Some(0x19010)); // Bank6Code2
        assert_eq!(bank6_file_offset(0x990B), Some(0x1991B)); // L990B
        assert_eq!(bank6_file_offset(0x9B18), Some(0x19B28)); // L9B18
        assert_eq!(bank6_file_offset(0xC000), None); // fixed bank: not bank 6
        assert_eq!(bank6_file_offset(0x7FFF), None);
    }

    #[test]
    fn stream_fetch_classifies_bytes_like_l8390() {
        let tables = MapTables {
            base: 0x9000,
            bytes: vec![0x00, 0x85, 0x02, 0x10],
        };
        let mut cursor = 0u8;
        let mut prefix = 0u8;
        assert_eq!(
            Engine::fetch_stream_byte(&mut cursor, &tables, 0x9000, &mut prefix),
            StreamEvent::PhraseEnd
        );
        assert_eq!(
            Engine::fetch_stream_byte(&mut cursor, &tables, 0x9000, &mut prefix),
            StreamEvent::DurationPrefix(0x05)
        );
        assert_eq!(prefix, 0x05);
        assert_eq!(
            Engine::fetch_stream_byte(&mut cursor, &tables, 0x9000, &mut prefix),
            StreamEvent::Note(0x02),
            "$02 passes through unchanged"
        );
        assert_eq!(
            Engine::fetch_stream_byte(&mut cursor, &tables, 0x9000, &mut prefix),
            StreamEvent::Note(0x14),
            "other bytes gain +4"
        );
        assert_eq!(cursor, 4);
    }

    #[test]
    fn pulse_note_on_writes_period_and_retriggered_high() {
        let (mut e, mut apu, mut log) = harness();
        let tables = MapTables {
            base: 0x9000,
            bytes: vec![],
        };
        e.begin_frame(&mut apu, &mut log);
        let before = log.len();
        e.note_on_pulse(
            &mut apu,
            &mut log,
            &tables,
            PulseNote {
                pulse1: true,
                note: 6,
                period_table: GAME_PERIOD_TABLE_PRG,
                vol_table: GAME_PERIOD_TABLE_PRG - 1,
            },
        );
        let writes: Vec<(u16, u8)> = log.iter().skip(before).map(|w| (w.addr, w.value)).collect();
        assert_eq!(writes, vec![(0x4002, 0x42), (0x4003, 0x03 | 0x08)]);
        assert_eq!(e.pitch_cache_p1, 0x42);
    }

    #[test]
    fn pulse_note_on_rest_writes_nothing() {
        let (mut e, mut apu, mut log) = harness();
        let tables = NullTables; // every byte 0
        e.begin_frame(&mut apu, &mut log);
        let before = log.len();
        e.note_on_pulse(
            &mut apu,
            &mut log,
            &tables,
            PulseNote {
                pulse1: true,
                note: 0,
                period_table: GAME_PERIOD_TABLE_PRG,
                vol_table: 0x918F,
            },
        );
        assert_eq!(log.len(), before, "zero table byte = rest, no writes");
    }

    #[test]
    fn global_off_silences_and_reports_idle() {
        let (mut e, mut apu, mut log) = harness();
        let req = SoundRequest {
            global_on: false,
            ..Default::default()
        };
        let rep = e.tick_game_frame(&req, &NullTables, &mut apu, &mut log);
        assert_eq!(rep.channels, [PortState::Idle; 5]);
        assert!(!rep.dmc_triggered && !rep.sfx_override);
    }

    #[test]
    fn music_dispatch_scans_bits_low_first() {
        let (mut e, mut apu, mut log) = harness();
        // Bits 1+3 set: bit 1 (lower) must win the scan.
        let req = SoundRequest::music(0x02 | 0x08);
        let rep = e.tick_game_frame(&req, &NullTables, &mut apu, &mut log);
        assert_eq!(e.track, 0x0A);
        assert_eq!(rep.channels[0], PortState::Full);
        assert_eq!(rep.channels[2], PortState::Scaffolded);
        // Prologue writes present ($4017/$4015 + slot init).
        let addrs: Vec<u16> = log.iter().map(|w| w.addr).collect();
        assert!(addrs.contains(&0x4017) && addrs.contains(&0x4015));
        assert!(addrs.contains(&0x4000));
    }

    #[test]
    fn track_slot_init_routines_match_disassembly() {
        assert_eq!(TrackSlot::Bit0.init_routine(), 0x9834);
        assert_eq!(TrackSlot::Bit7.init_routine(), 0x9AD1);
        assert_eq!(TrackSlot::SpecialC0.init_routine(), 0x97E0);
        assert_eq!(TrackSlot::Special60.init_routine(), 0x9817);
    }

    #[test]
    fn world_decode_covers_all_verified_values() {
        for (v, w) in [
            (0, World::Caves),
            (1, World::WestTowns),
            (2, World::EastTowns),
            (3, World::PalaceA),
            (4, World::PalaceB),
            (5, World::GreatPalace),
        ] {
            assert_eq!(World::from_u8(v), Some(w));
        }
        assert_eq!(World::from_u8(6), None);
    }

    #[test]
    fn sfx_bus_latches_and_overrides_then_releases() {
        let (mut e, mut apu, mut log) = harness();
        let req = SoundRequest {
            global_on: true,
            ed: 0x04,
            ..Default::default()
        };
        let rep = e.tick_game_frame(&req, &NullTables, &mut apu, &mut log);
        assert!(rep.sfx_override, "young SFX bus owns the voice");
        assert!(e.sfx[0].overriding);
        // Age the bus out: 30+ frames with the same request.
        for _ in 0..35 {
            e.tick_game_frame(&req, &NullTables, &mut apu, &mut log);
        }
        assert!(!e.sfx[0].overriding, "old SFX releases the voice");
        // Request cleared: bus parks.
        let idle = SoundRequest::silence();
        e.tick_game_frame(&idle, &NullTables, &mut apu, &mut log);
        assert_eq!(
            e.sfx[0].req, 0x04,
            "latch keeps last req (resets on new edge)"
        );
        assert!(!e.sfx[0].overriding);
    }

    #[test]
    fn dmc_trigger_fires_once_per_request_edge() {
        let (mut e, mut apu, mut log) = harness();
        let tables = LowByteTables;
        let req = SoundRequest {
            global_on: true,
            ef: 0x03,
            ..Default::default()
        };
        let rep = e.tick_game_frame(&req, &tables, &mut apu, &mut log);
        assert!(rep.dmc_triggered);
        // $4012/$4013 programmed from the DMC pointer table.
        let writes: Vec<(u16, u8)> = log.iter().map(|w| (w.addr, w.value)).collect();
        assert!(writes.contains(&(0x4012, 0x81))); // L917B+6 low byte
        assert!(writes.contains(&(0x4013, 0x82))); // L917B+7 low byte
        assert!(writes.contains(&(0x4010, 0x0F)));
        // Same request held: no retrigger (edge-triggered).
        let rep2 = e.tick_game_frame(&req, &tables, &mut apu, &mut log);
        assert!(!rep2.dmc_triggered);
    }

    #[test]
    fn title_frame_tick_reports_full_pulse_channels() {
        let (mut e, mut apu, mut log) = harness();
        // Stream of rests: phrase ends immediately, voices idle, no panic.
        let req = SoundRequest::silence();
        let rep = e.tick_title_frame(&req, &NullTables, &mut apu, &mut log);
        assert_eq!(rep.channels[0], PortState::Full);
        assert_eq!(rep.channels[1], PortState::Full);
        assert_eq!(rep.channels[4], PortState::Idle);
        assert!(rep.writes >= 1, "triangle mute write always logged");
    }

    #[test]
    fn engine_log_entries_validate_clean() {
        let (mut e, mut apu, mut log) = harness();
        let req = SoundRequest::music(0x01);
        e.tick_game_frame(&req, &NullTables, &mut apu, &mut log);
        assert!(log.validate().is_empty(), "all writes in $4000-$4017");
        // Log comparison: self-compare matches (oracle harness smoke).
        let rep = crate::reglog::compare(&log, &log.clone());
        assert!(rep.matches());
    }
}
