//! Save states for rollback netplay: [`GameState`] and the
//! [`Game::save_state`] / [`Game::save_state_into`] / [`Game::load_state`]
//! API.
//!
//! A snapshot is taken **between frames** (after one [`Game::step`] /
//! [`Game::step2`] returns, before the next is called). Loading it and
//! stepping with the same inputs reproduces the original run bit for bit:
//! RAM, WRAM, OAM, palette, the indexed framebuffer and every CPU / mapper /
//! PPU / co-op byte (proven by `tests/state_tests.rs`).
//!
//! # What a snapshot holds
//!
//! Every [`Game`] field is classified as one of:
//!
//! * **captured** – affects future emulation (or is observable output of the
//!   frame) and is saved and restored;
//! * **static** – fixed configuration set up before the run (ROM images,
//!   trap table); must be identical in the saving and loading `Game`;
//! * **diagnostic** – debugging aids that never feed back into emulation;
//!   left as they are on load;
//! * **frontend-owned** – consumed by the frontend between frames; reset on
//!   load.
//!
//! | `Game` field | class | notes |
//! |---|---|---|
//! | `ram` | captured | 2 KiB |
//! | `wram` | captured | 8 KiB (battery RAM is live state too) |
//! | `oam`, `palette` | captured | lockstep mirrors of the PPU copies |
//! | `frame` | captured | latched framebuffer (a faulting frame keeps the old one) |
//! | `cpu` | captured | registers, cycle clock, interrupt latches, pending tail jump |
//! | `mmc1` | captured | shift register, count, control, CHR/PRG selects |
//! | `ppu` ([`PpuBind`]) | captured | binding fields (beam origin, mapper key, `$2004` shadow address, sprite-0 override, traffic counters) and the model's state ([`z2_ppu::Ppu::copy_state_from`]): nametables, **both loaded 4 KiB CHR slots as bytes**, OAM, palette, registers and latches, sprite limit, the frame under construction, beam, hit, current line pipeline |
//! | `ppu` model event trace | diagnostic | not copied; destination keeps its own |
//! | `ppu` model render record | frontend-owned | enablement kept, buffers cleared on load; valid again after the next step |
//! | `apu` log | frontend-owned | **cleared on load**: audio for re-simulated frames must not be replayed; the frontend drains the log after each step as usual |
//! | `apu` counters (`reads`, `writes`, `last_write`) | diagnostic | never read back by emulation |
//! | `coop` | captured | whole [`CoopState`]: flag, options, swap block, contact state, respawn, counters, saved sprite limit |
//! | `pad1`, `pad2`, `strobe`, `shift1`, `shift2` | captured | controller latches (pad 2 stays latched across frames) |
//! | `frame_end_dots`, `frame_count` | captured | dot-exact frame clock |
//! | `exec_errors`, `last_exec_error` | captured | frontends pause on a changed count, so a re-simulated fault must not double count |
//! | `prg`, `chr` | static | ROM images (never stored; CHR slot bytes are copied instead of re-derived, see below) |
//! | `traps` | static | function pointers + untrapped set (only toggled transiently inside one instruction) |
//! | `traplog` | diagnostic | allocating ring of names; attribution only |
//!
//! CHR slots are copied as bytes (8 KiB, a fraction of a millisecond's
//! budget) rather than re-derived from `chr` by page number: re-deriving would
//! depend on the MMC1 cache key and on out-of-range selects leaving a slot
//! untouched, and costs about the same memcpy anyway.
//!
//! Co-op: loading a snapshot taken with co-op enabled registers the co-op
//! trap group if it is missing (idempotent). A snapshot taken with co-op off
//! loads fine into a `Game` whose group is registered: every co-op trap is a
//! pass-through while the flag is clear.
//!
//! # Hidden state outside `Game`
//!
//! None. Trap bodies are plain `fn(&mut Game)` pointers; `z2-core` and
//! `z2-ppu` have no `static mut`, `thread_local!`, `OnceCell`/`OnceLock`/
//! `LazyLock`, `Cell`/`RefCell` or atomics, and no cache outside `Game`
//! (the PPU's hit-probe memo and MMC1 key live in the snapshot).
//!
//! # Cost
//!
//! [`Game::save_state_into`] and [`Game::load_state`] never allocate once
//! the destination exists (plain field copies; the boxed framebuffers are
//! copied in place). A snapshot is ~[`GameState::approx_bytes`] bytes.
//!
//! # Serialization
//!
//! [`GameState::to_bytes`] / [`GameState::from_bytes`] give a versioned,
//! little-endian byte image ([`STATE_MAGIC`], [`STATE_VERSION`]) for late
//! join and desync forensics. The image does not include ROM or trap-table
//! data; the loading side must run the same ROM and trap groups.

use crate::coop::{CoopOptions, CoopState, ENEMY_SLOTS, SWAP_LEN};
use crate::cpu::{Cpu, ExecError, Mmc1};
use crate::game::{Game, FRAME_LEN};
use crate::ppu_bind::PpuBind;
use z2_ppu::SpriteLimit;

/// Magic prefix of [`GameState::to_bytes`].
pub const STATE_MAGIC: [u8; 4] = *b"Z2GS";
/// Format version of [`GameState::to_bytes`].
pub const STATE_VERSION: u16 = 1;

/// Complete snapshot of a [`Game`]'s live emulation state (see the module
/// docs for the per-field capture table).
#[derive(Debug, Clone)]
pub struct GameState {
    ram: [u8; 0x800],
    wram: [u8; 0x2000],
    oam: [u8; 256],
    palette: [u8; 32],
    frame: Box<[u8; FRAME_LEN]>,
    cpu: Cpu,
    mmc1: Mmc1,
    ppu: PpuBind,
    coop: CoopState,
    pad1: u8,
    pad2: u8,
    strobe: bool,
    shift1: u8,
    shift2: u8,
    frame_end_dots: u64,
    frame_count: u64,
    exec_errors: u64,
    last_exec_error: Option<ExecError>,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameState {
    /// Empty (power-on) snapshot, to be filled by [`Game::save_state_into`].
    /// Preallocate a ring of these once; saving into them never allocates.
    #[must_use]
    pub fn new() -> Self {
        let frame: Box<[u8]> = vec![0u8; FRAME_LEN].into_boxed_slice();
        let frame: Box<[u8; FRAME_LEN]> = match frame.try_into() {
            Ok(b) => b,
            Err(_) => unreachable!("vec has FRAME_LEN bytes"),
        };
        Self {
            ram: [0; 0x800],
            wram: [0; 0x2000],
            oam: [0; 256],
            palette: [0; 32],
            frame,
            cpu: Cpu::new(),
            mmc1: Mmc1::new(),
            ppu: PpuBind::new(),
            coop: CoopState::default(),
            pad1: 0,
            pad2: 0,
            strobe: false,
            shift1: 0,
            shift2: 0,
            frame_end_dots: 0,
            frame_count: 0,
            exec_errors: 0,
            last_exec_error: None,
        }
    }

    /// [`Game::frame_count`] at the time of the snapshot.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Snapshot RAM (`$0000-$07FF`).
    #[must_use]
    pub fn ram(&self) -> &[u8; 0x800] {
        &self.ram
    }

    /// Snapshot WRAM (`$6000-$7FFF`).
    #[must_use]
    pub fn wram(&self) -> &[u8; 0x2000] {
        &self.wram
    }

    /// Approximate in-memory size of one snapshot (inline fields plus the
    /// two boxed framebuffers).
    #[must_use]
    pub fn approx_bytes() -> usize {
        core::mem::size_of::<GameState>() + 2 * FRAME_LEN
    }

    /// Versioned byte image (see module docs, "Serialization").
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::with_capacity(Self::approx_bytes()));
        w.bytes(&STATE_MAGIC);
        w.u16(STATE_VERSION);
        w.bytes(&self.ram);
        w.bytes(&self.wram);
        w.bytes(&self.oam);
        w.bytes(&self.palette);
        w.bytes(&self.frame[..]);
        let c = &self.cpu;
        w.bytes(&[c.a, c.x, c.y, c.sp]);
        w.u16(c.pc);
        w.u8(c.p);
        w.u64(c.cycles);
        w.bool(c.nmi_pending);
        w.bool(c.irq_pending);
        w.opt_u16(c.trap_jump);
        let m = &self.mmc1;
        w.bytes(&[m.shift, m.count, m.ctrl, m.chr0, m.chr1, m.prg]);
        self.ppu.write_state(&mut w);
        write_coop(&mut w, &self.coop);
        w.bytes(&[self.pad1, self.pad2]);
        w.bool(self.strobe);
        w.bytes(&[self.shift1, self.shift2]);
        w.u64(self.frame_end_dots);
        w.u64(self.frame_count);
        w.u64(self.exec_errors);
        match self.last_exec_error {
            None => w.bytes(&[0, 0, 0, 0]),
            Some(ExecError::IllegalOpcode { opcode, at }) => {
                w.bytes(&[1, opcode]);
                w.u16(at);
            }
            Some(ExecError::MaxInstructions { at }) => {
                w.bytes(&[2, 0]);
                w.u16(at);
            }
        }
        w.0
    }

    /// Decode a [`GameState::to_bytes`] image.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StateError> {
        let mut r = Reader(bytes);
        if r.take(4)? != STATE_MAGIC {
            return Err(StateError::BadMagic);
        }
        let version = r.u16()?;
        if version != STATE_VERSION {
            return Err(StateError::Version(version));
        }
        let mut s = GameState::new();
        s.ram.copy_from_slice(r.take(0x800)?);
        s.wram.copy_from_slice(r.take(0x2000)?);
        s.oam.copy_from_slice(r.take(256)?);
        s.palette.copy_from_slice(r.take(32)?);
        s.frame.copy_from_slice(r.take(FRAME_LEN)?);
        let reg = r.take(4)?;
        s.cpu.a = reg[0];
        s.cpu.x = reg[1];
        s.cpu.y = reg[2];
        s.cpu.sp = reg[3];
        s.cpu.pc = r.u16()?;
        s.cpu.p = r.u8()?;
        s.cpu.cycles = r.u64()?;
        s.cpu.nmi_pending = r.bool()?;
        s.cpu.irq_pending = r.bool()?;
        s.cpu.trap_jump = r.opt_u16()?;
        let m = r.take(6)?;
        s.mmc1 = Mmc1 {
            shift: m[0],
            count: m[1],
            ctrl: m[2],
            chr0: m[3],
            chr1: m[4],
            prg: m[5],
        };
        s.ppu.read_state(&mut r)?;
        s.coop = read_coop(&mut r)?;
        s.pad1 = r.u8()?;
        s.pad2 = r.u8()?;
        s.strobe = r.bool()?;
        s.shift1 = r.u8()?;
        s.shift2 = r.u8()?;
        s.frame_end_dots = r.u64()?;
        s.frame_count = r.u64()?;
        s.exec_errors = r.u64()?;
        let e = r.take(2)?;
        let (tag, opcode) = (e[0], e[1]);
        let at = r.u16()?;
        s.last_exec_error = match tag {
            0 => None,
            1 => Some(ExecError::IllegalOpcode { opcode, at }),
            2 => Some(ExecError::MaxInstructions { at }),
            _ => return Err(StateError::Invalid("exec error")),
        };
        if !r.0.is_empty() {
            return Err(StateError::Trailing(r.0.len()));
        }
        Ok(s)
    }
}

fn write_coop(w: &mut Writer, c: &CoopState) {
    w.bool(c.enabled);
    let o = &c.opts;
    w.bytes(&[o.p2_x_offset, o.p2_attr_xor]);
    w.bool(o.unlimited_sprites);
    w.u16(o.respawn_frames);
    w.bool(o.mask_grounded_up);
    w.bool(o.p2_contact);
    w.bool(c.active);
    w.bytes(&c.block);
    w.bool(c.in_update);
    w.bool(c.in_display);
    w.bytes(&c.touch_mask);
    w.u8(c.p2_hp);
    w.u16(c.respawn_timer);
    w.bytes(&c.p2_deaths.to_le_bytes());
    for v in [
        c.cycles_saved_frame,
        c.cycles_saved_total,
        c.n_update,
        c.n_display,
        c.n_nested_display,
        c.n_p2_contacts,
    ] {
        w.u64(v);
    }
    w.u8(match c.saved_sprite_limit {
        None => 0,
        Some(SpriteLimit::Faithful8) => 1,
        Some(SpriteLimit::Unlimited) => 2,
    });
}

fn read_coop(r: &mut Reader) -> Result<CoopState, StateError> {
    let mut c = CoopState {
        enabled: r.bool()?,
        ..CoopState::default()
    };
    c.opts = CoopOptions {
        p2_x_offset: r.u8()?,
        p2_attr_xor: r.u8()?,
        unlimited_sprites: r.bool()?,
        respawn_frames: r.u16()?,
        mask_grounded_up: r.bool()?,
        p2_contact: r.bool()?,
    };
    c.active = r.bool()?;
    c.block.copy_from_slice(r.take(SWAP_LEN)?);
    c.in_update = r.bool()?;
    c.in_display = r.bool()?;
    c.touch_mask.copy_from_slice(r.take(ENEMY_SLOTS)?);
    c.p2_hp = r.u8()?;
    c.respawn_timer = r.u16()?;
    let d = r.take(4)?;
    c.p2_deaths = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
    c.cycles_saved_frame = r.u64()?;
    c.cycles_saved_total = r.u64()?;
    c.n_update = r.u64()?;
    c.n_display = r.u64()?;
    c.n_nested_display = r.u64()?;
    c.n_p2_contacts = r.u64()?;
    c.saved_sprite_limit = match r.u8()? {
        0 => None,
        1 => Some(SpriteLimit::Faithful8),
        2 => Some(SpriteLimit::Unlimited),
        _ => return Err(StateError::Invalid("saved sprite limit")),
    };
    Ok(c)
}

/// Error from [`GameState::from_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// Missing [`STATE_MAGIC`].
    BadMagic,
    /// Unsupported format version.
    Version(u16),
    /// Image ended early.
    Truncated,
    /// Bytes left over after the image.
    Trailing(usize),
    /// A field held an out-of-range value.
    Invalid(&'static str),
    /// The PPU section failed to decode.
    Ppu(z2_ppu::state::StateError),
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StateError::BadMagic => write!(f, "not a z2rs game state (bad magic)"),
            StateError::Version(v) => {
                write!(f, "game state version {v}, expected {STATE_VERSION}")
            }
            StateError::Truncated => write!(f, "game state image is truncated"),
            StateError::Trailing(n) => write!(f, "game state image has {n} trailing bytes"),
            StateError::Invalid(what) => write!(f, "game state has an invalid {what}"),
            StateError::Ppu(e) => write!(f, "game state PPU section: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

/// Little-endian byte sink for [`GameState::to_bytes`].
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn vec(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
    pub(crate) fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub(crate) fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub(crate) fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }
    pub(crate) fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn opt_u16(&mut self, v: Option<u16>) {
        match v {
            Some(x) => {
                self.u8(1);
                self.u16(x);
            }
            None => self.bytes(&[0, 0, 0]),
        }
    }
}

/// Bounds-checked little-endian cursor for [`GameState::from_bytes`].
pub(crate) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        if self.0.len() < n {
            return Err(StateError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn bool(&mut self) -> Result<bool, StateError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StateError::Invalid("flag")),
        }
    }
    pub(crate) fn u16(&mut self) -> Result<u16, StateError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, StateError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn opt_u16(&mut self) -> Result<Option<u16>, StateError> {
        let tag = self.bool()?;
        let v = self.u16()?;
        Ok(tag.then_some(v))
    }
}

impl Game {
    /// Snapshot the live emulation state (allocates one [`GameState`]; use
    /// [`Game::save_state_into`] on a preallocated state every frame).
    #[must_use]
    pub fn save_state(&self) -> GameState {
        let mut s = GameState::new();
        self.save_state_into(&mut s);
        s
    }

    /// Snapshot into an existing [`GameState`] without allocating.
    pub fn save_state_into(&self, s: &mut GameState) {
        s.ram = self.ram;
        s.wram = self.wram;
        s.oam = self.oam;
        s.palette = self.palette;
        *s.frame = self.frame;
        s.cpu = self.cpu;
        s.mmc1 = self.mmc1;
        s.ppu.copy_state_from(&self.ppu);
        s.coop.clone_from(&self.coop);
        s.pad1 = self.pad1;
        s.pad2 = self.pad2;
        s.strobe = self.strobe;
        s.shift1 = self.shift1;
        s.shift2 = self.shift2;
        s.frame_end_dots = self.frame_end_dots;
        s.frame_count = self.frame_count;
        s.exec_errors = self.exec_errors;
        s.last_exec_error = self.last_exec_error;
    }

    /// Restore a snapshot without allocating. The next [`Game::step`] /
    /// [`Game::step2`] continues exactly as the run the snapshot was taken
    /// from. The APU write log is cleared (audio for re-simulated frames is
    /// not replayed); the PPU render record is cleared (valid after the next
    /// step). The trap table and ROM images must match the saving `Game`.
    pub fn load_state(&mut self, s: &GameState) {
        self.ram = s.ram;
        self.wram = s.wram;
        self.oam = s.oam;
        self.palette = s.palette;
        self.frame = *s.frame;
        self.cpu = s.cpu;
        self.mmc1 = s.mmc1;
        self.ppu.copy_state_from(&s.ppu);
        if s.coop.enabled {
            crate::coop::register_coop_traps(self);
        }
        self.coop.clone_from(&s.coop);
        self.pad1 = s.pad1;
        self.pad2 = s.pad2;
        self.strobe = s.strobe;
        self.shift1 = s.shift1;
        self.shift2 = s.shift2;
        self.frame_end_dots = s.frame_end_dots;
        self.frame_count = s.frame_count;
        self.exec_errors = s.exec_errors;
        self.last_exec_error = s.last_exec_error;
        self.apu.log.clear();
    }
}
