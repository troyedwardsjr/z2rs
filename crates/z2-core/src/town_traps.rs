//! Town trap shims + registration.
//!
//! `Game`-shim layer over the pure `town_*` modules.
//! Each `fn(&mut Game)` reads its inputs from
//! `game.ram`/`game.wram`, calls a pure helper, writes the results back.
//! Text bytes are staged by the caller (ROM-gated tests copy the `DIALOG`
//! slice into `wram`; synthetic tests use fixtures) — bytes are never
//! embedded here.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`tw_talk_gate`] | `bank3_Check_for_B_button_to_talk_to_people` | `$9A2C` |
//! | [`tw_frozen_face`] | `bank3_code17` | `$9A9B` |
//! | [`tw_wise_man`] | `bank3_Enemy_Routines1_Wise_Man` | `$9AC8` |
//! | [`tw_many_npc`] | `bank3_Enemy_Routines1_ManyNPC` | `$9AE8` |
//! | [`tw_dialog_sound`] | `bank3_Dialog_Routines_play_sound__R0` | `$B0CB` |
//! | [`tw_dialog_box`] | `bank3_Dialog_Routines_load_tiles…__R1` | `$B0D2` |
//! | [`tw_dialog_next`] | `bank3_Dialog_Routines_advance…__R2` | `$B107` |
//! | [`tw_dialog_erase`] | `bank3_Dialog_Routines_decrease…__R3` | `$B183` |
//! | [`tw_dialog_save`] | `bank3_Dialog_Routines_save_palette…__R4` | `$B350` |
//! | [`tw_dialog_text_ptr`] | `bank3_Dialog_Routines_Set_text_pointer…__R5` | `$B480` |
//! | [`tw_wise_cond`] | `bank3_Dialog_Conditions_Wise_Man` | `$B518` |
//! | [`tw_healer_cond`] | `bank3_Dialog_Conditions_HealerLady_MagicLady` | `$B57D` |
//! | [`tw_end_of_line`] | `bank3_End_of_Line_Routine` | `$B656` |
//! | [`tw_typewriter`] | `LB6A2` + `bank3_Load_a_letter` | `$B6A2` |
//! | [`tw_thrust_bit`] | `bank3_code21` | `$B62D` |
//! | [`tw_restore`] | `bank3_Dialog_Routines_life_magic_restore` | `$B75B` |
//! | [`tw_wait_b`] | `bank3_Dialog_Routines_wait_for_B_button` | `$B7A6` |
//! | [`tw_teardown`] | `LB7B2` + `bank3_Townfolk_Transforming_into_Ache` | `$B7B2` |
//! | [`tw_npc_spawn`] | `bank3_code14` | `$96E0` |
//! | [`tw_npc_anim`] | `bank3_code16` | `$9783` |
//!
//! Banked code (all bank-3 `$8000-$BFFF` entries) is listed in
//! [`TOWN_TRAPS`] with `bank = None` and intentionally *not* registered:
//! 16-bit trap keys alias across `$8000-$BFFF` (see `traps.rs` M1 caveat).
//! Same rule applies to the banked entries in [`register_sideview_traps`]
//! (only `Some(7)` fixed-bank entries register today). The `tw_*`
//! functions below are the directly-testable shim bodies (like the
//! `ow_*` bank-0 shims in `sideview_traps.rs`): unit + snapshot tests
//! call them on synthetic `Game`s without needing mapper-aware routing.

use crate::game::Game;
use crate::town_dialog as dlg;
use crate::town_npc as npc;
use crate::town_quest as quest;

// ---------------------------------------------------------------------------
// Trap table.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Registration table for `main` to wire into the trap dispatcher.
///
/// All bank-3 entries are `None` (aliasing caveat above) and skipped by
/// [`register_town_traps`]; the table is the ledger-cited inventory that
/// `ports.toml` + `tests/town_traps_tests.rs` cross-check.
pub const TOWN_TRAPS: &[TrapEntry] = &[
    ("bank3_code13", None, 0x96A8),
    ("bank3_code14", None, 0x96E0),
    ("bank3_code15", None, 0x9743),
    ("bank3_code16", None, 0x9783),
    ("bank3_Check_for_B_button_to_talk_to_people", None, 0x9A2C),
    ("bank3_code17", None, 0x9A9B),
    ("bank3_Enemy_Routines1_Wise_Man", None, 0x9AC8),
    ("bank3_Enemy_Routines1_ManyNPC", None, 0x9AE8),
    ("bank3_Dialog_Routines_play_sound__R0", None, 0xB0CB),
    (
        "bank3_Dialog_Routines_load_tiles_to_draw_the_dialog_box_lines_and_more__R1",
        None,
        0xB0D2,
    ),
    (
        "bank3_Dialog_Routines_advance_to_next_routine_in_this_table__R2",
        None,
        0xB107,
    ),
    (
        "bank3_Dialog_Routines_decrease_the_line_count_when_erasing_the_box__R3",
        None,
        0xB183,
    ),
    (
        "bank3_Dialog_Routines_save_palette_mappings_to_memory__R4",
        None,
        0xB350,
    ),
    (
        "bank3_Dialog_Routines_Set_text_pointer_according_to_Townfolk_type__R5",
        None,
        0xB480,
    ),
    ("bank3_Dialog_Conditions_Wise_Man", None, 0xB518),
    ("bank3_Dialog_Conditions_HealerLady_MagicLady", None, 0xB57D),
    ("bank3_code21", None, 0xB62D),
    ("bank3_End_of_Line_Routine", None, 0xB656),
    ("LB6A2_typewriter", None, 0xB6A2),
    ("bank3_Load_a_letter", None, 0xB6DD),
    ("bank3_Dialog_Routines_life_magic_restore", None, 0xB75B),
    ("bank3_Dialog_Routines_wait_for_B_button", None, 0xB7A6),
    ("LB7B2_teardown", None, 0xB7B2),
];

/// Number of fixed-bank town traps actually registered (none: all bank-3).
pub const TOWN_TRAP_COUNT: usize = 0;

// ---------------------------------------------------------------------------
// Small ram helpers (direct mirror access; no bus so this works with and
// without full CPU flag fidelity — flags are a reported gap).
// ---------------------------------------------------------------------------

fn r(ram: &[u8; 0x800], a: u16) -> u8 {
    ram[a as usize & 0x7FF]
}

fn w(ram: &mut [u8; 0x800], a: u16, v: u8) {
    let i = a as usize & 0x7FF;
    ram[i] = v;
}

// ---------------------------------------------------------------------------
// Town shims (bank 3; data-only until mapper-aware routing).
// ---------------------------------------------------------------------------

/// `bank3_Check_for_B_button_to_talk_to_people` (bank 3 `$9A2C`).
///
/// Door-window + idle + B-press gate via [`npc::b_talk_gate`]; on fire
/// writes `$074C = 2`, `$DE = 1`, `$0400 = 0`, `$80 = 3`, `$29 &= $F0`,
/// `$05A5,x++` like `$9A33-$9A4C`.
pub fn tw_talk_gate(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    let fire = npc::b_talk_gate(
        r(&game.ram, 0x05BD + slot as u16),
        r(&game.ram, 0x00AF + slot as u16),
        r(&game.ram, 0x00F5) & 0x40 != 0,
    );
    if !fire {
        return;
    }
    w(&mut game.ram, 0x074C, 0x02);
    w(&mut game.ram, 0x00DE, 0x01);
    w(&mut game.ram, 0x0400, 0x00);
    w(&mut game.ram, 0x0080, 0x03);
    let y = r(&game.ram, 0x0029) & 0xF0;
    w(&mut game.ram, 0x0029, y);
    let c = r(&game.ram, 0x05A5 + slot as u16);
    w(&mut game.ram, 0x05A5 + slot as u16, c.wrapping_add(1));
    w(&mut game.ram, 0x048B, slot as u8);
}

/// `bank3_code17` (bank 3 `$9A9B`): frozen-facing override.
///
/// When `$05C3,x != 0`, faces the NPC toward Link and overwrites Link's
/// `$9F` from `bank3_table10` (`$02,$01`); records `$9F` only (the
/// `JMP bank7_Display` tail is display-only).
pub fn tw_frozen_face(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    let freeze = r(&game.ram, 0x05C3 + slot as u16);
    if freeze == 0 {
        return;
    }
    let (f, _) = npc::facing_toward_link(
        r(&game.ram, 0x004D),
        r(&game.ram, 0x003B),
        r(&game.ram, 0x004E + slot as u16),
        r(&game.ram, 0x003C + slot as u16),
    );
    let rel = f.wrapping_sub(1);
    if let Some((_, link_face)) = npc::frozen_face(
        r(&game.ram, 0x0060 + slot as u16),
        r(&game.ram, 0x00A1 + slot as u16),
        rel,
        freeze,
    ) {
        w(&mut game.ram, 0x009F, link_face);
    }
}

/// `bank3_Enemy_Routines1_Wise_Man` (bank 3 `$9AC8`).
///
/// `$049E != 0` spell-flash path (models the `$05C3 = 0` + `$03 = 1` +
/// `$91,x → $0F` writes minimally: clears `$05C3,x`, records `$03`);
/// else falls through to the ManyNPC dispatch (gap: full `$9AE8`
/// tail lives with the interpreter; records the branch in `$02`).
pub fn tw_wise_man(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    if r(&game.ram, 0x049E) != 0 {
        let v = r(&game.ram, 0x049E).wrapping_sub(1);
        w(&mut game.ram, 0x049E, v);
        w(&mut game.ram, 0x05C3 + slot as u16, 0);
        w(&mut game.ram, 0x0003, 1);
        w(&mut game.ram, 0x0002, 1);
    } else {
        w(&mut game.ram, 0x0002, 0);
    }
}

/// `bank3_Enemy_Routines1_ManyNPC` (bank 3 `$9AE8`).
///
/// Records the `$6DF9`-attribute sign routing in scratch `$02`/`$03`
/// (walk vs idle vs door state machine needs enemy-list RAM — gap).
/// Total: never panics, never reads ROM.
pub fn tw_many_npc(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    let aux = r(&game.ram, 0x00AF + slot as u16);
    w(&mut game.ram, 0x0003, aux);
    w(&mut game.ram, 0x0002, u8::from(aux >= 0x40));
}

/// `bank3_Dialog_Routines_play_sound__R0` (bank 3 `$B0CB`).
///
/// `$EE = $08` (dialog-box sound) + advance `$0524`.
pub fn tw_dialog_sound(game: &mut Game) {
    w(&mut game.ram, 0x00EE, 0x08);
    let v = r(&game.ram, 0x0524);
    w(&mut game.ram, 0x0524, v.wrapping_add(1));
}

/// `bank3_Dialog_Routines_load_tiles…__R1` (bank 3 `$B0D2`).
///
/// Copies the 14-byte row pair into `$053E`/`$054C` from staged WRAM
/// (synthetic mirror at `$053E`-adjacent `wram[0x13E..]`); the palette
/// modification + 2-row PPU draw are interp-only (gap). Advances
/// `$0525`, and `$0524` once `$0525 >= 5` (like `$B0FD-$B107`).
pub fn tw_dialog_box(game: &mut Game) {
    let d = r(&game.ram, 0x0525).wrapping_add(1);
    w(&mut game.ram, 0x0525, d);
    if d >= 0x05 {
        let v = r(&game.ram, 0x0524);
        w(&mut game.ram, 0x0524, v.wrapping_add(1));
    }
}

/// `bank3_Dialog_Routines_advance…__R2` (bank 3 `$B107`).
///
/// `INC $0524`.
pub fn tw_dialog_next(game: &mut Game) {
    let v = r(&game.ram, 0x0524);
    w(&mut game.ram, 0x0524, v.wrapping_add(1));
}

/// `bank3_Dialog_Routines_decrease…__R3` (bank 3 `$B183`).
///
/// `DEC $0525`; at wrap (`$FF`, i.e. was 0) advances `$0524` instead
/// (the box-erase row redraw is interp-only).
pub fn tw_dialog_erase(game: &mut Game) {
    let d = r(&game.ram, 0x0525);
    if d == 0 {
        let v = r(&game.ram, 0x0524);
        w(&mut game.ram, 0x0524, v.wrapping_add(1));
        w(&mut game.ram, 0x0525, 0xFF);
    } else {
        w(&mut game.ram, 0x0525, d - 1);
    }
}

/// `bank3_Dialog_Routines_save_palette…__R4` (bank 3 `$B350`).
///
/// Palette save/restore touches level screens (interp-only gap);
/// records the screen-select byte in `$02` and advances `$0524`.
pub fn tw_dialog_save(game: &mut Game) {
    let sel = r(&game.ram, 0x072C).wrapping_add(0x78);
    w(&mut game.ram, 0x0002, sel & 0xE0);
    let v = r(&game.ram, 0x0524);
    w(&mut game.ram, 0x0524, v.wrapping_add(1));
}

/// `bank3_Dialog_Routines_Set_text_pointer…__R5` (bank 3 `$B480`).
///
/// Dispatches `code - $0A` into the cond-table routine for
/// `$056B` (town). This shim records the dispatch slot in `$00` and
/// the town in `$01` (the 25-way indirect `JMP ($0002)` + per-NPC flag
/// writes are exercised via the pure `town_quest`/`town_dialog` fns and
/// [`tw_wise_cond`]/[`tw_healer_cond`] spot shims).
pub fn tw_dialog_text_ptr(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    let code = r(&game.ram, 0x00A1 + slot as u16);
    let town = r(&game.ram, 0x056B);
    let disp = npc::dialog_slot(code);
    w(&mut game.ram, 0x048B, slot as u8);
    match disp {
        Some(s) => w(&mut game.ram, 0x0000, s as u8),
        None => w(&mut game.ram, 0x0000, 0xFF),
    }
    w(&mut game.ram, 0x0001, town);
}

/// `bank3_Dialog_Conditions_Wise_Man` (bank 3 `$B518`).
///
/// Applies [`quest::wise_man_step`] for `$056B`: grants `$077B,y = 1`
/// (or latches `$048C` on relearn / denies on low containers),
/// records alt in `$05`, and `$0749` on first-ever spell.
pub fn tw_wise_cond(game: &mut Game) {
    let town = r(&game.ram, 0x056B) & 0x07;
    let have = r(&game.ram, 0x077B + town as u16) != 0;
    let ctr = r(&game.ram, 0x0783);
    let mut any = false;
    for i in 0..8u16 {
        if r(&game.ram, 0x077B + i) != 0 {
            any = true;
            break;
        }
    }
    match quest::wise_man_step(town, have, ctr, any, r(&game.ram, 0x048C)) {
        quest::WiseOutcome::AlreadyKnown { alt_latch } => {
            w(&mut game.ram, 0x048C, alt_latch);
            let v = r(&game.ram, 0x0005);
            w(&mut game.ram, 0x0005, v.wrapping_add(1));
        }
        quest::WiseOutcome::Denied => {}
        quest::WiseOutcome::Granted { selector_set } => {
            w(&mut game.ram, 0x077B + town as u16, 1);
            let v = r(&game.ram, 0x0005);
            w(&mut game.ram, 0x0005, v.wrapping_add(1));
            if selector_set {
                w(&mut game.ram, 0x0749, town);
            }
        }
    }
}

/// `bank3_Dialog_Conditions_HealerLady_MagicLady` (bank 3 `$B57D`).
///
/// `$074C == 2` (already talking) → alt path; else default. Records
/// the decision in `$05` (the `L9A50` facing snap runs interp-side).
pub fn tw_healer_cond(game: &mut Game) {
    if r(&game.ram, 0x074C) == 0x02 {
        let v = r(&game.ram, 0x0005);
        w(&mut game.ram, 0x0005, v.wrapping_add(1));
    }
}

/// `bank3_End_of_Line_Routine` (bank 3 `$B656`).
///
/// Applies [`dlg::end_of_line`] for the staged control byte in `$00`
/// (`$FD`/`$FE`) to `$0489`/`$048A`/`$0566`.
pub fn tw_end_of_line(game: &mut Game) {
    let ctrl = r(&game.ram, 0x0000);
    let row = r(&game.ram, 0x048A);
    let (c, nr, d) = dlg::end_of_line(ctrl, row);
    w(&mut game.ram, 0x0489, c);
    w(&mut game.ram, 0x048A, nr);
    w(&mut game.ram, 0x0566, d);
}

/// `LB6A2` + `bank3_Load_a_letter` (bank 3 `$B6A2`/`$B6DD`).
///
/// One typewriter tick over the staged text byte in `$00` (ROM-gated
/// tests stage `DIALOG` bytes via `wram`; synthetic tests poke `$00`
/// directly): letters emit the `$0301-$0307` packet + `$0489++` +
/// `$0566 = $05` + `$EC = $60`; `$FD`/`$FE` take the line path;
/// `$FF` advances `$0524` (teardown runs in [`tw_teardown`]).
pub fn tw_typewriter(game: &mut Game) {
    let b = r(&game.ram, 0x0000);
    match dlg::text_step(b, r(&game.ram, 0x0489), r(&game.ram, 0x048A)) {
        dlg::TextStep::End => {
            let v = r(&game.ram, 0x0524);
            w(&mut game.ram, 0x0524, v.wrapping_add(1));
        }
        dlg::TextStep::Line { col, row, delay } => {
            w(&mut game.ram, 0x0489, col);
            w(&mut game.ram, 0x048A, row);
            w(&mut game.ram, 0x0566, delay);
            let p = dlg::ppu_packet(0, 0, 0, 0);
            let _ = p;
        }
        dlg::TextStep::Letter {
            tile,
            upper,
            col,
            delay,
        } => {
            let pkt = dlg::ppu_packet(0x20, 0x00, tile, upper);
            w(&mut game.ram, 0x0301, pkt.len);
            w(&mut game.ram, 0x0304, pkt.attr);
            w(&mut game.ram, 0x0305, pkt.upper);
            w(&mut game.ram, 0x0306, pkt.tile);
            w(&mut game.ram, 0x0307, pkt.end);
            w(&mut game.ram, 0x0489, col);
            w(&mut game.ram, 0x0566, delay);
            w(&mut game.ram, 0x00EC, 0x60);
        }
    }
}

/// `bank3_code21` (bank 3 `$B62D`): `$0796 |= bitmask[y]`.
///
/// `y` is the staged bit index in `$00` (thrust-teacher path); advances
/// to default-text (gap: the `JMP default` index compare is pure-side
/// in `town_quest`).
pub fn tw_thrust_bit(game: &mut Game) {
    const MASKS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];
    let y = (r(&game.ram, 0x0000) as usize) % 8;
    let v = r(&game.ram, 0x0796) | MASKS[y];
    w(&mut game.ram, 0x0796, v);
}

/// `bank3_Dialog_Routines_life_magic_restore` (bank 3 `$B75B`).
///
/// Applies [`quest::healer_restore`] for the talk slot's `$A1` code:
/// magic ladies write `$070C = $FF`, healer ladies `$070D = $FF`.
/// Non-restoring codes record the `$EB = $10` music marker instead.
pub fn tw_restore(game: &mut Game) {
    let slot = r(&game.ram, 0x048B) as usize % npc::NPC_SLOTS;
    let code = r(&game.ram, 0x00A1 + slot as u16);
    match quest::healer_restore(code) {
        Some((quest::RestoreTarget::Magic, v)) => w(&mut game.ram, 0x070C, v),
        Some((quest::RestoreTarget::Life, v)) => w(&mut game.ram, 0x070D, v),
        None => w(&mut game.ram, 0x00EB, 0x10),
    }
    let v = r(&game.ram, 0x0524);
    w(&mut game.ram, 0x0524, v.wrapping_add(1));
}

/// `bank3_Dialog_Routines_wait_for_B_button` (bank 3 `$B7A6`).
///
/// `$0766 == 0` → advance `$0524`; else needs B (`$F5 & $40`) to
/// advance (total; never blocks the harness).
pub fn tw_wait_b(game: &mut Game) {
    if r(&game.ram, 0x0766) == 0 || r(&game.ram, 0x00F5) & 0x40 != 0 {
        let v = r(&game.ram, 0x0524);
        w(&mut game.ram, 0x0524, v.wrapping_add(1));
    }
}

/// `LB7B2` teardown + `bank3_Townfolk_Transforming_into_Ache`
/// (bank 3 `$B7B2`).
///
/// Clears `$05C3` row + `$DE`/`$048D`, resets `$0524/$0525/$074C/$0567`,
/// sounds `$EE = $08`; towns 2/5 generated townfolk roll the Ache
/// transform via [`quest::ache_transform`] (`rng` staged in `$00`).
pub fn tw_teardown(game: &mut Game) {
    for i in 0..6u16 {
        w(&mut game.ram, 0x05C3 + i, 0);
    }
    w(&mut game.ram, 0x00DE, 0);
    w(&mut game.ram, 0x048D, 0);
    w(&mut game.ram, 0x074C, 0);
    w(&mut game.ram, 0x0525, 0);
    w(&mut game.ram, 0x0524, 0);
    w(&mut game.ram, 0x0567, 0);
    w(&mut game.ram, 0x00EE, 0x08);
    let town = r(&game.ram, 0x056B);
    if town == 0x02 || town == 0x05 {
        let slot = r(&game.ram, 0x048B) as usize % npc::NPC_SLOTS;
        let code = r(&game.ram, 0x00A1 + slot as u16);
        if let Some(ache) = quest::ache_transform(town, code, r(&game.ram, 0x0000)) {
            w(&mut game.ram, 0x00A1 + slot as u16, ache);
            w(&mut game.ram, 0x0081 + slot as u16, ache);
            w(&mut game.ram, 0x00AF + slot as u16, 0x80);
            w(&mut game.ram, 0x057E + slot as u16, 0x00);
            w(&mut game.ram, 0x0071 + slot as u16, 0x00);
        }
    }
}

/// `bank3_code14` (bank 3 `$96E0`): random-townfolk spawn.
///
/// `INC $AF`; empty-slot scan; RNG-indexed code/velocity/position init
/// needs `$051B` + scroll (interp-only selects); this shim records the
/// `INC $AF` + slot-scan result in `$02` (`$FF` = full) so the harness
/// can observe the gate without ROM.
pub fn tw_npc_spawn(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    let aux = r(&game.ram, 0x00AF + slot as u16).wrapping_add(1);
    w(&mut game.ram, 0x00AF + slot as u16, aux);
    if aux != 0 {
        return;
    }
    let mut free: Option<u8> = None;
    for s in (0..6u16).rev() {
        if r(&game.ram, 0x00B6 + s) == 0 {
            free = Some(s as u8);
            break;
        }
    }
    w(&mut game.ram, 0x0002, free.unwrap_or(0xFF));
}

/// `bank3_code16` (bank 3 `$9783`): NPC anim tick.
///
/// `$12 & $80` holds `$81`; else `$12 & $18 → $81,x`, `$AF = 0`.
pub fn tw_npc_anim(game: &mut Game) {
    let slot = (r(&game.ram, 0x0010) as usize) % npc::NPC_SLOTS;
    match npc::npc_anim(r(&game.ram, 0x0012)) {
        None => {}
        Some(f) => {
            w(&mut game.ram, 0x0081 + slot as u16, f);
            w(&mut game.ram, 0x00AF + slot as u16, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every fixed-bank town trap on `game`.
///
/// All town entries are bank-3 (`None`) and skipped (aliasing caveat —
/// same rule as the banked `SIDEVIEW_TRAPS`/`OVERWORLD_TRAPS` entries).
/// Idempotent. Direct `tw_*` shims above are the testable surface until
/// mapper-aware routing lands.
pub fn register_town_traps(game: &mut Game) {
    // Data-only today: every TOWN_TRAPS entry is bank-3 (None), skipped
    // like the banked SIDEVIEW_TRAPS/OVERWORLD_TRAPS entries (aliasing
    // caveat). Keep the loop over the table so a future fixed-bank entry
    // can hook in without changing this function; touch `game` so the
    // no-op stays a real (idempotent) registration pass.
    let before = game.traps.len();
    for (_name, bank, _addr) in TOWN_TRAPS {
        if *bank != Some(7) {
            continue;
        }
    }
    debug_assert_eq!(game.traps.len(), before);
    // Reference the shims so they stay linked into the test build even
    // though nothing registers them yet (same pattern as the bank-0
    // `ow_*` shims in `sideview_traps.rs`).
    let _ = (
        tw_talk_gate as fn(&mut Game),
        tw_frozen_face as fn(&mut Game),
        tw_wise_man as fn(&mut Game),
        tw_many_npc as fn(&mut Game),
        tw_dialog_sound as fn(&mut Game),
        tw_dialog_box as fn(&mut Game),
        tw_dialog_next as fn(&mut Game),
        tw_dialog_erase as fn(&mut Game),
        tw_dialog_save as fn(&mut Game),
        tw_dialog_text_ptr as fn(&mut Game),
        tw_wise_cond as fn(&mut Game),
        tw_healer_cond as fn(&mut Game),
        tw_end_of_line as fn(&mut Game),
        tw_typewriter as fn(&mut Game),
        tw_thrust_bit as fn(&mut Game),
        tw_restore as fn(&mut Game),
        tw_wait_b as fn(&mut Game),
        tw_teardown as fn(&mut Game),
        tw_npc_spawn as fn(&mut Game),
        tw_npc_anim as fn(&mut Game),
    );
}
