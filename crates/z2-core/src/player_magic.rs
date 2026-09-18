//! Player magic/items/leveling/death: spells, meter, pause menu, exp, death.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod player_magic;` (see `lib.rs`; trap shims
//! live in `player_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Movement/combat/fairy physics live in the sibling `player.rs`.
//!
//! # Sources
//!
//! Bank 0 `third_party/z2disassembly/src/prg0.asm` (`prg0.asm $xxxx` below),
//! bank 7 `src/prg7.asm` for the per-frame spell tick, death, and item
//! pickup paths.
//!
//! | fn | label | addr |
//! |---|---|---|
//! | [`spell_cost`] | `Table_for_Magic_Needed_for_Spells` | `$8D7B` |
//! | [`SPELL_BITS`] | `Table_for_Spell_effects` | `$8DBB` |
//! | [`cast_gate`] / [`cast_spell`] | `Spell_Casting_Routine` | `$8DC3` |
//! | [`spell_tick`] | `L8E1F` + `L8E28` per-spell loop | `$8E1F` / `$8E28` |
//! | [`jump_spell_fx`] | `Jump_Spell` | `$8E58` |
//! | [`life_spell_fx`] | `Life_Spell` | `$8E5D` |
//! | [`spell_spell_fx`] | `Spell_Spell` + `spells_routines2` | `$8E73` / `$8ED8` |
//! | [`shield_spell_fx`] | `Shield_Spell` | `$8E8D` |
//! | [`reflect_spell_fx`] | `Reflect_Spell` | `$8E96` |
//! | [`fairy_spell_fx`] | `bank0_Fairy_Spell` | `$91A4` |
//! | [`spell_spell_clear`] | `L91AF` (Spell-spell enemy revert) | `$91AF` |
//! | [`thunder_spell_fx`] | `Thunder_Spell` | `$91E6` |
//! | [`magic_regen_tick`] | `LD3E9` meter refill (`$D3F9-$D431`) | `$D3F9` |
//! | [`exp_trickle`] | `LD433` exp drip (`$D443-$D4A8`) | `$D443` |
//! | [`level_ready`] | `Hub_Update_Routine` threshold compare | `$968D` |
//! | [`next_level_for`] | `Table_for_levelup_experience_high/low` | `$9659` / `$9671` |
//! | [`pause_step`] | pause/select menu (`$0524`, `$0749`/`$074A`) | `$C14E`-region |
//! | [`item_pickup`] | `bank7_get_item` | `$E771` |
//! | [`container_pickup`] | `LE7BB` container path | `$E7BB` |
//! | [`item_passive`] | item effects (candle/glove/raft/boots/flute/cross/hammer/key) | `$9282`-region |
//! | [`death_check`] | `bank7_check_if_link_died…` | `$D3CC` |
//! | [`continue_flow`] | `bank7_Reset_Number_of_Lives__to_3_` + lives screen | `$C35A` / `$C3D4` |
//!
//! # Preserved quirks (BUG comments)
//!
//! * `$8E3B` free-cast: when the cost subtraction underflows to exactly
//!   `$FF`, the routine falls through and casts with the meter set to `$00`
//!   instead of failing (intended fail = keep meter, no cast).
//! * Exp drip (`$D44B-$D474`): pending exp below 10 drains 1/frame, else 10;
//!   exp-loss (`$05E8`, Moa drain) ticks 1/frame only while total exp is
//!   nonzero, so a zero-exp drain never fires the meter sound.
//! * Container pickup (`$E7CD`): `INC $0775,x` with `X = $0E/$0F` bumps the
//!   *exp-high* byte as a side effect of the container-indexed addressing.
//!
//! # Gaps (honest)
//!
//! * Fire-spell projectile (`bank0_Fire_Spell`, `$97F1`) and the exact
//!   Thunder per-enemy damage application (`LE726`, `$E726` via `$922A`)
//!   need enemy-slot state; modelled as flags + fixed power here, full slot
//!   writes live with the interpreter (trap shim records the flag half).
//! * The `$9659` high-byte listing bytes disagree with the adjacent
//!   human-readable exp-chart comment (see [`NEXT_LEVEL_HI`]); the chart is
//!   used and the raw bytes are ROM-gated.
//! * Pause-pane tile emission and HUD string bytes are display-only
//!   (PPU scope); only state transitions are modelled.

// ---------------------------------------------------------------------------
// Addresses (duplicated per player*.rs file on purpose).
// ---------------------------------------------------------------------------

/// Current magic (`$773`).
pub const ADDR_MP: u16 = 0x0773;
/// Current life (`$774`).
pub const ADDR_HP: u16 = 0x0774;
/// Exp (BE u16 `$775-$776`).
pub const ADDR_EXP: u16 = 0x0775;
/// Attack level (`$777`, 1-8).
pub const ADDR_ATK_LVL: u16 = 0x0777;
/// Magic level (`$778`, 1-8).
pub const ADDR_MAG_LVL: u16 = 0x0778;
/// Life level (`$779`, 1-8).
pub const ADDR_LIFE_LVL: u16 = 0x0779;
/// Exp needed next level (BE u16 `$770-$771`).
pub const ADDR_EXP_NEXT: u16 = 0x0770;
/// Spells possessed (`$77B-$782`, index = spell).
pub const ADDR_SPELLS: u16 = 0x077B;
/// Items possessed (`$785-$78C`, index = item).
pub const ADDR_ITEMS: u16 = 0x0785;
/// Magic containers (`$783`).
pub const ADDR_MAG_CTR: u16 = 0x0783;
/// Heart containers (`$784`).
pub const ADDR_HEART_CTR: u16 = 0x0784;
/// Magic state bits (`$76F`: 1 shield, 2 jump, 4 life, 8 fairy, 10 fire,
/// 20 reflect, 40 spell, 80 thunder).
pub const ADDR_MAGIC_STATE: u16 = 0x076F;
/// Jump-spell modifier (`$D0`).
pub const ADDR_JUMP_FX: u16 = 0x00D0;
/// Shield-spell tint (`$70F`).
pub const ADDR_SHIELD_FX: u16 = 0x070F;
/// Reflect flag (`$710`).
pub const ADDR_REFLECT: u16 = 0x0710;
/// Thunder modifier (`$D9`).
pub const ADDR_THUNDER: u16 = 0x00D9;
/// Lives (`$700`).
pub const ADDR_LIVES: u16 = 0x0700;
/// Deaths/continues (`$79F`).
pub const ADDR_DEATHS: u16 = 0x079F;
/// Game state (`$76C`: 2 = die, 6 = lives screen).
pub const ADDR_GAME_STATE: u16 = 0x076C;
/// Kill-Link flag (`$494`).
pub const ADDR_KILL: u16 = 0x0494;
/// Injured timer (`$50C`).
pub const ADDR_INJURED: u16 = 0x050C;
/// Menu control (`$524`).
pub const ADDR_MENU: u16 = 0x0524;
/// Dialog type (`$74C`: 0 none, 1 level-up, 2 talking).
pub const ADDR_DIALOG: u16 = 0x074C;
/// Magic selector pos (`$749`) / last-cast pos (`$74A`).
pub const ADDR_SELECTOR: u16 = 0x0749;
/// Last-cast selector (`$74A`).
pub const ADDR_LAST_CAST: u16 = 0x074A;
/// Pause-pane dirty (`$74F`, bit7 = spell flash).
pub const ADDR_PANE: u16 = 0x074F;
/// Spell flash counter (`$74B`).
pub const ADDR_FLASH: u16 = 0x074B;
/// Pending magic refill (`$70C`) / pending life (`$70D`).
pub const ADDR_PEND_MAG: u16 = 0x070C;
/// Pending life refill (`$70D`).
pub const ADDR_PEND_LIFE: u16 = 0x070D;
/// Pending exp (`$755-$756`, hi-lo).
pub const ADDR_PEND_EXP_HI: u16 = 0x0755;
/// Pending exp low (`$756`).
pub const ADDR_PEND_EXP_LO: u16 = 0x0756;
/// Exp-loss pending (`$5E8`).
pub const ADDR_EXP_LOSS: u16 = 0x05E8;
/// Keys (`$793`).
pub const ADDR_KEYS: u16 = 0x0793;
/// Seven-containers flag (`$79D`, bit3 Kasuto).
pub const ADDR_SEVEN_FLAG: u16 = 0x079D;
/// Spell-lock (`$DE`: 1 = Spell-spell active, blocks movement).
pub const ADDR_SPELL_LOCK: u16 = 0x00DE;
/// Big-door counter (`$763`: `$10` = already used).
pub const ADDR_DOOR_CTR: u16 = 0x0763;

// ---------------------------------------------------------------------------
// Spell / item ids.
// ---------------------------------------------------------------------------

/// Spell index: shield.
pub const SPELL_SHIELD: usize = 0;
/// Spell index: jump.
pub const SPELL_JUMP: usize = 1;
/// Spell index: life.
pub const SPELL_LIFE: usize = 2;
/// Spell index: fairy.
pub const SPELL_FAIRY: usize = 3;
/// Spell index: fire.
pub const SPELL_FIRE: usize = 4;
/// Spell index: reflect.
pub const SPELL_REFLECT: usize = 5;
/// Spell index: spell.
pub const SPELL_SPELL: usize = 6;
/// Spell index: thunder.
pub const SPELL_THUNDER: usize = 7;
/// Spell count.
pub const SPELL_COUNT: usize = 8;

/// Item index: candle.
pub const ITEM_CANDLE: usize = 0;
/// Item index: glove.
pub const ITEM_GLOVE: usize = 1;
/// Item index: raft.
pub const ITEM_RAFT: usize = 2;
/// Item index: boots.
pub const ITEM_BOOTS: usize = 3;
/// Item index: flute.
pub const ITEM_FLUTE: usize = 4;
/// Item index: cross.
pub const ITEM_CROSS: usize = 5;
/// Item index: hammer.
pub const ITEM_HAMMER: usize = 6;
/// Item index: magic key.
pub const ITEM_KEY: usize = 7;
/// Item count.
pub const ITEM_COUNT: usize = 8;

/// Effect bits (`Table_for_Spell_effects`, bank 0 `$8DBB`).
pub const SPELL_BITS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

/// Magic costs (`Table_for_Magic_Needed_for_Spells`, bank 0 `$8D7B`,
/// rows = spell 0-7, cols = magic level 1-8).
pub const SPELL_COSTS: [[u8; 8]; 8] = [
    [0x40, 0x30, 0x30, 0x20, 0x20, 0x20, 0x20, 0x20],
    [0x60, 0x50, 0x40, 0x40, 0x28, 0x20, 0x18, 0x10],
    [0x8C, 0x8C, 0x78, 0x78, 0x64, 0x64, 0x64, 0x64],
    [0xA0, 0xA0, 0x78, 0x78, 0x50, 0x50, 0x50, 0x50],
    [0xF0, 0xA0, 0x78, 0x3C, 0x20, 0x20, 0x20, 0x20],
    [0xF0, 0xF0, 0xA0, 0x60, 0x50, 0x40, 0x30, 0x20],
    [0xF0, 0xE0, 0xC0, 0xA0, 0x60, 0x40, 0x30, 0x20],
    [0xF0, 0xF0, 0xF0, 0xF0, 0xF0, 0xF0, 0xC8, 0x80],
];

/// Look up the meter cost for `spell` at 1-based `magic_level`.
pub fn spell_cost(spell: usize, magic_level: u8) -> u8 {
    let lvl = magic_level.clamp(1, 8) - 1;
    SPELL_COSTS[spell % SPELL_COUNT][usize::from(lvl)]
}

// ---------------------------------------------------------------------------
// Cast gate (Spell_Casting_Routine, bank 0 $8DC3).
// ---------------------------------------------------------------------------

/// Cast-gate inputs (explicit; no RAM dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastGate {
    /// `$74C` dialog active.
    pub dialog: u8,
    /// `$524` menu/routine index.
    pub menu: u8,
    /// `$DE` spell lock.
    pub lock: u8,
    /// Selector pos `$749` (+1) vs last-cast `$74A` (equal = re-cast guard).
    pub selector: u8,
    /// Last-cast pos `$74A`.
    pub last_cast: u8,
    /// Select button pressed (`$F5 & $20`).
    pub select_pressed: bool,
    /// Spell learned (`$077B,y != 0`).
    pub learned: bool,
    /// Current meter `$773`.
    pub meter: u8,
    /// Cost from [`spell_cost`].
    pub cost: u8,
}

/// Cast-gate outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastOutcome {
    /// No cast (blocked / re-cast / unlearned / broke).
    None,
    /// Cast, new meter value.
    Cast { meter: u8 },
    /// BUG ($8E3B): underflow landed exactly on `$FF` → cast with meter 0.
    CastFree,
}

/// Cast gate (`$8DC3-$8DF5` + `$8E3B` quirk).
///
/// Blocked when dialog/menu/lock nonzero, selector unchanged, Select not
/// pressed, or spell unlearned. Else `meter - cost`: borrow (result !=
/// `$FF`) fails; borrow to exactly `$FF` is the free-cast quirk; else the
/// cast lands and the meter stores the remainder.
pub const fn cast_gate(g: CastGate) -> CastOutcome {
    if g.dialog != 0 || g.menu != 0 || g.lock != 0 {
        return CastOutcome::None;
    }
    if g.selector.wrapping_add(1) == g.last_cast {
        return CastOutcome::None;
    }
    if !g.select_pressed || !g.learned {
        return CastOutcome::None;
    }
    let (rest, borrow) = g.meter.overflowing_sub(g.cost);
    if !borrow {
        CastOutcome::Cast { meter: rest }
    } else if rest == 0xFF {
        CastOutcome::CastFree
    } else {
        CastOutcome::None
    }
}

/// Post-cast bookkeeping (`$8DF5-$8E1D`): OR the effect bit into `$76F`,
/// `last_cast = selector + 1`, `$74F |= $80`, flash `$20` (`OR $80` when the
/// new `last_cast >= 6`, i.e. Reflect/Spell/Thunder flash decor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastFx {
    /// New `$76F`.
    pub magic_state: u8,
    /// New `$74A`.
    pub last_cast: u8,
    /// New `$74F`.
    pub pane: u8,
    /// New `$74B` flash counter.
    pub flash: u8,
}

/// Compute post-cast bookkeeping. `selector` is `$749`, `last` the current
/// `$74A` (normally equal to `selector + 1` after the gate passes).
pub const fn cast_spell(selector: u8, magic_state: u8, pane: u8) -> CastFx {
    let bit = SPELL_BITS[(selector as usize) % SPELL_COUNT];
    let last = selector.wrapping_add(1);
    let flash_base: u8 = 0x20;
    let flash = if last >= 0x06 {
        flash_base | 0x80
    } else {
        flash_base
    };
    CastFx {
        magic_state: magic_state | bit,
        last_cast: last,
        pane: pane | 0x80,
        flash,
    }
}

// ---------------------------------------------------------------------------
// Per-spell effects.
// ---------------------------------------------------------------------------

/// Jump spell (`Jump_Spell`, `$8E58`): `$D0 = 1` every tick the bit is set.
pub const fn jump_spell_fx() -> u8 {
    0x01
}

/// Life spell (`Life_Spell`, `$8E5D`): clears its own bit from `$76F` and
/// queues `$30` into pending life `$70D` (saturating at `$FF`).
pub const fn life_spell_fx(magic_state: u8, pend_life: u8) -> (u8, u8) {
    let (nl, carry) = pend_life.overflowing_add(0x30);
    let sat = if carry { 0xFF } else { nl };
    (magic_state & 0xFB, sat)
}

/// Shield spell (`Shield_Spell`, `$8E8D`): `$070F = $16` tint + `$69DE` mirror.
pub const SHIELD_TINT: u8 = 0x16;

/// Reflect spell (`Reflect_Spell`, `$8E96`): `$0710 = 1`.
pub const REFLECT_ON: u8 = 0x01;

/// Fairy-spell cast is `player::fairy_cast` (bank 0 `$91A4`); re-export the
/// floor here so trap code has one import.
pub const FAIRY_MIN_Y: u8 = 0x20;

/// Spell-spell availability (`Spell_Spell`, `$8E73-$8E87`): only in world
/// 1-2 (`$707` 1/2) and area `$14`; sets the `$DE` lock and runs
/// `spells_routines2`. Returns true when the lock engages.
pub const fn spell_spell_fx(world: u8, area: u8) -> bool {
    if world == 0 || world >= 0x03 {
        return false;
    }
    area == 0x14
}

/// Spell-spell used-flag (`spells_routines2`, `$8ED8-$8EE3`): `$0763 ==
/// $10` means already used → clear `$DE`; `$0F` means ready. Returns
/// `(lock_held, used_up)`.
pub const fn spell_door_step(door_ctr: u8) -> (bool, bool) {
    if door_ctr == 0x10 {
        (false, true)
    } else {
        (true, door_ctr == 0x0F)
    }
}

/// Spell-spell clear (`L91AF`, `$91AF-$91E5`): clears the Spell bit from
/// `$76F` and reverts every live non-immune enemy slot to Bot (code 4,
/// zeroed velocity/timer/HP). Returns `(new_magic_state, slots_reverted)`
/// where `slots_reverted` counts slots with `exists != 0` and vulnerable bit
/// (`$6DF9 & $10`) clear.
pub fn spell_spell_clear(magic_state: u8, exists: &[u8], immune: &[bool]) -> (u8, usize) {
    let mut n = 0usize;
    for (i, e) in exists.iter().enumerate() {
        if *e != 0 && !immune.get(i).copied().unwrap_or(true) {
            n += 1;
        }
    }
    (magic_state & 0xBF, n)
}

/// Thunder attack power (`bank7_Attack_Power…`, `$E675`: `$32`).
pub const THUNDER_POWER: u8 = 0x32;
/// Thunder normal power index quirk (`$9228`: `LDY #$01` for the
/// Thunderbird-special path).
pub const THUNDER_BIRD_POWER_IDX: usize = 1;

/// Thunder cast (`Thunder_Spell`, `$91E6-$9234`): clears the Thunder bit,
/// sets `$D9` (0 when the lead-in `A` was 0 — always, via `LDA #$00`), and
/// damages every live slot through the shared sword-damage path (`LE726`,
/// `$E726`); Thunderbird-special uses power index 1. Returns
/// `(new_magic_state, thunder_mod, slots_hit)`.
pub fn thunder_spell_fx(magic_state: u8, exists: &[u8], on_screen: &[bool]) -> (u8, u8, usize) {
    let mut n = 0usize;
    for (i, e) in exists.iter().enumerate() {
        if *e == 1 && on_screen.get(i).copied().unwrap_or(false) {
            n += 1;
        }
    }
    (magic_state & 0x7F, 0x00, n)
}

/// Per-frame spell tick (`L8E1F` + `L8E28`, `$8E1F-$8E3A`): `$D0 = 0`, then
/// for each set bit `Y = 7..0` dispatch `L8E44` (`Shield/Jump/Life/Fairy/
/// Fire/Reflect/Spell/Thunder`). Returns the `$D0` value after the tick
/// (1 iff Jump bit set) plus the life-spell pending delta flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellTick {
    /// New `$D0` (Jump modifier).
    pub jump_fx: u8,
    /// Whether Life fired this tick (queues `$30` via [`life_spell_fx`]).
    pub life_fired: bool,
    /// Whether Fairy may cast (bit set; Y-gate lives in `player::fairy_cast`).
    pub fairy_armed: bool,
    /// Whether Thunder fired (clears its bit; see [`thunder_spell_fx`]).
    pub thunder_fired: bool,
    /// Whether Spell-spell clear fired (clears its bit).
    pub spell_cleared: bool,
}

/// Compute the per-tick dispatch set from `$76F`.
pub const fn spell_tick(magic_state: u8) -> SpellTick {
    SpellTick {
        jump_fx: if magic_state & 0x02 != 0 { 0x01 } else { 0x00 },
        life_fired: magic_state & 0x04 != 0,
        fairy_armed: magic_state & 0x08 != 0,
        thunder_fired: magic_state & 0x80 != 0,
        spell_cleared: magic_state & 0x40 != 0,
    }
}

// ---------------------------------------------------------------------------
// Meter refill + exp drip (LD3E9 region, bank 7 $D3E9-$D4AB).
// ---------------------------------------------------------------------------

/// Meter cap from containers (`$D3FC-$D407`): `(ctr << 5) - 1`.
pub const fn meter_cap(containers: u8) -> u8 {
    (containers << 5).wrapping_sub(1)
}

/// Meter refill tick (`$D3F1-$D431`): alternates magic/life by frame parity
/// (`$12 & 1 → X`); when pending (`$070C,x != 0`) decrement pending and add
/// 2 to the meter (`$0773,x`), saturating at the container cap (on overflow
/// or reaching cap: pending = 0, meter = cap). Returns
/// `(meter, pending, pane_dirty_bit_set)`. `pane` bit = `table13[X]`
/// (`$D3CA`) — caller ORs `$40`-family bit `$D4A3` separately.
pub const fn magic_regen_tick(meter: u8, pending: u8, cap: u8, frame_parity: u8) -> (u8, u8, bool) {
    if pending == 0 || frame_parity > 1 {
        return (meter, pending, false);
    }
    let pend = pending - 1;
    let (m1, ov) = meter.overflowing_add(0x02);
    if ov || m1 >= cap {
        (cap, 0x00, true)
    } else {
        (m1, pend, true)
    }
}

/// Exp drip (`$D443-$D474`): pending nonzero drains 10/frame (or 1 when the
/// 16-bit pending is < 10) into `$775-$776` exp. Returns
/// `(exp_lo, exp_hi, pend_lo, pend_hi)` (exp as lo/hi pair, little pair? —
/// note `$775` is MSB, `$776` LSB per ram-map; kept as given order).
pub const fn exp_trickle(exp_hi: u8, exp_lo: u8, pend_hi: u8, pend_lo: u8) -> (u8, u8, u8, u8) {
    if pend_hi == 0 && pend_lo == 0 {
        return (exp_hi, exp_lo, pend_hi, pend_lo);
    }
    let step: u8 = if pend_hi == 0 && pend_lo < 0x0A {
        0x01
    } else {
        0x0A
    };
    let (pl, b1) = pend_lo.overflowing_sub(step);
    let ph = if b1 { pend_hi.wrapping_sub(1) } else { pend_hi };
    let (el, c1) = exp_lo.overflowing_add(step);
    let eh = if c1 { exp_hi.wrapping_add(1) } else { exp_hi };
    (eh, el, ph, pl)
}

/// Exp-loss tick (`$D477-$D4A1`): `$05E8` nonzero + total exp nonzero drains
/// 1 exp/frame. Returns `(exp_hi, exp_lo, loss)`.
pub const fn exp_loss_tick(exp_hi: u8, exp_lo: u8, loss: u8) -> (u8, u8, u8) {
    if loss == 0 || (exp_hi == 0 && exp_lo == 0) {
        return (exp_hi, exp_lo, loss);
    }
    let (el, b) = exp_lo.overflowing_sub(1);
    let eh = if b { exp_hi.wrapping_sub(1) } else { exp_hi };
    (eh, el, loss.wrapping_sub(1))
}

// ---------------------------------------------------------------------------
// Leveling (Hub_Update_Routine $968D + exp tables $9659/$9671).
// ---------------------------------------------------------------------------

/// Level-up exp chart, high bytes per (stat, level→next): attack / magic /
/// life × levels 1-8. From the human-readable chart comment at `$9659`
/// (`00 00 00 00 01 01 03 23` attack …); the raw listing bytes disagree
/// (ROM-gated gap — see module docs).
pub const NEXT_LEVEL_HI: [[u8; 8]; 3] = [
    [0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x03, 0x23],
    [0x00, 0x01, 0x02, 0x04, 0x08, 0x0D, 0x17, 0x23],
    [0x00, 0x00, 0x01, 0x03, 0x05, 0x09, 0x0F, 0x23],
];

/// Level-up exp chart, low bytes (`14 32 64 C8 2C F4 20 28` attack …).
pub const NEXT_LEVEL_LO: [[u8; 8]; 3] = [
    [0x14, 0x32, 0x64, 0xC8, 0x2C, 0xF4, 0x20, 0x28],
    [0x64, 0x2C, 0xBC, 0xB0, 0x98, 0xAC, 0x70, 0x28],
    [0x32, 0x96, 0x90, 0x20, 0xDC, 0xC4, 0xA0, 0x28],
];

/// Stat select: 0 attack, 1 magic, 2 life.
pub const STAT_ATTACK: usize = 0;
/// Stat select: magic.
pub const STAT_MAGIC: usize = 1;
/// Stat select: life.
pub const STAT_LIFE: usize = 2;

/// Threshold (BE u16) to go from `level` (1-8) to `level + 1` for `stat`.
/// Level 8 (max) returns `0xFFFF` (never — matches the `$23 $28` cap row).
pub fn next_level_for(stat: usize, level: u8) -> u16 {
    let s = stat % 3;
    let l = level.clamp(1, 8) - 1;
    if l >= 7 {
        return 0xFFFF;
    }
    u16::from(NEXT_LEVEL_HI[s][usize::from(l)]) << 8 | u16::from(NEXT_LEVEL_LO[s][usize::from(l)])
}

/// Level-ready check (`$968D-$969E`): `exp >= next` (BE compare with borrow)
/// raises `$074C = 1`. `exp`/`next` are BE u16 values.
pub const fn level_ready(exp: u16, next: u16) -> bool {
    exp >= next
}

/// Level-up choice: bump the chosen stat (cap 8), recompute `$770-$771`
/// from the chart for the new level. Returns `(new_level, new_next)`.
pub fn level_up_choice(stat: usize, level: u8) -> (u8, u16) {
    let nl = (level + 1).min(8);
    (nl, next_level_for(stat, nl.min(7)))
}

// ---------------------------------------------------------------------------
// Pause / select menu.
// ---------------------------------------------------------------------------

/// Menu states on `$524` (overworld values; sideview reuses the byte).
pub const MENU_CLOSED: u8 = 0x00;
/// Menu states on `$524`: paused.
pub const MENU_PAUSED: u8 = 0x01;
/// Menu states on `$524`: unpausing.
pub const MENU_UNPAUSE: u8 = 0x03;

/// Pause toggle: Start-pressed flips closed ↔ paused; unpause transient
/// returns to closed. Dialog open (`$74C != 0`) blocks pausing.
pub const fn pause_step(menu: u8, start_pressed: bool, dialog: u8) -> u8 {
    if !start_pressed || dialog != 0 {
        return if menu == MENU_UNPAUSE {
            MENU_CLOSED
        } else {
            menu
        };
    }
    match menu {
        MENU_CLOSED => MENU_PAUSED,
        MENU_PAUSED => MENU_UNPAUSE,
        _ => MENU_CLOSED,
    }
}

/// Selector move: Up/Down-pressed walks `$749` in `0..spell_count`
/// (learned-only skipping lives with the caller); wraps.
pub const fn selector_step(selector: u8, up: bool, down: bool, spell_count: u8) -> u8 {
    let n = if spell_count == 0 { 1 } else { spell_count };
    if up && !down {
        (selector + n - 1) % n
    } else if down && !up {
        (selector + 1) % n
    } else {
        selector
    }
}

// ---------------------------------------------------------------------------
// Items (bank7_get_item $E771 + passives).
// ---------------------------------------------------------------------------

/// Item codes on `$AF` (`LE755` region): `< 8` inventory items, 8 key,
/// `$0E/$0F` containers, `$10+` jars/doll/child/trophy/medicine.
pub const ITEM_CODE_KEY: u8 = 0x08;
/// Item codes: magic container.
pub const ITEM_CODE_MAG_CTR: u8 = 0x0E;
/// Item codes: heart container.
pub const ITEM_CODE_HEART_CTR: u8 = 0x0F;
/// Item codes: first jar code.
pub const ITEM_CODE_JAR_BASE: u8 = 0x10;

/// Pickup outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pickup {
    /// Inventory item bit set (`$0785,y |= 1`).
    Inventory { slot: usize },
    /// Key (`INC $0793`; `$0728` boss-lock cleared when set).
    Key { unlock_boss: bool },
    /// Container (`INC $0775,x` w/ `$0E/$0F` side effect; pending refill).
    Container { magic: bool, pending: u8 },
    /// Jar: pending magic/life refill (`$070C += …`).
    Jar { magic_add: u8, life_add: u8 },
    /// Doll: extra life (`INC $0700`).
    Doll,
    /// No-op / flag-only pickups (child/trophy/medicine bits).
    Flag { byte: u8, bit: u8 },
}

/// Classify an item-code pickup (`$E771-$E86D`).
///
/// `magic_containers` feeds the jar refill scaling (`ASL×4`); `boss_lock`
/// is `$0728 != 0`; `level_shift` is the container pending value
/// (`level << 4` after the `ASL×4` at `$E7E3`).
pub const fn item_pickup(code: u8, magic_containers: u8, boss_lock: bool) -> Pickup {
    if code < 0x08 {
        Pickup::Inventory {
            slot: (code & 0x07) as usize,
        }
    } else if code == ITEM_CODE_KEY {
        Pickup::Key {
            unlock_boss: boss_lock,
        }
    } else if code == ITEM_CODE_MAG_CTR {
        Pickup::Container {
            magic: true,
            pending: 0,
        }
    } else if code == ITEM_CODE_HEART_CTR {
        Pickup::Container {
            magic: false,
            pending: 0,
        }
    } else if code == 0x12 {
        Pickup::Doll
    } else if code == 0x13 {
        Pickup::Flag {
            byte: 0x9C,
            bit: 0x20,
        }
    } else if code == 0x14 {
        Pickup::Flag {
            byte: 0x98,
            bit: 0x10,
        }
    } else if code == 0x15 {
        Pickup::Flag {
            byte: 0x9A,
            bit: 0x40,
        }
    } else {
        // Jars `$10/$11`: blue restores `$10` magic-ish, red scales with
        // containers (`$05E3 = ctr << 4`, `$E854-$E86B`).
        let scaled = magic_containers << 4;
        if code == 0x10 {
            Pickup::Jar {
                magic_add: 0x10,
                life_add: 0,
            }
        } else {
            Pickup::Jar {
                magic_add: 0,
                life_add: scaled,
            }
        }
    }
}

/// Container pickup bookkeeping (`$E7C3-$E7ED`): `INC` containers (cap 8 —
/// the listing has no explicit cap; hardware wraps, caller clamps), the
/// `$0775,x` exp-high side effect, the 7-magic Kasuto bit (`$079D |= 8`),
/// and pending refill `(level+1?) << 4` into `$06FE,x`.
///
/// Returns `(containers, kasuto_bit, pending)`.
pub const fn container_pickup(containers: u8, level: u8, magic: bool) -> (u8, bool, u8) {
    let bumped = containers.saturating_add(1);
    let nc = if bumped > 8 { 8 } else { bumped };
    let kasuto = magic && nc >= 7;
    (nc, kasuto, level << 4)
}

/// Item passives (explicit predicates; ROM tile/flag sources cited).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemPassives {
    /// Candle lights dark rooms (`$9272`: world/candle check picks palette).
    pub candle_lit: bool,
    /// Glove lets stabs break `$61` (see `sideview_collision::stab_breaks`).
    pub glove_break: bool,
    /// Raft enables water walking (overworld use; sideview: no sink).
    pub raft_float: bool,
    /// Boots enable water walking (sideview shallow stride).
    pub boots_stride: bool,
    /// Flute reveals the hidden palace spot (overworld use).
    pub flute_song: bool,
    /// Cross reveals invisible enemies (Moa visibility).
    pub cross_sight: bool,
    /// Hammer breaks forest/rock tiles (overworld use).
    pub hammer_smash: bool,
    /// Magic key opens locked doors without consuming keys.
    pub magic_key: bool,
}

/// Derive passives from the `$785-$78C` possession row.
pub const fn item_passive(items: [u8; 8]) -> ItemPassives {
    ItemPassives {
        candle_lit: items[ITEM_CANDLE] != 0,
        glove_break: items[ITEM_GLOVE] != 0,
        raft_float: items[ITEM_RAFT] != 0,
        boots_stride: items[ITEM_BOOTS] != 0,
        flute_song: items[ITEM_FLUTE] != 0,
        cross_sight: items[ITEM_CROSS] != 0,
        hammer_smash: items[ITEM_HAMMER] != 0,
        magic_key: items[ITEM_KEY] != 0,
    }
}

// ---------------------------------------------------------------------------
// Death / lives / continue.
// ---------------------------------------------------------------------------

/// Death state (`$76C` values).
pub const STATE_RESTART_CASTLE: u8 = 0x00;
/// Death state: in game.
pub const STATE_INGAME: u8 = 0x01;
/// Death state: die.
pub const STATE_DIE: u8 = 0x02;
/// Death state: lives screen then restart.
pub const STATE_LIVES_SCREEN: u8 = 0x06;

/// Death gate (`bank7_check_if_link_died…`, `$D3CC-$D3E6`): `$494 != 0` +
/// `$050C == 0` (injured window expired) commits death (`$76C = 2`, sound
/// `$EC = 1`, `$494` cleared); otherwise no change. Returns
/// `Some((game_state, sound))` on commit.
pub const fn death_check(kill_flag: u8, injured: u8) -> Option<(u8, u8)> {
    if kill_flag != 0 && injured == 0 {
        Some((STATE_DIE, 0x01))
    } else {
        None
    }
}

/// Lives-screen step (`bank7_Load_Lives_Remaining_Screen`, `$C3D4-$C407`):
/// lives nonzero → respawn in place (`$29/$CC/$80/$13` reset); zero →
/// game over (continue flow). Returns true when the run continues.
pub const fn lives_screen(lives: u8) -> bool {
    lives != 0
}

/// Continue flow: deaths (`$79F`)++, lives reset to 3
/// (`bank7_Reset_Number_of_Lives__to_3_`, `$C35A`), state back to in-game.
/// Returns `(lives, deaths, game_state)`.
pub const fn continue_flow(deaths: u8) -> (u8, u8, u8) {
    (0x03, deaths.wrapping_add(1), STATE_INGAME)
}

/// Lives-screen Link reset values (`$C3F7-$C407`): `$29 = $CC = 0`,
/// `$80 = 0`, `$13 = 0` (plus `$076F = 0` fairy-clear first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivesReset {
    /// New `$29`.
    pub y: u8,
    /// New `$CC`.
    pub screen_x: u8,
    /// New `$80`.
    pub anim: u8,
    /// New `$13`.
    pub fairy: u8,
}

/// Lives-screen reset constants.
pub const fn lives_reset() -> LivesReset {
    LivesReset {
        y: 0x00,
        screen_x: 0x00,
        anim: 0x00,
        fairy: 0x00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_table_spot_checks() {
        // Shield L1 = $40, Thunder L8 = $80, Fairy L1 = $A0.
        assert_eq!(spell_cost(0, 1), 0x40);
        assert_eq!(spell_cost(7, 8), 0x80);
        assert_eq!(spell_cost(3, 1), 0xA0);
        assert_eq!(SPELL_BITS, [1, 2, 4, 8, 0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn cast_gate_paths() {
        let base = CastGate {
            dialog: 0,
            menu: 0,
            lock: 0,
            selector: 0,
            last_cast: 5,
            select_pressed: true,
            learned: true,
            meter: 0x80,
            cost: 0x40,
        };
        assert_eq!(cast_gate(base), CastOutcome::Cast { meter: 0x40 });
        // Re-cast guard.
        let r = CastGate {
            selector: 4,
            last_cast: 5,
            ..base
        };
        assert_eq!(cast_gate(r), CastOutcome::None);
        // Short meter fails…
        let s = CastGate {
            meter: 0x10,
            ..base
        };
        assert_eq!(cast_gate(s), CastOutcome::None);
        // …unless it underflows to exactly $FF (free-cast quirk).
        let q = CastGate {
            meter: 0x3F,
            cost: 0x40,
            ..base
        };
        assert_eq!(cast_gate(q), CastOutcome::CastFree);
        // Blocked while talking.
        let b = CastGate { dialog: 2, ..base };
        assert_eq!(cast_gate(b), CastOutcome::None);
    }

    #[test]
    fn cast_bookkeeping() {
        let fx = cast_spell(0, 0, 0);
        assert_eq!(fx.magic_state, 0x01);
        assert_eq!(fx.last_cast, 0x01);
        assert_eq!((fx.pane, fx.flash), (0x80, 0x20));
        let t = cast_spell(6, 0, 0);
        assert_eq!(t.flash, 0xA0);
    }

    #[test]
    fn meter_and_exp_ticks() {
        assert_eq!(meter_cap(4), 0x7F);
        assert_eq!(magic_regen_tick(0x10, 0x02, 0x7F, 0), (0x12, 0x01, true));
        assert_eq!(magic_regen_tick(0x7E, 0x02, 0x7F, 1), (0x7F, 0x00, true));
        assert_eq!(magic_regen_tick(0x10, 0x00, 0x7F, 0).1, 0x00);
        // Exp drip: 10/frame, 1/frame under 10.
        assert_eq!(
            exp_trickle(0x00, 0x00, 0x00, 0x14),
            (0x00, 0x0A, 0x00, 0x0A)
        );
        assert_eq!(
            exp_trickle(0x00, 0x00, 0x00, 0x05),
            (0x00, 0x01, 0x00, 0x04)
        );
        assert_eq!(exp_loss_tick(0x01, 0x00, 0x03), (0x00, 0xFF, 0x02));
    }

    #[test]
    fn leveling_chart() {
        // Attack L1→L2 costs 0x14 per the chart comment.
        assert_eq!(next_level_for(STAT_ATTACK, 1), 0x0014);
        assert_eq!(next_level_for(STAT_MAGIC, 2), 0x012C);
        assert!(level_ready(0x012C, 0x012C));
        assert!(!level_ready(0x012B, 0x012C));
        assert_eq!(level_up_choice(STAT_ATTACK, 1), (2, next_level_for(0, 2)));
        assert_eq!(next_level_for(0, 8), 0xFFFF);
    }

    #[test]
    fn pickups_and_items() {
        assert_eq!(item_pickup(0x00, 4, false), Pickup::Inventory { slot: 0 });
        assert_eq!(
            item_pickup(0x08, 4, true),
            Pickup::Key { unlock_boss: true }
        );
        assert_eq!(
            item_pickup(0x0E, 4, false),
            Pickup::Container {
                magic: true,
                pending: 0
            }
        );
        assert_eq!(item_pickup(0x12, 4, false), Pickup::Doll);
        let (nc, kasuto, _) = container_pickup(6, 3, true);
        assert_eq!((nc, kasuto), (7, true));
        let p = item_passive([1, 1, 0, 0, 0, 1, 0, 1]);
        assert!(p.candle_lit && p.glove_break && p.cross_sight && p.magic_key);
        assert!(!p.raft_float);
    }

    #[test]
    fn death_and_continue() {
        assert_eq!(death_check(1, 0), Some((STATE_DIE, 0x01)));
        assert_eq!(death_check(1, 5), None);
        assert_eq!(death_check(0, 0), None);
        assert!(lives_screen(2));
        assert!(!lives_screen(0));
        assert_eq!(continue_flow(0xFE), (3, 0xFF, STATE_INGAME));
    }

    #[test]
    fn pause_and_selector() {
        assert_eq!(pause_step(0, true, 0), MENU_PAUSED);
        assert_eq!(pause_step(1, true, 0), MENU_UNPAUSE);
        assert_eq!(pause_step(3, false, 0), MENU_CLOSED);
        assert_eq!(pause_step(0, true, 2), MENU_CLOSED);
        assert_eq!(selector_step(0, true, false, 8), 7);
        assert_eq!(selector_step(7, false, true, 8), 0);
    }
}
