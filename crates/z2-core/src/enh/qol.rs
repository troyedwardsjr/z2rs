//! Quality-of-life enhancements (group `Q`).
//!
//! Options ([`QolOpts`]) and their ZALiA reference behaviour:
//!
//! * `continue_from` ([`ContinueFrom`]; ZALiA `mod_ContinueFrom`): after a
//!   game over in a palace, continue at that palace's entrance; after one
//!   anywhere else, continue at the last town visited. The original always
//!   continues at the North Palace (the Great Palace keeps its own original
//!   restart). Loading a file from File Select is unchanged.
//! * `gameover_keep_xp_pct` (`0` = original, `0..=100`): share of the XP
//!   kept on game over (ZALiA `mod_Gameover_XP_PENALTY=0.25`, i.e. keep 25%).
//! * `lives_from_dolls`: lives on a continue start at 3 plus one per Life
//!   Doll collected, so dolls are permanent (ZALiA `mod_START_RUN_LIVES=1`).
//! * `enter_town_from_side`: walking into a town while moving right on the
//!   overworld starts Link at the town's left end, facing right; any other
//!   approach keeps the original right-end start (ZALiA
//!   `mod_EnterTownSideOption=1`).
//! * `wise_men_restore_mp`: a wise man refills MP when he teaches his spell
//!   (ZALiA `mod_WISEMEN_RESTORE_MP=1`).
//! * `no_mp_requirement_for_spells`: learning a spell no longer requires a
//!   minimum magic-container count (ZALiA `mod_AcquireSpellRequirement=1`).
//! * `overworld_softlock_warp`: pause on the overworld, then hold
//!   **Select + A + B** for 3 s (180 frames): Link dies on the spot and the
//!   game-over CONTINUE/SAVE screen follows, as after a real game over, but
//!   no XP is lost (ZALiA failsafe, `OverworldSoftlock_DURATION0 = 3*60`).
//!
//! # Mechanism (all clean-room, from the z2rs disassembly notes)
//!
//! * **Death** (`bank7_code16`, `$CA24`, hooked over its port): reads the XP
//!   word (`$0775`/`$0776`, big-endian) before the original zeroes it on a
//!   game over, then writes back `pct`% of it (all of it for a softlock
//!   warp).
//! * **Game-over choice** (`LCA85`, `$CA85`, hooked over its port): the
//!   original Continue path zeroes XP a second time; the hook restores the
//!   kept value. Continuing from anywhere but the Great Palace sets
//!   `$076C = 0`, so the hook marks a pending continue. The Great Palace
//!   restart (`$076C = 1`) only gets the lives rule.
//! * **Restart** (`$076C = 0` handler, `$C34F`: clear RAM, `$AA08` puts
//!   region/world/area at the North Palace, lives = 3): with a pending
//!   continue the hook then points `$0706` (overworld region), `$070A` and
//!   `$0748` (overworld key-area slot) at the recorded palace or town. The
//!   original loader chain (mode 0 → … → mode 6, `$CB35`) then enters that
//!   area exactly as if Link had walked onto its overworld tile. Without a
//!   pending continue `$C34F` is a fresh game start (title / File Select),
//!   and the hook clears the records and the doll count.
//! * **Area entry from the overworld** (`bank7_code17`, `$CB35`, hooked over
//!   its port): when the entered area is a town (`$0707` 1-2) or a palace
//!   (`$0707` 3-5), records the overworld region (`$0706`, `$070A`) and slot
//!   (`$0748`) read *before* the original runs. For towns it also applies
//!   `enter_town_from_side`: the area-map byte gives the entry index
//!   (`$075C`, 3 = right end on every town tile); the hook rewrites it to
//!   0 (left end) and the side bit `$0701` to 0 when Link's overworld
//!   facing `$0562` is right (`$01`).
//! * **Item grant** (`bank7_get_item`, `$E771`): item code `$12` is the
//!   Life Doll; the hook counts it before the original runs.
//! * **Wise men** (bank 3 `$B518-$B54B`): the container check is
//!   `LDA $0783 : CMP $01 : BCC deny` (`$B526`); `no_mp_requirement_for_spells`
//!   patches the `BCC` at `$B52B` to two `NOP`s. A learned spell shows up as
//!   a new bit in `$077B-$0782`; `wise_men_restore_mp` watches for that at
//!   the end of each frame while in a town and requests the healer's full
//!   magic refill (`$070C = $FF`).
//! * **Softlock warp** (end of frame): overworld (`$0736 == 5`, `$0707 ==
//!   0`) and paused (`$0524 == 2`) with the chord held counts
//!   `EnhState::timers[0]`; at 180 the hook sets lives to 1 and starts the
//!   die routine (`$076C = 2`).
//!
//! # Runtime state ([`super::EnhState`] slots owned here)
//!
//! * `continue_loc[0]`: last town, `(region << 6) | slot`.
//! * `continue_loc[1]`: palace entered, `(region << 6) | slot`.
//! * `continue_loc[2]`: `$070A` read when that palace was entered.
//! * `continue_loc[3]`: bit 0 town recorded, bit 1 palace recorded.
//! * `continue_valid`: `continue_loc[3] != 0`.
//! * `timers[0]`: softlock-warp chord hold counter (frames).
//! * `flags` bits 16-23: spells known last frame (`$077B-$0782` as bits);
//!   bit 24: that snapshot is primed; bit 25: continue pending; bit 26:
//!   softlock warp in progress (keep all XP).
//! * `scratch[7]`: Life Dolls collected since the game was started or
//!   loaded (a save state keeps it; a battery save does not, so a file
//!   loaded from File Select starts at 0).

use serde::{Deserialize, Serialize};

use crate::game::Game;

/// Where to continue after a game over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueFrom {
    /// Original: the North Palace.
    #[default]
    Og,
    /// In a palace: that palace's entrance (overworld game overs stay
    /// original).
    DungeonEntrance,
    /// The last town visited.
    LastTown,
    /// Palace entrance inside a palace, last town elsewhere (ZALiA).
    Both,
}

impl ContinueFrom {
    /// Stable identity byte.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            ContinueFrom::Og => 0,
            ContinueFrom::DungeonEntrance => 1,
            ContinueFrom::LastTown => 2,
            ContinueFrom::Both => 3,
        }
    }

    /// Every value, in identity order (for menus).
    pub const ALL: [ContinueFrom; 4] = [
        ContinueFrom::Og,
        ContinueFrom::DungeonEntrance,
        ContinueFrom::LastTown,
        ContinueFrom::Both,
    ];

    /// Menu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ContinueFrom::Og => "Original (North Palace)",
            ContinueFrom::DungeonEntrance => "Palace entrance",
            ContinueFrom::LastTown => "Last town visited",
            ContinueFrom::Both => "Palace entrance or last town",
        }
    }

    /// Whether a game over inside a palace continues at its entrance.
    #[must_use]
    pub fn uses_palace(self) -> bool {
        matches!(self, ContinueFrom::DungeonEntrance | ContinueFrom::Both)
    }

    /// Whether a game over outside a palace continues at the last town.
    #[must_use]
    pub fn uses_town(self) -> bool {
        matches!(self, ContinueFrom::LastTown | ContinueFrom::Both)
    }
}

/// Quality-of-life options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QolOpts {
    /// Continue location after a game over.
    pub continue_from: ContinueFrom,
    /// Percent of XP kept on game over (`0` = original).
    pub gameover_keep_xp_pct: u8,
    /// Lives = 3 + Life Dolls collected.
    pub lives_from_dolls: bool,
    /// Enter towns from the side you approached.
    pub enter_town_from_side: bool,
    /// Wise men refill MP.
    pub wise_men_restore_mp: bool,
    /// No magic-container requirement to learn spells.
    pub no_mp_requirement_for_spells: bool,
    /// Overworld softlock failsafe warp.
    pub overworld_softlock_warp: bool,
}

impl QolOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        out.push(self.continue_from.id());
        out.push(self.gameover_keep_xp_pct);
        out.push(u8::from(self.lives_from_dolls));
        out.push(u8::from(self.enter_town_from_side));
        out.push(u8::from(self.wise_men_restore_mp));
        out.push(u8::from(self.no_mp_requirement_for_spells));
        out.push(u8::from(self.overworld_softlock_warp));
    }

    /// Whether the game-over / continue hooks are needed.
    fn needs_gameover_hooks(&self) -> bool {
        self.continue_from != ContinueFrom::Og
            || self.gameover_keep_xp_pct > 0
            || self.lives_from_dolls
            || self.overworld_softlock_warp
    }
}

// ------------------------------------------------------------ addresses

/// Lives (`$0700`).
pub const ADDR_LIVES: u16 = 0x0700;
/// Side flag of the area entry (`$0701`, `$075C & 1`).
pub const ADDR_ENTRY_SIDE: u16 = 0x0701;
/// Overworld region (`$0706`: 0 West, 1 Death Mountain / Maze Island, 2 East).
pub const ADDR_REGION: u16 = 0x0706;
/// World (`$0707`: 0 overworld areas, 1-2 towns, 3-5 palaces).
pub const ADDR_WORLD: u16 = 0x0707;
/// Previous region (`$070A`; picks Death Mountain vs Maze Island banks).
pub const ADDR_PREV_REGION: u16 = 0x070A;
/// Pending magic refill (`$070C`; the healer writes `$FF`).
pub const ADDR_MAGIC_REFILL: u16 = 0x070C;
/// Menu control (`$0524`: overworld pause 1 → 2 paused → 3 → 0).
pub const ADDR_MENU: u16 = 0x0524;
/// Overworld facing (`$0562`: d-pad bits, `$01` right, `$02` left).
pub const ADDR_OW_FACING: u16 = 0x0562;
/// Game mode (`$0736`; 5 = overworld, 6 = enter area).
pub const ADDR_MODE: u16 = 0x0736;
/// Key-area slot / area index (`$0748`).
pub const ADDR_AREA: u16 = 0x0748;
/// Area entry index (`$075C`; 0 left end, 3 right end).
pub const ADDR_ENTRY: u16 = 0x075C;
/// Boot stage / special routine (`$076C`: 0 restart, 1 in game, 2 die).
pub const ADDR_STAGE: u16 = 0x076C;
/// Current magic (`$0773`).
pub const ADDR_MAGIC: u16 = 0x0773;
/// Experience, big-endian (`$0775` high, `$0776` low).
pub const ADDR_XP_HI: u16 = 0x0775;
/// Experience low byte (`$0776`).
pub const ADDR_XP_LO: u16 = 0x0776;
/// Spells known (`$077B-$0782`).
pub const ADDR_SPELLS: u16 = 0x077B;
/// Magic containers (`$0783`).
pub const ADDR_MAGIC_CONTAINERS: u16 = 0x0783;

/// `bank7_code16` (death entry).
pub const HOOK_DIE: u16 = 0xCA24;
/// `LCA85` (game-over Start/Select handler).
pub const HOOK_GAMEOVER_CHOICE: u16 = 0xCA85;
/// `$076C = 0` restart handler (`bank7_pointer_table2` entry 0).
pub const HOOK_RESTART: u16 = 0xC34F;
/// `bank7_code17` (mode 6: enter an area from the overworld).
pub const HOOK_AREA_ENTRY: u16 = 0xCB35;
/// `bank7_get_item`.
pub const HOOK_GET_ITEM: u16 = 0xE771;
/// Wise-man container check branch (bank 3, `BCC deny`).
pub const WISE_MAN_CHECK: u16 = 0xB52B;
/// Original bytes at [`WISE_MAN_CHECK`] (`BCC $B54B`).
pub const WISE_MAN_CHECK_BYTES: [u8; 2] = [0x90, 0x1E];

/// Life Doll item code (`$AF,x & $7F`).
pub const ITEM_DOLL: u8 = 0x12;
/// Softlock-warp chord, in [`Game::step`] input bits: Select + A + B.
pub const SOFTLOCK_CHORD: u8 =
    (1 << crate::game::INPUT_SELECT) | (1 << crate::game::INPUT_A) | (1 << crate::game::INPUT_B);
/// Frames the chord must be held (3 s).
pub const SOFTLOCK_FRAMES: u16 = 180;
/// Most lives the lives screen can show as one digit.
pub const MAX_LIVES: u8 = 9;

const LOC_TOWN: usize = 0;
const LOC_PALACE: usize = 1;
const LOC_PALACE_PREV: usize = 2;
const LOC_BITS: usize = 3;
const BIT_TOWN: u8 = 0x01;
const BIT_PALACE: u8 = 0x02;

const FLAG_SPELL_SHIFT: u32 = 16;
const FLAG_SPELLS_PRIMED: u32 = 1 << 24;
const FLAG_CONTINUE_PENDING: u32 = 1 << 25;
const FLAG_WARP: u32 = 1 << 26;
const FLAGS_OWNED: u32 = 0x07FF_0000;
const SCRATCH_DOLLS: usize = 7;

// ------------------------------------------------------------ pure helpers

/// Pack an overworld region and key-area slot into one byte.
#[must_use]
pub const fn pack_loc(region: u8, slot: u8) -> u8 {
    ((region & 0x03) << 6) | (slot & 0x3F)
}

/// Inverse of [`pack_loc`]: `(region, slot)`.
#[must_use]
pub const fn unpack_loc(b: u8) -> (u8, u8) {
    (b >> 6, b & 0x3F)
}

/// XP kept on game over: `pct`% of `xp`, rounded down (`pct` capped at 100).
#[must_use]
pub fn kept_xp(xp: u16, pct: u8) -> u16 {
    let pct = u32::from(pct.min(100));
    (u32::from(xp) * pct / 100) as u16
}

/// Lives on a continue: 3 plus one per doll, capped at [`MAX_LIVES`].
#[must_use]
pub const fn lives_with_dolls(dolls: u8) -> u8 {
    let n = 3u16 + dolls as u16;
    if n > MAX_LIVES as u16 {
        MAX_LIVES
    } else {
        n as u8
    }
}

/// Town entry index for an overworld facing: moving right enters at the
/// left end (0), moving left at the right end (3), otherwise `og`.
#[must_use]
pub const fn town_entry_for_facing(facing: u8, og: u8) -> u8 {
    match facing {
        0x01 => 0,
        0x02 => 3,
        _ => og,
    }
}

/// Where a continue should restart, as `(region, prev_region, slot)`, or
/// `None` for the original North Palace. `world` is `$0707` at the game
/// over; `loc` is [`super::EnhState::continue_loc`].
#[must_use]
pub fn continue_target(mode: ContinueFrom, world: u8, loc: [u8; 4]) -> Option<(u8, u8, u8)> {
    let in_palace = matches!(world, 3 | 4);
    if in_palace && mode.uses_palace() && loc[LOC_BITS] & BIT_PALACE != 0 {
        let (r, s) = unpack_loc(loc[LOC_PALACE]);
        return Some((r, loc[LOC_PALACE_PREV], s));
    }
    if mode.uses_town() && loc[LOC_BITS] & BIT_TOWN != 0 {
        let (r, s) = unpack_loc(loc[LOC_TOWN]);
        return Some((r, r, s));
    }
    None
}

// ------------------------------------------------------------ RAM helpers

fn rd(game: &Game, addr: u16) -> u8 {
    game.ram[usize::from(addr) & 0x07FF]
}

fn wr(game: &mut Game, addr: u16, v: u8) {
    game.ram[usize::from(addr) & 0x07FF] = v;
}

fn xp(game: &Game) -> u16 {
    u16::from_be_bytes([rd(game, ADDR_XP_HI), rd(game, ADDR_XP_LO)])
}

fn set_xp(game: &mut Game, v: u16) {
    let [hi, lo] = v.to_be_bytes();
    wr(game, ADDR_XP_HI, hi);
    wr(game, ADDR_XP_LO, lo);
}

fn spells_mask(game: &Game) -> u8 {
    (0..8u16).fold(0u8, |m, i| {
        if rd(game, ADDR_SPELLS + i) != 0 {
            m | (1 << i)
        } else {
            m
        }
    })
}

/// Life Dolls counted since the game was started (see the module docs).
#[must_use]
pub fn dolls_collected(game: &Game) -> u8 {
    game.enh_state.scratch[SCRATCH_DOLLS]
}

// ------------------------------------------------------------ hooks

/// `bank7_code16`: keep `pct`% (or, after a softlock warp, all) of the XP
/// the original zeroes on a game over.
fn hook_die(game: &mut Game) {
    let before = xp(game);
    super::call_original(game, HOOK_DIE);
    let game_over = rd(game, ADDR_LIVES) == 0 && rd(game, ADDR_STAGE) != 6;
    if !game_over {
        return;
    }
    let warp = game.enh_state.flags & FLAG_WARP != 0;
    let pct = game.enh.qol.gameover_keep_xp_pct;
    if warp {
        set_xp(game, before);
    } else if pct > 0 {
        set_xp(game, kept_xp(before, pct));
    }
}

/// `LCA85`: undo the Continue path's second XP wipe, flag the restart for
/// [`hook_restart`], and give the Great Palace restart its doll lives.
fn hook_gameover_choice(game: &mut Game) {
    let xp_before = xp(game);
    let stage_before = rd(game, ADDR_STAGE);
    let mode_before = rd(game, ADDR_MODE);
    super::call_original(game, HOOK_GAMEOVER_CHOICE);
    let stage = rd(game, ADDR_STAGE);
    let q = game.enh.qol;
    let warp = game.enh_state.flags & FLAG_WARP != 0;
    if stage != stage_before {
        // Continue chosen: 0 = normal restart, 1 = Great Palace restart.
        if warp || q.gameover_keep_xp_pct > 0 {
            set_xp(game, xp_before);
        }
        if stage == 0 {
            game.enh_state.flags |= FLAG_CONTINUE_PENDING;
        } else if q.lives_from_dolls {
            let lives = lives_with_dolls(dolls_collected(game));
            wr(game, ADDR_LIVES, lives);
        }
        game.enh_state.flags &= !FLAG_WARP;
    } else if rd(game, ADDR_MODE) != mode_before {
        // SAVE chosen (the save then returns to the title).
        game.enh_state.flags &= !FLAG_WARP;
    }
}

/// `$076C = 0` restart: on a pending continue, aim the loader at the
/// recorded palace or town and apply the doll lives; otherwise (new game
/// or File Select) forget everything recorded.
fn hook_restart(game: &mut Game) {
    let world = rd(game, ADDR_WORLD);
    super::call_original(game, HOOK_RESTART);
    let q = game.enh.qol;
    let st = &mut game.enh_state;
    if st.flags & FLAG_CONTINUE_PENDING == 0 {
        st.continue_loc = [0; 4];
        st.continue_valid = false;
        st.scratch[SCRATCH_DOLLS] = 0;
        st.flags &= !FLAGS_OWNED;
        st.timers[0] = 0;
        return;
    }
    st.flags &= !FLAG_CONTINUE_PENDING;
    let loc = st.continue_loc;
    if let Some((region, prev, slot)) = continue_target(q.continue_from, world, loc) {
        wr(game, ADDR_REGION, region);
        wr(game, ADDR_PREV_REGION, prev);
        wr(game, ADDR_AREA, slot);
    }
    if q.lives_from_dolls {
        let lives = lives_with_dolls(dolls_collected(game));
        wr(game, ADDR_LIVES, lives);
    }
}

/// `bank7_code17` (mode 6): record towns and palaces entered from the
/// overworld; enter towns from the approach side.
fn hook_area_entry(game: &mut Game) {
    let region = rd(game, ADDR_REGION);
    let prev = rd(game, ADDR_PREV_REGION);
    let slot = rd(game, ADDR_AREA);
    let facing = rd(game, ADDR_OW_FACING);
    super::call_original(game, HOOK_AREA_ENTRY);
    // The town/palace branch sets the world and restarts the mode at 0.
    if rd(game, ADDR_MODE) != 0 {
        return;
    }
    let world = rd(game, ADDR_WORLD);
    let q = game.enh.qol;
    match world {
        1 | 2 => {
            let st = &mut game.enh_state;
            st.continue_loc[LOC_TOWN] = pack_loc(region, slot);
            st.continue_loc[LOC_BITS] |= BIT_TOWN;
            st.continue_valid = true;
            if q.enter_town_from_side {
                let og = rd(game, ADDR_ENTRY);
                let entry = town_entry_for_facing(facing, og);
                if entry != og {
                    wr(game, ADDR_ENTRY, entry);
                    wr(game, ADDR_ENTRY_SIDE, entry & 1);
                }
            }
        }
        3..=5 => {
            let st = &mut game.enh_state;
            st.continue_loc[LOC_PALACE] = pack_loc(region, slot);
            st.continue_loc[LOC_PALACE_PREV] = prev;
            st.continue_loc[LOC_BITS] |= BIT_PALACE;
            st.continue_valid = true;
        }
        _ => {}
    }
}

/// `bank7_get_item`: count Life Dolls.
fn hook_get_item(game: &mut Game) {
    let x = usize::from(game.cpu.x);
    let code = game.ram[(0x00AF + x) & 0x07FF] & 0x7F;
    if code == ITEM_DOLL {
        let d = &mut game.enh_state.scratch[SCRATCH_DOLLS];
        *d = d.saturating_add(1);
    }
    super::call_original(game, HOOK_GET_ITEM);
}

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &QolOpts) {
    if opts.needs_gameover_hooks() {
        super::hook(
            game,
            "enh_qol_gameover_choice",
            Some(7),
            HOOK_GAMEOVER_CHOICE,
            hook_gameover_choice,
            None,
        );
        // Interpreted underneath (no default port): the ROM bytes charge
        // their own cycles.
        super::hook(
            game,
            "enh_qol_restart",
            Some(7),
            HOOK_RESTART,
            hook_restart,
            Some(0),
        );
    }
    if opts.gameover_keep_xp_pct > 0 || opts.overworld_softlock_warp {
        super::hook(game, "enh_qol_die", Some(7), HOOK_DIE, hook_die, None);
    }
    if opts.continue_from != ContinueFrom::Og || opts.enter_town_from_side {
        super::hook(
            game,
            "enh_qol_area_entry",
            Some(7),
            HOOK_AREA_ENTRY,
            hook_area_entry,
            None,
        );
    }
    if opts.lives_from_dolls {
        let cycles = if game.traps.get(HOOK_GET_ITEM).is_some() {
            None
        } else {
            Some(0)
        };
        super::hook(
            game,
            "enh_qol_get_item",
            Some(7),
            HOOK_GET_ITEM,
            hook_get_item,
            cycles,
        );
    }
    if opts.no_mp_requirement_for_spells {
        let off = 3 * 0x4000 + usize::from(WISE_MAN_CHECK & 0x3FFF);
        if game.prg.get(off..off + 2) == Some(&WISE_MAN_CHECK_BYTES[..]) {
            super::patch_prg_at(game, 3, WISE_MAN_CHECK, &[0xEA, 0xEA]);
        }
    }
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
pub(crate) fn end_of_frame(game: &mut Game, opts: &QolOpts) {
    if opts.wise_men_restore_mp {
        let cur = spells_mask(game);
        let flags = game.enh_state.flags;
        let prev = ((flags >> FLAG_SPELL_SHIFT) & 0xFF) as u8;
        let primed = flags & FLAG_SPELLS_PRIMED != 0;
        if primed && cur & !prev != 0 && matches!(rd(game, ADDR_WORLD), 1 | 2) {
            wr(game, ADDR_MAGIC_REFILL, 0xFF);
        }
        game.enh_state.flags = (flags & !(0xFF << FLAG_SPELL_SHIFT))
            | (u32::from(cur) << FLAG_SPELL_SHIFT)
            | FLAG_SPELLS_PRIMED;
    }
    if opts.overworld_softlock_warp {
        let paused_overworld = rd(game, ADDR_MODE) == 5
            && rd(game, ADDR_WORLD) == 0
            && rd(game, ADDR_MENU) == 2
            && rd(game, ADDR_STAGE) == 1;
        if paused_overworld && game.pad1 & SOFTLOCK_CHORD == SOFTLOCK_CHORD {
            let t = game.enh_state.timers[0].saturating_add(1);
            game.enh_state.timers[0] = t;
            if t >= SOFTLOCK_FRAMES {
                game.enh_state.timers[0] = 0;
                game.enh_state.flags |= FLAG_WARP;
                wr(game, ADDR_MENU, 0);
                wr(game, ADDR_LIVES, 1);
                wr(game, ADDR_STAGE, 2);
            }
        } else {
            game.enh_state.timers[0] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loc_packing_round_trips() {
        for region in 0..3u8 {
            for slot in 0..0x3Fu8 {
                assert_eq!(unpack_loc(pack_loc(region, slot)), (region, slot));
            }
        }
    }

    #[test]
    fn kept_xp_rounds_down_and_caps() {
        assert_eq!(kept_xp(1000, 0), 0);
        assert_eq!(kept_xp(1000, 25), 250);
        assert_eq!(kept_xp(999, 25), 249);
        assert_eq!(kept_xp(0xFFFF, 100), 0xFFFF);
        assert_eq!(kept_xp(0xFFFF, 200), 0xFFFF);
    }

    #[test]
    fn doll_lives_cap_at_one_digit() {
        assert_eq!(lives_with_dolls(0), 3);
        assert_eq!(lives_with_dolls(2), 5);
        assert_eq!(lives_with_dolls(6), 9);
        assert_eq!(lives_with_dolls(200), 9);
    }

    #[test]
    fn town_entry_follows_facing() {
        assert_eq!(town_entry_for_facing(0x01, 3), 0);
        assert_eq!(town_entry_for_facing(0x02, 3), 3);
        assert_eq!(town_entry_for_facing(0x04, 3), 3);
        assert_eq!(town_entry_for_facing(0x08, 3), 3);
        assert_eq!(town_entry_for_facing(0x00, 3), 3);
    }

    #[test]
    fn continue_target_picks_palace_town_or_original() {
        let town = pack_loc(0, 0x2D);
        let palace = pack_loc(2, 0x34);
        let both = [town, palace, 7, BIT_TOWN | BIT_PALACE];
        // Original never moves.
        assert_eq!(continue_target(ContinueFrom::Og, 3, both), None);
        // In a palace.
        assert_eq!(
            continue_target(ContinueFrom::DungeonEntrance, 3, both),
            Some((2, 7, 0x34))
        );
        assert_eq!(
            continue_target(ContinueFrom::Both, 4, both),
            Some((2, 7, 0x34))
        );
        assert_eq!(
            continue_target(ContinueFrom::LastTown, 3, both),
            Some((0, 0, 0x2D))
        );
        // Outside a palace.
        assert_eq!(
            continue_target(ContinueFrom::DungeonEntrance, 0, both),
            None
        );
        assert_eq!(
            continue_target(ContinueFrom::Both, 0, both),
            Some((0, 0, 0x2D))
        );
        // The Great Palace keeps the original restart path.
        assert_eq!(
            continue_target(ContinueFrom::DungeonEntrance, 5, both),
            None
        );
        // Nothing recorded.
        assert_eq!(continue_target(ContinueFrom::Both, 3, [0; 4]), None);
        assert_eq!(
            continue_target(ContinueFrom::Both, 3, [town, 0, 0, BIT_TOWN]),
            Some((0, 0, 0x2D))
        );
    }

    #[test]
    fn register_hooks_only_what_is_on() {
        let mut g = Game::new();
        let base = g.traps.len();
        super::super::apply(
            &mut g,
            &super::super::Enhancements {
                qol: QolOpts {
                    enter_town_from_side: true,
                    ..QolOpts::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(g.enh_hooks.trap_count(), 1);
        assert_eq!(
            g.traps.get(HOOK_AREA_ENTRY).unwrap().name,
            "enh_qol_area_entry"
        );
        super::super::apply(&mut g, &super::super::Enhancements::default());
        assert_eq!(g.traps.len(), base);
        assert_eq!(g.enh_hooks.trap_count(), 0);
    }

    #[test]
    fn softlock_chord_counts_and_fires() {
        let mut g = Game::new();
        let opts = QolOpts {
            overworld_softlock_warp: true,
            ..QolOpts::default()
        };
        g.ram[0x0736] = 5;
        g.ram[0x0707] = 0;
        g.ram[0x0524] = 2;
        g.ram[0x076C] = 1;
        g.ram[0x0700] = 3;
        g.pad1 = SOFTLOCK_CHORD;
        for _ in 0..SOFTLOCK_FRAMES - 1 {
            end_of_frame(&mut g, &opts);
        }
        assert_eq!(g.enh_state.timers[0], SOFTLOCK_FRAMES - 1);
        assert_eq!(g.ram[0x076C], 1);
        // Releasing a button resets the count.
        g.pad1 = SOFTLOCK_CHORD & !1;
        end_of_frame(&mut g, &opts);
        assert_eq!(g.enh_state.timers[0], 0);
        g.pad1 = SOFTLOCK_CHORD;
        for _ in 0..SOFTLOCK_FRAMES {
            end_of_frame(&mut g, &opts);
        }
        assert_eq!(g.ram[0x076C], 2, "die routine started");
        assert_eq!(g.ram[0x0700], 1, "last life");
        assert_ne!(g.enh_state.flags & FLAG_WARP, 0);
    }

    #[test]
    fn wise_man_spell_requests_magic_refill_in_town_only() {
        let mut g = Game::new();
        let opts = QolOpts {
            wise_men_restore_mp: true,
            ..QolOpts::default()
        };
        g.ram[0x0707] = 1;
        g.ram[0x077B + 2] = 1; // known before the option saw a frame
        end_of_frame(&mut g, &opts);
        assert_eq!(g.ram[0x070C], 0, "first frame only primes");
        g.ram[0x077B + 3] = 1;
        end_of_frame(&mut g, &opts);
        assert_eq!(g.ram[0x070C], 0xFF);
        g.ram[0x070C] = 0;
        end_of_frame(&mut g, &opts);
        assert_eq!(g.ram[0x070C], 0, "once per spell");
        g.ram[0x0707] = 0;
        g.ram[0x077B + 4] = 1;
        end_of_frame(&mut g, &opts);
        assert_eq!(g.ram[0x070C], 0, "outside towns");
    }
}
