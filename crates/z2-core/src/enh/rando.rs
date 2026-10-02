//! Randomizer and start loadout (group `R`).
//!
//! Options ([`RandoOpts`]) and their ZALiA reference behaviour (ZALiA sets
//! these up at File Select -> RANDO, with an editable seed):
//!
//! * `seed`: randomizer seed for the shuffles and per-entry variance below.
//!   Every random choice is a pure function of the seed (a splitmix64 mix of
//!   `seed`, a table id and an entry index, see [`mix`]), so netplay peers
//!   with the same options build byte-identical patches without exchanging
//!   anything.
//! * `start_attack` / `start_magic` / `start_life` (`0` = original level 1,
//!   else `1..=8`): starting levels (ZALiA START ATTACK/MAGIC/LIFE).
//! * `start_spells`: bitmask of spells known at start, bit 0..7 = SHIELD,
//!   JUMP, LIFE, FAIRY, FIRE, REFLECT, SPELL, THUNDER (ZALiA START SPELLS).
//! * `start_items`: bitmask of items owned at start, bit 0..7 = CANDLE,
//!   GLOVE, RAFT, BOOTS, FLUTE, CROSS, HAMMER, MAGIC KEY; bit 8 =
//!   downward thrust, bit 9 = upward thrust (ZALiA START ITEMS/SKILLS).
//! * `start_containers_heart` / `start_containers_magic` (`0` = original 4,
//!   else `1..=8`): starting container counts.
//! * `enemy_hp_pct`, `enemy_dmg_pct`, `xp_pct`, `level_cost_pct`,
//!   `spell_cost_pct` (`-25..=25`, `0` = original): percentage scaling
//!   (ZALiA ENEMY HP/DAMAGE +/-25%, XP and LEVEL COSTS +/-25%, SPELL COSTS
//!   -25%..+10%).
//! * `palette_rando` (`0` off, `1` Link and palaces, `2` all scenes; stable
//!   per seed): ZALiA PALETTE RANDO.
//! * `item_shuffle`: shuffle the eight major items among their vanilla
//!   locations, logic-checked (ZALiA ITEMS, reduced to the major items).
//!
//! Out-of-range values are clamped ([`RandoOpts::clamped`]); the netplay
//! identity still encodes the raw values.
//!
//! # Start loadout (new game)
//!
//! A new file in RAM looks exactly like `bank5_Beginning_Values` (bank 5
//! `$BAE3`, copied to `$0777` onwards): in the original game levels 1/1/1,
//! no XP, no spells or items, 4/4 containers, no thrust techniques, first
//! quest. [`end_of_frame`] watches for that "pristine" image — compared
//! against the beginning values **in the running PRG** ([`ram_is_new_file_for`]),
//! so a ROM the `z2-rando` ROM randomizer gave other start values is still
//! detected — and, when the loadout differs from it, writes the
//! loadout over it: levels `$0777-$0779`, spells `$077B-$0782`, containers
//! `$0783/$0784` (plus the seven-magic-container bit `$079D & $08` the game
//! sets at seven, so New Kasuto behaves), items `$0785-$078C`, thrusts
//! `$0796` (`$10` down, `$04` up), refills both meters the way `LCB18`
//! does and recomputes "exp to next level" `$0770/$0771` the way
//! `update_next_level_exp` (bank 0 `$A057`) does. The file is no longer
//! pristine afterwards, so this runs once per new game; when the player
//! saves, the game copies the loadout to SRAM like any other progress.
//! (At power-on the RAM also holds the beginning image, so the loadout is
//! written there too; selecting a file reloads it from SRAM and a new file
//! is detected again. No RAM is touched while a non-pristine file is
//! loaded.)
//!
//! # Scalers (PRG patches at `register`)
//!
//! Each entry gets its own seeded percentage between half and all of the
//! requested one (same sign), like ZALiA's per-enemy variance:
//! `+20` scales every entry by `+10%..+20%`. Results are rounded and
//! clamped to the ranges below; zero entries stay zero.
//!
//! | option | table | range |
//! |---|---|---|
//! | `enemy_hp_pct` | enemy HP, banks 1-5 `$9421` (36 ids; the game copies it to WRAM `$6D21` on a world load) | `1..=$EF`; entries `>= $F0` (special/boss markers) untouched |
//! | `enemy_dmg_pct` | Link damage, bank 7 `$E2AF` (`LE2AE[class*8 + life level]`, classes 0-6) | `1..=$FE`; class 7 (`$FF`, instant kill) untouched |
//! | `xp_pct` | enemy XP, bank 7 `$DDC0` lo / `$DDDC` hi (16 entries) | `1..=9999`; entry 0 stays 0 |
//! | `level_cost_pct` | level-up XP, bank 0 `$9659` hi / `$9671` lo (3 stats x 8) | `1..=8999`, non-decreasing per stat; the level-8 cap (9000) untouched |
//! | `spell_cost_pct` | MP cost, bank 0 `$8D7B` (8 spells x 8 levels) | `1..=$F0` |
//!
//! The enemy HP copy in WRAM refreshes on the next world load, so changing
//! the options mid-area takes effect after the next area transition.
//!
//! # Palette randomizer
//!
//! Rotates the hue of every chromatic NES colour (hues `1..=12`; greys and
//! blacks stay) by a seeded step per palette region, keeping brightness:
//!
//! * `1`: Link's sprite palette (`18 36 2A` in banks 0-5 and the fixed
//!   bank; one rotation everywhere) and the palace palettes (banks 4/5
//!   `$800E-$809D` area sets, bank 4 `$8470-$84CF`, bank 5 `$84D0-$84DF`).
//! * `2`: also the overworld (bank 7 `$C45B-$C46A`) and the town / field
//!   area sets (banks 1-3 `$800E-$809D`).
//!
//! Every Link site is patched only when it holds the vanilla bytes.
//!
//! # Item shuffle
//!
//! Locations are named by the item they hold in the original game; the
//! shuffle is a permutation between them. The pickup routine
//! (`bank7_get_item`, bank 7 `$E771`; the only writer of `$0785-$078C`) is
//! hooked: when the item object's code (`$AF,x & $7F`, `x = $10`) is one of
//! the eight major items, the hook substitutes the shuffled item for the
//! call and puts the code back afterwards. The object still *looks* like
//! the vanilla item (sprite not remapped). Items granted by `start_items`
//! stay out of the shuffle (their locations keep the vanilla item, a
//! harmless duplicate).
//!
//! Logic: a placement is accepted only if every location is reachable by a
//! fixed-point sweep over this requirement graph (items only; spells come
//! from towns whose access the graph's item sets already cover). It is
//! deliberately **conservative** — a superset of what the original game
//! needs — so an accepted seed is beatable; the vanilla placement always
//! passes.
//!
//! | location (vanilla item) | requires |
//! |---|---|
//! | Parapa Palace, P1 (candle) | - |
//! | Midoro Palace, P2 (glove) | - |
//! | Island Palace, P3 (raft) | hammer (boulder on the way), glove |
//! | Maze Island Palace, P4 (boots) | raft (east Hyrule), glove |
//! | Palace on the Sea, P5 (flute) | raft, boots (walk on water), glove |
//! | Three-Eye Rock Palace, P6 (cross) | raft, hammer, flute, boots, glove, magic key |
//! | Death Mountain (hammer) | candle (dark caves) |
//! | New Kasuto (magic key) | raft, hammer (forest), flute (river devil), boots |
//!
//! Up to [`SHUFFLE_ATTEMPTS`] seeded Fisher-Yates permutations are tried in
//! order; the first that passes wins (the vanilla placement if none does).
//! [`spoiler`] prints the result.

use serde::{Deserialize, Serialize};

use crate::game::Game;

/// Largest start level / container count.
pub const MAX_START_LEVEL: u8 = 8;
/// Bound of the percentage scalers (`-25..=25`).
pub const MAX_SCALE_PCT: i8 = 25;
/// Largest [`RandoOpts::palette_rando`].
pub const MAX_PALETTE_RANDO: u8 = 2;
/// Permutations tried by the item shuffle before falling back to vanilla.
pub const SHUFFLE_ATTEMPTS: u32 = 4096;

/// Randomizer and start-loadout options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RandoOpts {
    /// Shuffle seed.
    pub seed: u64,
    /// Starting attack level (`0` = original).
    pub start_attack: u8,
    /// Starting magic level (`0` = original).
    pub start_magic: u8,
    /// Starting life level (`0` = original).
    pub start_life: u8,
    /// Spells known at start (bitmask).
    pub start_spells: u8,
    /// Items and skills owned at start (bitmask).
    pub start_items: u16,
    /// Starting heart containers (`0` = original).
    pub start_containers_heart: u8,
    /// Starting magic containers (`0` = original).
    pub start_containers_magic: u8,
    /// Enemy HP scaling, percent (`-25..=25`).
    pub enemy_hp_pct: i8,
    /// Enemy damage scaling, percent.
    pub enemy_dmg_pct: i8,
    /// XP gain scaling, percent.
    pub xp_pct: i8,
    /// Level-up cost scaling, percent.
    pub level_cost_pct: i8,
    /// Spell MP cost scaling, percent.
    pub spell_cost_pct: i8,
    /// Palette randomizer (`0..=2`).
    pub palette_rando: u8,
    /// Shuffle item locations.
    pub item_shuffle: bool,
}

impl RandoOpts {
    /// Whether anything in this group is on. A seed on its own changes
    /// nothing, so it does not count.
    #[must_use]
    pub fn is_active(&self) -> bool {
        Self { seed: 0, ..*self } != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&[
            self.start_attack,
            self.start_magic,
            self.start_life,
            self.start_spells,
        ]);
        out.extend_from_slice(&self.start_items.to_le_bytes());
        out.extend_from_slice(&[
            self.start_containers_heart,
            self.start_containers_magic,
            self.enemy_hp_pct as u8,
            self.enemy_dmg_pct as u8,
            self.xp_pct as u8,
            self.level_cost_pct as u8,
            self.spell_cost_pct as u8,
            self.palette_rando,
            u8::from(self.item_shuffle),
        ]);
    }

    /// The options with every value forced into its documented range:
    /// levels and containers `0..=8`, percentages `-25..=25`, palette
    /// `0..=2`, items bits `0..=9`.
    #[must_use]
    pub fn clamped(&self) -> Self {
        let lvl = |v: u8| v.min(MAX_START_LEVEL);
        let pct = |v: i8| v.clamp(-MAX_SCALE_PCT, MAX_SCALE_PCT);
        Self {
            seed: self.seed,
            start_attack: lvl(self.start_attack),
            start_magic: lvl(self.start_magic),
            start_life: lvl(self.start_life),
            start_spells: self.start_spells,
            start_items: self.start_items & 0x03FF,
            start_containers_heart: lvl(self.start_containers_heart),
            start_containers_magic: lvl(self.start_containers_magic),
            enemy_hp_pct: pct(self.enemy_hp_pct),
            enemy_dmg_pct: pct(self.enemy_dmg_pct),
            xp_pct: pct(self.xp_pct),
            level_cost_pct: pct(self.level_cost_pct),
            spell_cost_pct: pct(self.spell_cost_pct),
            palette_rando: self.palette_rando.min(MAX_PALETTE_RANDO),
            item_shuffle: self.item_shuffle,
        }
    }

    /// Whether the new-game loadout changes anything.
    #[must_use]
    pub fn has_loadout(&self) -> bool {
        let o = self.clamped();
        let differs = |v: u8, og: u8| v != 0 && v != og;
        differs(o.start_attack, 1)
            || differs(o.start_magic, 1)
            || differs(o.start_life, 1)
            || differs(o.start_containers_heart, 4)
            || differs(o.start_containers_magic, 4)
            || o.start_spells != 0
            || o.start_items != 0
    }
}

// ------------------------------------------------------------ PRNG

/// splitmix64 finaliser (Steele/Lea/Flood). Bijective on `u64`.
#[must_use]
pub const fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic random word for entry `index` of table `domain` under
/// `seed` (order-independent, so every table entry draws its own value).
#[must_use]
pub const fn mix(seed: u64, domain: u64, index: u64) -> u64 {
    splitmix64(splitmix64(seed ^ splitmix64(domain)) ^ index)
}

/// Small sequential generator (splitmix64 stream) for the shuffle.
#[derive(Debug, Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // `splitmix64` adds the stream increment itself.
        let out = splitmix64(self.0);
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        out
    }

    /// Uniform in `0..n` (`n > 0`; modulo bias is irrelevant at n <= 8).
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// Table ids for `mix` (stable: they shape every seed's output).
const DOM_HP: u64 = 0x4850; // "HP"
const DOM_DMG: u64 = 0x0044_4D47; // "DMG"
const DOM_XP: u64 = 0x5850; // "XP"
const DOM_LVL: u64 = 0x004C_564C; // "LVL"
const DOM_MP: u64 = 0x4D50; // "MP"
const DOM_PAL: u64 = 0x0050_414C; // "PAL"
const DOM_SHUF: u64 = 0x5348_5546; // "SHUF"

/// Seeded percentage for one entry: between half and all of `pct`, same
/// sign (`0` for `pct == 0`).
#[must_use]
pub fn entry_pct(seed: u64, domain: u64, index: u64, pct: i8) -> i32 {
    let p = i32::from(pct.clamp(-MAX_SCALE_PCT, MAX_SCALE_PCT));
    if p == 0 {
        return 0;
    }
    let lo = p / 2;
    let span = (p - lo).unsigned_abs() + 1;
    let r = (mix(seed, domain, index) % u64::from(span)) as i32;
    if p > 0 {
        lo + r
    } else {
        lo - r
    }
}

/// `v * (100 + pct) / 100`, rounded half up, clamped to `lo..=hi`. Zero
/// stays zero.
#[must_use]
pub fn scale_value(v: u32, pct: i32, lo: u32, hi: u32) -> u32 {
    if v == 0 {
        return 0;
    }
    let num = i64::from(v) * i64::from(100 + pct);
    let r = ((num + 50) / 100).max(0) as u32;
    r.clamp(lo, hi)
}

// ------------------------------------------------------------ PRG tables

/// Offset of `(bank, cpu_addr)` in the PRG image.
#[must_use]
pub const fn prg_offset(bank: u8, cpu_addr: u16) -> usize {
    bank as usize * 0x4000 + (cpu_addr & 0x3FFF) as usize
}

/// Banks holding a copy of the enemy tables (`$9400` block).
pub const ENEMY_BANKS: [u8; 5] = [1, 2, 3, 4, 5];
/// Enemy HP table (`$9421`, copied to WRAM `$6D21`).
pub const ENEMY_HP_ADDR: u16 = 0x9421;
/// Enemy ids per table.
pub const ENEMY_COUNT: usize = 36;
/// Link damage table entries (`LE2AE[class*8 + level]`), classes 0-6.
pub const DAMAGE_ADDR: u16 = 0xE2AF;
/// Damage bytes scaled (7 classes x 8 levels; class 7 is instant kill).
pub const DAMAGE_LEN: usize = 56;
/// XP low / high tables (bank 7).
pub const XP_LO_ADDR: u16 = 0xDDC0;
/// XP high bytes.
pub const XP_HI_ADDR: u16 = 0xDDDC;
/// XP entries.
pub const XP_COUNT: usize = 16;
/// Level-up cost high / low tables (bank 0, `stat*8 + level-1`).
pub const LEVEL_HI_ADDR: u16 = 0x9659;
/// Level-up cost low bytes.
pub const LEVEL_LO_ADDR: u16 = 0x9671;
/// Spell MP cost table (bank 0, `spell*8 + level-1`).
pub const SPELL_COST_ADDR: u16 = 0x8D7B;

/// One planned PRG write: image offset and bytes.
pub type Patch = (usize, Vec<u8>);

/// [`prg_offset`] in this image: bank 7 is the fixed (last) bank, so the
/// 256 KiB layout of an expanded ROM randomizer seed is patched in the right
/// place ([`super::bank_offset`]).
fn bank_off(prg: &[u8], bank: u8, cpu_addr: u16) -> usize {
    super::bank_offset(prg.len(), bank, cpu_addr)
}

fn read(prg: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    prg.get(off..off + len)
}

fn push_if_changed(out: &mut Vec<Patch>, prg: &[u8], off: usize, bytes: Vec<u8>) {
    if read(prg, off, bytes.len()).is_some_and(|o| o != bytes.as_slice()) {
        out.push((off, bytes));
    }
}

fn hp_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    for bank in ENEMY_BANKS {
        let off = bank_off(prg, bank, ENEMY_HP_ADDR);
        let Some(orig) = read(prg, off, ENEMY_COUNT) else {
            continue;
        };
        let new: Vec<u8> = orig
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                if v >= 0xF0 {
                    return v;
                }
                let p = entry_pct(
                    o.seed,
                    DOM_HP,
                    u64::from(bank) << 8 | i as u64,
                    o.enemy_hp_pct,
                );
                scale_value(u32::from(v), p, 1, 0xEF) as u8
            })
            .collect();
        push_if_changed(out, prg, off, new);
    }
}

fn dmg_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    let off = bank_off(prg, 7, DAMAGE_ADDR);
    let Some(orig) = read(prg, off, DAMAGE_LEN) else {
        return;
    };
    let new: Vec<u8> = orig
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            if v == 0xFF {
                return v;
            }
            let p = entry_pct(o.seed, DOM_DMG, i as u64, o.enemy_dmg_pct);
            scale_value(u32::from(v), p, 1, 0xFE) as u8
        })
        .collect();
    push_if_changed(out, prg, off, new);
}

fn xp_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    let (lo_off, hi_off) = (bank_off(prg, 7, XP_LO_ADDR), bank_off(prg, 7, XP_HI_ADDR));
    let (Some(lo), Some(hi)) = (read(prg, lo_off, XP_COUNT), read(prg, hi_off, XP_COUNT)) else {
        return;
    };
    let mut nlo = lo.to_vec();
    let mut nhi = hi.to_vec();
    for i in 0..XP_COUNT {
        let v = u32::from(hi[i]) << 8 | u32::from(lo[i]);
        let p = entry_pct(o.seed, DOM_XP, i as u64, o.xp_pct);
        let n = scale_value(v, p, 1, 9999);
        nlo[i] = n as u8;
        nhi[i] = (n >> 8) as u8;
    }
    push_if_changed(out, prg, lo_off, nlo);
    push_if_changed(out, prg, hi_off, nhi);
}

fn level_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    let (hi_off, lo_off) = (
        bank_off(prg, 0, LEVEL_HI_ADDR),
        bank_off(prg, 0, LEVEL_LO_ADDR),
    );
    let (Some(hi), Some(lo)) = (read(prg, hi_off, 24), read(prg, lo_off, 24)) else {
        return;
    };
    let mut nhi = hi.to_vec();
    let mut nlo = lo.to_vec();
    for stat in 0..3 {
        let mut floor = 1u32;
        // Entries 0..7 are the costs of levels 2..8; entry 7 is the cap.
        for l in 0..7 {
            let i = stat * 8 + l;
            let v = u32::from(hi[i]) << 8 | u32::from(lo[i]);
            let p = entry_pct(o.seed, DOM_LVL, i as u64, o.level_cost_pct);
            let n = scale_value(v, p, 1, 8999).max(floor);
            floor = n;
            nhi[i] = (n >> 8) as u8;
            nlo[i] = n as u8;
        }
    }
    push_if_changed(out, prg, hi_off, nhi);
    push_if_changed(out, prg, lo_off, nlo);
}

fn spell_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    let off = bank_off(prg, 0, SPELL_COST_ADDR);
    let Some(orig) = read(prg, off, 64) else {
        return;
    };
    let new: Vec<u8> = orig
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let p = entry_pct(o.seed, DOM_MP, i as u64, o.spell_cost_pct);
            scale_value(u32::from(v), p, 1, 0xF0) as u8
        })
        .collect();
    push_if_changed(out, prg, off, new);
}

/// Rotate a chromatic NES colour's hue by `step` (`1..=11`); greys,
/// blacks and non-colour bytes (`>= $40`) are returned unchanged.
#[must_use]
pub const fn rotate_hue(c: u8, step: u8) -> u8 {
    if c >= 0x40 {
        return c;
    }
    let hue = c & 0x0F;
    if hue == 0 || hue > 12 {
        return c;
    }
    let h = (hue - 1 + step % 12) % 12 + 1;
    (c & 0x30) | h
}

/// Seeded hue step (`1..=11`) for palette region `region`.
#[must_use]
pub fn hue_step(seed: u64, region: u64) -> u8 {
    (mix(seed, DOM_PAL, region) % 11) as u8 + 1
}

/// Link's vanilla sprite palette.
pub const LINK_PALETTE: [u8; 3] = [0x18, 0x36, 0x2A];

/// Candidate Link palette sites `(bank, cpu_addr)`; patched only when they
/// hold [`LINK_PALETTE`].
pub const LINK_PALETTE_SITES: [(u8, u16); 24] = [
    (0, 0xA84A),
    (1, 0x809F),
    (1, 0x80AF),
    (1, 0x80BF),
    (1, 0x80CF),
    (2, 0x809F),
    (2, 0x80AF),
    (2, 0x80BF),
    (2, 0x80CF),
    (3, 0x809F),
    (3, 0x80AF),
    (3, 0x80BF),
    (3, 0x80CF),
    (3, 0x80DF),
    (4, 0x809F),
    (4, 0x80AF),
    (4, 0x80BF),
    (4, 0x80CF),
    (5, 0x809F),
    (5, 0x80AF),
    (5, 0x80BF),
    (5, 0x80CF),
    (7, 0xC454),
    (7, 0xC46C),
];

/// Area palette sets (`$800E`, 9 x 16 bytes) per bank.
const AREA_SETS_ADDR: u16 = 0x800E;
const AREA_SETS_LEN: usize = 9 * 16;

/// Palace-only palette blocks `(bank, addr, len)`.
const PALACE_BLOCKS: [(u8, u16, usize); 2] = [(4, 0x8470, 6 * 16), (5, 0x84D0, 16)];
/// Overworld background palette (bank 7, 16 bytes after the `3F 00 20`
/// PPU header at `$C458`).
const OVERWORLD_BG: (u8, u16, usize) = (7, 0xC45B, 16);

fn rotate_block(prg: &[u8], off: usize, len: usize, step: u8, out: &mut Vec<Patch>) {
    if let Some(orig) = read(prg, off, len) {
        let new: Vec<u8> = orig.iter().map(|&c| rotate_hue(c, step)).collect();
        push_if_changed(out, prg, off, new);
    }
}

fn palette_patches(prg: &[u8], o: &RandoOpts, out: &mut Vec<Patch>) {
    if o.palette_rando == 0 {
        return;
    }
    // Link: one rotation everywhere.
    let link = hue_step(o.seed, 0);
    let link_new = LINK_PALETTE.map(|c| rotate_hue(c, link));
    for (bank, addr) in LINK_PALETTE_SITES {
        let off = bank_off(prg, bank, addr);
        if read(prg, off, 3) == Some(&LINK_PALETTE[..]) {
            push_if_changed(out, prg, off, link_new.to_vec());
        }
    }
    // Area sets: palaces (banks 4/5) at level 1, every bank at level 2.
    let banks: &[u8] = if o.palette_rando >= 2 {
        &[1, 2, 3, 4, 5]
    } else {
        &[4, 5]
    };
    for &bank in banks {
        for set in 0..AREA_SETS_LEN / 16 {
            let step = hue_step(o.seed, 0x100 | u64::from(bank) << 4 | set as u64);
            let off = bank_off(prg, bank, AREA_SETS_ADDR) + set * 16;
            rotate_block(prg, off, 16, step, out);
        }
    }
    for (i, (bank, addr, len)) in PALACE_BLOCKS.into_iter().enumerate() {
        for set in 0..len / 16 {
            let step = hue_step(o.seed, 0x200 | (i as u64) << 4 | set as u64);
            let off = bank_off(prg, bank, addr) + set * 16;
            rotate_block(prg, off, 16, step, out);
        }
    }
    if o.palette_rando >= 2 {
        let (bank, addr, len) = OVERWORLD_BG;
        rotate_block(
            prg,
            bank_off(prg, bank, addr),
            len,
            hue_step(o.seed, 0x300),
            out,
        );
    }
}

/// Every PRG write the scalers and the palette randomizer make for `opts`
/// over the PRG image `prg` (a pure function: the same options and ROM
/// give the same patches on every peer). Writes that would not change a
/// byte are left out.
#[must_use]
pub fn prg_patches(prg: &[u8], opts: &RandoOpts) -> Vec<Patch> {
    let o = opts.clamped();
    let mut out = Vec::new();
    if o.enemy_hp_pct != 0 {
        hp_patches(prg, &o, &mut out);
    }
    if o.enemy_dmg_pct != 0 {
        dmg_patches(prg, &o, &mut out);
    }
    if o.xp_pct != 0 {
        xp_patches(prg, &o, &mut out);
    }
    if o.level_cost_pct != 0 {
        level_patches(prg, &o, &mut out);
    }
    if o.spell_cost_pct != 0 {
        spell_patches(prg, &o, &mut out);
    }
    palette_patches(prg, &o, &mut out);
    out
}

// ------------------------------------------------------------ item shuffle

/// Item / location indices (`$0785 + i`).
pub const CANDLE: usize = 0;
/// Handy glove.
pub const GLOVE: usize = 1;
/// Raft.
pub const RAFT: usize = 2;
/// Boots.
pub const BOOTS: usize = 3;
/// Flute.
pub const FLUTE: usize = 4;
/// Cross.
pub const CROSS: usize = 5;
/// Hammer.
pub const HAMMER: usize = 6;
/// Magic key.
pub const MAGIC_KEY: usize = 7;

/// Item names in `$0785` order.
pub const ITEM_NAMES: [&str; 8] = [
    "CANDLE",
    "GLOVE",
    "RAFT",
    "BOOTS",
    "FLUTE",
    "CROSS",
    "HAMMER",
    "MAGIC KEY",
];

/// Location names, indexed by the item they hold in the original game.
pub const LOCATION_NAMES: [&str; 8] = [
    "Parapa Palace (P1)",
    "Midoro Palace (P2)",
    "Island Palace (P3)",
    "Maze Island Palace (P4)",
    "Palace on the Sea (P5)",
    "Three-Eye Rock Palace (P6)",
    "Death Mountain",
    "New Kasuto",
];

const fn bit(i: usize) -> u8 {
    1 << i
}

/// Items each location needs (bitmask over [`ITEM_NAMES`]); see the module
/// docs for the reasoning. Conservative: a superset of the original game's
/// needs.
pub const LOCATION_REQUIRES: [u8; 8] = [
    0,
    0,
    bit(HAMMER) | bit(GLOVE),
    bit(RAFT) | bit(GLOVE),
    bit(RAFT) | bit(BOOTS) | bit(GLOVE),
    bit(RAFT) | bit(HAMMER) | bit(FLUTE) | bit(BOOTS) | bit(GLOVE) | bit(MAGIC_KEY),
    bit(CANDLE),
    bit(RAFT) | bit(HAMMER) | bit(FLUTE) | bit(BOOTS),
];

/// Whether every location is reachable when `placement[loc]` is the item
/// at `loc`, starting from the items in `start` (bitmask).
#[must_use]
pub fn placement_is_beatable(placement: &[u8; 8], start: u8) -> bool {
    let mut have = start;
    let mut done = 0u8;
    loop {
        let mut progressed = false;
        for loc in 0..8 {
            if done & bit(loc) == 0 && LOCATION_REQUIRES[loc] & !have == 0 {
                done |= bit(loc);
                have |= bit(usize::from(placement[loc] & 7));
                progressed = true;
            }
        }
        if done == 0xFF {
            return true;
        }
        if !progressed {
            return false;
        }
    }
}

/// The item shuffle: `placement[loc]` is the item found at location `loc`
/// (location = its vanilla item). Identity when `item_shuffle` is off.
#[must_use]
pub fn item_placement(opts: &RandoOpts) -> [u8; 8] {
    let vanilla = [0, 1, 2, 3, 4, 5, 6, 7];
    if !opts.item_shuffle {
        return vanilla;
    }
    let start = (opts.start_items & 0xFF) as u8;
    let pool: Vec<usize> = (0..8).filter(|&i| start & bit(i) == 0).collect();
    let mut rng = Rng(mix(opts.seed, DOM_SHUF, 0));
    for _ in 0..SHUFFLE_ATTEMPTS {
        let mut items = pool.clone();
        for i in (1..items.len()).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
        let mut placement = vanilla;
        for (&loc, &item) in pool.iter().zip(&items) {
            placement[loc] = item as u8;
        }
        if placement_is_beatable(&placement, start) {
            return placement;
        }
    }
    vanilla
}

/// Human-readable spoiler for `opts`: seed, item placement, start
/// loadout, scalers and palette mode (no ROM needed).
#[must_use]
pub fn spoiler(opts: &RandoOpts) -> String {
    use std::fmt::Write as _;
    let o = opts.clamped();
    let mut s = String::new();
    let _ = writeln!(s, "z2rs randomizer spoiler");
    let _ = writeln!(s, "seed: {}", o.seed);
    let _ = writeln!(
        s,
        "item shuffle: {}",
        if o.item_shuffle { "on" } else { "off" }
    );
    let placement = item_placement(&o);
    for (loc, &item) in placement.iter().enumerate() {
        let _ = writeln!(
            s,
            "  {:<28} {}",
            LOCATION_NAMES[loc],
            ITEM_NAMES[usize::from(item & 7)]
        );
    }
    let lvl = |v: u8, og: u8| if v == 0 { og } else { v };
    let _ = writeln!(
        s,
        "start levels: attack {} magic {} life {}",
        lvl(o.start_attack, 1),
        lvl(o.start_magic, 1),
        lvl(o.start_life, 1)
    );
    let _ = writeln!(
        s,
        "start containers: heart {} magic {}",
        lvl(o.start_containers_heart, 4),
        lvl(o.start_containers_magic, 4)
    );
    const SPELLS: [&str; 8] = [
        "SHIELD", "JUMP", "LIFE", "FAIRY", "FIRE", "REFLECT", "SPELL", "THUNDER",
    ];
    let spells: Vec<&str> = (0..8)
        .filter(|&i| o.start_spells & bit(i) != 0)
        .map(|i| SPELLS[i])
        .collect();
    let _ = writeln!(s, "start spells: {}", list(&spells));
    let mut items: Vec<&str> = (0..8)
        .filter(|&i| o.start_items & (1 << i) != 0)
        .map(|i| ITEM_NAMES[i])
        .collect();
    if o.start_items & 0x100 != 0 {
        items.push("DOWNWARD THRUST");
    }
    if o.start_items & 0x200 != 0 {
        items.push("UPWARD THRUST");
    }
    let _ = writeln!(s, "start items: {}", list(&items));
    let _ = writeln!(
        s,
        "scaling: enemy HP {:+}% enemy damage {:+}% XP {:+}% level costs {:+}% spell costs {:+}%",
        o.enemy_hp_pct, o.enemy_dmg_pct, o.xp_pct, o.level_cost_pct, o.spell_cost_pct
    );
    let _ = writeln!(
        s,
        "palettes: {}",
        match o.palette_rando {
            0 => "original",
            1 => "Link and palaces",
            _ => "all scenes",
        }
    );
    s
}

fn list(v: &[&str]) -> String {
    if v.is_empty() {
        "none".to_string()
    } else {
        v.join(", ")
    }
}

// ------------------------------------------------------------ RAM loadout

/// Levels (`$0777-$0779`).
const ADDR_LEVELS: usize = 0x0777;
/// Exp needed for the next level (BE `$0770/$0771`).
const ADDR_EXP_NEXT: usize = 0x0770;
/// Current MP / HP (`$0773` / `$0774`).
const ADDR_MP: usize = 0x0773;
const ADDR_HP: usize = 0x0774;
/// Experience (BE `$0775/$0776`).
const ADDR_EXP: usize = 0x0775;
/// Spells (`$077B-$0782`).
const ADDR_SPELLS: usize = 0x077B;
/// Magic / heart containers.
const ADDR_MAG_CTR: usize = 0x0783;
const ADDR_HEART_CTR: usize = 0x0784;
/// Items (`$0785-$078C`).
const ADDR_ITEMS: usize = 0x0785;
/// Thrust techniques (`$0796`: `$10` down, `$04` up).
const ADDR_THRUST: usize = 0x0796;
/// Seven-magic-containers flag (`$079D & $08`).
const ADDR_SEVEN: usize = 0x079D;
/// Quest (`$07A0`).
const ADDR_QUEST: usize = 0x07A0;

/// `bank5_Beginning_Values`: bank 5 `$BAE3`, copied to RAM from `$0777`.
pub const BEGINNING_VALUES_ADDR: u16 = 0xBAE3;
/// RAM address the beginning values are copied to.
const BEGINNING_RAM: usize = ADDR_LEVELS;

/// Whether RAM holds an untouched new file of the **running image**: the
/// levels, spells, containers, items and thrust bits equal the PRG's
/// `bank5_Beginning_Values` (which the `z2-rando` ROM randomizer may have
/// changed), with no XP and the first quest. Falls back to
/// [`ram_is_new_file`] (the original values) when the PRG is too short.
#[must_use]
pub fn ram_is_new_file_for(ram: &[u8; 0x800], prg: &[u8]) -> bool {
    let base = prg_offset(5, BEGINNING_VALUES_ADDR);
    let len = ADDR_THRUST - BEGINNING_RAM + 1;
    let Some(img) = prg.get(base..base + len) else {
        return ram_is_new_file(ram);
    };
    let rom = |addr: usize| img[addr - BEGINNING_RAM];
    let same = |addr: usize, n: usize| (addr..addr + n).all(|a| ram[a] == rom(a));
    same(ADDR_LEVELS, 3)
        && ram[ADDR_EXP] == 0
        && ram[ADDR_EXP + 1] == 0
        && same(ADDR_SPELLS, 8)
        && same(ADDR_MAG_CTR, 1)
        && same(ADDR_HEART_CTR, 1)
        && same(ADDR_ITEMS, 8)
        && ram[ADDR_THRUST] & 0x14 == rom(ADDR_THRUST) & 0x14
        && ram[ADDR_QUEST] == 0
}

/// Whether RAM holds an untouched new file of the original game
/// (`bank5_Beginning_Values` of the vanilla ROM).
#[must_use]
pub fn ram_is_new_file(ram: &[u8; 0x800]) -> bool {
    ram[ADDR_LEVELS..ADDR_LEVELS + 3] == [1, 1, 1]
        && ram[ADDR_EXP] == 0
        && ram[ADDR_EXP + 1] == 0
        && ram[ADDR_SPELLS..ADDR_SPELLS + 8].iter().all(|&b| b == 0)
        && ram[ADDR_MAG_CTR] == 4
        && ram[ADDR_HEART_CTR] == 4
        && ram[ADDR_ITEMS..ADDR_ITEMS + 8].iter().all(|&b| b == 0)
        && ram[ADDR_THRUST] & 0x14 == 0
        && ram[ADDR_QUEST] == 0
}

/// `update_next_level_exp` (bank 0 `$A057`) over the in-memory PRG: the
/// smallest level-up cost among the three stats at their current levels.
pub(crate) fn recompute_exp_next(game: &mut Game) {
    let mut next: u16 = 0xFFFF;
    for x in (0..3usize).rev() {
        let level = usize::from(game.ram[ADDR_LEVELS + x]);
        let y = x * 8 + level;
        let lo = prg_offset(0, LEVEL_LO_ADDR - 1) + y;
        let hi = prg_offset(0, LEVEL_HI_ADDR - 1) + y;
        let (Some(&l), Some(&h)) = (game.prg.get(lo), game.prg.get(hi)) else {
            return;
        };
        let v = u16::from(h) << 8 | u16::from(l);
        if v < next {
            next = v;
        }
    }
    game.ram[ADDR_EXP_NEXT] = (next >> 8) as u8;
    game.ram[ADDR_EXP_NEXT + 1] = next as u8;
}

/// Write the new-game loadout over a pristine file in RAM.
pub(crate) fn apply_loadout(game: &mut Game, opts: &RandoOpts) {
    let o = opts.clamped();
    let ram = &mut game.ram;
    for (i, v) in [o.start_attack, o.start_magic, o.start_life]
        .into_iter()
        .enumerate()
    {
        if v != 0 {
            ram[ADDR_LEVELS + i] = v;
        }
    }
    for i in 0..8 {
        if o.start_spells & bit(i) != 0 {
            ram[ADDR_SPELLS + i] = 1;
        }
        if o.start_items & (1 << i) != 0 {
            ram[ADDR_ITEMS + i] = 1;
        }
    }
    if o.start_items & 0x100 != 0 {
        ram[ADDR_THRUST] |= 0x10;
    }
    if o.start_items & 0x200 != 0 {
        ram[ADDR_THRUST] |= 0x04;
    }
    if o.start_containers_heart != 0 {
        ram[ADDR_HEART_CTR] = o.start_containers_heart;
    }
    if o.start_containers_magic != 0 {
        ram[ADDR_MAG_CTR] = o.start_containers_magic;
    }
    if ram[ADDR_MAG_CTR] >= 7 {
        ram[ADDR_SEVEN] |= 0x08;
    }
    ram[ADDR_MP] = crate::save::refill_meter(ram[ADDR_MAG_CTR]);
    ram[ADDR_HP] = crate::save::refill_meter(ram[ADDR_HEART_CTR]);
    recompute_exp_next(game);
}

// ------------------------------------------------------------ hooks

/// `bank7_get_item` entry.
pub const GET_ITEM_ADDR: u16 = 0xE771;

/// Item pickup hook: swap the object's major-item code for the shuffled
/// item while the original routine runs.
fn get_item_hook(game: &mut Game) {
    let slot = usize::from(game.ram[0x0010]) % 6;
    let at = 0x00AF + slot;
    let raw = game.ram[at];
    let code = raw & 0x7F;
    let mut swapped = None;
    if code < 8 {
        let placement = item_placement(&game.enh.rando);
        let item = placement[usize::from(code)];
        if item != code {
            let new = (raw & 0x80) | item;
            game.ram[at] = new;
            swapped = Some(new);
        }
    }
    super::call_original(game, GET_ITEM_ADDR);
    if let Some(new) = swapped {
        if game.ram[at] == new {
            game.ram[at] = raw;
        }
    }
}

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &RandoOpts) {
    for (off, bytes) in prg_patches(&game.prg, opts) {
        super::patch_prg(game, off, &bytes);
    }
    if opts.item_shuffle && item_placement(opts) != [0, 1, 2, 3, 4, 5, 6, 7] {
        super::hook(
            game,
            "enh_rando_get_item",
            Some(7),
            GET_ITEM_ADDR,
            get_item_hook,
            None,
        );
    }
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on):
/// the new-game loadout.
pub(crate) fn end_of_frame(game: &mut Game, opts: &RandoOpts) {
    if opts.has_loadout() && ram_is_new_file_for(&game.ram, &game.prg) {
        apply_loadout(game, opts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_prg() -> Vec<u8> {
        let mut prg = vec![0u8; 0x20000];
        for (i, b) in prg.iter_mut().enumerate() {
            *b = (i % 200) as u8 + 1;
        }
        // A few marker values that must survive.
        prg[prg_offset(1, ENEMY_HP_ADDR) + 3] = 0xFF;
        prg[prg_offset(1, ENEMY_HP_ADDR) + 4] = 0;
        prg[prg_offset(7, DAMAGE_ADDR) + 5] = 0xFF;
        prg
    }

    fn all_on(seed: u64) -> RandoOpts {
        RandoOpts {
            seed,
            enemy_hp_pct: 25,
            enemy_dmg_pct: -20,
            xp_pct: 10,
            level_cost_pct: -25,
            spell_cost_pct: 25,
            palette_rando: 2,
            item_shuffle: true,
            ..RandoOpts::default()
        }
    }

    #[test]
    fn splitmix_is_stable() {
        // Reference outputs of splitmix64 from state 0 (Vigna).
        let mut r = Rng(0);
        assert_eq!(r.next(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next(), 0x6E78_9E6A_A1B9_65F4);
        assert_ne!(mix(1, DOM_HP, 0), mix(2, DOM_HP, 0));
        assert_ne!(mix(1, DOM_HP, 0), mix(1, DOM_XP, 0));
    }

    #[test]
    fn same_seed_same_patches() {
        let prg = fake_prg();
        let a = prg_patches(&prg, &all_on(42));
        let b = prg_patches(&prg, &all_on(42));
        assert!(!a.is_empty());
        assert_eq!(a, b);
        assert_ne!(a, prg_patches(&prg, &all_on(43)));
        assert_eq!(item_placement(&all_on(7)), item_placement(&all_on(7)));
        assert_eq!(spoiler(&all_on(7)), spoiler(&all_on(7)));
        assert!(prg_patches(&prg, &RandoOpts::default()).is_empty());
    }

    #[test]
    fn entry_pct_stays_within_the_request() {
        for pct in [-25i8, -7, -1, 1, 3, 25, 100, -100] {
            let p = i32::from(pct.clamp(-25, 25));
            for i in 0..500 {
                let e = entry_pct(9, DOM_HP, i, pct);
                if p > 0 {
                    assert!((p / 2..=p).contains(&e), "{pct} -> {e}");
                } else {
                    assert!((p..=p / 2).contains(&e), "{pct} -> {e}");
                }
            }
        }
        assert_eq!(entry_pct(1, 2, 3, 0), 0);
    }

    #[test]
    fn scaled_tables_are_clamped() {
        let prg = fake_prg();
        let o = all_on(5);
        let mut img = prg.clone();
        for (off, b) in prg_patches(&prg, &o) {
            img[off..off + b.len()].copy_from_slice(&b);
        }
        for bank in ENEMY_BANKS {
            let off = prg_offset(bank, ENEMY_HP_ADDR);
            for i in 0..ENEMY_COUNT {
                let (v, n) = (prg[off + i], img[off + i]);
                if v >= 0xF0 || v == 0 {
                    assert_eq!(n, v);
                } else {
                    assert!((1..=0xEF).contains(&n));
                    let max = (u32::from(v) * 125).div_ceil(100).min(0xEF);
                    assert!(u32::from(n) >= u32::from(v).min(0xEF) && u32::from(n) <= max);
                }
            }
        }
        assert_eq!(img[prg_offset(7, DAMAGE_ADDR) + 5], 0xFF);
        let off = prg_offset(0, SPELL_COST_ADDR);
        assert!(img[off..off + 64].iter().all(|&b| (1..=0xF0).contains(&b)));
        // Level costs: non-decreasing per stat, cap entry untouched.
        for stat in 0..3 {
            let get = |i: usize| {
                u16::from(img[prg_offset(0, LEVEL_HI_ADDR) + i]) << 8
                    | u16::from(img[prg_offset(0, LEVEL_LO_ADDR) + i])
            };
            for l in 1..7 {
                assert!(get(stat * 8 + l) >= get(stat * 8 + l - 1));
                assert!(get(stat * 8 + l) <= 8999);
            }
            assert_eq!(
                img[prg_offset(0, LEVEL_HI_ADDR) + stat * 8 + 7],
                prg[prg_offset(0, LEVEL_HI_ADDR) + stat * 8 + 7]
            );
        }
        // Out-of-range options clamp.
        let wild = RandoOpts {
            start_attack: 99,
            start_containers_magic: 200,
            enemy_hp_pct: 127,
            xp_pct: -128,
            palette_rando: 9,
            start_items: 0xFFFF,
            ..RandoOpts::default()
        }
        .clamped();
        assert_eq!(wild.start_attack, 8);
        assert_eq!(wild.start_containers_magic, 8);
        assert_eq!(wild.enemy_hp_pct, 25);
        assert_eq!(wild.xp_pct, -25);
        assert_eq!(wild.palette_rando, 2);
        assert_eq!(wild.start_items, 0x3FF);
    }

    #[test]
    fn hue_rotation_keeps_greys_and_brightness() {
        for step in 1..=11u8 {
            for c in 0..0x40u8 {
                let r = rotate_hue(c, step);
                assert_eq!(r & 0x30, c & 0x30);
                let h = c & 0x0F;
                if h == 0 || h > 12 {
                    assert_eq!(r, c);
                } else {
                    assert_ne!(r, c);
                    assert!((1..=12).contains(&(r & 0x0F)));
                }
            }
            assert_eq!(rotate_hue(0xFF, step), 0xFF);
        }
        for region in 0..100 {
            assert!((1..=11).contains(&hue_step(3, region)));
        }
    }

    #[test]
    fn vanilla_placement_is_beatable_and_shuffles_are_logical() {
        assert!(placement_is_beatable(&[0, 1, 2, 3, 4, 5, 6, 7], 0));
        // The hammer behind Island Palace, which needs the hammer: locked.
        let mut bad = [0u8, 1, 6, 3, 4, 5, 2, 7];
        assert!(!placement_is_beatable(&bad, 0));
        // Candle and glove swapped: both palaces are open, still fine.
        assert!(placement_is_beatable(&[1, 0, 2, 3, 4, 5, 6, 7], 0));
        bad = [0, 1, 2, 3, 4, 5, 6, 7];
        bad.swap(CANDLE, HAMMER); // candle at Death Mountain, which needs it
        assert!(!placement_is_beatable(&bad, 0));
        assert!(placement_is_beatable(&bad, bit(CANDLE)));
        let mut shuffled = 0;
        for seed in 0..200u64 {
            let o = RandoOpts {
                seed,
                item_shuffle: true,
                ..RandoOpts::default()
            };
            let p = item_placement(&o);
            let mut sorted = p;
            sorted.sort_unstable();
            assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6, 7], "permutation");
            assert!(placement_is_beatable(&p, 0));
            if p != [0, 1, 2, 3, 4, 5, 6, 7] {
                shuffled += 1;
            }
        }
        assert!(shuffled > 150, "most seeds actually shuffle: {shuffled}");
        // Start items stay at their vanilla locations.
        let o = RandoOpts {
            seed: 11,
            item_shuffle: true,
            start_items: 1 << RAFT,
            ..RandoOpts::default()
        };
        assert_eq!(item_placement(&o)[RAFT], RAFT as u8);
        let text = spoiler(&o);
        assert!(text.contains("Island Palace (P3)"));
        assert!(text.contains("start items: RAFT"));
    }

    /// Vanilla `bank5_Beginning_Values` (levels .. thrust) over `prg`.
    fn put_beginning(prg: &mut [u8], levels: [u8; 3], spells: u8, hearts: u8) {
        let base = prg_offset(5, BEGINNING_VALUES_ADDR);
        let len = ADDR_THRUST - BEGINNING_RAM + 1;
        prg[base..base + len].fill(0);
        prg[base..base + 3].copy_from_slice(&levels);
        for i in 0..8 {
            prg[base + ADDR_SPELLS - BEGINNING_RAM + i] = (spells >> i) & 1;
        }
        prg[base + ADDR_MAG_CTR - BEGINNING_RAM] = 4;
        prg[base + ADDR_HEART_CTR - BEGINNING_RAM] = hearts;
    }

    #[test]
    fn new_file_detection_follows_the_rom_beginning_values() {
        let mut prg = fake_prg();
        put_beginning(&mut prg, [1, 1, 1], 0, 4);
        let mut ram = [0u8; 0x800];
        ram[ADDR_LEVELS..ADDR_LEVELS + 3].copy_from_slice(&[1, 1, 1]);
        ram[ADDR_MAG_CTR] = 4;
        ram[ADDR_HEART_CTR] = 4;
        assert!(ram_is_new_file(&ram));
        assert!(ram_is_new_file_for(&ram, &prg));
        // A ROM-randomized start (level 3 attack, SHIELD, 6 hearts): the
        // vanilla check misses it, the ROM-aware one finds it.
        put_beginning(&mut prg, [3, 1, 1], 1, 6);
        ram[ADDR_LEVELS] = 3;
        ram[ADDR_SPELLS] = 1;
        ram[ADDR_HEART_CTR] = 6;
        assert!(!ram_is_new_file(&ram));
        assert!(ram_is_new_file_for(&ram, &prg));
        ram[ADDR_EXP + 1] = 5;
        assert!(!ram_is_new_file_for(&ram, &prg));
        // Too short a PRG falls back to the original values.
        assert!(!ram_is_new_file_for(&ram, &[]));
    }

    #[test]
    fn loadout_applies_once_to_a_new_file() {
        let mut g = Game::new();
        g.prg = fake_prg();
        put_beginning(&mut g.prg, [1, 1, 1], 0, 4);
        g.ram[ADDR_LEVELS..ADDR_LEVELS + 3].copy_from_slice(&[1, 1, 1]);
        g.ram[ADDR_MAG_CTR] = 4;
        g.ram[ADDR_HEART_CTR] = 4;
        assert!(ram_is_new_file(&g.ram));
        let o = RandoOpts {
            start_attack: 5,
            start_life: 8,
            start_spells: 0b1000_0001,
            start_items: 0x301,
            start_containers_magic: 7,
            ..RandoOpts::default()
        };
        assert!(o.has_loadout());
        end_of_frame(&mut g, &o);
        assert_eq!(&g.ram[ADDR_LEVELS..ADDR_LEVELS + 3], &[5, 1, 8]);
        assert_eq!(g.ram[ADDR_SPELLS], 1);
        assert_eq!(g.ram[ADDR_SPELLS + 7], 1);
        assert_eq!(g.ram[ADDR_ITEMS], 1);
        assert_eq!(g.ram[ADDR_THRUST], 0x14);
        assert_eq!(g.ram[ADDR_MAG_CTR], 7);
        assert_eq!(g.ram[ADDR_SEVEN] & 0x08, 0x08);
        assert_eq!(g.ram[ADDR_MP], 7 * 32 - 1);
        assert_eq!(g.ram[ADDR_HP], 4 * 32 - 1);
        assert!(!ram_is_new_file(&g.ram));
        // A started file is left alone.
        g.ram[ADDR_LEVELS] = 2;
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[ADDR_LEVELS], 2);
        assert!(!RandoOpts::default().has_loadout());
        assert!(!RandoOpts {
            start_attack: 1,
            start_containers_heart: 4,
            ..RandoOpts::default()
        }
        .has_loadout());
    }
}
