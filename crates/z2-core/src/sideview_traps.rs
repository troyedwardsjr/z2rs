//! Sideview trap shims + registration.
//!
//! `Game`-shim layer over the pure `sideview_*` modules.
//! Each `fn(&mut Game)` reads its inputs from
//! `game.ram`/`game.wram`, calls a pure helper, writes the results back.
//! Background bytes are pushed through
//! `bank7_ppu_queue::{queue_begin,push,finish}` (read-only reuse — that
//! module is never copied here) and drained by the existing `LD2EC` trap.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`sv_item_spawn`] | `LC9A5` | `$C9A5` |
//! | [`sv_key_entry`] | `bank7_code17` | `$CB35` |
//! | [`sv_get_entry`] | `bank7_Get_Area_Code__Enter_Code_and_Direction` | `$CC97` |
//! | [`sv_go_outside`] | `bank7_go_outside` | `$CCB3` |
//! | [`sv_elev_exit`] | `bank7_take_elevator_exit` | `$C644` |
//! | [`sv_side_exit`] | `bank7_take_side_exit` | `$CF4C` |
//! | [`sv_door_exit`] | `bank7_take_door_exit` | `$CFFC` |
//! | [`sv_spawn_gate`] | `bank7_code22` | `$D603` |
//! | [`sv_enemy_spawn`] | `LD625` | `$D625` |
//! | [`sv_link_collision`] | `bank7_Link_Collision_Detection` | `$D6C1` |
//! | [`sv_elevator`] | `bank7_Enemy_Routines1_Elevator` | `$D8C2` |
//! | [`sv_locked_door`] | `bank7_Enemy_Routines1_Locked_Door` | `$D991` |
//! | [`sv_despawn`] | `LDE6C` | `$DE6C` |
//! | [`sv_scroll_gate`] | `LE16F` | `$E16F` |
//! | [`sv_probe`] | `LE1BE` | `$E1BE` |
//! | [`sv_lava_check`] | `bank7_Related_to_Link_falling_in_Lava_Water` | `$E079` |
//!
//! # Register-exact ports (lockstep pass)
//!
//! Every shim above replicates the ROM routine's observable effects the
//! way the bank-7 ports do: memory (RAM through `game.ram`, WRAM / ROM
//! tables through the bus), `A`/`X`/`Y`/`P`, dead stack bytes from inner
//! `JSR`/`PHA`/`PHP` traffic, and per-path CPU cycles under the `cpu.rs`
//! cost model (base + taken branch, page-cross extras where the index can
//! cross — including the taken branches that cross a page edge). The
//! original synthetic shims (staged headers, proxy tables, boolean
//! scratch results) were the first persistent lockstep divergences of the
//! any% movie (`$A7`/`$03E6` from frame 187, `$0561` at 895, `$7014` at
//! 920); `xtask verify --trap-set bank7,sideview` now matches the bank7-only
//! baseline for 3000 frames.
//!
//! Sub-routines outside this group are reached the way the interpreter
//! would reach them: [`jsr_sub`] fires the target's trap when one is
//! registered and interprets it otherwise, and a trailing `JMP` is a
//! [`Game::trap_jump`] (the dispatcher fires the target's trap or continues
//! interpreting there; nothing is pushed). Small ROM helpers the group's
//! routines fall through into (`LE1B8`/`LE1BE`, `LDD3D`, `$E292`, `$DC91`,
//! `$EAE8`, the sideview-bank `$850C`, `LC690`, `LCC40`/`LCC50`, `LCFEC`,
//! `$D03D`) are ported alongside as private bodies.
//!
//! Note on the mode-table entries (`$CB35`, `$CCB3`, `$C644`, `$CF4C`,
//! `$CFFC`): the bank-7 dispatcher port (`bank7_dispatch::jump_indexed_table`)
//! reaches its targets through [`Game::trap_jump`], so these traps fire in
//! the default configuration as well as with the trampolines interpreted
//! (`--untrap D382,D385`); both were used to verify them.
//!
//! Banked code (`Side_View_Initialization_when_entering_a_Key_Area`, bank 0
//! `$8CE1`; `bank1_code0` object dispatch, bank 1 `$80EE`) is listed in
//! [`SIDEVIEW_TRAPS`] with `bank = None` and intentionally *not* registered:
//! 16-bit trap keys alias across `$8000-$BFFF` (see `traps.rs` M1 caveat).
//! Same rule applies to the `OVERWORLD_TRAPS` banked entries in
//! [`register_overworld_traps`]: only `Some(7)` (fixed-bank) entries are
//! registered today; banked ones stay data-only pending mapper-aware routing.
//! One fixed-bank entry is also left to the ROM on purpose: the mode-0
//! loader `bank7_code18` (`$CD40`), see the overworld section below.
//!
//! The ROM-owned area setup/map draw routines (`$C4CB`, `$C755`, `$C82B`,
//! `$C89D`) and ground-find routine (`$E030`) are intentionally absent from
//! the trap table: their synthetic replacements used proxy data or omitted
//! tile writes, producing blank `$F4` nametables, missing floor rows, or
//! leaving Link at the falling-entry position on a real cartridge.
//!
//! Overworld: [`register_overworld_traps`] binds the
//! [`OVERWORLD_TRAPS`](crate::overworld::OVERWORLD_TRAPS) with shims that
//! call the overworld pure fns over `game.ram`/`game.wram` slices. `traps.rs` stays
//! untouched (this module is the only registration surface).

use crate::bank7_common::{adc_val, cmp_val, inc_val, inner_jsr_frame, jsr_sub, sbc_val, set_nz};
use crate::cpu::{bus_read, bus_read16, bus_write, FLAG_C, FLAG_N, FLAG_V, FLAG_Z};
use crate::game::Game;
use crate::overworld::OVERWORLD_TRAPS;
use crate::overworld_encounter as enc;
use crate::overworld_map as omap;
use crate::overworld_transition as otrans;
use crate::sideview_area as area;
use crate::sideview_scroll as scroll;
use crate::sideview_spawn as spawn;

// ---------------------------------------------------------------------------
// Trap table.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Registration table for `main` to wire into the trap dispatcher.
///
/// Fixed-bank (`Some(7)`) entries are safe today; `None` entries are
/// data-only (aliasing caveat above) and skipped by
/// [`register_sideview_traps`].
pub const SIDEVIEW_TRAPS: &[TrapEntry] = &[
    ("LC9A5", Some(7), 0xC9A5),
    ("bank7_code17", Some(7), 0xCB35),
    (
        "bank7_Get_Area_Code__Enter_Code_and_Direction",
        Some(7),
        0xCC97,
    ),
    ("bank7_go_outside", Some(7), 0xCCB3),
    ("bank7_take_elevator_exit", Some(7), 0xC644),
    ("bank7_take_side_exit", Some(7), 0xCF4C),
    ("bank7_take_door_exit", Some(7), 0xCFFC),
    ("bank7_code22", Some(7), 0xD603),
    ("LD625", Some(7), 0xD625),
    ("bank7_Link_Collision_Detection", Some(7), 0xD6C1),
    ("bank7_Enemy_Routines1_Elevator", Some(7), 0xD8C2),
    ("bank7_Enemy_Routines1_Locked_Door", Some(7), 0xD991),
    ("LDE6C", Some(7), 0xDE6C),
    ("LE16F", Some(7), 0xE16F),
    ("LE1BE", Some(7), 0xE1BE),
    (
        "bank7_Related_to_Link_falling_in_Lava_Water",
        Some(7),
        0xE079,
    ),
    // Banked code: listed, never registered (aliasing).
    (
        "Side_View_Initialization_when_entering_a_Key_Area",
        None,
        0x8CE1,
    ),
    ("bank1_code0_Object_Dispatch", None, 0x80EE),
];

/// Number of fixed-bank sideview traps actually registered.
pub const SIDEVIEW_TRAP_COUNT: usize = 16;

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
// Register-exact micro helpers for the `$E079` family (bank-7 shim style: the
// port keeps A/X/Y/P and the cycle clock aligned with the ROM bytes).
// ---------------------------------------------------------------------------

/// Zero-page,X effective address (wraps inside the page like the 6502).
fn zpx(base: u8, x: u8) -> usize {
    usize::from(base.wrapping_add(x))
}

/// `LSR A`: `C` = old bit 0, `N` cleared, `Z` from the result.
fn lsr_a(game: &mut Game) {
    let a = game.cpu.a;
    if a & 1 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = a >> 1;
    set_nz(&mut game.cpu.p, a >> 1);
}

/// `PHA`: the byte lands at `$0100+SP` and stays as a dead stack byte.
fn pha(game: &mut Game) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = game.cpu.a;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// `PLA`: pops into `A`, `N`/`Z` from the value.
fn pla(game: &mut Game) {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    let a = game.ram[0x0100 + usize::from(game.cpu.sp)];
    game.cpu.a = a;
    set_nz(&mut game.cpu.p, a);
}

/// `PHP`: pushes `P` with `B`/`U` set (mirrors `cpu.rs`).
fn php(game: &mut Game) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = game.cpu.p | 0x30;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// `PLP`: pops `P` (`B` cleared, `U` set, mirrors `cpu.rs`).
fn plp(game: &mut Game) {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    let p = game.ram[0x0100 + usize::from(game.cpu.sp)];
    game.cpu.p = (p & !0x10) | 0x20;
}

/// `ASL A`: `C` = old bit 7, `N`/`Z` from the result.
fn asl_a(game: &mut Game) {
    let a = game.cpu.a;
    if a & 0x80 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = a << 1;
    set_nz(&mut game.cpu.p, a << 1);
}

/// `BIT abs`: `N`/`V` copy memory bits 7/6, `Z` = `(A & mem) == 0`.
fn bit_mem(p: &mut u8, a: u8, m: u8) {
    *p = (*p & !(FLAG_N | FLAG_V)) | (m & (FLAG_N | FLAG_V));
    if a & m == 0 {
        *p |= FLAG_Z;
    } else {
        *p &= !FLAG_Z;
    }
}

/// `DEC zp`: wraps, `N`/`Z` from the new value (`C` untouched).
fn dec_zp(game: &mut Game, addr: usize) -> u8 {
    let v = game.ram[addr].wrapping_sub(1);
    game.ram[addr] = v;
    set_nz(&mut game.cpu.p, v);
    v
}

/// `LDA`-style register load: sets `N`/`Z`.
fn lda(game: &mut Game, v: u8) {
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDY`-style register load: sets `N`/`Z`.
fn ldy(game: &mut Game, v: u8) {
    game.cpu.y = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDX`-style register load: sets `N`/`Z`.
fn ldx(game: &mut Game, v: u8) {
    game.cpu.x = v;
    set_nz(&mut game.cpu.p, v);
}

/// `INC abs/zp` through the bus: wraps, `N`/`Z` from the new value.
fn inc_mem(game: &mut Game, addr: u16) {
    let v = bus_read(game, addr);
    let v = inc_val(&mut game.cpu.p, v);
    bus_write(game, addr, v);
}

/// Page-cross extra cycle for an indexed read (`base` vs `base + idx`).
fn cross(base: u16, idx: u8) -> u64 {
    let ea = base.wrapping_add(u16::from(idx));
    u64::from((base & 0xFF00) != (ea & 0xFF00))
}

/// Charge `n` CPU cycles (per-path counting, `cpu.rs` cost model:
/// base + taken-branch, page-cross extras only where computed).
fn cyc(game: &mut Game, n: u64) {
    game.cpu.cycles += n;
}

// ---------------------------------------------------------------------------
// Sideview shims (fixed bank).
// ---------------------------------------------------------------------------

/// `bank7_code13` (bank 7 `$C4CB`): select the map set for `$0707`/`$0706`.
///
/// Reads world/region, calls [`area::map_set_select`], records the choice
/// in scratch `$00` (0 = first, 1 = second) for the pointer-fetch path.
pub fn sv_code13(game: &mut Game) {
    let world = r(&game.ram, 0x0707);
    let region = r(&game.ram, 0x0706);
    let set = area::map_set_select(world, region);
    w(
        &mut game.ram,
        0x0000,
        match set {
            area::MapSet::First => 0,
            area::MapSet::Second => 1,
        },
    );
}

/// `bank7_process_map_data` (bank 7 `$C755`): decode the 4-byte header at
/// the `$D4` pointer (ROM bytes are not readable without a ROM image, so
/// this shim decodes the *staged* header copy at `$0480-$0483` when present
/// and otherwise preserves RAM).
///
/// Writes `$072E` (len), `$010C` (ground), `$0486` (no-ceil),
/// `$0731` (init obj), `$07AF` (pal-back). Always total (no panic).
pub fn sv_process_map(game: &mut Game) {
    // Staged header mirror used by synthetic tests; real ROM bytes come
    // from ($D4) via the bus (interp-only path, gap: needs PRG image).
    let raw = [
        game.ram[0x480],
        game.ram[0x481],
        game.ram[0x482],
        game.ram[0x483],
    ];
    // Heuristic: uninitialised mirror (all zero) with zero length means no
    // staged header — leave RAM untouched (pure-interp path runs the ASM).
    if raw == [0, 0, 0, 0] {
        return;
    }
    if let Some(h) = area::AreaHeader::decode(&raw) {
        w(&mut game.ram, 0x072E, h.len);
        w(&mut game.ram, 0x010C, h.ground());
        w(&mut game.ram, 0x0486, u8::from(h.no_ceiling()));
        w(&mut game.ram, 0x0731, h.init_obj());
        w(&mut game.ram, 0x07AF, h.pal_back);
    }
}

/// `LC89D` (bank 7 `$C89D`): second-pass object walk setup.
///
/// Derives the area width from staged header byte 1 (`$0481`) via
/// [`area::AreaHeader::width`] into `$D1`.
pub fn sv_draw_pass(game: &mut Game) {
    let flags = game.ram[0x481];
    let h = area::AreaHeader::decode(&[game.ram[0x480], flags, game.ram[0x482], game.ram[0x483]]);
    if let Some(h) = h {
        w(&mut game.ram, 0x00D1, h.width());
    }
}

/// `LC9A5` (bank 7 `$C9A5`): item-object spawn for `$0730`/`$0731`.
///
/// Picks the highest free `$B6` slot (5 down to 0; all busy → slot 0),
/// `$10 = slot`, `$3C,x = $0717`, then asks the item-presence bitmap
/// (`LC2A6`, `$C2A6`, `X` = screen) whether the item is still there —
/// zero → RTS. Otherwise `ROR $BC,x` (carry set from `CMP #$00`, so bit 7
/// is set), position from `$0730` ([`spawn::item_pos`]), `$1A/$B6/$A1/
/// $057E,x = 1`, item code `$AF,x = ($D4),$072F`. Exit: `X` = slot, `Y` =
/// `$072F`, `A` = item code.
pub fn sv_item_spawn(game: &mut Game) {
    // LDX #$05 : LC9A7: LDA $B6,x : BEQ LC9AF : DEX : BPL LC9A7 : INX
    cyc(game, 2);
    let mut x = 5u8;
    loop {
        let busy = game.ram[zpx(0xB6, x)];
        lda(game, busy);
        cyc(game, 4 + 2);
        if busy == 0 {
            cyc(game, 1);
            break;
        }
        cyc(game, 2);
        if x == 0 {
            // DEX wraps to $FF, BPL falls through, INX → 0.
            cyc(game, 2 + 2);
            break;
        }
        x -= 1;
        cyc(game, 3);
    }
    ldx(game, x);
    // LC9AF: STX $10 : LDA $0717 : STA $3C,x : TAX : JSR LC2A6 : LDX $10 :
    // CMP #$00 : BEQ LC9E9
    game.ram[0x10] = x;
    let screen = game.ram[0x0717];
    lda(game, screen);
    game.ram[zpx(0x3C, x)] = screen;
    ldx(game, screen);
    cyc(game, 3 + 4 + 4 + 2 + 6);
    jsr_sub(game, 0xC9B7, 0xC2A6);
    ldx(game, x);
    let present = game.cpu.a;
    cmp_val(&mut game.cpu.p, present, 0);
    cyc(game, 3 + 2 + 2);
    if present == 0 {
        cyc(game, 1 + 6);
        return;
    }
    // ROR $BC,x (C = 1 from the compare)
    let v = game.ram[zpx(0xBC, x)];
    let rot = (v >> 1) | 0x80;
    if v & 1 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    set_nz(&mut game.cpu.p, rot);
    game.ram[zpx(0xBC, x)] = rot;
    // LDA $0730 : PHA : ASL x4 : CLC : ADC #$03 : STA $4E,x : PLA :
    // AND #$F0 : CLC : ADC #$20 : STA $2A,x
    let pos = game.ram[0x0730];
    lda(game, pos);
    pha(game);
    game.cpu.p &= !FLAG_C;
    let ix = adc_val(&mut game.cpu.p, pos << 4, 3);
    game.cpu.a = ix;
    game.ram[zpx(0x4E, x)] = ix;
    pla(game);
    game.cpu.p &= !FLAG_C;
    let iy = adc_val(&mut game.cpu.p, pos & 0xF0, 0x20);
    game.cpu.a = iy;
    game.ram[zpx(0x2A, x)] = iy;
    // LDA #$01 : STA $1A,x : STA $B6,x : STA $A1,x : STA $057E,x
    lda(game, 1);
    game.ram[zpx(0x1A, x)] = 1;
    game.ram[zpx(0xB6, x)] = 1;
    game.ram[zpx(0xA1, x)] = spawn::ENEMY_ITEM;
    game.ram[0x057E + usize::from(x)] = 1;
    // LDY $072F : LDA ($D4),y : STA $AF,x : RTS
    let off = game.ram[0x072F];
    ldy(game, off);
    let d4 = u16::from(game.ram[0xD4]) | (u16::from(game.ram[0xD5]) << 8);
    let extra = cross(d4, off);
    let code = bus_read(game, d4.wrapping_add(u16::from(off)));
    lda(game, code);
    game.ram[zpx(0xAF, x)] = code;
    cyc(
        game,
        6 + 4 + 3 + 8 + 2 + 2 + 4 + 4 + 2 + 2 + 2 + 4 + 2 + 4 + 4 + 4 + 5 + 4 + 5 + 4 + 6 + extra,
    );
}

/// `bank7_Mute_music_when_loading_between_areas` (bank 7 `$D03D`):
/// `$EB = $80`. Exit `A = $80` (`N` set). 11 cycles, RTS included.
fn sv_mute(game: &mut Game) {
    lda(game, 0x80);
    game.ram[0xEB] = 0x80;
    cyc(game, 2 + 3 + 6);
}

/// `LCC50` (bank 7 `$CC50`): sideview entry position setup, entered with
/// `A` = start-page code and `Y` = area index.
///
/// `$0701 = A & 1`, Link at `$4D = $70`, `$0734/$0735 = $0B/$06`,
/// `$FD = $072C = 0`, page `$3B = $072A = $075C`, spawn window
/// `$0732/$0733 = page ∓ 1`, then mode `$0736 = $12` (or `$0C` when area
/// byte 3 bit 7 is set). Exit `X` = page + 1, `A` = mode.
fn sv_entry_setup(game: &mut Game) {
    let y = game.cpu.y;
    let dir = game.cpu.a & 0x01;
    lda(game, dir);
    game.ram[0x0701] = dir;
    lda(game, 0x70);
    game.ram[0x4D] = 0x70;
    game.ram[0x0734] = 0x0B;
    game.ram[0x0735] = 0x06;
    game.ram[0xFD] = 0;
    game.ram[0x072C] = 0;
    let page = game.ram[0x075C];
    game.ram[0x3B] = page;
    game.ram[0x072A] = page;
    game.ram[0x0732] = page.wrapping_sub(1);
    ldx(game, page.wrapping_add(1));
    game.ram[0x0733] = page.wrapping_add(1);
    // LDA $6ABD,y : ASL : LDA #$12 : BCC LCC85 : LDA #$0C
    let b3 = bus_read(game, 0x6ABDu16.wrapping_add(u16::from(y)));
    lda(game, b3);
    asl_a(game);
    lda(game, 0x12);
    cyc(
        game,
        2 + 4
            + 2
            + 3
            + 2
            + 4
            + 2
            + 4
            + 2
            + 3
            + 4
            + 4
            + 3
            + 4
            + 2
            + 4
            + 2
            + 2
            + 4
            + 4
            + cross(0x6ABD, y)
            + 2
            + 2
            + 2,
    );
    if b3 & 0x80 != 0 {
        lda(game, 0x0C);
        cyc(game, 2);
    } else {
        cyc(game, 1);
    }
    // LCC85: STA $0736 : RTS
    game.ram[0x0736] = game.cpu.a;
    cyc(game, 4 + 6);
}

/// `LCC40` (bank 7 `$CC40`): entry-side fix-up, `Y` = area index.
///
/// Areas entered from the right (byte 3 bit 5) or with start page 0/3
/// take `LCC89`: RTS unless byte 3 bit 7, which forces page 1 into
/// [`sv_entry_setup`]; any other page goes straight to the setup.
fn sv_entry_finish(game: &mut Game) {
    let y = game.cpu.y;
    let b3 = bus_read(game, 0x6ABDu16.wrapping_add(u16::from(y)));
    lda(game, b3);
    let right = b3 & 0x20;
    lda(game, right);
    cyc(game, 4 + cross(0x6ABD, y) + 2 + 2);
    if right == 0 {
        // LDA $075C : BEQ LCC89 : CMP #$03 : BEQ LCC89
        let page = game.ram[0x075C];
        lda(game, page);
        cyc(game, 4 + 2);
        if page != 0 {
            cmp_val(&mut game.cpu.p, page, 3);
            cyc(game, 2 + 2);
            if page != 3 {
                sv_entry_setup(game);
                return;
            }
        }
        cyc(game, 1);
    } else {
        cyc(game, 1);
    }
    // LCC89: LDA $6ABD,y : ASL : BCC LCC88 : LDA #$01 : STA $075C : JMP LCC50
    lda(game, b3);
    asl_a(game);
    cyc(game, 4 + cross(0x6ABD, y) + 2 + 2);
    if b3 & 0x80 == 0 {
        cyc(game, 1 + 6);
        return;
    }
    lda(game, 1);
    game.ram[0x075C] = 1;
    cyc(game, 2 + 4 + 3);
    sv_entry_setup(game);
}

/// `LCBB8` (bank 7 `$CBB8`): outdoor / pass-through entry, `Y` = index.
///
/// Sets the outdoor music request (`$075F = 4`) when anything but the
/// first area of West Hyrule is entered with no raft animation, decodes
/// the map byte ([`sv_get_entry`]), then derives the entry side from area
/// byte 3 bits 6-5 and Link's overworld facing (`$0562`; `$0701`/`$075C`),
/// or for the `$20` kind reuses music request 1 as page 1 with mode
/// `$11`. Ends in `$0736++` and [`sv_entry_finish`].
fn sv_entry_passthrough(game: &mut Game) {
    let y = game.cpu.y;
    // LDA $0706 : ORA $0707 : ORA $0748 : BEQ LCBCD
    let any = game.ram[0x0706] | game.ram[0x0707] | game.ram[0x0748];
    lda(game, any);
    cyc(game, 4 + 4 + 4 + 2);
    if any != 0 {
        // LDA $07AC : BNE LCBCD : LDA #$04 : STA $075F
        let raft = game.ram[0x07AC];
        lda(game, raft);
        cyc(game, 4 + 2);
        if raft == 0 {
            lda(game, 4);
            game.ram[0x075F] = 4;
            cyc(game, 2 + 4);
        } else {
            cyc(game, 1);
        }
    } else {
        cyc(game, 1);
    }
    // LCBCD: JSR bank7_Get_Area_Code__Enter_Code_and_Direction
    cyc(game, 6);
    inner_jsr_frame(game, 0xCBCD, sv_get_entry);
    // LDA $6ABD,y : AND #$60 : BEQ LCBF2
    let b3 = bus_read(game, 0x6ABDu16.wrapping_add(u16::from(y)));
    lda(game, b3);
    let kind = b3 & 0x60;
    lda(game, kind);
    cyc(game, 4 + cross(0x6ABD, y) + 2 + 2);
    if kind != 0 {
        // AND #$20 : BNE LCBF8
        let k20 = kind & 0x20;
        lda(game, k20);
        cyc(game, 2 + 2);
        if k20 != 0 {
            // LCBF8: LDA #$01 : STA $0701 : LDA $075F : CMP #$01 : BNE LCBF2
            cyc(game, 1);
            lda(game, 1);
            game.ram[0x0701] = 1;
            let music = game.ram[0x075F];
            lda(game, music);
            cmp_val(&mut game.cpu.p, music, 1);
            cyc(game, 2 + 4 + 4 + 2 + 2);
            if music == 1 {
                // STA $075C : JSR LCC50 : LDA #$11 : JMP LCC85 (STA $0736 : RTS)
                game.ram[0x075C] = 1;
                cyc(game, 4 + 6);
                inner_jsr_frame(game, 0xCC07, sv_entry_setup);
                lda(game, 0x11);
                game.ram[0x0736] = 0x11;
                cyc(game, 2 + 3 + 4 + 6);
                return;
            }
            cyc(game, 2); // BNE taken across the $CC/$CB page edge
        } else {
            // LDA $0562 : CMP #$04 : BCC LCBE4 : LSR : LSR
            let facing = game.ram[0x0562];
            lda(game, facing);
            cmp_val(&mut game.cpu.p, facing, 4);
            cyc(game, 4 + 2 + 2);
            if facing >= 4 {
                lsr_a(game);
                lsr_a(game);
                cyc(game, 2 + 2);
            } else {
                cyc(game, 1);
            }
            // LCBE4: AND #$01 : EOR #$01 : STA $0701 : BEQ LCBEF : LDA #$03 :
            // LCBEF: STA $075C
            let dir = (game.cpu.a & 0x01) ^ 0x01;
            lda(game, dir);
            game.ram[0x0701] = dir;
            cyc(game, 2 + 2 + 4 + 2);
            if dir != 0 {
                lda(game, 3);
                cyc(game, 2);
            } else {
                cyc(game, 1);
            }
            game.ram[0x075C] = game.cpu.a;
            cyc(game, 4);
        }
    } else {
        cyc(game, 1);
    }
    // LCBF2: INC $0736 : JMP LCC40
    inc_mem(game, 0x0736);
    cyc(game, 6 + 3);
    sv_entry_finish(game);
}

/// `LCB59` (bank 7 `$CB59`): key-area entry body, `Y` = raw index.
///
/// Rebases `$0748` by the X-offset bits of area byte 1 (`>> 6`), then
/// area byte 0 bit 7 selects the indoor path: `$07DB = 0`, the map
/// decode ([`sv_get_entry`]), town code `$056B = ($0748 - $2C) / 2` (+4
/// outside region 0), palace code `$056C = $0748 - $34`, region
/// `$070A = $0706`, new region `$0706 = byte3 & 3`, world
/// `$0707 = (byte3 >> 2) & 7` (world 0 sets `$0709`), mode `$0736 = 0`;
/// otherwise [`sv_entry_passthrough`].
fn sv_entry_body(game: &mut Game) {
    let y = game.cpu.y;
    // LDA $6A3F,y : AND #$C0 : ASL : ROL : ROL : STA $00 : TYA : SEC :
    // SBC $00 : STA $0748
    let b1 = bus_read(game, 0x6A3Fu16.wrapping_add(u16::from(y)));
    lda(game, b1);
    let off = b1 >> 6;
    lda(game, off);
    game.cpu.p &= !FLAG_C;
    game.ram[0x00] = off;
    lda(game, y);
    game.cpu.p |= FLAG_C;
    let rebased = sbc_val(&mut game.cpu.p, y, off);
    game.cpu.a = rebased;
    game.ram[0x0748] = rebased;
    // LDA $6A00,y : ASL : BCC LCBB8
    let b0 = bus_read(game, 0x6A00u16.wrapping_add(u16::from(y)));
    lda(game, b0);
    asl_a(game);
    cyc(
        game,
        4 + cross(0x6A3F, y) + 2 + 2 + 2 + 2 + 3 + 2 + 2 + 3 + 4 + 4 + cross(0x6A00, y) + 2 + 2,
    );
    if b0 & 0x80 == 0 {
        cyc(game, 1);
        sv_entry_passthrough(game);
        return;
    }
    // LDA #$00 : STA $07DB : JSR bank7_Get_Area_Code__Enter_Code_and_Direction
    lda(game, 0);
    game.ram[0x07DB] = 0;
    cyc(game, 2 + 4 + 6);
    inner_jsr_frame(game, 0xCB75, sv_get_entry);
    // LDA $0748 : SEC : SBC #$2C : LSR : STA $056B : LDA $0748 : SEC :
    // SBC #$34 : STA $056C : LDA $0706 : STA $070A
    let idx = game.ram[0x0748];
    lda(game, idx);
    game.cpu.p |= FLAG_C;
    let town = sbc_val(&mut game.cpu.p, idx, 0x2C);
    game.cpu.a = town;
    lsr_a(game);
    game.ram[0x056B] = game.cpu.a;
    lda(game, idx);
    game.cpu.p |= FLAG_C;
    let palace = sbc_val(&mut game.cpu.p, idx, 0x34);
    game.cpu.a = palace;
    game.ram[0x056C] = palace;
    let region = game.ram[0x0706];
    lda(game, region);
    game.ram[0x070A] = region;
    // LDA $6ABD,y : PHA : AND #$03 : STA $0706 : BEQ LCBA5
    let b3 = bus_read(game, 0x6ABDu16.wrapping_add(u16::from(y)));
    lda(game, b3);
    pha(game);
    let new_region = b3 & 0x03;
    lda(game, new_region);
    game.ram[0x0706] = new_region;
    cyc(
        game,
        4 + 2 + 2 + 2 + 4 + 4 + 2 + 2 + 4 + 4 + 4 + 4 + cross(0x6ABD, y) + 3 + 2 + 4 + 2,
    );
    if new_region != 0 {
        // LDA $056B : CLC : ADC #$04 : STA $056B
        let t = game.ram[0x056B];
        lda(game, t);
        game.cpu.p &= !FLAG_C;
        let t = adc_val(&mut game.cpu.p, t, 4);
        game.cpu.a = t;
        game.ram[0x056B] = t;
        cyc(game, 4 + 2 + 2 + 4);
    } else {
        cyc(game, 1);
    }
    // LCBA5: PLA : LSR : LSR : AND #$07 : STA $0707 : BNE LCBB2 : INC $0709
    pla(game);
    lsr_a(game);
    lsr_a(game);
    let world = game.cpu.a & 0x07;
    lda(game, world);
    game.ram[0x0707] = world;
    cyc(game, 4 + 2 + 2 + 2 + 4 + 2);
    if world == 0 {
        inc_mem(game, 0x0709);
        cyc(game, 6);
    } else {
        cyc(game, 1);
    }
    // LCBB2: LDA #$00 : STA $0736 : RTS
    lda(game, 0);
    game.ram[0x0736] = 0;
    cyc(game, 2 + 4 + 6);
}

/// `bank7_Determine_the_Random_Battle_according_to_Links_position_in_OW`
/// (bank 7 `$CC0F`): synthesises key-area slot `$3E` for a random
/// encounter — map byte from the bank table at `$8409` indexed by
/// `(terrain - 4) * 2 + south` (south = `$73 >= $CB32[region]`), Y/X from
/// `$73`/`$74` — then runs [`sv_entry_body`] on it, clears `$075F` and
/// falls into [`sv_entry_finish`].
fn sv_random_battle(game: &mut Game) {
    // LDY $0706 : LDA $73 : CMP bank7_Height_of_frontier…,y : LDA #$00 :
    // ROL : STA $00
    let region = game.ram[0x0706];
    ldy(game, region);
    let ly = game.ram[0x73];
    lda(game, ly);
    let frontier = bus_read(game, 0xCB32u16.wrapping_add(u16::from(region)));
    cmp_val(&mut game.cpu.p, ly, frontier);
    let south = u8::from(game.cpu.p & FLAG_C != 0);
    lda(game, 0);
    lda(game, south);
    game.cpu.p &= !FLAG_C;
    game.ram[0x00] = south;
    // LDA $0563 : SEC : SBC #$04 : ASL : ADC $00 : TAY
    let terrain = game.ram[0x0563];
    lda(game, terrain);
    game.cpu.p |= FLAG_C;
    let t = sbc_val(&mut game.cpu.p, terrain, 4);
    game.cpu.a = t;
    asl_a(game);
    let idx = adc_val(&mut game.cpu.p, game.cpu.a, south);
    lda(game, idx);
    ldy(game, idx);
    // LDA L8409,y : LDY #$3E : STA $6A7E,y : LDA $73 : STA $6A00,y :
    // LDA $74 : STA $6A3F,y
    let map = bus_read(game, 0x8409u16.wrapping_add(u16::from(idx)));
    lda(game, map);
    ldy(game, 0x3E);
    bus_write(game, 0x6A7E + 0x3E, map);
    lda(game, ly);
    bus_write(game, 0x6A00 + 0x3E, ly);
    let lx = game.ram[0x74];
    lda(game, lx);
    bus_write(game, 0x6A3F + 0x3E, lx);
    cyc(
        game,
        4 + 3
            + 4
            + cross(0xCB32, region)
            + 2
            + 2
            + 3
            + 4
            + 2
            + 2
            + 2
            + 3
            + 2
            + 4
            + cross(0x8409, idx)
            + 2
            + 5
            + 3
            + 5
            + 3
            + 5
            + 6,
    );
    inner_jsr_frame(game, 0xCC38, sv_entry_body);
    // LDA #$00 : STA $075F ; falls into LCC40
    lda(game, 0);
    game.ram[0x075F] = 0;
    cyc(game, 2 + 4);
    sv_entry_finish(game);
}

/// `bank7_code17` (bank 7 `$CB35`): key-area → sideview entry selector
/// (mode-table entry).
///
/// Loads the saved bank, then `$0748 == $FF` → [`sv_random_battle`];
/// otherwise mutes the music unless entering West Hyrule's first area or
/// a raft animation is running (`$07AA`/`$07AC`/`$07AD`), and runs
/// [`sv_entry_body`] on the index.
pub fn sv_key_entry(game: &mut Game) {
    cyc(game, 6);
    jsr_sub(game, 0xCB35, 0xFFC9);
    // LDY $0748 : CPY #$FF : BNE LCB42 : JMP bank7_Determine_the_Random_Battle…
    let idx = game.ram[0x0748];
    ldy(game, idx);
    cmp_val(&mut game.cpu.p, idx, 0xFF);
    cyc(game, 4 + 2 + 2);
    if idx == 0xFF {
        cyc(game, 3);
        sv_random_battle(game);
        return;
    }
    cyc(game, 1);
    // LCB42: LDA $0706 : BNE LCB4B : CPY #$00 : BEQ LCB59
    let region = game.ram[0x0706];
    lda(game, region);
    cyc(game, 4 + 2);
    let mut check_mute = true;
    if region == 0 {
        cmp_val(&mut game.cpu.p, idx, 0);
        cyc(game, 2 + 2);
        if idx == 0 {
            cyc(game, 1);
            check_mute = false;
        }
    } else {
        cyc(game, 1);
    }
    if check_mute {
        // LCB4B: LDA $07AC : ORA $07AA : ORA $07AD : BNE LCB59 : JSR Mute
        let raft = game.ram[0x07AC] | game.ram[0x07AA] | game.ram[0x07AD];
        lda(game, raft);
        cyc(game, 4 + 4 + 4 + 2);
        if raft == 0 {
            cyc(game, 6);
            inner_jsr_frame(game, 0xCB56, sv_mute);
        } else {
            cyc(game, 1);
        }
    }
    sv_entry_body(game);
}

/// `bank7_Get_Area_Code__Enter_Code_and_Direction` (bank 7 `$CC97`).
///
/// Decodes area byte 2 at `$6A7E,Y` — `Y` is the caller's register (the
/// raw key-area index), not `$0748`, which `bank7_code17` has already
/// rebased by the time it calls here — via [`scroll::decode_entry`] into
/// `$0561` / `$075C` / `$0701`. Exit: `A` = direction (`N`/`Z` from it),
/// `C` = map-byte bit 5 (the last `ROL`), `X`/`Y` untouched, one dead
/// stack byte (`PHA`). Cycles: 41 (+1 page-cross), RTS included.
pub fn sv_get_entry(game: &mut Game) {
    let y = game.cpu.y;
    let extra = cross(0x6A7E, y);
    let map = bus_read(game, 0x6A7Eu16.wrapping_add(u16::from(y)));
    game.ram[0x0100 + usize::from(game.cpu.sp)] = map;
    let (area_code, enter, dir) = scroll::decode_entry(map);
    w(&mut game.ram, 0x0561, area_code);
    w(&mut game.ram, 0x075C, enter);
    w(&mut game.ram, 0x0701, dir);
    if map & 0x20 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    lda(game, dir);
    cyc(game, 41 + extra);
}

/// `bank7_go_outside` (bank 7 `$CCB3`): sideview → overworld return
/// (mode-table entry).
///
/// Clears `$0709`/`$075B`, restores `$73`/`$74` from key-area `$0748`
/// (bytes 0/1 masked; a zero Y byte means the raft spots: X `$3D` → `$51`,
/// else `$66`). Areas flagged in byte 3 bit 6 offset Link one square in
/// the direction he faced (`$0562` < 4: X, else Y; the step count comes
/// from `$5F`, or from `$0562` when byte 0 bit 7 is set — one square
/// forward, two back). Ends with bank 0 and `$0736++` (`LCF05`).
pub fn sv_go_outside(game: &mut Game) {
    // LDA #$00 : STA $0709 : STA $075B : LDY $0748 : LDA $6A00,y : BNE LCCCC
    lda(game, 0);
    game.ram[0x0709] = 0;
    game.ram[0x075B] = 0;
    let y = game.ram[0x0748];
    ldy(game, y);
    let b0 = bus_read(game, 0x6A00u16.wrapping_add(u16::from(y)));
    lda(game, b0);
    cyc(game, 2 + 4 + 4 + 4 + 4 + cross(0x6A00, y) + 2);
    let b1 = bus_read(game, 0x6A3Fu16.wrapping_add(u16::from(y)));
    if b0 == 0 {
        // LDA #$3D : CMP $6A3F,y : BNE LCCAF (LDA #$66 : BNE LCCCC) : LDA #$51
        lda(game, 0x3D);
        cmp_val(&mut game.cpu.p, 0x3D, b1);
        cyc(game, 2 + 4 + cross(0x6A3F, y) + 2);
        if b1 != 0x3D {
            lda(game, 0x66);
            cyc(game, 1 + 2 + 3);
        } else {
            lda(game, 0x51);
            cyc(game, 2);
        }
    } else {
        cyc(game, 1);
    }
    // LCCCC: AND #$7F : STA $73 : LDA $6A3F,y : AND #$3F : STA $74 :
    // LDA $6ABD,y : AND #$40 : BEQ LCCF9
    let ty = game.cpu.a & 0x7F;
    lda(game, ty);
    game.ram[0x73] = ty;
    lda(game, b1);
    let tx = b1 & 0x3F;
    lda(game, tx);
    game.ram[0x74] = tx;
    let b3 = bus_read(game, 0x6ABDu16.wrapping_add(u16::from(y)));
    lda(game, b3);
    let offset = b3 & 0x40;
    lda(game, offset);
    cyc(
        game,
        2 + 3 + 4 + cross(0x6A3F, y) + 2 + 3 + 4 + cross(0x6ABD, y) + 2 + 2,
    );
    if offset != 0 {
        // LDX $0562 : CPX #$04 : BCS LCCFF
        let facing = game.ram[0x0562];
        ldx(game, facing);
        cmp_val(&mut game.cpu.p, facing, 4);
        cyc(game, 4 + 2 + 2);
        if facing < 4 {
            // INC $74 : LDX $5F : LDA $6A00,y : ASL : BCC LCCF2 : LDX $0562
            inc_mem(game, 0x0074);
            ldx(game, game.ram[0x5F]);
            lda(game, b0);
            asl_a(game);
            cyc(game, 5 + 3 + 4 + cross(0x6A00, y) + 2 + 2);
            if b0 & 0x80 != 0 {
                ldx(game, facing);
                cyc(game, 4);
            } else {
                cyc(game, 1);
            }
            // LCCF2: DEX : BEQ LCCF9 : DEC $74 : DEC $74
            let steps = game.cpu.x.wrapping_sub(1);
            ldx(game, steps);
            cyc(game, 2 + 2);
            if steps != 0 {
                dec_zp(game, 0x74);
                dec_zp(game, 0x74);
                cyc(game, 5 + 5);
            } else {
                cyc(game, 1);
            }
        } else {
            // LCCFF: INC $73 : LDX $5F : LDA $6A00,y : ASL : BCC LCD0F :
            // LDA $0562 : LSR : LSR : TAX
            cyc(game, 1);
            inc_mem(game, 0x0073);
            ldx(game, game.ram[0x5F]);
            lda(game, b0);
            asl_a(game);
            cyc(game, 5 + 3 + 4 + cross(0x6A00, y) + 2 + 2);
            if b0 & 0x80 != 0 {
                lda(game, facing);
                lsr_a(game);
                lsr_a(game);
                ldx(game, game.cpu.a);
                cyc(game, 4 + 2 + 2 + 2);
            } else {
                cyc(game, 1);
            }
            // LCD0F: DEX : BEQ LCCF9 : DEC $73 : DEC $73 : JMP LCCF9
            let steps = game.cpu.x.wrapping_sub(1);
            ldx(game, steps);
            cyc(game, 2 + 2);
            if steps != 0 {
                dec_zp(game, 0x73);
                dec_zp(game, 0x73);
                cyc(game, 5 + 5 + 3);
            } else {
                cyc(game, 2); // BEQ taken across the $CD/$CC page edge
            }
        }
    } else {
        cyc(game, 1);
    }
    // LCCF9: JSR SwapToPRG0 : JMP LCF05 (INC $0736 : RTS)
    cyc(game, 6);
    jsr_sub(game, 0xCCF9, 0xFFC5);
    inc_mem(game, 0x0736);
    cyc(game, 3 + 6 + 6);
}

/// `LC690` (bank 7 `$C690`): exit bookkeeping for return slot `X` (4 for
/// elevators, `$075B + 4` for doors).
///
/// Records Link's state for the way back: `$69B3,x = $FD & $F0`,
/// `$69BA,x = $071F`, `$69AC,x = $072C & $F0`, the scroll page/columns
/// around it (`$6997/$6982/$69A5/$6989/$697B/$699E/$6990,x`, computed
/// with the `-$60`/`+$60` borrow/carry through `PHP`/`PLP`), and Link's
/// page/X (`$05CC,x`, `$05D3,x = $4D & $F0`). 156 cycles, RTS included.
fn sv_exit_bookkeeping(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    // LDA $FD : AND #$F0 : STA $69B3,x : LDA $071F : STA $69BA,x
    let fd = game.ram[0xFD];
    lda(game, fd);
    lda(game, fd & 0xF0);
    bus_write(game, 0x69B3 + x, fd & 0xF0);
    let half = game.ram[0x071F];
    lda(game, half);
    bus_write(game, 0x69BA + x, half);
    // LDA $072C : AND #$F0 : STA $69AC,x : PHA : SEC : SBC #$60 : PHP
    let scroll_lo = game.ram[0x072C];
    lda(game, scroll_lo);
    let sl = scroll_lo & 0xF0;
    lda(game, sl);
    bus_write(game, 0x69AC + x, sl);
    pha(game);
    game.cpu.p |= FLAG_C;
    let back = sbc_val(&mut game.cpu.p, sl, 0x60);
    game.cpu.a = back;
    php(game);
    // ORA #$10 : LSR x4 : STA $6997,x : AND #$0E : STA $6982,x
    let o = back | 0x10;
    lda(game, o);
    for _ in 0..4 {
        lsr_a(game);
    }
    let col = game.cpu.a;
    bus_write(game, 0x6997 + x, col);
    lda(game, col & 0x0E);
    bus_write(game, 0x6982 + x, col & 0x0E);
    // LDA $072A : STA $69A5,x : PLP : SBC #$00 : AND #$03 : STA $6989,x :
    // STA $697B,x
    let scroll_hi = game.ram[0x072A];
    lda(game, scroll_hi);
    bus_write(game, 0x69A5 + x, scroll_hi);
    plp(game);
    let pg = sbc_val(&mut game.cpu.p, scroll_hi, 0) & 0x03;
    lda(game, pg);
    bus_write(game, 0x6989 + x, pg);
    bus_write(game, 0x697B + x, pg);
    // PLA : CLC : ADC #$60 : PHP : AND #$E0 : LSR x4 : STA $699E,x
    pla(game);
    game.cpu.p &= !FLAG_C;
    let fwd = adc_val(&mut game.cpu.p, game.cpu.a, 0x60);
    game.cpu.a = fwd;
    php(game);
    lda(game, fwd & 0xE0);
    for _ in 0..4 {
        lsr_a(game);
    }
    let col2 = game.cpu.a;
    bus_write(game, 0x699E + x, col2);
    // LDA $072A : PLP : ADC #$01 : AND #$03 : STA $6990,x
    lda(game, scroll_hi);
    plp(game);
    let pg2 = adc_val(&mut game.cpu.p, scroll_hi, 1) & 0x03;
    lda(game, pg2);
    bus_write(game, 0x6990 + x, pg2);
    // LDA $3B : STA $05CC,x : LDA $4D : AND #$F0 : STA $05D3,x : RTS
    let page = game.ram[0x3B];
    lda(game, page);
    game.ram[0x05CC + usize::from(x)] = page;
    let lx = game.ram[0x4D];
    lda(game, lx);
    lda(game, lx & 0xF0);
    game.ram[0x05D3 + usize::from(x)] = lx & 0xF0;
    cyc(game, 156);
}

/// `bank7_take_elevator_exit` (bank 7 `$C644`; mode-table entry).
///
/// Connectivity byte `$6AFC[area * 4 + 1 (+1 when Up is held)]`: bits 7-2
/// = next area (`$0561`), bits 1-0 = page (`$3B`, `$072A`; spawn window
/// `$0733 = page + 1`, `$0732 = page - 1`, wrapped to 3). Start page code
/// 4 (`$075C`), [`sv_exit_bookkeeping`] for slot 4, `$0705++`, then
/// `LC722`: `$073D = 0`, `$0726++`, `$0736++`.
pub fn sv_elev_exit(game: &mut Game) {
    cyc(game, 6);
    jsr_sub(game, 0xC644, 0xFFC9);
    // LDA $0561 : ASL : ASL : ADC #$01 : LDY $0743 : CPY #$08 : BNE LC657 :
    // ADC #$00
    let area = game.ram[0x0561];
    lda(game, area);
    asl_a(game);
    asl_a(game);
    let idx = adc_val(&mut game.cpu.p, game.cpu.a, 1);
    game.cpu.a = idx;
    let dir = game.ram[0x0743];
    ldy(game, dir);
    cmp_val(&mut game.cpu.p, dir, scroll::ELEV_UP);
    cyc(game, 4 + 2 + 2 + 2 + 4 + 2 + 2);
    if dir == scroll::ELEV_UP {
        let up = adc_val(&mut game.cpu.p, idx, 0);
        game.cpu.a = up;
        cyc(game, 2);
    } else {
        cyc(game, 1);
    }
    // LC657: TAY : LDA $6AFC,y : LSR : LSR : STA $0561 : LDA $6AFC,y :
    // AND #$03 : STA $3B : STA $072A
    let idx = game.cpu.a;
    ldy(game, idx);
    let conn = bus_read(game, 0x6AFCu16.wrapping_add(u16::from(idx)));
    lda(game, conn);
    lsr_a(game);
    lsr_a(game);
    game.ram[0x0561] = game.cpu.a;
    lda(game, conn);
    let page = conn & 0x03;
    lda(game, page);
    game.ram[0x3B] = page;
    game.ram[0x072A] = page;
    // CLC : ADC #$01 : STA $0733 : SEC : SBC #$02 : CMP #$FF : BNE LC679 :
    // LDA #$03
    game.cpu.p &= !FLAG_C;
    let right = adc_val(&mut game.cpu.p, page, 1);
    game.cpu.a = right;
    game.ram[0x0733] = right;
    game.cpu.p |= FLAG_C;
    let left = sbc_val(&mut game.cpu.p, right, 2);
    game.cpu.a = left;
    cmp_val(&mut game.cpu.p, left, 0xFF);
    cyc(
        game,
        2 + 4
            + cross(0x6AFC, idx)
            + 2
            + 2
            + 4
            + 4
            + cross(0x6AFC, idx)
            + 2
            + 3
            + 4
            + 2
            + 2
            + 4
            + 2
            + 2
            + 2
            + 2,
    );
    if left == 0xFF {
        lda(game, 3);
        cyc(game, 2);
    } else {
        cyc(game, 1);
    }
    // LC679: STA $0732 : LC67C: LDX #$04 : STX $075C : JSR LC690 :
    // INC $0705 : JMP LC722
    game.ram[0x0732] = game.cpu.a;
    ldx(game, scroll::ELEV_MIDDLE_PAGE);
    game.ram[0x075C] = scroll::ELEV_MIDDLE_PAGE;
    cyc(game, 4 + 2 + 4 + 6);
    inner_jsr_frame(game, 0xC681, sv_exit_bookkeeping);
    inc_mem(game, 0x0705);
    cyc(game, 6 + 3);
    // LC722: LDA #$00 : STA $073D : INC $0726 : JMP LCF05 (INC $0736 : RTS)
    lda(game, 0);
    game.ram[0x073D] = 0;
    inc_mem(game, 0x0726);
    inc_mem(game, 0x0736);
    cyc(game, 2 + 4 + 6 + 3 + 6 + 6);
}

/// `LCFEC` (bank 7 `$CFEC`): exit music/mode tail — `$075F = 0` unless an
/// elevator ride is pending (`$0704`), then mode `$0736 = 7`. Exit `A = 7`.
fn sv_exit_music(game: &mut Game) {
    let ride = game.ram[0x0704];
    ldx(game, ride);
    cyc(game, 4 + 2);
    if ride == 0 {
        lda(game, 0);
        game.ram[0x075F] = 0;
        cyc(game, 2 + 4);
    } else {
        cyc(game, 1);
    }
    lda(game, 7);
    game.ram[0x0736] = 7;
    cyc(game, 2 + 4 + 6);
}

/// `bank7_take_side_exit` (bank 7 `$CF4C`; mode-table entry).
///
/// Hides the sprites, loads the saved bank, and reads the connectivity
/// byte `$6AFC[area * 4 + $3B]` (areas >= `$1D` of world 0 use area 0).
/// `$FC`-class bytes leave the area: `$0748 += bits 1-0`, `$07FF` and the
/// area sound cleared, pulse 1 `$4000 = $90`; a non-zero world mutes,
/// resets `$0707`, sets `$0709` and mode 0, while world 0 plays sound 4
/// (0 for West Hyrule area 0) and takes mode 1. Otherwise the byte is
/// the next room: `$0561`, `$075C`/`$0701` (page kept when `$0704`),
/// door depth `$075B` shifts the start page by 4 and counts down (leaving
/// a non-town-7 house mutes and requests music 2), then [`sv_exit_music`].
pub fn sv_side_exit(game: &mut Game) {
    cyc(game, 6);
    jsr_sub(game, 0xCF4C, 0xD24C);
    cyc(game, 6);
    jsr_sub(game, 0xCF4F, 0xFFC9);
    // LDA $0561 : LDY $0707 : BNE LCF60 : CMP #$1D : BCC LCF60 : LDA #$00
    let area = game.ram[0x0561];
    lda(game, area);
    let world = game.ram[0x0707];
    ldy(game, world);
    cyc(game, 4 + 4 + 2);
    if world == 0 {
        cmp_val(&mut game.cpu.p, area, 0x1D);
        cyc(game, 2 + 2);
        if area >= 0x1D {
            lda(game, 0);
            cyc(game, 2);
        } else {
            cyc(game, 1);
        }
    } else {
        cyc(game, 1);
    }
    // LCF60: ASL : ASL : ADC $3B : TAY : LDA $6AFC,y : PHA : AND #$FC :
    // CMP #$FC : BNE LCFB2
    asl_a(game);
    asl_a(game);
    let idx = adc_val(&mut game.cpu.p, game.cpu.a, game.ram[0x3B]);
    lda(game, idx);
    ldy(game, idx);
    let conn = bus_read(game, 0x6AFCu16.wrapping_add(u16::from(idx)));
    lda(game, conn);
    pha(game);
    let class = conn & 0xFC;
    lda(game, class);
    cmp_val(&mut game.cpu.p, class, area::CONN_WALL);
    cyc(game, 2 + 2 + 3 + 2 + 4 + cross(0x6AFC, idx) + 3 + 2 + 2 + 2);
    if class != area::CONN_WALL {
        // LCFB2: LSR : LSR : STA $0561 : PLA : LDX $0704 : BNE LCFC2 :
        // AND #$03 : STA $075C
        cyc(game, 1);
        lsr_a(game);
        lsr_a(game);
        game.ram[0x0561] = game.cpu.a;
        pla(game);
        let ride = game.ram[0x0704];
        ldx(game, ride);
        cyc(game, 2 + 2 + 4 + 4 + 4 + 2);
        if ride == 0 {
            let page = game.cpu.a & 0x03;
            lda(game, page);
            game.ram[0x075C] = page;
            cyc(game, 2 + 4);
        } else {
            cyc(game, 1);
        }
        // LCFC2: AND #$01 : STA $0701 : LDA $0561 : CMP #$24 : BCS LCFEC
        let dir = game.cpu.a & 0x01;
        lda(game, dir);
        game.ram[0x0701] = dir;
        let next = game.ram[0x0561];
        lda(game, next);
        cmp_val(&mut game.cpu.p, next, 0x24);
        cyc(game, 2 + 4 + 4 + 2 + 2);
        if next < 0x24 {
            // LDA $075B : BEQ LCFEC : CLC : ADC #$04 : STA $075C :
            // DEC $075B : BNE LCFEC
            let depth = game.ram[0x075B];
            lda(game, depth);
            cyc(game, 4 + 2);
            if depth != 0 {
                game.cpu.p &= !FLAG_C;
                let start = adc_val(&mut game.cpu.p, depth, 4);
                game.cpu.a = start;
                game.ram[0x075C] = start;
                let left = depth.wrapping_sub(1);
                set_nz(&mut game.cpu.p, left);
                game.ram[0x075B] = left;
                cyc(game, 2 + 2 + 4 + 6 + 2);
                if left == 0 {
                    // LDA $056B : CMP #$07 : BEQ LCFEC : JSR Mute : LDA #$02 :
                    // BNE LCFF3 (STA $075F : LDA #$07 : STA $0736 : RTS)
                    let town = game.ram[0x056B];
                    lda(game, town);
                    cmp_val(&mut game.cpu.p, town, 7);
                    cyc(game, 4 + 2 + 2);
                    if town != 7 {
                        cyc(game, 6);
                        inner_jsr_frame(game, 0xCFE5, sv_mute);
                        lda(game, 2);
                        game.ram[0x075F] = 2;
                        lda(game, 7);
                        game.ram[0x0736] = 7;
                        cyc(game, 2 + 3 + 4 + 2 + 4 + 6);
                        return;
                    }
                    cyc(game, 1);
                } else {
                    cyc(game, 1);
                }
            } else {
                cyc(game, 1);
            }
        } else {
            cyc(game, 1);
        }
        sv_exit_music(game);
        return;
    }
    // PLA : AND #$03 : CLC : ADC $0748 : STA $0748 : LDY #$00 : STY $07FF :
    // STY $05E9 : LDY #$90 : STY $4000 : LDA #$01 : LDY $0707 : BEQ LCF9D
    pla(game);
    let step = conn & 0x03;
    lda(game, step);
    game.cpu.p &= !FLAG_C;
    let next_index = adc_val(&mut game.cpu.p, step, game.ram[0x0748]);
    game.cpu.a = next_index;
    game.ram[0x0748] = next_index;
    ldy(game, 0);
    game.ram[0x07FF] = 0;
    game.ram[0x05E9] = 0;
    ldy(game, 0x90);
    bus_write(game, 0x4000, 0x90);
    lda(game, 1);
    ldy(game, world);
    cyc(game, 4 + 2 + 2 + 4 + 4 + 2 + 4 + 4 + 2 + 4 + 2 + 4 + 2);
    if world != 0 {
        // JSR Mute : LDY #$00 : STY $0707 : INC $0709 : LDA #$00 : JMP LCFF8
        cyc(game, 6);
        inner_jsr_frame(game, 0xCF8D, sv_mute);
        ldy(game, 0);
        game.ram[0x0707] = 0;
        inc_mem(game, 0x0709);
        lda(game, 0);
        game.ram[0x0736] = 0;
        cyc(game, 2 + 4 + 6 + 2 + 3 + 4 + 6);
        return;
    }
    // LCF9D: LDY #$04 : STY $05E9 : LDY $0706 : BNE LCF9A : LDY $0561 :
    // BNE LCF9A : STY $05E9 : JMP LCFF8 (STA $0736 : RTS)
    cyc(game, 1);
    ldy(game, 4);
    game.ram[0x05E9] = 4;
    let region = game.ram[0x0706];
    ldy(game, region);
    cyc(game, 2 + 4 + 4 + 2);
    if region != 0 {
        game.ram[0x0736] = 1;
        cyc(game, 1 + 3 + 4 + 6);
        return;
    }
    let area = game.ram[0x0561];
    ldy(game, area);
    cyc(game, 4 + 2);
    if area != 0 {
        game.ram[0x0736] = 1;
        cyc(game, 1 + 3 + 4 + 6);
        return;
    }
    game.ram[0x05E9] = 0;
    game.ram[0x0736] = 1;
    cyc(game, 4 + 3 + 4 + 6);
}

/// `bank7_take_door_exit` (bank 7 `$CFFC`; mode-table entry).
///
/// Door byte `$8817[area * 4 + $3B]` (sideview bank): bits 7-2 = next
/// area, bits 1-0 = start page / facing. Books the return in slot
/// `$075B + 4` ([`sv_exit_bookkeeping`]), runs [`sv_exit_music`], and when
/// this is the first door (`$075B == 1`) of a non-town-7 house requests
/// indoor music 8 and mutes (`$EB = $80`).
pub fn sv_door_exit(game: &mut Game) {
    cyc(game, 6);
    jsr_sub(game, 0xCFFC, 0xFFC9);
    // LDA $0561 : ASL : ASL : ADC $3B : TAY : LDA L8817,y : AND #$FC : LSR :
    // LSR : STA $0561 : LDA L8817,y : AND #$03 : STA $075C : AND #$01 :
    // STA $0701 : LDA $075B : CLC : ADC #$04 : TAX : JSR LC690
    let area = game.ram[0x0561];
    lda(game, area);
    asl_a(game);
    asl_a(game);
    let idx = adc_val(&mut game.cpu.p, game.cpu.a, game.ram[0x3B]);
    lda(game, idx);
    ldy(game, idx);
    let door = bus_read(game, 0x8817u16.wrapping_add(u16::from(idx)));
    lda(game, door);
    lda(game, door & 0xFC);
    lsr_a(game);
    lsr_a(game);
    game.ram[0x0561] = game.cpu.a;
    lda(game, door);
    let page = door & 0x03;
    lda(game, page);
    game.ram[0x075C] = page;
    lda(game, page & 0x01);
    game.ram[0x0701] = page & 0x01;
    let depth = game.ram[0x075B];
    lda(game, depth);
    game.cpu.p &= !FLAG_C;
    let slot = adc_val(&mut game.cpu.p, depth, 4);
    lda(game, slot);
    ldx(game, slot);
    cyc(
        game,
        4 + 2
            + 2
            + 3
            + 2
            + 4
            + cross(0x8817, idx)
            + 2
            + 2
            + 2
            + 4
            + 4
            + cross(0x8817, idx)
            + 2
            + 4
            + 2
            + 4
            + 4
            + 2
            + 2
            + 2
            + 6,
    );
    inner_jsr_frame(game, 0xD025, sv_exit_bookkeeping);
    cyc(game, 6);
    inner_jsr_frame(game, 0xD028, sv_exit_music);
    // LDX $075B : DEX : BNE LD041
    let depth = game.ram[0x075B];
    ldx(game, depth);
    ldx(game, depth.wrapping_sub(1));
    cyc(game, 4 + 2 + 2);
    if depth != 1 {
        cyc(game, 1 + 6);
        return;
    }
    // LDA $056B : CMP #$07 : BEQ LD041 : LDA #$08 : STA $075F : LDA #$80 :
    // STA $EB : RTS
    let town = game.ram[0x056B];
    lda(game, town);
    cmp_val(&mut game.cpu.p, town, 7);
    cyc(game, 4 + 2 + 2);
    if town == 7 {
        cyc(game, 1 + 6);
        return;
    }
    lda(game, 8);
    game.ram[0x075F] = 8;
    lda(game, 0x80);
    game.ram[0xEB] = 0x80;
    cyc(game, 2 + 4 + 2 + 3 + 6);
}

/// `bank7_code22` (bank 7 `$D603`): spawn gate (enemy-routine table
/// entry, reached through the dispatcher's `JMP ($0E)`).
///
/// `$0759 != 0` → RTS. `X = ($071F >> 1) ^ 1` picks the screen pair for
/// [`sv_enemy_spawn`], which runs when `$0732 < $0733`, or `$0732 == 2`
/// with `X == 0`, or `$0732 != 2` with `X == 1` (the [`spawn::spawn_gate`]
/// predicate); otherwise RTS with the compare flags.
pub fn sv_spawn_gate(game: &mut Game) {
    let fairy = game.ram[0x0759];
    lda(game, fairy);
    cyc(game, 4 + 2);
    if fairy != 0 {
        cyc(game, 1 + 6);
        return;
    }
    // LDA $071F : LSR : EOR #$01 : TAX
    let half = game.ram[0x071F];
    lda(game, half);
    lsr_a(game);
    let x = game.cpu.a ^ 1;
    lda(game, x);
    game.cpu.x = x;
    // LDA $0732 : CMP $0733 : BCC LD625
    let left = game.ram[0x0732];
    let right = game.ram[0x0733];
    lda(game, left);
    cmp_val(&mut game.cpu.p, left, right);
    cyc(game, 4 + 2 + 2 + 2 + 4 + 4 + 2);
    if left < right {
        cyc(game, 1);
        sv_enemy_spawn(game);
        return;
    }
    // CMP #$02 : BEQ LD620
    cmp_val(&mut game.cpu.p, left, 2);
    cyc(game, 2 + 2);
    if left == 2 {
        // LD620: CPX #$00 : BEQ LD625 : RTS
        cmp_val(&mut game.cpu.p, x, 0);
        cyc(game, 1 + 2 + 2);
        if x == 0 {
            cyc(game, 1);
            sv_enemy_spawn(game);
            return;
        }
        cyc(game, 6);
        return;
    }
    // CPX #$01 : BEQ LD625 : RTS
    cmp_val(&mut game.cpu.p, x, 1);
    cyc(game, 2 + 2);
    if x == 1 {
        cyc(game, 1);
        sv_enemy_spawn(game);
        return;
    }
    cyc(game, 6);
}

/// `bank7_Determine_Enemy_Facing_Direction_relative_to_Link` (bank 7
/// `$DC91`): `$60,x` = 1 (Link is right of the enemy) or 2 (left) from
/// the 16-bit compare of `Link X + 8` (carry in from the caller) against
/// the enemy's X; `$0E/$0F` hold the intermediate values. Exit `Y` =
/// `$60,x - 1`, `A` = high-byte difference.
fn sv_facing(game: &mut Game) {
    let x = game.cpu.x;
    ldy(game, 1);
    // LDA $4D : ADC #$08 : PHA : LDA $3B : ADC #$00 : STA $0E
    let lx = game.ram[0x4D];
    lda(game, lx);
    let lo = adc_val(&mut game.cpu.p, lx, 8);
    game.cpu.a = lo;
    pha(game);
    let hx = game.ram[0x3B];
    lda(game, hx);
    let hi = adc_val(&mut game.cpu.p, hx, 0);
    game.cpu.a = hi;
    game.ram[0x0E] = hi;
    // PLA : SBC $4E,x : STA $0F : LDA $0E : SBC $3C,x : BPL LDCAA : INY
    pla(game);
    let dlo = sbc_val(&mut game.cpu.p, game.cpu.a, game.ram[zpx(0x4E, x)]);
    game.cpu.a = dlo;
    game.ram[0x0F] = dlo;
    lda(game, hi);
    let dhi = sbc_val(&mut game.cpu.p, hi, game.ram[zpx(0x3C, x)]);
    game.cpu.a = dhi;
    cyc(game, 2 + 3 + 2 + 3 + 3 + 2 + 3 + 4 + 4 + 3 + 3 + 4 + 2);
    if dhi & 0x80 != 0 {
        ldy(game, 2);
        cyc(game, 2);
    } else {
        cyc(game, 1);
    }
    // LDCAA: STY $60,x : DEY : RTS
    let facing = game.cpu.y;
    game.ram[zpx(0x60, x)] = facing;
    ldy(game, facing.wrapping_sub(1));
    cyc(game, 4 + 2 + 6);
}

/// `LD625` (bank 7 `$D625`): spawn one enemy from the area's enemy list.
///
/// Entry `X` selects the screen pair (`$0732,x` → `$00`, `$0734,x` →
/// `$01`); slot `X = $10`. Walks the `($D6)` list (byte 0 = length, then
/// 2-byte entries) for an entry whose screen (`b1 >> 6`) and column
/// (`b0 & $0F`) match; an entry already flagged (`b0` bit 7) → RTS. On a
/// match: `$B6,x++`, flag the entry, `$BC,x` = list index, Y from the
/// `LD5FB` row table (`b0` bits 6-4), `$3C,x`/`$4E,x` from `$00`/`$01`,
/// enemy code `$A1,x = b1 & $3F`, facing ([`sv_facing`]) with the
/// `bank7_table15` velocity into `$71,x`, `$1A,x = 1`, the state bytes
/// (`$040E/$AF/$81/$043E/$057E/$0444/$04A0,x`) zeroed, HP `$C2,x` from
/// `$6D21,y`, then the tail `JMP ($0E)` into the init routine at
/// `$6D45,code*2` ([`Game::trap_jump`]).
pub fn sv_enemy_spawn(game: &mut Game) {
    let xin = game.cpu.x;
    // LDA $0732,x : STA $00 : LDA $0734,x : STA $01 : LDX $10 : LDY #$01
    let screen = bus_read(game, 0x0732u16.wrapping_add(u16::from(xin)));
    lda(game, screen);
    game.ram[0x00] = screen;
    let col = bus_read(game, 0x0734u16.wrapping_add(u16::from(xin)));
    lda(game, col);
    game.ram[0x01] = col;
    let x = game.ram[0x10];
    ldx(game, x);
    ldy(game, 1);
    cyc(game, 4 + 3 + 4 + 3 + 3 + 2);
    let list = u16::from(game.ram[0xD6]) | (u16::from(game.ram[0xD7]) << 8);
    let mut idx = 1u8;
    let hit = loop {
        // LD633: TYA : LDY #$00 : CMP ($D6),y : BCS LD624
        lda(game, idx);
        ldy(game, 0);
        let len = bus_read(game, list);
        cmp_val(&mut game.cpu.p, idx, len);
        cyc(game, 2 + 2 + 5 + 2);
        if idx >= len {
            cyc(game, 1 + 6);
            return;
        }
        // TAY : INY : LDA ($D6),y : ASL : ROL : ROL : AND #$03 : CMP $00
        ldy(game, idx.wrapping_add(1));
        let e1 = bus_read(game, list.wrapping_add(u16::from(idx) + 1));
        cyc(
            game,
            2 + 2 + 5 + cross(list, idx.wrapping_add(1)) + 2 + 2 + 2 + 2 + 3,
        );
        let scr = e1 >> 6;
        lda(game, scr);
        if e1 & 0x20 != 0 {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        cmp_val(&mut game.cpu.p, scr, screen);
        cyc(game, 2);
        if scr == screen {
            // LD64B: DEY : LDA ($D6),y : AND #$0F : CMP $01 : BEQ LD658
            cyc(game, 1);
            ldy(game, idx);
            let e0 = bus_read(game, list.wrapping_add(u16::from(idx)));
            let c = e0 & 0x0F;
            lda(game, c);
            cmp_val(&mut game.cpu.p, c, col);
            cyc(game, 2 + 5 + cross(list, idx) + 2 + 3 + 2);
            if c == col {
                cyc(game, 1);
                break e0;
            }
            // INY : JMP LD647
            ldy(game, idx.wrapping_add(1));
            cyc(game, 2 + 3);
        }
        // LD647: INY : JMP LD633
        idx = idx.wrapping_add(2);
        ldy(game, idx);
        cyc(game, 2 + 3);
    };
    // LD658: LDA ($D6),y : ASL : BCS LD6C0
    lda(game, hit);
    let shifted = hit << 1;
    if hit & 0x80 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    set_nz(&mut game.cpu.p, shifted);
    game.cpu.a = shifted;
    cyc(game, 5 + cross(list, idx) + 2 + 2);
    if hit & 0x80 != 0 {
        cyc(game, 1 + 6);
        return;
    }
    // INC $B6,x : PHA : SEC : ROR : STA ($D6),y : STY $02 : PLA
    let live = inc_val(&mut game.cpu.p, game.ram[zpx(0xB6, x)]);
    game.ram[zpx(0xB6, x)] = live;
    pha(game);
    let flagged = (shifted >> 1) | 0x80;
    game.cpu.p &= !FLAG_C; // ROR shifts (b0 << 1)'s clear bit 0 into C.
    set_nz(&mut game.cpu.p, flagged);
    bus_write(game, list.wrapping_add(u16::from(idx)), flagged);
    game.ram[0x02] = idx;
    pla(game);
    // LSR x5 : AND #$07 : TAY : LDA LD5FB,y : STA $2A,x
    let row = (shifted >> 5) & 0x07;
    if shifted & 0x10 != 0 {
        game.cpu.p |= FLAG_C; // fifth LSR shifts out bit 4.
    } else {
        game.cpu.p &= !FLAG_C;
    }
    lda(game, row);
    ldy(game, row);
    let ey = bus_read(game, 0xD5FB + u16::from(row));
    lda(game, ey);
    game.ram[zpx(0x2A, x)] = ey;
    // LDY $02 : STY $BC,x : LDA $00 : STA $3C,x : LDA $01 : ASL x4 : STA $4E,x
    ldy(game, idx);
    game.ram[zpx(0xBC, x)] = idx;
    lda(game, screen);
    game.ram[zpx(0x3C, x)] = screen;
    lda(game, col);
    let ex = col << 4;
    if col & 0x10 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    lda(game, ex);
    game.ram[zpx(0x4E, x)] = ex;
    // INY : LDA ($D6),y : AND #$3F : STA $A1,x : JSR $DC91
    ldy(game, idx.wrapping_add(1));
    let e1 = bus_read(game, list.wrapping_add(u16::from(idx) + 1));
    let code = e1 & 0x3F;
    lda(game, code);
    game.ram[zpx(0xA1, x)] = code;
    cyc(
        game,
        6 + 3
            + 2
            + 2
            + 6
            + 3
            + 4
            + 10
            + 2
            + 2
            + 4
            + 4
            + 3
            + 4
            + 3
            + 4
            + 3
            + 8
            + 4
            + 2
            + 5
            + cross(list, idx.wrapping_add(1))
            + 2
            + 4
            + 6,
    );
    inner_jsr_frame(game, 0xD68B, sv_facing);
    // LDA bank7_table15,y : STA $71,x : LDA #$01 : STA $1A,x : LSR :
    // STA $040E,x : STA $AF,x : STA $81,x : STA $043E,x : STA $057E,x :
    // STA $0444,x : STA $04A0,x
    let y = game.cpu.y;
    let vel = bus_read(game, 0xD5F9u16.wrapping_add(u16::from(y)));
    lda(game, vel);
    game.ram[zpx(0x71, x)] = vel;
    lda(game, 1);
    game.ram[zpx(0x1A, x)] = 1;
    lsr_a(game);
    let xs = usize::from(x);
    game.ram[0x040E + xs] = 0;
    game.ram[zpx(0xAF, x)] = 0;
    game.ram[zpx(0x81, x)] = 0;
    game.ram[0x043E + xs] = 0;
    game.ram[0x057E + xs] = 0;
    game.ram[0x0444 + xs] = 0;
    game.ram[0x04A0 + xs] = 0;
    // LDY $A1,x : LDA $6D21,y : STA $C2,x : TYA : ASL : TAY : LDA $6D45,y :
    // STA $0E : LDA $6D46,y : JMP $D6D6 (STA $0F : JMP ($0E))
    ldy(game, code);
    let hp = bus_read(game, 0x6D21 + u16::from(code));
    lda(game, hp);
    game.ram[zpx(0xC2, x)] = hp;
    lda(game, code);
    let doubled = code << 1;
    if code & 0x80 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    lda(game, doubled);
    ldy(game, doubled);
    let plo = bus_read(game, 0x6D45 + u16::from(doubled));
    lda(game, plo);
    game.ram[0x0E] = plo;
    let phi = bus_read(game, 0x6D46 + u16::from(doubled));
    lda(game, phi);
    game.ram[0x0F] = phi;
    cyc(
        game,
        4 + cross(0xD5F9, y)
            + 4
            + 2
            + 4
            + 2
            + 5
            + 4
            + 4
            + 5
            + 5
            + 5
            + 5
            + 4
            + 4
            + 4
            + 2
            + 2
            + 2
            + 4
            + 3
            + 4
            + 3
            + 3
            + 5,
    );
    let target = u16::from(plo) | (u16::from(phi) << 8);
    game.trap_jump(target);
}

/// `bank7_Link_Collision_Detection` (bank 7 `$D6C1`): frozen-enemy gate.
///
/// `($A8,x & $10) == 0` → RTS (`A = 0`, `Z` set); else tail `JMP
/// bank7_Link_Hit_Routine` (`$E2EF`, [`Game::trap_jump`]).
pub fn sv_link_collision(game: &mut Game) {
    let x = game.cpu.x;
    let state = game.ram[zpx(0xA8, x)] & 0x10;
    lda(game, state);
    cyc(game, 4 + 2 + 2);
    if state == 0 {
        cyc(game, 1 + 6);
        return;
    }
    cyc(game, 3);
    game.trap_jump(0xE2EF);
}

/// `bank7_Enemy_Routines1_Elevator` (bank 7 `$D8C2`; slot `X`).
///
/// Inactive cabin (`$A8,x & $10 == 0`) → tail `JMP LDE40` (display).
/// Active: `$0754 = $10`, `$0479 = $057D = 0`, cabin velocity `$057E,x`
/// from the `$D8BF` table by `$0743 >> 2` ([`scroll::elev_velocity`]);
/// with a direction held, a ground/ceiling contact that disagrees with it
/// plays `$EF = $20` and moves the cabin (`bank7_Simple_Vertical_Movement`,
/// `$DEC8`), else Link's Y is skipped; Link rides at cabin `+8`. Cabin at
/// `>= $D8` recentres Link (`$4D = $70`, `$0735 = 6`, `$0734 = $0B`,
/// `$FD = $072C = 0`) and leaves through mode `$13` ([`sv_mode_exit`]),
/// otherwise tail `JMP LDE40`.
pub fn sv_elevator(game: &mut Game) {
    let x = game.cpu.x;
    let state = game.ram[zpx(0xA8, x)] & 0x10;
    lda(game, state);
    cyc(game, 4 + 2 + 2);
    if state == 0 {
        cyc(game, 2 + 3); // BEQ taken across the $D8/$D9 page edge + JMP
        game.trap_jump(0xDE40);
        return;
    }
    // STA $0754 : LDA #$00 : STA $0479 : STA $057D
    game.ram[0x0754] = state;
    lda(game, 0);
    game.ram[0x0479] = 0;
    game.ram[0x057D] = 0;
    // LDA $0743 : LSR : LSR : TAY : LDA $D8BF,y : STA $057E,x : LDA $0743 :
    // BEQ LD8F4
    let dir = game.ram[0x0743];
    lda(game, dir);
    lsr_a(game);
    lsr_a(game);
    let y = game.cpu.a;
    ldy(game, y);
    let vel = bus_read(game, 0xD8BFu16.wrapping_add(u16::from(y)));
    lda(game, vel);
    game.ram[0x057E + usize::from(x)] = vel;
    lda(game, dir);
    cyc(
        game,
        4 + 2 + 4 + 4 + 4 + 2 + 2 + 2 + 4 + cross(0xD8BF, y) + 5 + 4 + 2,
    );
    let mut track = true;
    if dir != 0 {
        // LDA $A7 : AND #$0C : EOR $0743 : BEQ LD8FB
        let contact = (game.ram[0xA7] & 0x0C) ^ dir;
        lda(game, contact);
        cyc(game, 3 + 2 + 4 + 2);
        if contact == 0 {
            cyc(game, 1);
            track = false;
        } else {
            // LDA #$20 : STA $EF : JSR bank7_Simple_Vertical_Movement
            lda(game, 0x20);
            game.ram[0xEF] = 0x20;
            cyc(game, 2 + 3 + 6);
            jsr_sub(game, 0xD8F1, 0xDEC8);
        }
    } else {
        cyc(game, 1);
    }
    let x = game.cpu.x;
    if track {
        // LD8F4: LDA $2A,x : CLC : ADC #$08 : STA $29
        let cabin = game.ram[zpx(0x2A, x)];
        lda(game, cabin);
        game.cpu.p &= !FLAG_C;
        let ly = adc_val(&mut game.cpu.p, cabin, 8);
        game.cpu.a = ly;
        game.ram[0x29] = ly;
        cyc(game, 4 + 2 + 2 + 3);
    }
    // LD8FB: LDA $2A,x : CMP #$D8 : BCC LD91B
    let cabin = game.ram[zpx(0x2A, x)];
    lda(game, cabin);
    cmp_val(&mut game.cpu.p, cabin, 0xD8);
    cyc(game, 4 + 2 + 2);
    if cabin < 0xD8 {
        cyc(game, 2 + 3); // BCC taken across the $D8/$D9 page edge + JMP
        game.trap_jump(0xDE40);
        return;
    }
    // LDA #$70 : STA $4D : LDA #$06 : STA $0735 : LDA #$0B : STA $0734 :
    // LDA #$00 : STA $FD : STA $072C : LDA #$13 : JMP LE187
    game.ram[0x4D] = 0x70;
    game.ram[0x0735] = 0x06;
    game.ram[0x0734] = 0x0B;
    game.ram[0xFD] = 0;
    game.ram[0x072C] = 0;
    lda(game, 0x13);
    cyc(game, 2 + 3 + 2 + 4 + 2 + 4 + 2 + 3 + 4 + 2 + 3);
    sv_mode_exit(game);
}

/// `bank7_Enemy_Routines1_Locked_Door` (bank 7 `$D991`; slot `X`).
///
/// Draws first (`LDE40`), then: a door already opening (`$AF,x != 0`)
/// counts up and at `$11` spawns the key-flash projectile (`$87 = $F0`,
/// `$54/$42` from the door, `$30 = $AC`, `$20 = $EF = 1`, `$DE = 0`) and
/// tail-jumps to `bank7_remove_enemy_or_item` (`$DD47`). A closed door
/// records the touch in `$05E7`; on contact (`bank7_code37`, `$E371`, when
/// not a fairy; [`sv_facing`] → `$05E7 = (Y + 1) ^ 3`) it consumes a key
/// (`$0793--`, magic key `$078C` bypasses; no key → RTS), clears the item
/// bit (`Set_Item_RAM_bit_to_0`, `$C295`, `X` = door screen), plays
/// `$EC = $80`, `$DE = $80`, stops Link (`$70 = $057D = 0`) and starts the
/// opening count (`$AF,x++`).
pub fn sv_locked_door(game: &mut Game) {
    cyc(game, 6);
    jsr_sub(game, 0xD991, 0xDE40);
    let x = game.cpu.x;
    // LDY $AF,x : BEQ bank7_link_door_collision_maybe
    let progress = game.ram[zpx(0xAF, x)];
    ldy(game, progress);
    cyc(game, 4 + 2);
    if progress != 0 {
        // INC $AF,x : CPY #$11 : BNE LD9FD
        let v = inc_val(&mut game.cpu.p, progress);
        game.ram[zpx(0xAF, x)] = v;
        cmp_val(&mut game.cpu.p, progress, 0x11);
        cyc(game, 6 + 2 + 2);
        if progress != 0x11 {
            cyc(game, 1 + 6);
            return;
        }
        // LDA #$F0 : STA $87 : LDA $4E,x : STA $54 : LDA $3C,x : STA $42 :
        // LDA #$AC : STA $30 : LDA #$01 : STA $20 : STA $EF : LSR : STA $DE :
        // JMP bank7_remove_enemy_or_item
        game.ram[0x87] = 0xF0;
        game.ram[0x54] = game.ram[zpx(0x4E, x)];
        game.ram[0x42] = game.ram[zpx(0x3C, x)];
        game.ram[0x30] = 0xAC;
        lda(game, 1);
        game.ram[0x20] = 1;
        game.ram[0xEF] = 1;
        lsr_a(game);
        game.ram[0xDE] = 0;
        cyc(game, 2 + 3 + 4 + 3 + 4 + 3 + 2 + 3 + 2 + 3 + 3 + 2 + 3 + 3);
        game.trap_jump(0xDD47);
        return;
    }
    // bank7_link_door_collision_maybe: LDA $A8,x : AND #$10 : STA $05E7 :
    // BEQ LD9FD
    let touch = game.ram[zpx(0xA8, x)] & 0x10;
    lda(game, touch);
    game.ram[0x05E7] = touch;
    cyc(game, 1 + 4 + 2 + 4 + 2);
    if touch == 0 {
        cyc(game, 1 + 6);
        return;
    }
    // LDA $13 : BNE LD9D0 : LDA #$00 : STA $05 : STA $0B : JSR bank7_code37
    let fairy = game.ram[0x13];
    lda(game, fairy);
    cyc(game, 3 + 2);
    if fairy == 0 {
        lda(game, 0);
        game.ram[0x05] = 0;
        game.ram[0x0B] = 0;
        cyc(game, 2 + 3 + 3 + 6);
        jsr_sub(game, 0xD9CD, 0xE371);
    } else {
        cyc(game, 1);
    }
    // LD9D0: JSR $DC91 : INY : TYA : EOR #$03 : STA $05E7
    cyc(game, 6);
    inner_jsr_frame(game, 0xD9D0, sv_facing);
    let x = game.cpu.x;
    let y = game.cpu.y.wrapping_add(1);
    ldy(game, y);
    let rel = y ^ 0x03;
    lda(game, rel);
    game.ram[0x05E7] = rel;
    // LDA $078C : BNE LD9E7 : LDA $0793 : BEQ LD9FD : DEC $0793
    let magic = game.ram[0x078C];
    lda(game, magic);
    cyc(game, 2 + 2 + 2 + 4 + 4 + 2);
    if magic == 0 {
        let keys = game.ram[0x0793];
        lda(game, keys);
        cyc(game, 4 + 2);
        if keys == 0 {
            cyc(game, 1 + 6);
            return;
        }
        let left = keys.wrapping_sub(1);
        set_nz(&mut game.cpu.p, left);
        game.ram[0x0793] = left;
        cyc(game, 6);
    } else {
        cyc(game, 1);
    }
    // LD9E7: LDA $3C,x : TAX : JSR Set_Item_RAM_bit_to_0__Bits_0_3 : LDX $10
    let screen = game.ram[zpx(0x3C, x)];
    lda(game, screen);
    ldx(game, screen);
    cyc(game, 4 + 2 + 6);
    jsr_sub(game, 0xD9EA, 0xC295);
    let slot = game.ram[0x10];
    ldx(game, slot);
    // LDA #$80 : STA $EC : STA $DE : ASL : STA $70 : STA $057D : INC $AF,x :
    // RTS
    lda(game, 0x80);
    game.ram[0xEC] = 0x80;
    game.ram[0xDE] = 0x80;
    game.cpu.p |= FLAG_C; // ASL shifts bit 7 out.
    lda(game, 0);
    game.ram[0x70] = 0;
    game.ram[0x057D] = 0;
    let v = inc_val(&mut game.cpu.p, game.ram[zpx(0xAF, slot)]);
    game.ram[zpx(0xAF, slot)] = v;
    cyc(game, 3 + 2 + 3 + 3 + 2 + 3 + 4 + 6 + 6);
}

/// `LDE6C` (bank 7 `$DE6C`; slot `X`): despawn one slot outside the
/// scroll window.
///
/// Elevators (`$13`) and codes below 3 are kept. Otherwise the window
/// `[$072A:$072C - $60, $072B:$072D + $60]` (16-bit, page + 1 in `$00`/
/// `$02`, low bytes in `$01`/`$03`, `$0F = 1`) is compared against the
/// slot's `$3C,x + 1 : $4E,x`; outside either edge → [`sv_kill_slot`].
/// Exit `A`/`Y`/flags from the last compare.
pub fn sv_despawn(game: &mut Game) {
    let x = game.cpu.x;
    // LDA $A1,x : CMP #$13 : BEQ LDEB7 : CMP #$03 : BCC LDEB7
    let code = game.ram[zpx(0xA1, x)];
    lda(game, code);
    cmp_val(&mut game.cpu.p, code, spawn::ENEMY_ELEVATOR);
    cyc(game, 4 + 2 + 2);
    if code == spawn::ENEMY_ELEVATOR {
        cyc(game, 1 + 6);
        return;
    }
    cmp_val(&mut game.cpu.p, code, spawn::ENEMY_MYU);
    cyc(game, 2 + 2);
    if code < spawn::ENEMY_MYU {
        cyc(game, 1 + 6);
        return;
    }
    // LDY #$01 : STY $0F : LDA $072C : SEC : SBC #$60 : STA $01 :
    // LDA $072A : SBC $0F : STA $00 : INC $00
    ldy(game, 1);
    game.ram[0x0F] = 1;
    game.cpu.p |= FLAG_C;
    let lo = sbc_val(&mut game.cpu.p, game.ram[0x072C], spawn::DESPAWN_MARGIN);
    game.ram[0x01] = lo;
    let hi = sbc_val(&mut game.cpu.p, game.ram[0x072A], 1);
    let hi = inc_val(&mut game.cpu.p, hi);
    game.ram[0x00] = hi;
    // LDA $072D : CLC : ADC #$60 : STA $03 : LDA $072B : ADC $0F : STA $02 :
    // INC $02
    game.cpu.p &= !FLAG_C;
    let rlo = adc_val(&mut game.cpu.p, game.ram[0x072D], spawn::DESPAWN_MARGIN);
    game.ram[0x03] = rlo;
    let rhi = adc_val(&mut game.cpu.p, game.ram[0x072B], 1);
    let rhi = inc_val(&mut game.cpu.p, rhi);
    game.ram[0x02] = rhi;
    // LDA $4E,x : CMP $01 : LDY $3C,x : INY : TYA : SBC $00 : BMI LDEB4
    let ex = game.ram[zpx(0x4E, x)];
    let page = game.ram[zpx(0x3C, x)].wrapping_add(1);
    cmp_val(&mut game.cpu.p, ex, lo);
    ldy(game, page);
    let d = sbc_val(&mut game.cpu.p, page, hi);
    game.cpu.a = d;
    cyc(
        game,
        2 + 3
            + 4
            + 2
            + 2
            + 3
            + 4
            + 3
            + 3
            + 5
            + 4
            + 2
            + 2
            + 3
            + 4
            + 3
            + 3
            + 5
            + 4
            + 3
            + 4
            + 2
            + 2
            + 3
            + 2,
    );
    if d & 0x80 != 0 {
        cyc(game, 1);
    } else {
        // LDA $4E,x : CMP $03 : LDY $3C,x : INY : TYA : SBC $02 : BMI LDEB7
        cmp_val(&mut game.cpu.p, ex, rlo);
        ldy(game, page);
        let d = sbc_val(&mut game.cpu.p, page, rhi);
        game.cpu.a = d;
        cyc(game, 4 + 3 + 4 + 2 + 2 + 3 + 2);
        if d & 0x80 != 0 {
            cyc(game, 1 + 6);
            return;
        }
    }
    // LDEB4: JSR LDD3D : RTS
    cyc(game, 6);
    inner_jsr_frame(game, 0xDEB4, sv_kill_slot);
    cyc(game, 6);
}

/// `bank7_code33` (bank 7 `$E030`): ground-find Link Y from `$AF` down.
///
/// Probes solidity via the staged screen column at `$6100` (synthetic) —
/// first solid row wins, else `$AF`. Records collision bit 2 (below) in
/// `$A7` like `bank7_code51`/`$E038` would.
pub fn sv_ground_find(game: &mut Game) {
    let mut y = 0xAFu8;
    for row in 0x10u8..=0xAFu8 {
        // Synthetic screen column: `$6100 + row` nonzero = solid.
        let solid = game
            .wram
            .get((0x0100 + row as usize) % 0x2000)
            .copied()
            .unwrap_or(0)
            != 0;
        if solid {
            y = row;
            break;
        }
    }
    w(&mut game.ram, 0x0029, y.wrapping_add(1));
    w(&mut game.ram, 0x00A7, 0x04);
}

// ---------------------------------------------------------------------------
// Link-vs-level tick: the `$E079` family.
//
// `bank7_Related_to_Link_falling_in_Lava_Water` (`$E079`) is the per-frame
// sideview routine that rebuilds Link's collision bits (`$A7`), classifies
// the tile under his feet (lava / water / breakable / jump-through /
// chimney), probes the side walls, and owns every room exit: side exits
// (`LE16F`), hole falls (`LE19E`) and the exit bookkeeping (`LE18A`, mode
// write + monster kill loop). Its helpers `LE1BE` (probe loop, also called
// by the enemy level-collision at `$EA4D-$EA8F`) and `LE16F` are trap
// entries of their own, so the whole family is ported here register-exact
// (A/X/Y/P, dead stack bytes, per-path cycles) and the entry shims dispatch
// into shared bodies. The generic tile test (`$EAE8`) and the sideview-bank
// false-wall test (`$850C`) are ported alongside because both are pure
// functions of RAM + ROM tables read through the bus.
// ---------------------------------------------------------------------------

/// `bank7_Generic_Collision_Test_with_Level_Objects` (bank 7 `$EAE8`),
/// entered with `Y` = probe index and `X` = object slot (0 = Link).
///
/// Positions the probe at (`$4D,x + tab28[Y]`, `$29,x + tabC0[Y]`), builds
/// the level-RAM row pointer in `$0E/$0F` from the page tables at
/// `$EAE0/$EAE4`, stores the row in `$02` and the tile read at `($0E),y`
/// in `$03` (`$40` = solid sentinel when the page is 4 or more, or the
/// row is `$D0` or more; that path also runs the `BIT $0EB1` flag side
/// effect). Exit: `A` = tile, `Y` = row (entry index on the page-overflow
/// path), `X` untouched. Cycles: 94 on the row paths (+1 page-cross on
/// the level read), 42 on the page-overflow path; RTS included.
pub fn sv_collision_test(game: &mut Game) {
    let x = game.cpu.x;
    let y0 = game.cpu.y;
    // STY $0C : LDA $4D,x : CLC : ADC bank7_table28,y : STA $0E
    game.ram[0x0C] = y0;
    let xl = game.ram[zpx(0x4D, x)];
    game.cpu.p &= !FLAG_C;
    let t28 = bus_read(game, 0xEAA0u16.wrapping_add(u16::from(y0)));
    let lo = adc_val(&mut game.cpu.p, xl, t28);
    game.ram[0x0E] = lo;
    // LDA $3B,x : ADC #$00 : CMP #$04 : BCS LEB1D
    let xh = game.ram[zpx(0x3B, x)];
    let page = adc_val(&mut game.cpu.p, xh, 0);
    cmp_val(&mut game.cpu.p, page, 4);
    if page >= 4 {
        // LEB1D: LDA #$40 : BIT $0EB1 (mirror of $06B1) : STA $03 : RTS
        cyc(game, 43); // BCS taken across the $EA/$EB page edge
        lda(game, 0x40);
        let m = game.ram[0x06B1];
        bit_mem(&mut game.cpu.p, 0x40, m);
        game.ram[0x03] = 0x40;
        return;
    }
    // TAY : LDA $0E : LSR x4 : CLC : ADC $EAE0,y : STA $0E : LDA $EAE4,y :
    // STA $0F
    ldy(game, page);
    game.cpu.p &= !FLAG_C;
    let e0 = bus_read(game, 0xEAE0 + u16::from(page));
    let ptr_lo = adc_val(&mut game.cpu.p, lo >> 4, e0);
    game.ram[0x0E] = ptr_lo;
    let ptr_hi = bus_read(game, 0xEAE4 + u16::from(page));
    lda(game, ptr_hi);
    game.ram[0x0F] = ptr_hi;
    // LDY $0C : LDA $29,x : CLC : ADC LEAC0,y : AND #$F0 : STA $02 : TAY
    ldy(game, y0);
    let ly = game.ram[zpx(0x29, x)];
    game.cpu.p &= !FLAG_C;
    let c0 = bus_read(game, 0xEAC0u16.wrapping_add(u16::from(y0)));
    let row = adc_val(&mut game.cpu.p, ly, c0) & 0xF0;
    lda(game, row);
    game.ram[0x02] = row;
    ldy(game, row);
    // CPY #$D0 : BCC Label_EB20
    cmp_val(&mut game.cpu.p, row, 0xD0);
    if row >= 0xD0 {
        cyc(game, 94);
        lda(game, 0x40);
        let m = game.ram[0x06B1];
        bit_mem(&mut game.cpu.p, 0x40, m);
        game.ram[0x03] = 0x40;
        return;
    }
    // Label_EB20: LDA ($0E),y : STA $03 : RTS
    let base = u16::from(ptr_lo) | (u16::from(ptr_hi) << 8);
    let extra = cross(base, row);
    let tile = bus_read(game, base.wrapping_add(u16::from(row)));
    lda(game, tile);
    game.ram[0x03] = tile;
    cyc(game, 94 + extra);
}

/// Sideview-bank `$850C` false-wall test (same code in banks 1-5):
/// `C` = tile >= table[(tile >> 6) - 1] where the table pointer is the
/// `CMP abs,Y` operand at `$8517` (read through the bus, so no per-bank
/// constant). If the mapped bank does not carry that opcode at `$8516`
/// (overworld bank 0 holds unrelated code there) the routine is handed to
/// the interpreter inside the `JSR` frame instead ([`Game::trap_jump`]).
/// Exit: `A` = tile, `Y` = `(tile >> 6) - 1`, `X`
/// untouched, one dead stack byte (`PHA`). Cycles: 31 (+1 page-cross on
/// the table read); RTS included.
fn sv_false_wall(game: &mut Game) {
    if bus_read(game, 0x8516) != 0xD9 {
        game.trap_jump(0x850C);
        return;
    }
    let a = game.cpu.a;
    // PHA : AND #$C0 : CLC : ROL : ROL : ROL : TAY : DEY : PLA
    game.ram[0x0100 + usize::from(game.cpu.sp)] = a;
    let idx = ((a >> 6) & 3).wrapping_sub(1);
    game.cpu.y = idx;
    // CMP table,y : RTS
    let tbl = bus_read16(game, 0x8517);
    let extra = cross(tbl, idx);
    let m = bus_read(game, tbl.wrapping_add(u16::from(idx)));
    cmp_val(&mut game.cpu.p, a, m);
    game.cpu.a = a;
    cyc(game, 31 + extra);
}

/// `LE1BE` (bank 7 `$E1BE`): countdown probe loop.
///
/// Probes row index `$00` (generic test + false-wall carry); on a hit ORs
/// `bank7_table21[$00]` (`$E04E`) into `$A7,x` and returns, else
/// decrements `$00`/`$01` and retries while `$01` stays non-negative
/// (`BPL`), so a caller staging `$01 = n` gets `n + 1` rows. Exit (hit):
/// `A` = new `$A7,x`, `Y` = `$00`, `C` set; (miss): `A` = last tile, `Y`
/// from the false-wall test, `N` set, `C` clear. `X` untouched.
pub fn sv_probe(game: &mut Game) {
    loop {
        // LDY $00 : JSR $EAE8 : JSR L850C : BCC LE1D2
        let y = game.ram[0x00];
        ldy(game, y);
        cyc(game, 3 + 6);
        inner_jsr_frame(game, 0xE1C0, sv_collision_test);
        cyc(game, 6);
        inner_jsr_frame(game, 0xE1C3, sv_false_wall);
        if game.cpu.p & FLAG_C != 0 {
            // LDY $00 : LDA $A7,x : ORA bank7_table21,y : STA $A7,x : RTS
            let y = game.ram[0x00];
            ldy(game, y);
            let slot = zpx(0xA7, game.cpu.x);
            let extra = cross(0xE04E, y);
            let t = bus_read(game, 0xE04Eu16.wrapping_add(u16::from(y)));
            let v = game.ram[slot] | t;
            lda(game, v);
            game.ram[slot] = v;
            cyc(game, 2 + 3 + 4 + 4 + 4 + 6 + extra);
            return;
        }
        // LE1D2: DEC $00 : DEC $01 : BPL LE1BE : RTS
        cyc(game, 3 + 5 + 5);
        dec_zp(game, 0x00);
        let left = dec_zp(game, 0x01);
        if left & 0x80 == 0 {
            cyc(game, 3);
            continue;
        }
        cyc(game, 2 + 6);
        return;
    }
}

/// `LE1B8` (bank 7 `$E1B8`): two-row span probe — `$00 = Y`, `$01 = 1`,
/// then the [`sv_probe`] loop over rows `Y` and `Y - 1`.
fn sv_probe_span(game: &mut Game) {
    game.ram[0x00] = game.cpu.y;
    lda(game, 1);
    game.ram[0x01] = 1;
    cyc(game, 3 + 2 + 3);
    sv_probe(game);
}

/// `LDD3D` (bank 7 `$DD3D`): retire slot `X` — clear bit 7 of its
/// spawn-list byte (`($D6),y`, `Y = $BC,x`, skipped when `Y` is negative)
/// then free the slot (`$B6,x = 0`, the `bank7_remove_enemy_or_item`
/// tail). Exit `A = 0`, `Y = $BC,x`.
fn sv_kill_slot(game: &mut Game) {
    let x = game.cpu.x;
    let y = game.ram[zpx(0xBC, x)];
    ldy(game, y);
    cyc(game, 4);
    if y & 0x80 == 0 {
        // LDA ($D6),y : AND #$7F : STA ($D6),y
        let base = u16::from(game.ram[0xD6]) | (u16::from(game.ram[0xD7]) << 8);
        let extra = cross(base, y);
        let ea = base.wrapping_add(u16::from(y));
        let v = bus_read(game, ea) & 0x7F;
        bus_write(game, ea, v);
        cyc(game, 2 + 5 + 2 + 6 + extra);
    } else {
        cyc(game, 3);
    }
    // LDA #$00 : STA $B6,x : RTS
    lda(game, 0);
    game.ram[zpx(0xB6, x)] = 0;
    cyc(game, 2 + 4 + 6);
}

/// `bank7_find_next_free_41A_X_or_something…` (bank 7 `$E292`): pick the
/// highest free debris slot (`$041A,x == 0`, `X` from 4 down, else 0) and
/// stamp the row (`$042E,x = $02`) and level-RAM pointer
/// (`$0433,x/$0438,x = $0E/$0F`). Exit `X` = slot, `Y` = `$02`, `A` = `$0F`.
fn sv_find_free_debris(game: &mut Game) {
    cyc(game, 2);
    let mut x = 4u8;
    loop {
        cyc(game, 4);
        if game.ram[0x041A + usize::from(x)] == 0 {
            cyc(game, 3);
            break;
        }
        cyc(game, 2 + 2);
        if x == 0 {
            // DEX wraps to $FF: BPL falls through to LDX #$00.
            cyc(game, 2 + 2);
            break;
        }
        x -= 1;
        cyc(game, 3);
    }
    game.cpu.x = x;
    // LDY $02 : TYA : STA $042E,x : LDA $0E : STA $0433,x : LDA $0F :
    // STA $0438,x : RTS
    let row = game.ram[0x02];
    ldy(game, row);
    lda(game, row);
    game.ram[0x042E + usize::from(x)] = row;
    let lo = game.ram[0x0E];
    game.ram[0x0433 + usize::from(x)] = lo;
    let hi = game.ram[0x0F];
    lda(game, hi);
    game.ram[0x0438 + usize::from(x)] = hi;
    cyc(game, 3 + 2 + 5 + 3 + 5 + 3 + 5 + 6);
}

/// `LE18A` (bank 7 `$E18A`): exit bookkeeping — `$0726++`, then retire
/// every live slot (`$B6,x != 0`: [`sv_kill_slot`] then `$B6,x++`, i.e.
/// the slot is left at 1), `X = $10`, RTS.
fn sv_exit_kill(game: &mut Game) {
    inc_mem(game, 0x0726);
    cyc(game, 6 + 2);
    let mut x = 5u8;
    loop {
        game.cpu.x = x;
        let live = game.ram[zpx(0xB6, x)];
        lda(game, live);
        cyc(game, 4 + 2);
        if live != 0 {
            cyc(game, 6);
            inner_jsr_frame(game, 0xE193, sv_kill_slot);
            let v = inc_val(&mut game.cpu.p, game.ram[zpx(0xB6, x)]);
            game.ram[zpx(0xB6, x)] = v;
            cyc(game, 6);
        } else {
            cyc(game, 1);
        }
        cyc(game, 2 + 2);
        if x == 0 {
            break;
        }
        x -= 1;
        cyc(game, 1);
    }
    let slot = game.ram[0x10];
    ldx(game, slot);
    cyc(game, 3 + 6);
}

/// `LE187` (bank 7 `$E187`): `$0736 = A`, then [`sv_exit_kill`].
fn sv_mode_exit(game: &mut Game) {
    let mode = game.cpu.a;
    game.ram[0x0736] = mode;
    cyc(game, 4);
    sv_exit_kill(game);
}

/// `LE19E` (bank 7 `$E19E`): hole-fall check. Falls (fairy: `$29 >= $E4`;
/// else `$19 >= 2`) → `$13 = 0`, `$0736++`, [`sv_exit_kill`]; otherwise
/// RTS with `A`/flags from the last compare.
fn sv_fall_check(game: &mut Game) {
    let fairy = game.ram[0x13];
    lda(game, fairy);
    cyc(game, 3 + 2);
    let mut fall = false;
    if fairy != 0 {
        // LDA $29 : CMP #$E4 : BCS LE1AE
        let ly = game.ram[0x29];
        lda(game, ly);
        cmp_val(&mut game.cpu.p, ly, 0xE4);
        cyc(game, 3 + 2 + 2);
        if ly >= 0xE4 {
            cyc(game, 1);
            fall = true;
        }
    } else {
        cyc(game, 1);
    }
    if !fall {
        // LE1A8: LDA $19 : CMP #$02 : BCC LE19D
        let v = game.ram[0x19];
        lda(game, v);
        cmp_val(&mut game.cpu.p, v, 2);
        cyc(game, 3 + 2 + 2);
        if v < 2 {
            cyc(game, 1 + 6);
            return;
        }
    }
    // LE1AE: LDA #$00 : STA $13 : INC $0736 : JMP LE18A
    lda(game, 0);
    game.ram[0x13] = 0;
    inc_mem(game, 0x0736);
    cyc(game, 2 + 3 + 6 + 3);
    sv_exit_kill(game);
}

/// `LE16F` (bank 7 `$E16F`): side-exit gate on the entry `A` (`$C8`).
///
/// `A & 6 == 0` → [`sv_fall_check`]; else `A & 4` steps `$3B` up (the
/// caller already decremented for a left exit), clears `$13`/`$0759`/
/// `$075A`/`$70`, and takes exit mode `$10` through [`sv_mode_exit`].
pub fn sv_scroll_gate(game: &mut Game) {
    let a = game.cpu.a & 0x06;
    lda(game, a);
    cyc(game, 2 + 2);
    if a == 0 {
        cyc(game, 3);
        sv_fall_check(game);
        return;
    }
    let a = a & 0x04;
    lda(game, a);
    cyc(game, 2 + 2);
    if a != 0 {
        cyc(game, 5);
        let v = inc_val(&mut game.cpu.p, game.ram[0x3B]);
        game.ram[0x3B] = v;
    } else {
        cyc(game, 1);
    }
    // LE179: LDA #$00 : STA $13 : STA $0759 : STA $075A : STA $70 : LDA #$10
    game.ram[0x13] = 0;
    game.ram[0x0759] = 0;
    game.ram[0x075A] = 0;
    game.ram[0x70] = 0;
    lda(game, 0x10);
    cyc(game, 2 + 3 + 4 + 4 + 3 + 2);
    sv_mode_exit(game);
}

/// `LE12B` (bank 7 `$E12B`): side-wall probes then the exit dispatch.
///
/// Probes the facing-side row pair (`$5F / 2 + $13`, then `+ 2 + $13`,
/// single row each) into `$A7`, then: `$B5 != 1` → RTS; `$0503 != 0` →
/// [`sv_fall_check`]; `$0728 == 0` → [`sv_scroll_gate`] on `$C8`; else a
/// frozen room only folds the pushed-wall bit (`($C8 & 9) >> 3 + 1 ==
/// $5F`) into `$A7`.
fn sv_side_probes(game: &mut Game) {
    let fairy = game.ram[0x13];
    // LDA $5F : LSR : CLC : ADC $13 : STA $00 : PHA : LDA #$00 : STA $01 :
    // JSR LE1BE
    let facing = game.ram[0x5F];
    lda(game, facing);
    lsr_a(game);
    game.cpu.p &= !FLAG_C;
    let a = adc_val(&mut game.cpu.p, game.cpu.a, fairy);
    game.cpu.a = a;
    game.ram[0x00] = a;
    pha(game);
    lda(game, 0);
    game.ram[0x01] = 0;
    cyc(game, 3 + 2 + 2 + 3 + 3 + 3 + 2 + 3 + 6);
    inner_jsr_frame(game, 0xE138, sv_probe);
    // PLA : CLC : ADC #$02 : ADC $13 : STA $00 : LDA #$00 : STA $01 :
    // JSR LE1BE
    pla(game);
    game.cpu.p &= !FLAG_C;
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 2);
    let a = adc_val(&mut game.cpu.p, a, fairy);
    game.cpu.a = a;
    game.ram[0x00] = a;
    lda(game, 0);
    game.ram[0x01] = 0;
    cyc(game, 4 + 2 + 2 + 3 + 3 + 2 + 3 + 6);
    inner_jsr_frame(game, 0xE147, sv_probe);
    // LDA $B5 : CMP #$01 : BNE LE19D
    let b5 = game.ram[0xB5];
    lda(game, b5);
    cmp_val(&mut game.cpu.p, b5, 1);
    cyc(game, 3 + 2);
    if b5 != 1 {
        cyc(game, 3 + 6);
        return;
    }
    // LDA $0503 : BNE LE19E
    let t = game.ram[0x0503];
    lda(game, t);
    cyc(game, 2 + 4);
    if t != 0 {
        cyc(game, 3);
        sv_fall_check(game);
        return;
    }
    // LDA $C8 : LDY $0728 : BEQ LE16F
    let c8 = game.ram[0xC8];
    lda(game, c8);
    let frozen = game.ram[0x0728];
    ldy(game, frozen);
    cyc(game, 2 + 3 + 4);
    if frozen == 0 {
        cyc(game, 3);
        sv_scroll_gate(game);
        return;
    }
    // (BEQ LE16F not taken) AND #$09 : BEQ LE16E
    let a = c8 & 0x09;
    lda(game, a);
    cyc(game, 2 + 2 + 2);
    if a == 0 {
        cyc(game, 1 + 6);
        return;
    }
    // LSR x3 : TAY : INY : CPY $5F : BNE LE16E
    lsr_a(game);
    lsr_a(game);
    lsr_a(game);
    let y = game.cpu.a.wrapping_add(1);
    ldy(game, y);
    cmp_val(&mut game.cpu.p, y, facing);
    cyc(game, 6 + 2 + 2 + 3 + 2);
    if y != facing {
        cyc(game, 1 + 6);
        return;
    }
    // TYA : ORA $A7 : STA $A7 : RTS
    let v = y | game.ram[0xA7];
    lda(game, v);
    game.ram[0xA7] = v;
    cyc(game, 2 + 3 + 3 + 6);
}

/// `LE0FC` (bank 7 `$E0FC`): step-on breakable — stamp the shattered
/// tile (`$8F`) at `($0E) + $02`, claim a debris slot
/// ([`sv_find_free_debris`]) with `$041A,x = $81`, sound `$ED = 2`, and
/// Link's tile-aligned position (`$0429,x`/`$0424,x`/`$041F,x`), then
/// `X = 0` and [`sv_side_probes`].
fn sv_shatter_step(game: &mut Game) {
    // LDA #$8F : LDY $02 : STA ($0E),y : JSR $E292
    let row = game.ram[0x02];
    let base = u16::from(game.ram[0x0E]) | (u16::from(game.ram[0x0F]) << 8);
    lda(game, 0x8F);
    ldy(game, row);
    bus_write(game, base.wrapping_add(u16::from(row)), 0x8F);
    cyc(game, 2 + 3 + 6 + 6);
    inner_jsr_frame(game, 0xE102, sv_find_free_debris);
    let x = usize::from(game.cpu.x);
    // LDA #$81 : STA $041A,x : LDA #$02 : STA $ED
    game.ram[0x041A + x] = 0x81;
    game.ram[0xED] = 0x02;
    // LDA $29 : CLC : ADC #$20 : AND #$F0 : STA $0429,x
    game.cpu.p &= !FLAG_C;
    let ly = adc_val(&mut game.cpu.p, game.ram[0x29], 0x20) & 0xF0;
    game.ram[0x0429 + x] = ly;
    // LDA $4D : CLC : ADC #$0F : AND #$F0 : STA $0424,x
    game.cpu.p &= !FLAG_C;
    let lx = adc_val(&mut game.cpu.p, game.ram[0x4D], 0x0F) & 0xF0;
    game.ram[0x0424 + x] = lx;
    // LDA $3B : ADC #$00 (carry from the X add) : STA $041F,x : LDX #$00
    let page = adc_val(&mut game.cpu.p, game.ram[0x3B], 0);
    game.cpu.a = page;
    game.ram[0x041F + x] = page;
    ldx(game, 0);
    cyc(
        game,
        2 + 5 + 2 + 3 + 3 + 2 + 2 + 2 + 5 + 3 + 2 + 2 + 2 + 5 + 3 + 2 + 5 + 2,
    );
    sv_side_probes(game);
}

/// `LE0E6` (bank 7 `$E0E6`): jump-through / door tile under Link. Down
/// held (`$F7 & 8`) while grounded (`$0479 == 0`) → `$075B++` and exit
/// mode `$16` via [`sv_mode_exit`]; down held in mid-air → RTS; else
/// [`sv_side_probes`].
fn sv_down_tile(game: &mut Game) {
    let held = game.ram[0xF7] & 0x08;
    lda(game, held);
    cyc(game, 3 + 2);
    if held == 0 {
        cyc(game, 3 + 3);
        sv_side_probes(game);
        return;
    }
    let air = game.ram[0x0479];
    lda(game, air);
    cyc(game, 2 + 4);
    if air != 0 {
        cyc(game, 3 + 6);
        return;
    }
    inc_mem(game, 0x075B);
    lda(game, 0x16);
    cyc(game, 2 + 6 + 2 + 3);
    sv_mode_exit(game);
}

/// `bank7_Related_to_Link_falling_in_Lava_Water` (bank 7 `$E079`): the
/// per-frame Link-vs-level tick (trap entry; `X` forced to 0).
///
/// Clears `$A7`, probes the head span (`Y = 7 + $13`) and the foot span
/// (`Y = 5 + $13`, two rows each via [`sv_probe_span`], `$0D` = head
/// tile), reads the foot-centre tile (`Y = $1D`, [`sv_collision_test`]),
/// clears `$0752`, then classifies against the sideview bank's tile codes
/// (`$851A-$8522`, read through the bus): lava → `$E9 = 1`, `$050C = $10`,
/// `$B5++`, RTS; water with `$29 >= $A5` → `$0752 = $20`; step-on
/// breakable → [`sv_shatter_step`]; the three jump-through/door codes →
/// [`sv_down_tile`]; the chimney code with the shield down (`$17 == 0`) →
/// `$070E++`, RTS; everything else → [`sv_side_probes`]. Exit registers
/// and flags follow the path taken (see the helpers); cycles counted per
/// path, RTS included.
pub fn sv_lava_check(game: &mut Game) {
    // LDX #$00 : STX $A7
    ldx(game, 0);
    game.ram[0xA7] = 0;
    // LDA #$07 : CLC : ADC $13 : TAY : JSR LE1B8
    let fairy = game.ram[0x13];
    game.cpu.p &= !FLAG_C;
    let y = adc_val(&mut game.cpu.p, 7, fairy);
    game.cpu.a = y;
    ldy(game, y);
    cyc(game, 2 + 3 + 2 + 2 + 3 + 2 + 6);
    inner_jsr_frame(game, 0xE083, sv_probe_span);
    // LDA $03 : STA $0D
    let head = game.ram[0x03];
    lda(game, head);
    game.ram[0x0D] = head;
    // LDA #$05 : CLC : ADC $13 : TAY : JSR LE1B8
    game.cpu.p &= !FLAG_C;
    let y = adc_val(&mut game.cpu.p, 5, fairy);
    game.cpu.a = y;
    ldy(game, y);
    cyc(game, 3 + 3 + 2 + 2 + 3 + 2 + 6);
    inner_jsr_frame(game, 0xE090, sv_probe_span);
    // LDY #$1D : JSR $EAE8
    ldy(game, 0x1D);
    cyc(game, 2 + 6);
    inner_jsr_frame(game, 0xE095, sv_collision_test);
    // LDA #$00 : STA $0752 : LDA $03
    game.ram[0x0752] = 0;
    let tile = game.ram[0x03];
    lda(game, tile);
    cyc(game, 2 + 4 + 3);
    // CMP L8520 : BNE LE0B0
    let lava = bus_read(game, 0x8520);
    cmp_val(&mut game.cpu.p, tile, lava);
    cyc(game, 4);
    if tile == lava {
        // bank7_Link_touched_Lava_Water: LDA #$01 : STA $E9 : LDA #$10 :
        // STA $050C : INC $B5 : RTS
        game.ram[0xE9] = 1;
        lda(game, 0x10);
        game.ram[0x050C] = 0x10;
        let b5 = inc_val(&mut game.cpu.p, game.ram[0xB5]);
        game.ram[0xB5] = b5;
        cyc(game, 2 + 2 + 3 + 2 + 4 + 5 + 6);
        return;
    }
    cyc(game, 3);
    // LE0B0: CMP L8521 : BEQ LE0BA : CMP L8522 : BNE LE0C5
    let water_a = bus_read(game, 0x8521);
    cmp_val(&mut game.cpu.p, tile, water_a);
    cyc(game, 4);
    let water = if tile == water_a {
        cyc(game, 3);
        true
    } else {
        cyc(game, 2);
        let water_b = bus_read(game, 0x8522);
        cmp_val(&mut game.cpu.p, tile, water_b);
        cyc(game, 4);
        if tile == water_b {
            cyc(game, 2);
            true
        } else {
            cyc(game, 3);
            false
        }
    };
    if water {
        // LE0BA: LDY $29 : CPY #$A5 : BCC LE0C5 : LDY #$20 : STY $0752
        let ly = game.ram[0x29];
        ldy(game, ly);
        cmp_val(&mut game.cpu.p, ly, 0xA5);
        cyc(game, 3 + 2);
        if ly >= 0xA5 {
            ldy(game, 0x20);
            game.ram[0x0752] = 0x20;
            cyc(game, 2 + 2 + 4);
        } else {
            cyc(game, 3);
        }
    }
    // LE0C5: CMP L851F : BEQ LE0FC
    let breakable = bus_read(game, 0x851F);
    cmp_val(&mut game.cpu.p, tile, breakable);
    cyc(game, 4);
    if tile == breakable {
        cyc(game, 3);
        sv_shatter_step(game);
        return;
    }
    cyc(game, 2);
    // CMP L851A : BEQ LE0E6 : CMP L851C : BEQ LE0E6 : CMP L851B : BEQ LE0E6
    for code in [0x851Au16, 0x851C, 0x851B] {
        let m = bus_read(game, code);
        cmp_val(&mut game.cpu.p, tile, m);
        cyc(game, 4);
        if tile == m {
            cyc(game, 3);
            sv_down_tile(game);
            return;
        }
        cyc(game, 2);
    }
    // CMP L851D : BNE LE0F9 (JMP LE12B)
    let chimney = bus_read(game, 0x851D);
    cmp_val(&mut game.cpu.p, tile, chimney);
    cyc(game, 4);
    if tile != chimney {
        cyc(game, 3 + 3);
        sv_side_probes(game);
        return;
    }
    cyc(game, 2);
    // LDA $17 : BNE LE0F9 (JMP LE12B) : INC $070E : RTS
    let shield = game.ram[0x17];
    lda(game, shield);
    cyc(game, 3);
    if shield != 0 {
        cyc(game, 3 + 3);
        sv_side_probes(game);
        return;
    }
    inc_mem(game, 0x070E);
    cyc(game, 2 + 6 + 6);
}

/// `bank7_code14` (bank 7 `$C82B`): ceiling/floor tile select.
///
/// Replicates the no-ceiling branch: `$0486 != 0` starts at row 2 with
/// `$0A |= $20`, `$010E = $38`, `$010F = 3`; else `$010E = $E0`,
/// `$010F = 5`. Records both bytes for the draw loop.
pub fn sv_ceiling(game: &mut Game) {
    if r(&game.ram, 0x0486) != 0 {
        let oa = r(&game.ram, 0x000A) | 0x20;
        w(&mut game.ram, 0x000A, oa);
        w(&mut game.ram, 0x010E, 0x38);
        w(&mut game.ram, 0x010F, 0x03);
    } else {
        w(&mut game.ram, 0x010E, 0xE0);
        w(&mut game.ram, 0x010F, 0x05);
    }
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every fixed-bank sideview trap on `game`.
///
/// Banked (`None`) entries are skipped (aliasing caveat). Idempotent.
pub fn register_sideview_traps(game: &mut Game) {
    // Keep the ROM's area setup/draw path live. These routines consume the
    // banked map data through the CPU mapper; replacing them with the
    // synthetic-header shims or the bookkeeping-only floor shim leaves the
    // nametables filled with blank `$F4` tiles or omits the floor rows on
    // real cartridges.
    game.trap_register("LC9A5", Some(7), 0xC9A5, sv_item_spawn);
    game.trap_register("bank7_code17", Some(7), 0xCB35, sv_key_entry);
    game.trap_register(
        "bank7_Get_Area_Code__Enter_Code_and_Direction",
        Some(7),
        0xCC97,
        sv_get_entry,
    );
    game.trap_register("bank7_go_outside", Some(7), 0xCCB3, sv_go_outside);
    game.trap_register("bank7_take_elevator_exit", Some(7), 0xC644, sv_elev_exit);
    game.trap_register("bank7_take_side_exit", Some(7), 0xCF4C, sv_side_exit);
    game.trap_register("bank7_take_door_exit", Some(7), 0xCFFC, sv_door_exit);
    game.trap_register("bank7_code22", Some(7), 0xD603, sv_spawn_gate);
    game.trap_register("LD625", Some(7), 0xD625, sv_enemy_spawn);
    game.trap_register(
        "bank7_Link_Collision_Detection",
        Some(7),
        0xD6C1,
        sv_link_collision,
    );
    game.trap_register(
        "bank7_Enemy_Routines1_Elevator",
        Some(7),
        0xD8C2,
        sv_elevator,
    );
    game.trap_register(
        "bank7_Enemy_Routines1_Locked_Door",
        Some(7),
        0xD991,
        sv_locked_door,
    );
    game.trap_register("LDE6C", Some(7), 0xDE6C, sv_despawn);
    // The ROM routine performs the real tile collision probe after seeding
    // Link at $AF.  The old shim scanned a synthetic $6100 proxy and could
    // leave Link at the falling-entry Y position, so let the cartridge code
    // own this transition as well.
    game.trap_register("LE16F", Some(7), 0xE16F, sv_scroll_gate);
    game.trap_register("LE1BE", Some(7), 0xE1BE, sv_probe);
    game.trap_register(
        "bank7_Related_to_Link_falling_in_Lava_Water",
        Some(7),
        0xE079,
        sv_lava_check,
    );
    // Keep the ROM's ceiling/floor writer live; the shim only initialized
    // scratch registers and skipped the actual level-RAM tile fill.
}

// ---------------------------------------------------------------------------
// Overworld trap shims.
//
// Every registered entry is an instruction-level replica (A/X/Y/P, memory,
// inner-JSR stack framing, per-path cycles): `$DF3F`, `$DF01`, `$E001`,
// `$DFEF` from the first lockstep pass; `$DF79` and the four
// `SwapToSavedPRG` / banked call / `SwapToPRG0` wrappers (`$DFD2`, `$DFF8`,
// `$E01B`, `$E024`) from the dispatcher-untrapped pass (`--untrap
// D382,D385`, the only configuration in which mode-table targets such as
// `$CCB3` fire — see the module docs). `$CCB3` and `$E16F` share the
// sideview group's exact ports.
//
// `bank7_code18` (`$CD40`, game mode 0) is listed in `OVERWORLD_TRAPS` but
// deliberately *not* registered. It is the world loader: bank select +
// `SwapPRG`, then ~2.6 KiB of banked copy loops (enemy data `$7000-$73FF`,
// overworld RLE `$7C00-$7F7F`, area tables `$6A00-$6C57`, palace palettes
// `$7919-$79F8`, the `$2A1`-byte `$9400/$A900 -> $6D00` transfer, town
// velocity rows) totalling on the order of 85k cycles — roughly three
// NMI periods. A trap body runs atomically (`Game::fire_trap`), so a port
// would collapse the NMIs the ROM services mid-loader into one; and a
// table cost large enough for `cpu::split_trap_target` to interpret it
// across the frame boundary would mean the port never runs. Its old shim
// was a synthetic placeholder (region bank into `$02`) that stalled the
// game in mode 0 whenever it fired (any% lockstep: `$0769`/`$7000` at
// frame 23, sprites from 32, palette from 151). The ROM routine runs
// instead; `region_prg_bank` in `overworld.rs` documents its bank select.
// ---------------------------------------------------------------------------

use crate::bank7_common::rol_mem;

/// Set/clear `C`.
fn set_c(p: &mut u8, c: bool) {
    if c {
        *p |= FLAG_C;
    } else {
        *p &= !FLAG_C;
    }
}

/// `ASL A`: `C` = old bit 7, `N`/`Z` from the result.
fn asl_acc(p: &mut u8, a: u8) -> u8 {
    set_c(p, a & 0x80 != 0);
    let r = a << 1;
    set_nz(p, r);
    r
}

/// `LSR A`: `C` = old bit 0, `N` = 0, `Z` from the result.
fn lsr_acc(p: &mut u8, a: u8) -> u8 {
    set_c(p, a & 1 != 0);
    let r = a >> 1;
    set_nz(p, r);
    r
}

/// `PHA` (mirrors `cpu.rs::push`).
fn push_byte(game: &mut Game, v: u8) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = v;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// `PLA` without the flag update (mirrors `cpu.rs::pop`; the caller sets
/// `N`/`Z`).
fn pop_byte(game: &mut Game) -> u8 {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    game.ram[0x0100 + usize::from(game.cpu.sp)]
}

/// `LDA ($0E),y` through the bus (WRAM or the currently mapped PRG bank):
/// loads `A`, sets `N`/`Z`, and reports whether the index crossed a page
/// (+1 cycle on hardware).
fn lda_ind_y_0e(game: &mut Game) -> (u8, bool) {
    let lo = r(&game.ram, 0x000E);
    let hi = r(&game.ram, 0x000F);
    let y = game.cpu.y;
    let cross = u16::from(lo) + u16::from(y) > 0xFF;
    let v = bus_read(
        game,
        u16::from_le_bytes([lo, hi]).wrapping_add(u16::from(y)),
    );
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
    (v, cross)
}

/// `overworld1` (bank 0 `$8284`): demon spawn gate.
///
/// Calls [`enc::spawn_attempt`] + [`enc::spawn_group_for_terrain`]; records
/// the gate in scratch `$02` and the group in `$03` (`$FF` = none).
fn ow_overworld1(game: &mut Game) {
    let region = r(&game.ram, 0x0706);
    let tile_y = r(&game.ram, 0x0073);
    let tally = r(&game.ram, 0x0026);
    let wave = r(&game.ram, 0x0516);
    let go = enc::spawn_attempt(region, tile_y, tally, wave);
    w(&mut game.ram, 0x0002, u8::from(go));
    let terrain = r(&game.ram, 0x0563);
    match enc::spawn_group_for_terrain(terrain) {
        Some(g) => w(&mut game.ram, 0x0003, g as u8),
        None => w(&mut game.ram, 0x0003, 0xFF),
    }
}

/// `overworld2` (bank 0 `$84BF`): hammer/flute probe.
///
/// Calls [`omap::faced_tile`] + [`otrans::hammer_transform`]; writes the
/// faced coords to `$00`/`$01` and the transform to `$02`/`$03`.
fn ow_overworld2(game: &mut Game) {
    let facing = omap::facing_from_byte(r(&game.ram, 0x0562)).unwrap_or(omap::Facing::Right);
    let (fy, fx) = omap::faced_tile(r(&game.ram, 0x0073), r(&game.ram, 0x0074), facing);
    w(&mut game.ram, 0x0000, fy);
    w(&mut game.ram, 0x0001, fx);
    // Terrain at the faced tile needs the RLE walk (interp-only); use the
    // current terrain byte as the representative input (gap documented).
    let (next, revealed) = otrans::hammer_transform(r(&game.ram, 0x0563));
    w(&mut game.ram, 0x0002, next);
    w(&mut game.ram, 0x0003, u8::from(revealed));
}

/// `overworld3` (bank 0 `$8558`): frame pipeline dispatcher (gap: full
/// sequencing lives with the interpreter; this records the scroll commit
/// via [`omap::scroll_tick`]).
fn ow_overworld3(game: &mut Game) {
    let out = omap::scroll_tick(
        r(&game.ram, 0x007D),
        r(&game.ram, 0x0026),
        r(&game.ram, 0x0563),
        r(&game.ram, 0x0012),
    );
    if let Some((pl, st)) = out {
        w(&mut game.ram, 0x007D, pl);
        w(&mut game.ram, 0x0026, st);
    }
}

/// `overworld4` (bank 0 `$87F3`): row-pointer build via
/// [`omap::build_row_offsets`] over the WRAM RLE window.
fn ow_overworld4(game: &mut Game) {
    let blob = game.wram[0x1C00..0x2000].to_vec();
    let rows = omap::build_row_offsets(&blob);
    for (i, off) in rows.iter().enumerate().take(omap::ROW_COUNT) {
        let o = i * 2;
        if o + 1 < 0x2000 {
            game.wram[o] = (*off & 0xFF) as u8;
            game.wram[o + 1] = ((*off >> 8) & 0xFF) as u8;
        }
    }
}

/// `overworld6` (bank 0 `$8A1A`): map-patch path (gap: stone-palace +
/// hidden-town writes stay with the interpreter; records region select).
fn ow_overworld6(game: &mut Game) {
    let region = omap::region_select(r(&game.ram, 0x0706), r(&game.ram, 0x070A));
    w(
        &mut game.ram,
        0x0002,
        match region {
            omap::Region::West => 0,
            omap::Region::DeathMountain => 1,
            omap::Region::East => 2,
            omap::Region::MazeIsland => 3,
        },
    );
}

/// `overworld7` (bank 0 `$8B2E`): transition dispatch (gap: records the
/// key-area scan result via [`otrans::find_key_area`] on staged lanes).
fn ow_overworld7(game: &mut Game) {
    // Staged lanes are not in fixed RAM (gap); record a total default.
    w(&mut game.ram, 0x0002, 0);
}

/// `Check_if_Link_stepped_on_a_Key_Area` (bank 0 `$857D`).
fn ow_check_key_area(game: &mut Game) {
    // Full scan needs the 252-byte WRAM lanes (interp-only); record the
    // transition for the staged slot in `$03` (gap documented).
    let slot = if r(&game.ram, 0x0003) == 0xFF {
        None
    } else {
        Some(r(&game.ram, 0x0003) as usize)
    };
    match otrans::key_area_transition(
        slot,
        r(&game.ram, 0x0706),
        r(&game.ram, 0x0787) != 0,
        r(&game.ram, 0x0073),
        r(&game.ram, 0x0074),
    ) {
        otrans::Transition::None => w(&mut game.ram, 0x0002, 0),
        otrans::Transition::Sideview { .. } => w(&mut game.ram, 0x0002, 1),
        otrans::Transition::Raft { .. } => w(&mut game.ram, 0x0002, 2),
    }
}

/// `L841B` (bank 0 `$841B`): demon AI tick + integrate.
fn ow_l841b(game: &mut Game) {
    let mut d = enc::Demons::empty();
    for s in 0..enc::DEMON_SLOTS {
        d.y[s] = r(&game.ram, 0x002A + s as u16);
        d.x[s] = r(&game.ram, 0x004E + s as u16);
        d.timer[s] = game.ram[0x050E + s];
        d.kind[s] = game.ram[0x0082 + s];
    }
    enc::integrate(&mut d);
    for s in 0..enc::DEMON_SLOTS {
        w(&mut game.ram, 0x002A + s as u16, d.y[s]);
        w(&mut game.ram, 0x004E + s as u16, d.x[s]);
        game.ram[0x0082 + s] = d.kind[s];
    }
}

/// `L86AF` (bank 0 `$86AF`): scroll commit.
fn ow_l86af(game: &mut Game) {
    ow_overworld3(game);
}

/// `L85D5` (bank 0 `$85D5`): sideview-entry effects.
fn ow_l85d5(game: &mut Game) {
    let (delta, fx, dlg) = otrans::sideview_entry_effects();
    w(&mut game.ram, 0x0002, delta);
    w(&mut game.ram, 0x0768, fx);
    w(&mut game.ram, 0x074C, dlg);
}

/// `L8601` (bank 0 `$8601`): directional step.
fn ow_l8601(game: &mut Game) {
    let facing = omap::facing_from_byte(r(&game.ram, 0x0562)).unwrap_or(omap::Facing::Right);
    let (ny, nx) = omap::try_step(
        r(&game.ram, 0x0073),
        r(&game.ram, 0x0074),
        facing,
        r(&game.ram, 0x0563),
        r(&game.ram, 0x0788) != 0,
    );
    w(&mut game.ram, 0x0073, ny);
    w(&mut game.ram, 0x0074, nx);
}

/// `Blocked_by_Tile_or_Not_Routine` (bank 0 `$870F`).
fn ow_blocked(game: &mut Game) {
    let b = omap::is_blocked(r(&game.ram, 0x0563), r(&game.ram, 0x0788) != 0);
    w(&mut game.ram, 0x0002, u8::from(b));
}

/// `L8A07` (bank 0 `$8A07`): terrain-fetch helper (gap: records `tile_at`
/// for the staged blob at `$7C00`).
fn ow_l8a07(game: &mut Game) {
    let blob = game.wram[0x1C00..0x2000].to_vec();
    let t = omap::tile_at(&blob, 0, r(&game.ram, 0x0074), r(&game.ram, 0x0073));
    w(&mut game.ram, 0x0563, t as u8);
}

/// `L8C30` (bank 0 `$8C30`): row-pointer fetch setup (gap: records start).
fn ow_l8c30(game: &mut Game) {
    let v = r(&game.ram, 0x0073);
    w(&mut game.ram, 0x0002, v);
}

/// `L8C48` (bank 0 `$8C48`): per-cell terrain fetch.
fn ow_l8c48(game: &mut Game) {
    ow_l8a07(game);
}

/// `bank7_go_outside` (bank 7 `$CCB3`, game mode 1): the sideview group's
/// exact port ([`sv_go_outside`]). Entered through the mode table's
/// `JMP ($0E)`, so it only fires with the dispatcher untrapped.
fn ow_go_outside(game: &mut Game) {
    sv_go_outside(game);
}

/// `STA ($0E),y` through the bus (WRAM blob copy or RAM).
fn sta_ind_y_0e(game: &mut Game, v: u8) {
    let lo = r(&game.ram, 0x000E);
    let hi = r(&game.ram, 0x000F);
    let addr = u16::from_le_bytes([lo, hi]).wrapping_add(u16::from(game.cpu.y));
    bus_write(game, addr, v);
}

/// Shared body of the bank-7 `SwapToSavedPRG` / banked call / `SwapToPRG0`
/// wrappers (`$DFD2`, `$DFF8`, `$E01B`, `$E024`):
///
/// ```asm
/// entry:     JSR SwapToSavedPRG      ; pushes entry + 2
/// entry + 3: JSR target              ; pushes entry + 5
/// entry + 6: JMP SwapToPRG0
/// ```
/// The two mapper helpers go through their bank-7 traps ([`jsr_sub`] /
/// [`Game::trap_jump`]); `target` fires its trap when it has one (`$DF79`) and
/// is otherwise interpreted with the saved bank mapped — the banks 1/2
/// bodies behind `$8368`, `$879B` and `$83A1` differ per bank (`$879B`
/// tests a different palace set in each), so no single Rust body would
/// be exact. Exit `A`/`N`/`Z`/`C` come from `SwapToPRG0` (`A = 0`, `Z`
/// set, `C` = 0); `X`/`Y`/`V` from the banked body. Dead stack: `entry +
/// 2` under `entry + 5` (plus the body's own traffic).
///
/// Cycles (self-charged): `JSR` 6 + `JSR` 6 + `JMP` 3 = 15; the trapped
/// `SwapToSavedPRG` (38) and `SwapToPRG0` (39) charge themselves, as does
/// the target.
fn ow_saved_bank_call(game: &mut Game, entry: u16, target: u16) {
    jsr_sub(game, entry, 0xFFC9);
    jsr_sub(game, entry.wrapping_add(3), target);
    cyc(game, 6 + 6 + 3);
    game.trap_jump(0xFFC5);
}

/// `bank7_Overworld_Boundaries__Mountain_or_Water_Bank_1` (bank 7 `$DFEF`):
/// terrain lookup at map column `$00` / row `$01` through the saved bank's
/// `L83CF` (identical code in banks 1 and 2 — only the inner `JSR` target
/// differs, `$93AC` vs `$93BA`, both reached from `$83E0`).
///
/// ```asm
/// $DFEF: JSR SwapToSavedPRG : JSR L83CF : JMP SwapToPRG0
/// L83CF: LDA $00 : CMP #$40 : BCS east
///        LDA $01 : SEC : SBC #$1E : STA $04 : CMP #$4B : BCS east
///        JSR code8            ; A = row: ASL : TAY : LDA $6000,y : STA $0E
///                             ;   LDA $6001,y : STA $0F : LDY #$00 : RTS
///        INC $00 : LDA #$00 : STA $03 : LDX #$03
/// loop:  LDA ($0E),y : AND #$0F : STA $02
///        LDA ($0E),y : LSR x4 : SEC : ADC $03 : STA $03
///        CMP $00 : BCS done : INY : JMP loop
/// east:  LDA #$0C : STA $02
/// done:  RTS
/// ```
/// Effects: `$02` = terrain of the RLE run covering column `$00` (`$0C`,
/// water, outside the 64x75 map), `$03` = that run's end column, `$04` =
/// map row, `$00` incremented, `$0E/$0F` = row pointer from the `$6000`
/// table. Exit registers come from the trailing `SwapToPRG0` (`A = 0`,
/// `N = 0`, `Z = 1`, `C = 0`); `V` from the last `SBC`/`ADC`; `X = 3` and
/// `Y` = run index on the in-map path, both untouched on the water path.
/// The inner `JSR`s leave `$DFF1`, `$DFF4` and `$83E2` on the dead stack.
/// The old shim wrote a synthetic blocked flag to `$02` (any% lockstep:
/// `$04B8` at frame 459, sprites from 654).
///
/// Cycles (self-charged): wrapper 92 (`JSR` 6, `SwapToSavedPRG` 38, `JSR`
/// 6, `JMP` 3, `SwapToPRG0` 39) plus the `L83CF` body: 19 (water on
/// column), 33 (water on row), or 39 plus `code8` 26, 41 per continuing
/// run and 43 for the final run, with 1 more per page-crossing indexed
/// read.
pub fn ow_boundaries(game: &mut Game) {
    inner_jsr_frame(game, 0xDFEF, crate::bank7_mmc1::swap_to_saved_prg);
    inner_jsr_frame(game, 0xDFF2, ow_l83cf_body);
    crate::bank7_mmc1::swap_to_prg0(game);
    game.cpu.cycles += 92;
}

/// `L83CF` body (banks 1/2 `$83CF`; listing in [`ow_boundaries`]).
fn ow_l83cf_body(game: &mut Game) {
    // LDA $00 : CMP #$40 : BCS east.
    let mut a = r(&game.ram, 0x0000);
    game.cpu.a = a;
    set_nz(&mut game.cpu.p, a);
    cmp_val(&mut game.cpu.p, a, 0x40);
    if game.cpu.p & FLAG_C != 0 {
        ow_l83cf_east(game);
        // LDA 3 + CMP 2 + BCS taken 3, then the 11-cycle water tail.
        game.cpu.cycles += 8 + 11;
        return;
    }
    // LDA $01 : SEC : SBC #$1E : STA $04 : CMP #$4B : BCS east.
    a = r(&game.ram, 0x0001);
    set_nz(&mut game.cpu.p, a);
    game.cpu.p |= FLAG_C;
    a = sbc_val(&mut game.cpu.p, a, 0x1E);
    game.cpu.a = a;
    w(&mut game.ram, 0x0004, a);
    cmp_val(&mut game.cpu.p, a, 0x4B);
    if game.cpu.p & FLAG_C != 0 {
        ow_l83cf_east(game);
        // 7 (column check, BCS not taken) + 15 (row check, BCS taken) + 11.
        game.cpu.cycles += 22 + 11;
        return;
    }
    // JSR code8 (at $83E0, pushing $83E2); the body charges its own 26.
    inner_jsr_frame(game, 0x83E0, ow_code8_body);
    // INC $00 : LDA #$00 : STA $03 : LDX #$03.
    let n = inc_val(&mut game.cpu.p, r(&game.ram, 0x0000));
    w(&mut game.ram, 0x0000, n);
    set_nz(&mut game.cpu.p, 0);
    w(&mut game.ram, 0x0003, 0);
    game.cpu.x = 3;
    set_nz(&mut game.cpu.p, 3);
    // 21 (both checks fall through) + JSR 6 + INC 5 + LDA 2 + STA 3 + LDX 2.
    let mut cycles: u64 = 39;
    loop {
        // LDA ($0E),y : AND #$0F : STA $02.
        let (v, cross) = lda_ind_y_0e(game);
        let terrain = v & 0x0F;
        set_nz(&mut game.cpu.p, terrain);
        w(&mut game.ram, 0x0002, terrain);
        // LDA ($0E),y : LSR x4 : SEC : ADC $03 : STA $03 : CMP $00 : BCS done.
        let (v, _) = lda_ind_y_0e(game);
        a = v;
        for _ in 0..4 {
            a = lsr_acc(&mut game.cpu.p, a);
        }
        game.cpu.p |= FLAG_C;
        a = adc_val(&mut game.cpu.p, a, r(&game.ram, 0x0003));
        game.cpu.a = a;
        w(&mut game.ram, 0x0003, a);
        cmp_val(&mut game.cpu.p, a, r(&game.ram, 0x0000));
        // LDA (zp),y 5 + AND 2 + STA 3 + LDA (zp),y 5 + 4x LSR 8 + SEC 2 +
        // ADC 3 + STA 3 + CMP 3 (+1 per page-crossing read).
        cycles += 34 + 2 * u64::from(cross);
        if game.cpu.p & FLAG_C != 0 {
            // BCS taken 3 + RTS 6.
            cycles += 9;
            break;
        }
        // BCS not taken 2 + INY 2 + JMP 3.
        cycles += 7;
        game.cpu.y = game.cpu.y.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.y);
    }
    game.cpu.cycles += cycles;
}

/// `L83CF` water tail: `LDA #$0C : STA $02 : RTS` (11 cycles, charged by
/// the caller).
fn ow_l83cf_east(game: &mut Game) {
    game.cpu.a = 0x0C;
    set_nz(&mut game.cpu.p, 0x0C);
    w(&mut game.ram, 0x0002, 0x0C);
}

/// `bank1_code8` / `L93BA` (banks 1/2 `$93AC` / `$93BA`): `$0E/$0F` = the
/// `$6000` row-pointer word for row `A`, `Y = 0`.
fn ow_code8_body(game: &mut Game) {
    // ASL : TAY.
    let a = asl_acc(&mut game.cpu.p, game.cpu.a);
    game.cpu.a = a;
    game.cpu.y = a;
    set_nz(&mut game.cpu.p, a);
    // LDA $6000,y : STA $0E : LDA $6001,y : STA $0F.
    let y = u16::from(a);
    let lo = bus_read(game, 0x6000 + y);
    set_nz(&mut game.cpu.p, lo);
    w(&mut game.ram, 0x000E, lo);
    let hi = bus_read(game, 0x6001 + y);
    game.cpu.a = hi;
    set_nz(&mut game.cpu.p, hi);
    w(&mut game.ram, 0x000F, hi);
    // LDY #$00 : RTS.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    // ASL 2 + TAY 2 + LDA abs,y 4 + STA 3 + LDA abs,y 4 (+1 when $6001+y
    // crosses, i.e. y = $FF) + STA 3 + LDY 2 + RTS 6.
    game.cpu.cycles += 26 + u64::from(y == 0xFF);
}

/// `bank7_Check_for_Hidden_Palace_spot_Bank_1` (bank 7 `$DFF8`): the
/// flute probe. `JSR SwapToSavedPRG : JSR $8368 : JMP SwapToPRG0` — the
/// banked `$8368` (`bank1_/bank2_Check_for_Hidden_Palace_spot`) compares
/// `$0706/$73/$74` against the hidden-palace spot, else `$0563` against
/// the spider terrain, and jumps into the trapped `$DF79` (after `$83A1`
/// for the spider) to transform the tile. Called from bank 0 `$84E9` with
/// `$00`/`$01` = Link's map column/row. See [`ow_saved_bank_call`]. The
/// old shim wrote a synthetic hit flag to `$02`.
pub fn ow_hidden(game: &mut Game) {
    ow_saved_bank_call(game, 0xDFF8, 0x8368);
}

/// `bank7_Turn_Palaces_into_Stone_Bank_1` (bank 7 `$E01B`): `JSR
/// SwapToSavedPRG : JSR $879B : JMP SwapToPRG0` — the banked `$879B`
/// (`bank1_/bank2_Transform_completed_palaces_into_stone`) stamps a
/// mountain tile over each completed palace's WRAM map run; the two
/// banks test different palace sets (`LDX #$02` / `LDX #$03` + region
/// check), which is why the body is interpreted. Called from bank 0
/// `$8153` (`Initialization_stuff`, game mode 2). See
/// [`ow_saved_bank_call`]. The old shim wrote a no-op marker to `$02`.
pub fn ow_stone(game: &mut Game) {
    ow_saved_bank_call(game, 0xE01B, 0x879B);
}

/// `bank7_forest_chop_with_hammer` (bank 7 `$DF79`): transform the faced
/// overworld tile (hammer on rock/forest, flute spider, hidden palace).
///
/// ```asm
/// $DF79: JSR L83CF                    ; run lookup for column $00 / row $01
/// LDF7C: LDA ($0E),y : CMP $DF5E,x : BEQ LDF8B : DEX : BPL LDF7C
///        INX : STX $0725 : RTS        ; no match: PPU macro 0
/// LDF8B: TXA : BEQ LDFBD              ; entry 0 (rock): no extra checks
///        LDA $0706 : CMP #$02 : BNE LDFD1
///        CPX #$03 : BNE LDFB9
///        LDA $04 : CMP #$33 : BNE LDFD1
///        LDA $00 : CMP #$3E : BNE LDFD1
///        LDA #$5C : STA $0305 : LDA #$5D : STA $0306
///        LDA #$5E : STA $030A : LDA #$5F : STA $030B   ; hidden-town tiles
/// LDFB9: LDA #$10 : STA $EB           ; music
/// LDFBD: LDA $DF62,x : STA ($0E),y    ; write the transformed run byte
///        CPX #$02 : BCC LDFD1
///        DEX : DEX : LDY $DF66,x : LDA $DF68,x : STA $6A00,y
/// LDFD1: RTS
/// ```
/// `L83CF` is the same code in banks 1 and 2 ([`ow_l83cf_body`], as in
/// [`ow_boundaries`]); it leaves `X = 3` and `Y` = the run index on the
/// in-map path, so the table scan runs `X = 3..=0` over the four
/// transformable run bytes (`$DF5E`: rock, spider, desert, forest — each a
/// single-column run) and their replacements (`$DF62`). Entry 0 (rock)
/// transforms unconditionally; entries 1-3 only in region 2, entry 3 at
/// row `$33` / post-`INC` column `$3E` also stages the hidden-town tile
/// bytes at `$0305/$0306/$030A/$030B`; entries 2/3 patch the area table
/// (`$6A00 + $DF66[x - 2]` = `$DF68[x - 2]`). Reached from `LDFD2` (bank
/// 0 hammer/flute path, [`ow_ldfd2`]) and by `JMP` from the banked
/// `$8384`/`$8392` ([`ow_hidden`]). Exit `A`/`P` from the last
/// instruction of the taken path, `X` = matched entry (or 0 after `INX`
/// on no match; `x - 2` after an area-table patch), `Y` from the
/// area-table write or the run index; dead stack `$DF7B` (+ `$83E2` from
/// `code8`). The old shim ran the pure hammer probe and wrote `$00-$03`.
///
/// Cycles (self-charged, RTS included): `JSR` 6 + `L83CF` (charged by its
/// body) + per scanned entry 13 (+1 per page-crossing `($0E),y` /
/// `$DF5E,x` read; the last entry costs 12 on a match), then either 12
/// (no match) or 4 + the branch-dependent tail: 1 (entry 0), or 8 + 4 +
/// {1 (`X != 3`) | 7 + 7 + 24}, + 5 for the music write, + 14 (+1 on a
/// crossing `$DF62,x` read) for the run byte, + 7 (`X < 2`) or 27 (+1 per
/// crossing `$DF66,x` / `$DF68,x` read) for the area-table patch.
pub fn ow_chop(game: &mut Game) {
    // JSR L83CF (at $DF79, pushing $DF7B); the body charges itself.
    inner_jsr_frame(game, 0xDF79, ow_l83cf_body);
    let mut cycles: u64 = 6;
    // LDF7C: LDA ($0E),y : CMP $DF5E,x : BEQ LDF8B : DEX : BPL LDF7C.
    let matched = loop {
        let (v, cross_y) = lda_ind_y_0e(game);
        let x = game.cpu.x;
        let m = bus_read(game, 0xDF5Eu16.wrapping_add(u16::from(x)));
        cmp_val(&mut game.cpu.p, v, m);
        cycles += 5 + u64::from(cross_y) + 4 + cross(0xDF5E, x) + 2;
        if v == m {
            cycles += 1;
            break true;
        }
        let nx = x.wrapping_sub(1);
        game.cpu.x = nx;
        set_nz(&mut game.cpu.p, nx);
        cycles += 2 + 2;
        if nx & 0x80 != 0 {
            break false;
        }
        cycles += 1;
    };
    if !matched {
        // INX : STX $0725 : RTS.
        let nx = game.cpu.x.wrapping_add(1);
        game.cpu.x = nx;
        set_nz(&mut game.cpu.p, nx);
        bus_write(game, 0x0725, nx);
        game.cpu.cycles += cycles + 2 + 4 + 6;
        return;
    }
    // LDF8B: TXA : BEQ LDFBD.
    let x = game.cpu.x;
    lda(game, x);
    cycles += 2 + 2;
    if x == 0 {
        cycles += 1;
    } else {
        // LDA $0706 : CMP #$02 : BNE LDFD1.
        let region = bus_read(game, 0x0706);
        lda(game, region);
        cmp_val(&mut game.cpu.p, region, 2);
        cycles += 4 + 2 + 2;
        if region != 2 {
            game.cpu.cycles += cycles + 1 + 6;
            return;
        }
        // CPX #$03 : BNE LDFB9.
        cmp_val(&mut game.cpu.p, x, 3);
        cycles += 2 + 2;
        if x == 3 {
            // LDA $04 : CMP #$33 : BNE LDFD1.
            let row = r(&game.ram, 0x0004);
            lda(game, row);
            cmp_val(&mut game.cpu.p, row, 0x33);
            cycles += 3 + 2 + 2;
            if row != 0x33 {
                game.cpu.cycles += cycles + 1 + 6;
                return;
            }
            // LDA $00 : CMP #$3E : BNE LDFD1.
            let col = r(&game.ram, 0x0000);
            lda(game, col);
            cmp_val(&mut game.cpu.p, col, 0x3E);
            cycles += 3 + 2 + 2;
            if col != 0x3E {
                game.cpu.cycles += cycles + 1 + 6;
                return;
            }
            // LDA #$5C : STA $0305 : ... : LDA #$5F : STA $030B.
            for (v, addr) in [
                (0x5C, 0x0305),
                (0x5D, 0x0306),
                (0x5E, 0x030A),
                (0x5F, 0x030B),
            ] {
                lda(game, v);
                bus_write(game, addr, v);
            }
            cycles += 4 * (2 + 4);
        } else {
            cycles += 1;
        }
        // LDFB9: LDA #$10 : STA $EB.
        lda(game, 0x10);
        w(&mut game.ram, 0x00EB, 0x10);
        cycles += 2 + 3;
    }
    // LDFBD: LDA $DF62,x : STA ($0E),y : CPX #$02 : BCC LDFD1.
    let x = game.cpu.x;
    let v = bus_read(game, 0xDF62u16.wrapping_add(u16::from(x)));
    lda(game, v);
    sta_ind_y_0e(game, v);
    cmp_val(&mut game.cpu.p, x, 2);
    cycles += 4 + cross(0xDF62, x) + 6 + 2 + 2;
    if x < 2 {
        game.cpu.cycles += cycles + 1 + 6;
        return;
    }
    // DEX : DEX : LDY $DF66,x : LDA $DF68,x : STA $6A00,y : RTS.
    let nx = x.wrapping_sub(2);
    game.cpu.x = nx;
    set_nz(&mut game.cpu.p, nx);
    let y = bus_read(game, 0xDF66u16.wrapping_add(u16::from(nx)));
    ldy(game, y);
    let a = bus_read(game, 0xDF68u16.wrapping_add(u16::from(nx)));
    lda(game, a);
    bus_write(game, 0x6A00u16.wrapping_add(u16::from(y)), a);
    cycles += 2 + 2 + 4 + cross(0xDF66, nx) + 4 + cross(0xDF68, nx) + 5 + 6;
    game.cpu.cycles += cycles;
}

/// `LDF01` (bank 7 `$DF01`): overworld scroll anchor from Link's map
/// position (`$75` row / `$76` column, square units).
///
/// ```asm
/// LDA $75 : JSR LDF3F : STA $00 : TYA : ASL x4 : ORA $00 : STA $0A : STA $77
/// LDA #$08 : STA $00 : ASL : STA $7D
/// LDA $0A : ASL x5 : ROL $00 : ASL : ROL $00 : STA $01
/// LDA $76 : AND #$0F : ASL : ADC $01 : STA $7A
/// LDA $0A : AND #$10 : LSR : ORA $00 : STA $79
/// LDA #$00 : STA $7E : RTS
/// ```
/// `$0A`/`$77` = `(row / 15) << 4 | row % 15`; `$7D` = `$10` pixels to
/// move; `$79`/`$7A` = nametable address of the row to redraw; `$7E` = 0.
/// Exit `A = 0` (`Z` set), `Y = row / 15`, `C` = 0 (from the `LSR` of
/// `$0A & $10`), `V` from `ADC $01`; `X` untouched. The inner `JSR LDF3F`
/// leaves `$DF05` on the dead stack. The old shim wrote a pixel anchor to
/// `$02`/`$03` and none of the above (any% lockstep: `$0302` at frame 457,
/// sprites from 488, palette from 909).
///
/// Cycles (self-charged): 106 (`LDA` 3, `JSR` 6, `STA` 3, `TYA` 2, 4x
/// `ASL` 8, `ORA` 3, 2x `STA` 6, `LDA` 2, `STA` 3, `ASL` 2, `STA` 3, `LDA`
/// 3, 5x `ASL` 10, `ROL zp` 5, `ASL` 2, `ROL zp` 5, `STA` 3, `LDA` 3,
/// `AND` 2, `ASL` 2, `ADC` 3, `STA` 3, `LDA` 3, `AND` 2, `LSR` 2, `ORA` 3,
/// `STA` 3, `LDA` 2, `STA` 3, `RTS` 6) plus the `LDF3F` body, which
/// charges its own 18 + 9 * (row / 15).
pub fn ow_ldf01(game: &mut Game) {
    // LDA $75 : JSR LDF3F (at $DF03, pushing $DF05) : STA $00.
    let row = r(&game.ram, 0x0075);
    game.cpu.a = row;
    set_nz(&mut game.cpu.p, row);
    inner_jsr_frame(game, 0xDF03, ow_ldf3f);
    w(&mut game.ram, 0x0000, game.cpu.a);
    // TYA : ASL x4 : ORA $00 : STA $0A : STA $77.
    let mut a = game.cpu.y;
    set_nz(&mut game.cpu.p, a);
    for _ in 0..4 {
        a = asl_acc(&mut game.cpu.p, a);
    }
    a |= r(&game.ram, 0x0000);
    set_nz(&mut game.cpu.p, a);
    w(&mut game.ram, 0x000A, a);
    w(&mut game.ram, 0x0077, a);
    // LDA #$08 : STA $00 : ASL : STA $7D.
    a = 0x08;
    set_nz(&mut game.cpu.p, a);
    w(&mut game.ram, 0x0000, a);
    a = asl_acc(&mut game.cpu.p, a);
    w(&mut game.ram, 0x007D, a);
    // LDA $0A : ASL x5 : ROL $00 : ASL : ROL $00 : STA $01.
    a = r(&game.ram, 0x000A);
    set_nz(&mut game.cpu.p, a);
    for _ in 0..5 {
        a = asl_acc(&mut game.cpu.p, a);
    }
    let cin = game.cpu.p & FLAG_C != 0;
    let (m, cout) = rol_mem(&mut game.cpu.p, cin, r(&game.ram, 0x0000));
    w(&mut game.ram, 0x0000, m);
    set_c(&mut game.cpu.p, cout);
    a = asl_acc(&mut game.cpu.p, a);
    let cin = game.cpu.p & FLAG_C != 0;
    let (m, cout) = rol_mem(&mut game.cpu.p, cin, r(&game.ram, 0x0000));
    w(&mut game.ram, 0x0000, m);
    set_c(&mut game.cpu.p, cout);
    w(&mut game.ram, 0x0001, a);
    // LDA $76 : AND #$0F : ASL : ADC $01 : STA $7A.
    a = r(&game.ram, 0x0076);
    set_nz(&mut game.cpu.p, a);
    a &= 0x0F;
    set_nz(&mut game.cpu.p, a);
    a = asl_acc(&mut game.cpu.p, a);
    a = adc_val(&mut game.cpu.p, a, r(&game.ram, 0x0001));
    w(&mut game.ram, 0x007A, a);
    // LDA $0A : AND #$10 : LSR : ORA $00 : STA $79.
    a = r(&game.ram, 0x000A);
    set_nz(&mut game.cpu.p, a);
    a &= 0x10;
    set_nz(&mut game.cpu.p, a);
    a = lsr_acc(&mut game.cpu.p, a);
    a |= r(&game.ram, 0x0000);
    set_nz(&mut game.cpu.p, a);
    w(&mut game.ram, 0x0079, a);
    // LDA #$00 : STA $7E : RTS.
    game.cpu.a = 0;
    set_nz(&mut game.cpu.p, 0);
    w(&mut game.ram, 0x007E, 0);
    game.cpu.cycles += 106;
}

/// `LDF3F` (bank 7 `$DF3F`): divide `A` by 15 by repeated subtraction.
///
/// ```asm
/// LDY #$FF
/// LDF41: INY : SEC : SBC #$0F : BCS LDF41
/// ADC #$0F : RTS
/// ```
/// Exit `Y = A / 15`, `A = A % 15`, `N`/`Z`/`C`/`V` from the final
/// `ADC #$0F` (`C` = 1: the wrapped remainder re-crosses zero); `X`
/// untouched. Every caller (`$8175`, `$8856`, `$8B57`, [`ow_ldf01`])
/// consumes `Y` — the old shim left the registers alone and wrote
/// `$00 / 15` into `$01`/`$02` (any% lockstep: `$0747` at frame 450).
///
/// Cycles (self-charged, incl. `RTS`): 18 + 9 * (A / 15) — `LDY` 2, per
/// pass `INY` 2 + `SEC` 2 + `SBC` 2 + `BCS` 3 (final pass 2), `ADC` 2,
/// `RTS` 6.
pub fn ow_ldf3f(game: &mut Game) {
    // LDY #$FF.
    game.cpu.y = 0xFF;
    set_nz(&mut game.cpu.p, 0xFF);
    let mut a = game.cpu.a;
    let mut passes: u64 = 0;
    loop {
        // INY : SEC : SBC #$0F : BCS LDF41.
        game.cpu.y = game.cpu.y.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.y);
        game.cpu.p |= FLAG_C;
        a = sbc_val(&mut game.cpu.p, a, 0x0F);
        passes += 1;
        if game.cpu.p & FLAG_C == 0 {
            break;
        }
    }
    // ADC #$0F with the borrow still clear: undo the failing subtraction.
    game.cpu.a = adc_val(&mut game.cpu.p, a, 0x0F);
    game.cpu.cycles += 9 + 9 * passes;
}

/// `LDFD2` (bank 7 `$DFD2`): `JSR SwapToSavedPRG : JSR $DF79 : JMP
/// SwapToPRG0` — the bank-0 hammer/flute tail (`$850F`, right after
/// [`ow_le024`]) runs [`ow_chop`] with the saved bank mapped so `L83CF`
/// and the `($0E),y` run bytes resolve in banks 1/2. See
/// [`ow_saved_bank_call`]. The old shim called the pure hammer probe
/// without the bank bracket.
pub fn ow_ldfd2(game: &mut Game) {
    ow_saved_bank_call(game, 0xDFD2, 0xDF79);
}

/// `LE001` (bank 7 `$E001`): one RLE run of the overworld row at `($0E)`,
/// read with the saved PRG bank (`$0769`) mapped.
///
/// ```asm
/// JSR SwapToSavedPRG
/// LDA ($0E),y : AND #$0F : STA $02                ; run terrain
/// LDA ($0E),y : LSR x4 : SEC : ADC $03 : STA $03  ; run end column
/// PHA : JSR SwapToPRG0 : PLA : RTS
/// ```
/// Exit `A = $03` (`N`/`Z` from `PLA`), `C` = 0 (the `LSR`s of zero inside
/// `SwapToPRG0`), `V` from the `ADC`; `X`/`Y` untouched. The inner `JSR`s
/// leave `$E003`/`$E018` on the dead stack. Both callers (`$892D`,
/// `$8C30`) loop on `A` against `$76` / `#$41`, so the old shim (which
/// decoded `$00` into `$02`/`$03` and left `A`) corrupted the `$0480`
/// terrain cache and the `$6000` row-pointer table (any% lockstep:
/// `$0480`/`$6002` at frame 451, sprites from 472, palette from 909).
///
/// Cycles (self-charged): 133 + 1 per page-crossing indexed read (both
/// reads share the address, so 0 or 2): `JSR` 6 + `SwapToSavedPRG` 38,
/// `LDA (zp),y` 5, `AND` 2, `STA` 3, `LDA (zp),y` 5, 4x `LSR` 8, `SEC` 2,
/// `ADC` 3, `STA` 3, `PHA` 3, `JSR` 6 + `SwapToPRG0` 39, `PLA` 4, `RTS` 6.
pub fn ow_le001(game: &mut Game) {
    // JSR SwapToSavedPRG (at $E001, pushing $E003).
    inner_jsr_frame(game, 0xE001, crate::bank7_mmc1::swap_to_saved_prg);
    // LDA ($0E),y : AND #$0F : STA $02.
    let (v, cross) = lda_ind_y_0e(game);
    let terrain = v & 0x0F;
    set_nz(&mut game.cpu.p, terrain);
    w(&mut game.ram, 0x0002, terrain);
    // LDA ($0E),y : LSR x4 : SEC : ADC $03 : STA $03.
    let (v, _) = lda_ind_y_0e(game);
    let mut a = v;
    for _ in 0..4 {
        a = lsr_acc(&mut game.cpu.p, a);
    }
    game.cpu.p |= FLAG_C;
    a = adc_val(&mut game.cpu.p, a, r(&game.ram, 0x0003));
    w(&mut game.ram, 0x0003, a);
    // PHA : JSR SwapToPRG0 (at $E016, pushing $E018) : PLA.
    push_byte(game, a);
    inner_jsr_frame(game, 0xE016, crate::bank7_mmc1::swap_to_prg0);
    let a = pop_byte(game);
    game.cpu.a = a;
    set_nz(&mut game.cpu.p, a);
    game.cpu.cycles += 133 + 2 * u64::from(cross);
}

/// `LE024` (bank 7 `$E024`): `JSR SwapToSavedPRG : JSR $83A1 : JMP
/// SwapToPRG0` — the banked `$83A1` (`bank1_/bank2_code6`) calls the
/// trapped `$DF01` for the scroll anchor, clears `$7D`, stages the
/// 12-byte tile-redraw PPU macro at `$0301` from its bank table, patches
/// `$0302/$0307/$0303/$0308` from `$79/$7A`, and seeds `$00/$01` from
/// `$76/$75`. Called from bank 0 `$850C` (hammer/flute). See
/// [`ow_saved_bank_call`]. The old shim copied `$075A` to `$02`.
pub fn ow_le024(game: &mut Game) {
    ow_saved_bank_call(game, 0xE024, 0x83A1);
}

/// `LE16F` (bank 7 `$E16F`): sideview-exit gate shared with sideview
/// (see [`sv_scroll_gate`]).
fn ow_le16f(game: &mut Game) {
    sv_scroll_gate(game);
}

/// Register the overworld traps with `Game` shims.
///
/// Only fixed-bank (`Some(7)`) entries are registered (aliasing caveat);
/// banked (`Some(0)`/`None`) entries stay data-only in
/// [`OVERWORLD_TRAPS`] pending mapper-aware routing, and the mode-0
/// loader `bank7_code18` (`$CD40`) is left to the ROM (see the section
/// note above). Idempotent.
///
/// Every registered port charges its own path-dependent cycles (directly,
/// or through the traps / interpreted bodies it calls), so all of them
/// register with a zero table cost.
pub fn register_overworld_traps(game: &mut Game) {
    for (name, bank, addr) in OVERWORLD_TRAPS {
        if *bank != Some(7) {
            continue;
        }
        let func: fn(&mut Game) = match *name {
            "bank7_go_outside" => ow_go_outside,
            "bank7_Overworld_Boundaries__Mountain_or_Water_Bank_1" => ow_boundaries,
            "bank7_Check_for_Hidden_Palace_spot_Bank_1" => ow_hidden,
            "bank7_Turn_Palaces_into_Stone_Bank_1" => ow_stone,
            "bank7_forest_chop_with_hammer" => ow_chop,
            "LDF01" => ow_ldf01,
            "LDF3F" => ow_ldf3f,
            "LDFD2" => ow_ldfd2,
            "LE001" => ow_le001,
            "LE024" => ow_le024,
            "LE16F" => ow_le16f,
            // `bank7_code18` ($CD40): ROM-owned by design.
            _ => continue,
        };
        game.trap_register_cycles(name, *bank, *addr, func, 0);
    }
    // Bank-0 driver shims are implemented above for unit-test coverage but
    // intentionally not registered (aliasing with sideview `$8000-$BFFF`).
    // They are exercised directly by `sideview_traps_tests`.
    let _ = (
        ow_overworld1 as fn(&mut Game),
        ow_overworld2 as fn(&mut Game),
        ow_overworld3 as fn(&mut Game),
        ow_overworld4 as fn(&mut Game),
        ow_overworld6 as fn(&mut Game),
        ow_overworld7 as fn(&mut Game),
        ow_check_key_area as fn(&mut Game),
        ow_l841b as fn(&mut Game),
        ow_l86af as fn(&mut Game),
        ow_l85d5 as fn(&mut Game),
        ow_l8601 as fn(&mut Game),
        ow_blocked as fn(&mut Game),
        ow_l8a07 as fn(&mut Game),
        ow_l8c30 as fn(&mut Game),
        ow_l8c48 as fn(&mut Game),
    );
}
