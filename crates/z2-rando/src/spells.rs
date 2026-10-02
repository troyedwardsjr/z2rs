//! `spells` module: spell placement and spell behaviour switches.
//!
//! Options owned: [`crate::flags::SpellFlags`] (`ctx.flags.spells`).
//!
//! | Option | How it is done |
//! |---|---|
//! | `shuffle_life_refill` | The Life spell's refill amount (bank 0, the immediate the Life handler adds to the pending-life counter) becomes 1-5 bars. |
//! | `shuffle_spell_locations` | The spell menu is permuted (see below), so the wizard of town `i` (who always grants menu slot `i`) teaches another spell. |
//! | `disable_magic_container_requirements` | The wizard's "enough magic containers?" branch (bank 3) is removed. |
//! | `randomize_spell_spell_enemy` | The enemy id the Spell spell writes into enemy slots (bank 0) is drawn from a list of small enemies. |
//! | `swap_up_and_down_stab` | The two sword teachers' "already known" test and "learn" bit (bank 3 immediates) are exchanged. |
//! | `fire_option` | Linked: casting the partner also casts Fire and the other way round (a new per-slot "cast bits" table). Dash: resolved here, the code is in [`crate::asm_features`]. |
//! | `jump_always_on` | The normal jump launch velocities are replaced by the Jump-spell ones (bank 0 table). |
//! | `dash_always_on` | Done in [`crate::asm_features`] (shares the dash speed hook). |
//! | `permanent_beam_sword` | The full-health test before the sword beam (bank 0) is removed. |
//! | `flute_warp` | Not implemented (see README.md). |
//!
//! # The spell menu (bank 0)
//!
//! Everything about a spell is indexed by its menu slot: the cost row
//! (`$8D7B`, 8 magic levels per slot), the effect bit set in `$076F` when it
//! is cast and tested by the per-frame spell loop (`$8DBB`), the handler the
//! loop calls (`$8E48`, one word per slot), the name row of the pause menu
//! (`$9C2A`, 14 bytes per slot, 11 of them text) and whether the cast flashes
//! the backdrop. Wizards and the "starts with" bytes of the save template
//! (bank 5 `$BAE7`) also go by slot. Moving all of them together moves a
//! spell to another slot without touching any code; only the backdrop flash
//! (a slot-number compare in the vanilla code) needs a small table.
//!
//! The menu order is recorded in [`SpellState::menu`] (slot -> vanilla spell
//! index) for later modules. The logic world keeps spells by identity
//! ([`crate::world::ItemId`]); only the wizard rewards change.
//!
//! Later modules that read the cost table (`stats`) find it already in menu
//! order; per-row edits are unaffected, but row labels should come from
//! [`SpellState::menu`].

use crate::flags::FireOption;
use crate::world::{ItemId, LocKey, Town, TownSlot};
use crate::{asm, Ctx, RandoError};

/// Bank holding the spell code and tables.
pub const SPELL_BANK: u8 = 0;
/// Spell costs: 8 slots x 8 magic levels.
pub const COST_TABLE: u16 = 0x8D7B;
/// Effect bit per slot (cast and per-frame test).
pub const EFFECT_BITS: u16 = 0x8DBB;
/// `LDA EFFECT_BITS,Y` at the cast site (the cast reads this table).
pub const CAST_LOAD: u16 = 0x8DFB;
/// Backdrop-flash choice after a cast: `LDA #$20 / CPY #$06 / BCC / ORA #$80`.
pub const FLASH_SITE: u16 = 0x8E10;
/// Where [`FLASH_SITE`] stores the flash mode (`STA $074B`).
pub const FLASH_STORE: u16 = 0x8E18;
/// Per-frame handler words, one per slot.
pub const HANDLERS: u16 = 0x8E48;
/// Pause-menu name rows (14 bytes per slot, the first 11 are text).
pub const NAME_ROWS: u16 = 0x9C2A;
/// Bytes per name row.
pub const NAME_STRIDE: u16 = 14;
/// Text bytes per name row.
pub const NAME_LEN: usize = 11;
/// Life handler: `ADC #imm` operand added to the pending life refill.
pub const LIFE_REFILL: u16 = 0x8E6A;
/// Spell-spell handler: `LDA #imm` operand, the enemy id it creates.
pub const SPELL_ENEMY: u16 = 0x91DF;
/// Jump launch velocity table: `[spell walk, spell run, normal walk,
/// normal run]` then the same for the second byte at `+4`.
pub const JUMP_VELOCITY: u16 = 0x9470;
/// `BNE` that skips the sword beam unless life is full.
pub const BEAM_BRANCH: u16 = 0x985C;
/// Save template bank and the 8 "starts with spell" bytes (menu order).
pub const START_SPELLS: (u8, u16) = (5, 0xBAE7);
/// Bank with the town NPC reward code.
pub const TOWN_BANK: u8 = 3;
/// Wizard: `BCC` taken when the player has fewer magic containers than
/// the town number + 1.
pub const WIZARD_CONTAINER_BRANCH: u16 = 0xB52B;
/// Mido teacher: "already has" test (`AND #imm`) and learn bit (`ORA #imm`).
pub const MIDO_STAB_TEST: u16 = 0xB4C3;
/// See [`MIDO_STAB_TEST`].
pub const MIDO_STAB_LEARN: u16 = 0xB4CF;
/// Darunia teacher: "already has" test and learn bit.
pub const DARUNIA_STAB_TEST: u16 = 0xB4DB;
/// See [`DARUNIA_STAB_TEST`].
pub const DARUNIA_STAB_LEARN: u16 = 0xB4E7;
/// Technique bits in `$0796`.
pub const DOWNSTAB_BIT: u8 = 0x10;
/// See [`DOWNSTAB_BIT`].
pub const UPSTAB_BIT: u8 = 0x04;

/// Vanilla spell index of Fire.
pub const FIRE: u8 = 4;
/// Vanilla spell names (index = vanilla menu slot).
pub const SPELL_NAMES: [&str; 8] = [
    "Shield", "Jump", "Life", "Fairy", "Fire", "Reflect", "Spell", "Thunder",
];
/// Vanilla spells whose cast flashes the backdrop (the vanilla code tests
/// slot >= 5).
const FLASHES_BACKDROP: [bool; 8] = [false, false, false, false, false, true, true, true];
/// Enemy ids the Spell spell may create instead of the Bot. They are the
/// small, harmless sideview enemies whose id means the same thing in every
/// area bank (the list follows the community randomizer's behaviour).
pub const SPELL_ENEMY_CHOICES: [u8; 11] = [
    0x03, 0x04, 0x06, 0x07, 0x0E, 0x10, 0x11, 0x12, 0x18, 0x19, 0x1A,
];

/// What the Fire spell became.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FireMode {
    /// Vanilla Fire.
    #[default]
    Normal,
    /// Casting Fire also casts this spell (vanilla index) and the other way
    /// round.
    Linked(u8),
    /// Fire is replaced by Dash ([`crate::asm_features`] writes the code).
    Dash,
}

/// Results of the `spells` module for later modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellState {
    /// Vanilla spell index in each menu slot (identity unless spell
    /// locations were shuffled). The wizard of `Town::WIZARD_TOWNS[i]`
    /// teaches `menu[i]`.
    pub menu: [u8; 8],
    /// The resolved Fire option.
    pub fire: FireMode,
}

impl Default for SpellState {
    fn default() -> Self {
        SpellState {
            menu: [0, 1, 2, 3, 4, 5, 6, 7],
            fire: FireMode::Normal,
        }
    }
}

impl SpellState {
    /// Menu slot that holds vanilla spell `spell`.
    #[must_use]
    pub fn slot_of(&self, spell: u8) -> usize {
        self.menu.iter().position(|&s| s == spell).unwrap_or(0)
    }

    /// Display name of the spell in `slot`.
    #[must_use]
    pub fn name(&self, slot: usize) -> &'static str {
        let s = self.menu[slot];
        if s == FIRE && self.fire == FireMode::Dash {
            "Dash"
        } else {
            SPELL_NAMES[usize::from(s)]
        }
    }
}

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f = ctx.flags.spells.clone();
    let mut st = SpellState::default();

    // Resolve every switch first, in a fixed order (determinism).
    let shuffle_locations = ctx.tri(f.shuffle_spell_locations);
    let no_wizard_containers = ctx.tri(f.disable_magic_container_requirements);
    let spell_enemy = ctx.tri(f.randomize_spell_spell_enemy);
    let swap_stabs = ctx.tri(f.swap_up_and_down_stab);
    st.fire = resolve_fire(ctx, f.fire_option);

    if f.shuffle_life_refill {
        let bars = ctx.rng.range_u8(1, 5);
        if site(ctx, SPELL_BANK, LIFE_REFILL - 1, &[0x69], "Life refill ADC")? {
            ctx.rom
                .write_cpu(SPELL_BANK, LIFE_REFILL, &[bars.wrapping_mul(16)])?;
            ctx.spoiler
                .line("Spells", format!("Life spell refill: {bars} bars"));
        }
    }

    // Spells already in the general item pool (items module) keep the
    // vanilla menu; their wizards are filled by the item placement.
    if shuffle_locations && !crate::items::npc_rewards_in_pool(ctx.flags.items.include_spells) {
        ctx.rng.shuffle(&mut st.menu);
    }

    if spell_enemy {
        let id = *ctx.rng.pick(&SPELL_ENEMY_CHOICES).unwrap_or(&0x04);
        if site(ctx, SPELL_BANK, SPELL_ENEMY - 1, &[0xA9], "Spell enemy LDA")? {
            ctx.rom.write_cpu(SPELL_BANK, SPELL_ENEMY, &[id])?;
            ctx.spoiler
                .line("Spells", format!("Spell spell creates enemy id ${id:02X}"));
        }
    }

    if no_wizard_containers
        && site(
            ctx,
            TOWN_BANK,
            WIZARD_CONTAINER_BRANCH,
            &[0x90],
            "wizard BCC",
        )?
    {
        ctx.rom
            .write_cpu(TOWN_BANK, WIZARD_CONTAINER_BRANCH, &[0xEA, 0xEA])?;
        if ctx.state.world.built {
            ctx.state.world.set_wizard_magic_requirements(false);
        }
        ctx.spoiler
            .line("Spells", "Wizards need no magic containers");
    }

    if swap_stabs && !crate::items::npc_rewards_in_pool(ctx.flags.items.include_sword_techs) {
        swap_stab_teachers(ctx)?;
    }

    if f.jump_always_on {
        let off = ctx.rom.cpu_offset(SPELL_BANK, JUMP_VELOCITY)?;
        let mut t = ctx.rom.read_slice(off, 8)?.to_vec();
        // Spell entries (0, 1 and 4, 5) over the normal ones (2, 3 and 6, 7).
        t.copy_within(0..2, 2);
        t.copy_within(4..6, 6);
        ctx.rom.write(off, &t)?;
        ctx.spoiler.line("Spells", "Jump spell height is always on");
    }

    if f.permanent_beam_sword && site(ctx, SPELL_BANK, BEAM_BRANCH, &[0xD0], "beam BNE")? {
        ctx.rom.write_cpu(SPELL_BANK, BEAM_BRANCH, &[0xEA, 0xEA])?;
        ctx.spoiler.line("Spells", "Sword beam fires at any health");
    }

    write_menu(ctx, &st)?;
    update_world(ctx, &st);
    update_hint_spots(
        ctx,
        &st,
        swap_stabs && !crate::items::npc_rewards_in_pool(ctx.flags.items.include_sword_techs),
    );

    if st.menu != SpellState::default().menu || st.fire != FireMode::Normal {
        for (slot, town) in Town::WIZARD_TOWNS.iter().enumerate() {
            let name = st.name(slot);
            ctx.spoiler
                .line("Spells", format!("{} wizard: {name}", town.name()));
        }
        if let FireMode::Linked(p) = st.fire {
            ctx.spoiler.line(
                "Spells",
                format!("Fire is linked with {}", SPELL_NAMES[usize::from(p)]),
            );
        }
    }
    ctx.state.spells = st;
    Ok(())
}

/// Pick the Fire mode (`Random` chooses one of the three, then a partner
/// for the linked mode).
fn resolve_fire(ctx: &mut Ctx, opt: FireOption) -> FireMode {
    let opt = match opt {
        FireOption::Random => match ctx.rng.index(3) {
            0 => FireOption::Normal,
            1 => FireOption::PairWithRandom,
            _ => FireOption::ReplaceWithDash,
        },
        o => o,
    };
    match opt {
        FireOption::PairWithRandom => {
            let others: Vec<u8> = (0..8).filter(|&s| s != FIRE).collect();
            FireMode::Linked(*ctx.rng.pick(&others).unwrap_or(&0))
        }
        FireOption::ReplaceWithDash => FireMode::Dash,
        _ => FireMode::Normal,
    }
}

/// Whether a patch site holds the bytes we expect. `Ok(false)` when the
/// input is not the known ROM (only synthetic test images: the patch is
/// skipped), an error when the vanilla bytes are right but an earlier
/// module already changed them (two modules patching the same code).
pub(crate) fn site(
    ctx: &Ctx,
    bank: u8,
    addr: u16,
    want: &[u8],
    what: &str,
) -> Result<bool, RandoError> {
    let read = |r: &crate::rom::Rom| -> Result<Vec<u8>, RandoError> {
        Ok(r.read_slice(r.cpu_offset(bank, addr)?, want.len())?
            .to_vec())
    };
    let got = read(&ctx.rom)?;
    if got == want {
        return Ok(true);
    }
    if read(&ctx.vanilla)? != want {
        return Ok(false);
    }
    Err(RandoError::Rom(format!(
        "{what}: {bank}:${addr:04X} was already patched ({got:02X?}, want {want:02X?})"
    )))
}

/// Exchange the up- and downstab teachers.
fn swap_stab_teachers(ctx: &mut Ctx) -> Result<(), RandoError> {
    let sites = [
        (MIDO_STAB_TEST, 0x29, DOWNSTAB_BIT, UPSTAB_BIT),
        (MIDO_STAB_LEARN, 0x09, DOWNSTAB_BIT, UPSTAB_BIT),
        (DARUNIA_STAB_TEST, 0x29, UPSTAB_BIT, DOWNSTAB_BIT),
        (DARUNIA_STAB_LEARN, 0x09, UPSTAB_BIT, DOWNSTAB_BIT),
    ];
    for (addr, op, from, _) in sites {
        if !site(ctx, TOWN_BANK, addr - 1, &[op, from], "stab teacher")? {
            return Ok(());
        }
    }
    for (addr, _, _, to) in sites {
        ctx.rom.write_cpu(TOWN_BANK, addr, &[to])?;
    }
    if ctx.state.world.built {
        let w = &mut ctx.state.world;
        if let Some(l) = w.loc_mut(LocKey::Town(TownSlot::MidoTrainer)) {
            l.item = Some(ItemId::Upstab);
        }
        if let Some(l) = w.loc_mut(LocKey::Town(TownSlot::DaruniaTrainer)) {
            l.item = Some(ItemId::Downstab);
        }
    }
    ctx.spoiler
        .line("Spells", "Mido teaches upstab, Darunia teaches downstab");
    Ok(())
}

/// Rewrite the bank-0 menu tables (and the save template's starting
/// spells) for the slot order and Fire mode in `st`. A no-op for the
/// vanilla order with normal Fire.
fn write_menu(ctx: &mut Ctx, st: &SpellState) -> Result<(), RandoError> {
    let identity = SpellState::default().menu;
    let linked = match st.fire {
        FireMode::Linked(p) => Some(p),
        _ => None,
    };
    if st.menu == identity && linked.is_none() && st.fire != FireMode::Dash {
        return Ok(());
    }
    let b = SPELL_BANK;
    let read = |ctx: &Ctx, addr: u16, len: usize| -> Result<Vec<u8>, RandoError> {
        let off = ctx.rom.cpu_offset(b, addr)?;
        Ok(ctx.rom.read_slice(off, len)?.to_vec())
    };
    let costs = read(ctx, COST_TABLE, 64)?;
    let bits = read(ctx, EFFECT_BITS, 8)?;
    let handlers = read(ctx, HANDLERS, 16)?;
    let names = read(ctx, NAME_ROWS, usize::from(NAME_STRIDE) * 8)?;
    let start_off = ctx.rom.cpu_offset(START_SPELLS.0, START_SPELLS.1)?;
    let starts = ctx.rom.read_slice(start_off, 8)?.to_vec();

    let mut n_costs = costs.clone();
    let mut n_bits = bits.clone();
    let mut n_handlers = handlers.clone();
    let mut n_names = names.clone();
    let mut n_starts = starts.clone();
    for (slot, &s) in st.menu.iter().enumerate() {
        let s = usize::from(s);
        n_costs[slot * 8..slot * 8 + 8].copy_from_slice(&costs[s * 8..s * 8 + 8]);
        n_bits[slot] = bits[s];
        n_handlers[slot * 2..slot * 2 + 2].copy_from_slice(&handlers[s * 2..s * 2 + 2]);
        let row = usize::from(NAME_STRIDE);
        n_names[slot * row..slot * row + NAME_LEN]
            .copy_from_slice(&names[s * row..s * row + NAME_LEN]);
        n_starts[slot] = starts[s];
    }
    if st.fire == FireMode::Dash {
        let slot = st.slot_of(FIRE);
        let row = usize::from(NAME_STRIDE);
        let text = name_row("DASH")?;
        n_names[slot * row..slot * row + NAME_LEN].copy_from_slice(&text);
    }
    let write = |ctx: &mut Ctx, addr: u16, v: &[u8]| ctx.rom.write_cpu(b, addr, v);
    write(ctx, COST_TABLE, &n_costs)?;
    write(ctx, EFFECT_BITS, &n_bits)?;
    write(ctx, HANDLERS, &n_handlers)?;
    write(ctx, NAME_ROWS, &n_names)?;
    ctx.rom.write(start_off, &n_starts)?;

    // The backdrop flash follows the spell, not the slot.
    if st.menu != identity
        && site(ctx, b, FLASH_SITE, &[0xA9], "flash LDA")?
        && site(ctx, b, FLASH_STORE, &[0x8D], "flash STA")?
    {
        let table: Vec<u8> = st
            .menu
            .iter()
            .map(|&s| {
                if FLASHES_BACKDROP[usize::from(s)] {
                    0xA0
                } else {
                    0x20
                }
            })
            .collect();
        let at = ctx.rom.alloc_vanilla(b, table.len())?;
        ctx.rom.write_cpu(b, at, &table)?;
        // Y holds slot + 1 here.
        let src = format!(
            ".org ${FLASH_SITE:04X}\n LDA ${:04X},Y\n JMP ${FLASH_STORE:04X}\n",
            at - 1
        );
        let out = asm::assemble(&src)?;
        ctx.rom.apply_asm(b, &out)?;
    }

    // Linked Fire: the cast reads its own bit table, so the per-frame loop
    // still calls each handler for its own bit only (handlers that clear
    // their bit, such as Life, run once).
    let cast_ok = linked.is_some() && site(ctx, b, CAST_LOAD, &[0xB9], "cast LDA abs,Y")?;
    if let Some(p) = linked.filter(|_| cast_ok) {
        let fire_slot = st.slot_of(FIRE);
        let partner_slot = st.slot_of(p);
        let mut cast = n_bits.clone();
        let both = n_bits[fire_slot] | n_bits[partner_slot];
        cast[fire_slot] = both;
        cast[partner_slot] = both;
        let at = ctx.rom.alloc_vanilla(b, 8)?;
        ctx.rom.write_cpu(b, at, &cast)?;
        ctx.rom.write_cpu_word(b, CAST_LOAD + 1, at)?;
    }
    Ok(())
}

/// One 11-byte menu name row: the name, then dots.
fn name_row(name: &str) -> Result<Vec<u8>, RandoError> {
    let mut v = crate::text::encode(name)?;
    if v.len() > NAME_LEN {
        return Err(RandoError::Text(format!("spell name {name:?} too long")));
    }
    v.resize(NAME_LEN, 0xCF);
    Ok(v)
}

/// Tell the logic world which spell each wizard teaches.
fn update_world(ctx: &mut Ctx, st: &SpellState) {
    if !ctx.state.world.built {
        return;
    }
    let spell_item = |s: u8| -> ItemId {
        if s == FIRE && st.fire == FireMode::Dash {
            ItemId::Dash
        } else {
            ItemId::SPELLS[usize::from(s)]
        }
    };
    let w = &mut ctx.state.world;
    if !crate::items::npc_rewards_in_pool(ctx.flags.items.include_spells) {
        for (slot, town) in Town::WIZARD_TOWNS.iter().enumerate() {
            if let Some(l) = w.loc_mut(LocKey::Town(TownSlot::Wizard(*town))) {
                let item = spell_item(st.menu[slot]);
                l.vanilla = Some(item);
                l.item = Some(item);
            }
        }
    }
    if st.fire == FireMode::Dash {
        if w.start.has(ItemId::Fire) {
            w.start.remove(ItemId::Fire);
            w.start.add(ItemId::Dash);
        }
        for l in &mut w.locs {
            if l.item == Some(ItemId::Fire) {
                l.item = Some(ItemId::Dash);
            }
            if l.vanilla == Some(ItemId::Fire) {
                l.vanilla = Some(ItemId::Dash);
            }
        }
    }
}

/// Keep the hints' picture of the wizards and teachers in step (only when
/// no earlier module filled it; `items` rebuilds it from the world when it
/// places items).
fn update_hint_spots(ctx: &mut Ctx, st: &SpellState, stabs_swapped: bool) {
    use crate::hints::{self, Place};
    let spells_moved = !crate::items::npc_rewards_in_pool(ctx.flags.items.include_spells)
        && (st.menu != SpellState::default().menu || st.fire == FireMode::Dash);
    if !spells_moved && !stabs_swapped {
        return;
    }
    if ctx.state.hint_spots.is_empty() {
        ctx.state.hint_spots = hints::vanilla_spots();
    }
    for spot in &mut ctx.state.hint_spots {
        match spot.place {
            Place::Wizard(t) if spells_moved => {
                if let Some(slot) = hints::Town::WIZARDS.iter().position(|&w| w == t) {
                    spot.item = st.name(slot).to_ascii_uppercase();
                }
            }
            Place::Teacher(hints::Town::Mido) if stabs_swapped => spot.item = "UPSTAB".into(),
            Place::Teacher(hints::Town::Darunia) if stabs_swapped => {
                spot.item = "DOWNSTAB".into();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Tri;

    #[test]
    fn name_rows_pad_with_dots() {
        let r = name_row("DASH").unwrap();
        assert_eq!(r.len(), NAME_LEN);
        assert_eq!(&r[4..], &[0xCF; 7]);
        assert!(name_row("TWELVE CHARS").is_err());
    }

    #[test]
    fn slot_lookup_and_names() {
        let st = SpellState {
            menu: [7, 6, 5, 4, 3, 2, 1, 0],
            fire: FireMode::Dash,
        };
        assert_eq!(st.slot_of(7), 0);
        assert_eq!(st.slot_of(FIRE), 3);
        assert_eq!(st.name(3), "Dash");
        assert_eq!(st.name(0), "Thunder");
    }

    /// ROM-gated: every spell option on, several seeds. The menu tables
    /// move together, the patched sites hold what we wrote, and two runs
    /// agree.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_spell_options_move_tables_together() {
        use crate::flags::{Flags, FluteWarp};
        use crate::rom::Rom;
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let van = Rom::from_body(&body).unwrap();
        let mut f = Flags::default();
        let s = &mut f.spells;
        s.shuffle_life_refill = true;
        s.shuffle_spell_locations = Tri::On;
        s.disable_magic_container_requirements = Tri::On;
        s.randomize_spell_spell_enemy = Tri::On;
        s.swap_up_and_down_stab = Tri::On;
        s.fire_option = FireOption::Random;
        s.jump_always_on = true;
        s.dash_always_on = true;
        s.permanent_beam_sword = true;
        s.flute_warp = FluteWarp::None;
        let mut moved = 0;
        for seed in 0..12 {
            let seed = format!("spells-{seed}");
            let out = crate::randomize(&body, &seed, &f).unwrap();
            assert_eq!(out, crate::randomize(&body, &seed, &f).unwrap());
            let rom = Rom::from_body(&out.body).unwrap();
            let rd = |r: &Rom, bank: u8, a: u16, n: usize| {
                r.read_slice(r.cpu_offset(bank, a).unwrap(), n)
                    .unwrap()
                    .to_vec()
            };
            let vbits = rd(&van, 0, EFFECT_BITS, 8);
            let bits = rd(&rom, 0, EFFECT_BITS, 8);
            let vcost = rd(&van, 0, COST_TABLE, 64);
            let cost = rd(&rom, 0, COST_TABLE, 64);
            let vh = rd(&van, 0, HANDLERS, 16);
            let h = rd(&rom, 0, HANDLERS, 16);
            for slot in 0..8 {
                let sp = vbits
                    .iter()
                    .position(|&b| b == bits[slot])
                    .expect("a vanilla bit");
                assert_eq!(cost[slot * 8..slot * 8 + 8], vcost[sp * 8..sp * 8 + 8]);
                assert_eq!(h[slot * 2..slot * 2 + 2], vh[sp * 2..sp * 2 + 2]);
                moved += usize::from(sp != slot);
            }
            assert_eq!(rd(&rom, 3, WIZARD_CONTAINER_BRANCH, 2), [0xEA, 0xEA]);
            assert_eq!(rd(&rom, 3, MIDO_STAB_LEARN, 1), [UPSTAB_BIT]);
            assert_eq!(rd(&rom, 3, DARUNIA_STAB_LEARN, 1), [DOWNSTAB_BIT]);
            assert_eq!(rd(&rom, 0, BEAM_BRANCH, 2), [0xEA, 0xEA]);
            let refill = rd(&rom, 0, LIFE_REFILL, 1)[0];
            assert!(refill % 16 == 0 && (16..=80).contains(&refill));
            assert!(SPELL_ENEMY_CHOICES.contains(&rd(&rom, 0, SPELL_ENEMY, 1)[0]));
            let jv = rd(&rom, 0, JUMP_VELOCITY, 8);
            assert_eq!(jv[0..2], jv[2..4]);
            assert_eq!(jv[4..6], jv[6..8]);
            assert!(out.spoiler.contains("Rauru wizard: "));
            // Nothing in the fixed bank changes for these options.
            assert!(out.fixed_bank_changes.is_empty(), "{seed}");
        }
        assert!(moved > 0, "some seed moved a spell");
    }
}
