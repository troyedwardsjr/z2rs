//! ZALiA-inspired enhancements: optional gameplay changes, all **off by
//! default** (see README.md).
//!
//! ZALiA ("Zelda Again: Link is Adventuresome", HoverBat's GameMaker remake of
//! Zelda II) is used **only as a feature reference**: its code is "All rights
//! reserved", so nothing here is translated from it. Every option is
//! implemented clean-room on top of z2rs's own 6502 knowledge (trap table, RAM
//! map, the disassembly notes in `docs/`).
//!
//! # Rules
//!
//! * [`Enhancements::default`] is the original game. With everything off no
//!   trap is registered, no PRG byte is patched and [`Game::step`] runs no
//!   extra code, so a run is byte- and cycle-identical to one without this
//!   module (lockstep and movie paths never turn anything on).
//! * Gameplay options join the netplay identity through
//!   [`Enhancements::identity_bytes`] (folded into `session_trapset_id` by
//!   both frontends) and are forced off for `--movie` playback. With
//!   everything off the identity bytes are empty and the session identity is
//!   exactly what it was before this module existed.
//! * Display-only options live in [`DisplayEnh`] and never reach the game or
//!   the identity.
//!
//! # Layout
//!
//! One file per feature group; each owns its option struct, its identity
//! encoding and its mechanics:
//!
//! | file | options | struct |
//! |---|---|---|
//! | [`text`] | dialogue speed, PROTECT name | [`TextHudOpts`] |
//! | [`qol`] | continue location, XP kept on game over, lives, town entry side, wise men, ... | [`QolOpts`] |
//! | [`fixes`] | engine/physics fixes | [`FixesOpts`] |
//! | [`enemies`] | enemy and boss tweaks | [`EnemyOpts`] |
//! | [`abilities`] | double jump, stab frenzy, sword reach, ... | [`AbilityOpts`] |
//! | [`rando`] | start loadout, stat scaling, shuffles | [`RandoOpts`] |
//! | [`cheats`] | invincible, infinite magic/lives, max stats | [`CheatOpts`] |
//! | [`display`] | frontend-only effects (data only here) | [`DisplayEnh`] |
//!
//! Each group exposes `pub(crate) fn register(game, opts)` (called by
//! [`apply`] only when that group has something on) and
//! `pub(crate) fn end_of_frame(game, opts)` (called after every
//! [`Game::step`] while any enhancement is on).
//!
//! # Hooking the game from a group
//!
//! Use the helpers below from a group's `register`, never `game.traps`
//! directly, so [`apply`] can undo everything when the options change:
//!
//! * [`hook`] registers a trap at a CPU address and remembers what was there
//!   (a default-group port or nothing). Trap bodies are plain
//!   `fn(&mut Game)` and read their options from `game.enh` at run time.
//! * [`call_original`] runs whatever the hook replaced: the previous trap's
//!   body, or the ROM routine itself (interpreted) when there was none.
//! * [`patch_prg`] / [`patch_prg_at`] change bytes in the in-memory PRG copy
//!   (never the ROM file) and remember the originals.
//!
//! Hook names are part of the trap-set identity, so give them a stable
//! `"enh_<group>_<what>"` name.
//!
//! # Runtime state
//!
//! [`EnhState`] (in `Game::enh_state`) holds counters that must survive a
//! save state and roll back with netplay: it is captured by
//! [`crate::state::GameState`] and mixed into the netplay desync hash while
//! any enhancement is on. To extend it, **append** a field, then append its
//! bytes in [`EnhState::write`], read them in [`EnhState::read`] (fields
//! missing from an older, shorter blob keep their default; a field whose
//! default is not zero needs a manual `Default` impl). The blob is
//! length-prefixed in the state image, so appending needs no
//! `STATE_VERSION` bump.

pub mod abilities;
pub mod cheats;
pub mod display;
pub mod enemies;
pub mod fixes;
pub mod qol;
pub mod rando;
pub mod text;

use serde::{Deserialize, Serialize};

use crate::game::Game;
use crate::traps::{TrapFn, TrapInfo};

pub use abilities::AbilityOpts;
pub use cheats::CheatOpts;
pub use display::{DisplayEnh, FlashColor};
pub use enemies::EnemyOpts;
pub use fixes::FixesOpts;
pub use qol::{ContinueFrom, QolOpts};
pub use rando::RandoOpts;
pub use text::TextHudOpts;

/// Version byte leading [`Enhancements::identity_bytes`]. Bump it when the
/// encoding of an existing field changes meaning.
pub const IDENTITY_VERSION: u8 = 1;

/// Every gameplay enhancement, grouped. `Default` is the original game.
///
/// Serialized as JSON by the frontends (`--enh-json`, `launcher.json`,
/// `z2-native.json`, the web `set_enhancements`). Every field is
/// `#[serde(default)]`, so a file written by an older or newer build parses
/// with the missing options off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Enhancements {
    /// Text and HUD.
    pub text: TextHudOpts,
    /// Quality of life.
    pub qol: QolOpts,
    /// Engine and physics fixes.
    pub fixes: FixesOpts,
    /// Enemy and boss changes.
    pub enemies: EnemyOpts,
    /// New abilities.
    pub abilities: AbilityOpts,
    /// Randomizer and start loadout.
    pub rando: RandoOpts,
    /// Cheats.
    pub cheats: CheatOpts,
}

impl Enhancements {
    /// The original game (everything off). Same as `Default`.
    #[must_use]
    pub fn off() -> Self {
        Self::default()
    }

    /// ZALiA's own effective defaults where z2rs has an equivalent option:
    /// dialogue speed 2, continue from dungeon entrance and last town, keep
    /// 25% XP on game over, lives from dolls, enter towns from the approach
    /// side, wise men restore MP, no max-MP requirement for spells, the
    /// overworld softlock warp, every engine fix and every enemy tweak.
    /// Abilities, randomizer and cheats stay off (ZALiA has them off too, or
    /// ties them to new items).
    #[must_use]
    pub fn zalia_preset() -> Self {
        Self {
            text: TextHudOpts {
                dialogue_speed: 2,
                protect_spell_name: false,
            },
            qol: QolOpts {
                continue_from: ContinueFrom::Both,
                gameover_keep_xp_pct: 25,
                lives_from_dolls: true,
                enter_town_from_side: true,
                wise_men_restore_mp: true,
                no_mp_requirement_for_spells: true,
                overworld_softlock_warp: true,
            },
            fixes: FixesOpts {
                levelup_softlocks: true,
                iframe_update_skip: true,
                jump_direction_balance: true,
                shield_hitbox_symmetry: true,
                crumble_both_feet: true,
                xp_drain_fix: true,
            },
            enemies: EnemyOpts {
                ironknuckle_aggro: true,
                ra_hp_reduced: true,
                stalfos_upthrust_fix: true,
                wizard_teleport_wide: true,
                mago_balance: true,
                boss_first_attack_delay: true,
                helmethead_fix: true,
                carock_longer_vuln: true,
                barba_aim: true,
                p5_horsehead: true,
            },
            abilities: AbilityOpts::default(),
            rando: RandoOpts::default(),
            cheats: CheatOpts::default(),
        }
    }

    /// Whether any gameplay option is on (the cheap guard behind
    /// [`Game::step`]'s end-of-frame hook, the identity and the hash).
    #[must_use]
    pub fn any_gameplay_active(&self) -> bool {
        self.text.is_active()
            || self.qol.is_active()
            || self.fixes.is_active()
            || self.enemies.is_active()
            || self.abilities.is_active()
            || self.rando.is_active()
            || self.cheats.is_active()
    }

    /// Alias of [`Self::any_gameplay_active`] (the design doc's name).
    #[must_use]
    pub fn is_gameplay_active(&self) -> bool {
        self.any_gameplay_active()
    }

    /// Stable byte encoding of the gameplay options, for the netplay session
    /// identity. **Empty when everything is off**, so folding it into the
    /// identity leaves an all-off peer's identity unchanged.
    ///
    /// Layout: [`IDENTITY_VERSION`], then for each active group (fixed order)
    /// a tag byte (`b'T'`, `b'Q'`, `b'F'`, `b'E'`, `b'A'`, `b'R'`, `b'C'`)
    /// followed by that group's own encoding (`write_identity`). Inactive
    /// groups contribute nothing, so adding a field to a group only moves the
    /// identity of peers that turn that group on.
    #[must_use]
    pub fn identity_bytes(&self) -> Vec<u8> {
        if !self.any_gameplay_active() {
            return Vec::new();
        }
        let mut out = vec![IDENTITY_VERSION];
        if self.text.is_active() {
            out.push(b'T');
            self.text.write_identity(&mut out);
        }
        if self.qol.is_active() {
            out.push(b'Q');
            self.qol.write_identity(&mut out);
        }
        if self.fixes.is_active() {
            out.push(b'F');
            self.fixes.write_identity(&mut out);
        }
        if self.enemies.is_active() {
            out.push(b'E');
            self.enemies.write_identity(&mut out);
        }
        if self.abilities.is_active() {
            out.push(b'A');
            self.abilities.write_identity(&mut out);
        }
        if self.rando.is_active() {
            out.push(b'R');
            self.rando.write_identity(&mut out);
        }
        if self.cheats.is_active() {
            out.push(b'C');
            self.cheats.write_identity(&mut out);
        }
        out
    }

    /// Parse the JSON the frontends exchange. Missing fields are off;
    /// unknown fields are ignored.
    ///
    /// # Errors
    /// Malformed JSON or a value of the wrong type.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("enhancements JSON: {e}"))
    }

    /// Compact JSON (the inverse of [`Self::from_json`]).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The options that actually run on a cartridge image. On a ROM the
    /// `z2-rando` ROM randomizer changed (`rom_randomized`), the
    /// [`rando`] group's `item_shuffle` is turned off: it permutes the
    /// eight major items between their **vanilla** locations and checks its
    /// logic against the vanilla world, so on a randomized world it would
    /// break the seed's own placement logic. Everything else stays: the
    /// scalers and the palette randomizer rewrite whatever table values the
    /// image holds, and the start loadout detects a new file against the
    /// image's own beginning values.
    ///
    /// Frontends apply this before [`Game::set_enhancements`] **and** before
    /// computing the netplay identity, so peers agree on what runs.
    #[must_use]
    pub fn for_rom(&self, rom_randomized: bool) -> Self {
        let mut e = *self;
        if rom_randomized {
            e.rando.item_shuffle = false;
        }
        e
    }

    /// Plain-language notes for what [`Self::for_rom`] turned off (empty
    /// when nothing was), for the frontends' logs.
    #[must_use]
    pub fn rom_conflicts(&self, rom_randomized: bool) -> Vec<&'static str> {
        let mut v = Vec::new();
        if rom_randomized && self.rando.item_shuffle {
            v.push(ITEM_SHUFFLE_SKIPPED);
        }
        v
    }
}

/// Log line for the item shuffle being skipped on a randomized ROM
/// ([`Enhancements::for_rom`]).
pub const ITEM_SHUFFLE_SKIPPED: &str = "enhancements: item shuffle (Randomizer group) skipped: \
     the ROM randomizer already placed the items, and this shuffle assumes the original locations";

// ------------------------------------------------------------ runtime state

/// Runtime counters the enhancements keep between frames. Captured in save
/// states ([`crate::state::GameState`]) and mixed into the netplay desync
/// hash while any enhancement is on.
///
/// Groups may use the generic slots below (document which in the group's
/// module docs) or append dedicated fields; see the module docs, "Runtime
/// state", for the extension rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnhState {
    /// Frames stepped while any enhancement was on (quest timer basis).
    pub frames: u64,
    /// Saved continue location for [`qol`] (`continue_from`): last town,
    /// palace entered, `$070A` at palace entry, recorded bits (see the
    /// `qol` module docs). Meaningful only when `continue_valid`.
    pub continue_loc: [u8; 4],
    /// [`Self::continue_loc`] holds a location.
    pub continue_valid: bool,
    /// General-purpose down-counters. Slot owners:
    /// `0` qol (softlock-warp hold), `1` abilities (MP regen),
    /// `2` abilities (stab frenzy), `3` enemies (boss first-attack delay),
    /// `4..8` free.
    pub timers: [u16; 8],
    /// General-purpose flag bits. Bit owners: `0` abilities (double jump
    /// token used), `16..=26` qol (spell snapshot `16..=23`, primed `24`,
    /// continue pending `25`, softlock warp `26`); `1..16` and `27..32`
    /// free.
    pub flags: u32,
    /// General-purpose bytes (document the bytes you claim in your module
    /// docs). Owners: `5`, `6` fixes (last frame's `$074C` / `$0736`),
    /// `7` qol (Life Dolls collected), `8..=15` abilities (visited towns,
    /// flute-warp cursor, last game mode, rescue-fairy safe ground and
    /// state); `0..5` free.
    pub scratch: [u8; 16],
}

impl EnhState {
    /// Little-endian encoding (append-only; see the module docs).
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.frames.to_le_bytes());
        out.extend_from_slice(&self.continue_loc);
        out.push(u8::from(self.continue_valid));
        for t in self.timers {
            out.extend_from_slice(&t.to_le_bytes());
        }
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.scratch);
    }

    /// Bytes of [`Self::write`] as a fresh vector.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(64);
        self.write(&mut v);
        v
    }

    /// Decode [`Self::write`]. A shorter blob (written before a field was
    /// appended) leaves the missing trailing fields at their default; extra
    /// trailing bytes (written by a newer build) are ignored.
    ///
    /// # Errors
    /// A blob that ends in the middle of a field, or a bad flag byte.
    pub fn read(bytes: &[u8]) -> Result<Self, &'static str> {
        fn take<'a>(r: &mut &'a [u8], n: usize) -> Result<Option<&'a [u8]>, &'static str> {
            if r.is_empty() {
                return Ok(None);
            }
            if r.len() < n {
                return Err("enhancement state");
            }
            let (h, t) = r.split_at(n);
            *r = t;
            Ok(Some(h))
        }
        let mut s = Self::default();
        let mut r = bytes;
        let mut take = |n: usize| take(&mut r, n);
        if let Some(b) = take(8)? {
            s.frames = u64::from_le_bytes(b.try_into().map_err(|_| "enhancement state")?);
        }
        if let Some(b) = take(4)? {
            s.continue_loc.copy_from_slice(b);
        }
        if let Some(b) = take(1)? {
            s.continue_valid = match b[0] {
                0 => false,
                1 => true,
                _ => return Err("enhancement flag"),
            };
        }
        for i in 0..s.timers.len() {
            if let Some(b) = take(2)? {
                s.timers[i] = u16::from_le_bytes([b[0], b[1]]);
            }
        }
        if let Some(b) = take(4)? {
            s.flags = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        if let Some(b) = take(16)? {
            s.scratch.copy_from_slice(b);
        }
        Ok(s)
    }

    /// FNV-1a over [`Self::write`] (netplay desync hash input).
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.to_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

// ------------------------------------------------------------ hook registry

/// One trap [`hook`] installed: the address and what was registered there
/// before (restored by [`clear`]).
#[derive(Debug, Clone, Copy)]
struct SavedTrap {
    addr: u16,
    prev: Option<TrapInfo>,
}

/// One [`patch_prg`]: the PRG image offset and the original bytes.
#[derive(Debug, Clone)]
struct SavedPatch {
    offset: usize,
    original: Vec<u8>,
}

/// What the enhancements changed in a `Game` (traps, PRG bytes), so
/// [`apply`] can undo it. Static configuration like the trap table: not part
/// of a save state.
#[derive(Debug, Clone, Default)]
pub struct EnhHooks {
    traps: Vec<SavedTrap>,
    patches: Vec<SavedPatch>,
}

impl EnhHooks {
    /// Number of traps currently hooked by enhancements.
    #[must_use]
    pub fn trap_count(&self) -> usize {
        self.traps.len()
    }

    /// Number of PRG patches currently applied by enhancements.
    #[must_use]
    pub fn patch_count(&self) -> usize {
        self.patches.len()
    }
}

/// Register `func` as the trap at CPU address `addr` (ledger `bank`),
/// remembering the entry it replaces so [`clear`] restores it and
/// [`call_original`] can still run it.
///
/// `cycles`: `None` inherits the replaced trap's cost (or
/// [`crate::traps::default_trap_cycles`] when there was none). Hooking the
/// same address from two groups chains: the later hook's
/// [`call_original`] runs the earlier hook.
pub fn hook(
    game: &mut Game,
    name: &'static str,
    bank: Option<u8>,
    addr: u16,
    func: TrapFn,
    cycles: Option<u64>,
) {
    let prev = game.traps.get(addr).copied();
    let cycles = cycles.unwrap_or_else(|| {
        prev.map_or_else(|| crate::traps::default_trap_cycles(addr), |p| p.cycles)
    });
    game.enh_hooks.traps.push(SavedTrap { addr, prev });
    game.traps
        .register_with_cycles(name, bank, addr, func, cycles);
}

/// The trap body a [`hook`] at `addr` replaced, if there was one.
#[must_use]
pub fn prev_trap(game: &Game, addr: u16) -> Option<TrapFn> {
    game.enh_hooks
        .traps
        .iter()
        .rev()
        .find(|t| t.addr == addr)
        .and_then(|t| t.prev)
        .map(|t| t.func)
}

/// From inside a hook body at `addr`: run what the hook replaced — the
/// previous trap's body, or, when there was none, the original ROM routine
/// interpreted until its `RTS` (the hook is bypassed for that one call).
pub fn call_original(game: &mut Game, addr: u16) {
    if let Some(f) = prev_trap(game, addr) {
        f(game);
    } else {
        game.traps.set_untrapped(addr, true);
        game.call_asm(addr);
        game.traps.set_untrapped(addr, false);
    }
}

/// Overwrite `bytes` at `offset` in the in-memory PRG image (never the ROM
/// file), remembering the originals for [`clear`]. Returns `false` (and
/// changes nothing) when the range is outside the image.
pub fn patch_prg(game: &mut Game, offset: usize, bytes: &[u8]) -> bool {
    let end = offset + bytes.len();
    if end > game.prg.len() {
        return false;
    }
    let original = game.prg[offset..end].to_vec();
    game.enh_hooks.patches.push(SavedPatch { offset, original });
    game.prg[offset..end].copy_from_slice(bytes);
    true
}

/// Offset in a PRG image of `prg_len` bytes of 16 KiB `bank` at CPU
/// address `cpu_addr`. Bank 7 means **the fixed bank** (`$C000-$FFFF`),
/// which is the last bank of the image: bank 7 in the vanilla 128 KiB
/// layout, bank 15 in the 256 KiB layout the `z2-rando` ROM randomizer may
/// expand to (banks 0-6 do not move).
#[must_use]
pub const fn bank_offset(prg_len: usize, bank: u8, cpu_addr: u16) -> usize {
    let bank = if bank == 7 && prg_len > 8 * 0x4000 {
        prg_len / 0x4000 - 1
    } else {
        bank as usize
    };
    bank * 0x4000 + (cpu_addr & 0x3FFF) as usize
}

/// [`patch_prg`] by 16 KiB `bank` and CPU address (`$8000-$BFFF` for a
/// switchable bank, `$C000-$FFFF` for the fixed bank 7; see
/// [`bank_offset`]).
pub fn patch_prg_at(game: &mut Game, bank: u8, cpu_addr: u16, bytes: &[u8]) -> bool {
    let offset = bank_offset(game.prg.len(), bank, cpu_addr);
    patch_prg(game, offset, bytes)
}

/// Undo every [`hook`] and [`patch_prg`] (latest first). Leaves the options
/// and [`EnhState`] alone.
pub fn clear(game: &mut Game) {
    while let Some(t) = game.enh_hooks.traps.pop() {
        match t.prev {
            Some(p) => game
                .traps
                .register_with_cycles(p.name, p.bank, p.addr, p.func, p.cycles),
            None => {
                game.traps.unregister(t.addr);
            }
        }
    }
    while let Some(p) = game.enh_hooks.patches.pop() {
        let end = p.offset + p.original.len();
        if end <= game.prg.len() {
            game.prg[p.offset..end].copy_from_slice(&p.original);
        }
    }
}

// ------------------------------------------------------------ dispatch

/// [`Game::set_enhancements`]: undo whatever the previous options hooked,
/// store `e`, then let each group with something on register its hooks.
/// Call it after the default trap groups (and co-op / wide gameplay) are
/// registered, ideally before `Game::reset`. Never called by verification.
pub fn apply(game: &mut Game, e: &Enhancements) {
    clear(game);
    game.enh = *e;
    game.enh_on = e.any_gameplay_active();
    if !game.enh_on {
        return;
    }
    if e.text.is_active() {
        text::register(game, &e.text);
    }
    if e.qol.is_active() {
        qol::register(game, &e.qol);
    }
    if e.fixes.is_active() {
        fixes::register(game, &e.fixes);
    }
    if e.enemies.is_active() {
        enemies::register(game, &e.enemies);
    }
    if e.abilities.is_active() {
        abilities::register(game, &e.abilities);
    }
    if e.rando.is_active() {
        rando::register(game, &e.rando);
    }
    if e.cheats.is_active() {
        cheats::register(game, &e.cheats);
    }
}

/// End-of-frame hook ([`Game::step`], only while any enhancement is on):
/// advance [`EnhState::frames`], then run each active group's
/// `end_of_frame`.
pub(crate) fn end_of_frame(game: &mut Game) {
    game.enh_state.frames = game.enh_state.frames.wrapping_add(1);
    let e = game.enh;
    if e.text.is_active() {
        text::end_of_frame(game, &e.text);
    }
    if e.qol.is_active() {
        qol::end_of_frame(game, &e.qol);
    }
    if e.fixes.is_active() {
        fixes::end_of_frame(game, &e.fixes);
    }
    if e.enemies.is_active() {
        enemies::end_of_frame(game, &e.enemies);
    }
    if e.abilities.is_active() {
        abilities::end_of_frame(game, &e.abilities);
    }
    if e.rando.is_active() {
        rando::end_of_frame(game, &e.rando);
    }
    if e.cheats.is_active() {
        cheats::end_of_frame(game, &e.cheats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        let e = Enhancements::default();
        assert!(!e.any_gameplay_active());
        assert!(!e.is_gameplay_active());
        assert_eq!(e, Enhancements::off());
        assert!(DisplayEnh::default().is_default());
    }

    #[test]
    fn identity_is_empty_when_off_and_stable_when_on() {
        assert!(Enhancements::default().identity_bytes().is_empty());
        let z = Enhancements::zalia_preset();
        assert!(z.any_gameplay_active());
        let id = z.identity_bytes();
        assert_eq!(id[0], IDENTITY_VERSION);
        assert_eq!(id, Enhancements::zalia_preset().identity_bytes());
        let mut other = z;
        other.text.dialogue_speed = 3;
        assert_ne!(other.identity_bytes(), id);
        // Each group on its own produces a distinct, non-empty identity.
        let mut seen = std::collections::HashSet::new();
        for g in 0..7 {
            let mut e = Enhancements::default();
            match g {
                0 => e.text.protect_spell_name = true,
                1 => e.qol.lives_from_dolls = true,
                2 => e.fixes.xp_drain_fix = true,
                3 => e.enemies.barba_aim = true,
                4 => e.abilities.double_jump = true,
                5 => e.rando.item_shuffle = true,
                _ => e.cheats.invincible = true,
            }
            assert!(e.any_gameplay_active());
            assert!(seen.insert(e.identity_bytes()));
        }
    }

    #[test]
    fn json_round_trip_and_partial_files() {
        let z = Enhancements::zalia_preset();
        assert_eq!(Enhancements::from_json(&z.to_json()).unwrap(), z);
        let pretty = serde_json::to_string_pretty(&z).unwrap();
        assert_eq!(Enhancements::from_json(&pretty).unwrap(), z);
        assert_eq!(
            Enhancements::from_json("{}").unwrap(),
            Enhancements::default()
        );
        let e = Enhancements::from_json(
            r#"{"text":{"dialogue_speed":5},"qol":{"continue_from":"last_town"},"future":1}"#,
        )
        .unwrap();
        assert_eq!(e.text.dialogue_speed, 5);
        assert!(!e.text.protect_spell_name);
        assert_eq!(e.qol.continue_from, ContinueFrom::LastTown);
        assert!(Enhancements::from_json("{\"text\":3}").is_err());
        let d = DisplayEnh::zalia_preset();
        let back: DisplayEnh = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        let partial: DisplayEnh = serde_json::from_str(r#"{"flash_color":"red"}"#).unwrap();
        assert_eq!(partial.flash_color, FlashColor::Red);
        assert_eq!(partial.music_volume, DisplayEnh::default().music_volume);
    }

    #[test]
    fn enh_state_round_trips_and_tolerates_short_blobs() {
        let mut s = EnhState {
            frames: 0x1234_5678_9abc,
            continue_loc: [1, 2, 3, 4],
            continue_valid: true,
            flags: 0xdead_beef,
            ..EnhState::default()
        };
        s.timers[3] = 777;
        s.scratch[15] = 9;
        let b = s.to_bytes();
        assert_eq!(EnhState::read(&b).unwrap(), s);
        assert_eq!(EnhState::read(&[]).unwrap(), EnhState::default());
        // A blob cut at a field boundary decodes the prefix.
        let short = EnhState::read(&b[..8]).unwrap();
        assert_eq!(short.frames, s.frames);
        assert!(!short.continue_valid);
        // Cut inside a field: error.
        assert!(EnhState::read(&b[..5]).is_err());
        // Newer, longer blob: trailing bytes ignored.
        let mut long = b.clone();
        long.extend_from_slice(&[1, 2, 3]);
        assert_eq!(EnhState::read(&long).unwrap(), s);
        assert_ne!(s.hash(), EnhState::default().hash());
    }

    #[test]
    fn apply_with_everything_off_touches_nothing() {
        let mut g = Game::new();
        let traps = g.traps.len();
        let prg = g.prg.clone();
        g.set_enhancements(Enhancements::default());
        assert_eq!(g.traps.len(), traps);
        assert_eq!(g.prg, prg);
        assert!(!g.enh_active());
        g.set_enhancements(Enhancements::zalia_preset());
        assert!(g.enh_active());
        g.set_enhancements(Enhancements::default());
        assert!(!g.enh_active());
        assert_eq!(g.traps.len(), traps);
        assert_eq!(g.prg, prg);
    }

    fn dummy_a(game: &mut Game) {
        game.ram[0x10] = game.ram[0x10].wrapping_add(1);
    }
    fn dummy_b(game: &mut Game) {
        call_original(game, 0x9000);
        game.ram[0x11] = game.ram[0x11].wrapping_add(1);
    }

    #[test]
    fn hooks_chain_and_restore() {
        let mut g = Game::new();
        g.trap_register_cycles("base", Some(0), 0x9000, dummy_a, 42);
        hook(&mut g, "enh_test_b", Some(0), 0x9000, dummy_b, None);
        assert_eq!(g.traps.get(0x9000).unwrap().cycles, 42, "inherits cost");
        assert_eq!(g.traps.get(0x9000).unwrap().name, "enh_test_b");
        (g.traps.get(0x9000).unwrap().func)(&mut g);
        assert_eq!((g.ram[0x10], g.ram[0x11]), (1, 1));
        hook(&mut g, "enh_test_new", None, 0x9100, dummy_a, Some(7));
        assert!(patch_prg_at(&mut g, 0, 0x8010, &[0xEA, 0xEA]));
        assert_eq!(&g.prg[0x10..0x12], &[0xEA, 0xEA]);
        assert!(!patch_prg(&mut g, usize::MAX / 2, &[1]));
        assert_eq!(g.enh_hooks.trap_count(), 2);
        clear(&mut g);
        assert_eq!(g.traps.get(0x9000).unwrap().name, "base");
        assert!(g.traps.get(0x9100).is_none());
        assert_eq!(&g.prg[0x10..0x12], &[0, 0]);
        assert_eq!(g.enh_hooks.patch_count(), 0);
    }
}
