//! Boot-path trap shims + registration.
//!
//! `Game`-shim layer over the pure [`crate::boot`] module.
//! Each `fn(&mut Game)` replicates a fixed-bank boot routine's observable
//! effects (memory writes *and* the register/flag/stack state the original
//! tail-call would leave behind); banked routines stay data-only.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`bt_lc2ca`] | `LC2CA` | bank 7 `$C2CA` |
//! | [`bt_code5`] | `bank7_code5` | bank 7 `$C2E6` |
//! | [`bt_code6`] | `bank7_code6` | bank 7 `$C31E` |
//!
//! Banked code (every bank-5 `$8000-$BFFF` and bank-0 `$8000-$BFFF`
//! entry) is listed in [`BOOT_TRAPS`] with `bank = None` and
//! intentionally *not* registered: 16-bit trap keys alias across
//! `$8000-$BFFF` (see `traps.rs` M1 caveat). Same rule as the banked
//! entries in
//! [`register_sideview_traps`](crate::sideview_traps::register_sideview_traps):
//! only `Some(7)` (fixed-bank) entries are registered today.
//!
//! Deliberately unregistered fixed-bank entries (documented, never trapped):
//!
//! * `bank7_PowerON_code` (`$C000`) — entered by `JMP`; a `JMP` trap would
//!   pop a return address that was never pushed (same reason
//!   `bank7_reset` documents for `$FF70`).
//! * `LC388` (`$C388`: `LDA #$05 : JMP SwapPRG`) — `JMP`-only tail target
//!   (from `L_Bank6Code0` `$C03A` and the `LC37C`/`LC382` ending paths);
//!   trapping it would corrupt the caller's stack the same way.
//! * `LCF05` (`$CF05`) — already trapped by
//!   [`register_title_traps`](crate::title_traps::register_title_traps)
//!   (`tt_mode_inc`); not duplicated here.

use crate::game::Game;

// ---------------------------------------------------------------------------
// Trap table.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Registration table for `main` to wire into the trap dispatcher.
///
/// Fixed-bank (`Some(7)`) entries are safe today; `None` entries are
/// data-only (aliasing caveat above) and skipped by
/// [`register_boot_traps`]. The two `Some(7)` JMP-only entries
/// (`bank7_PowerON_code`, `LC388`) are additionally skipped by name (their
/// bodies never return — a trap's emulated `RTS` would pop unpushed
/// stack).
pub const BOOT_TRAPS: &[TrapEntry] = &[
    // Fixed bank: registered (JSR-entered, RTS/tail-call balanced).
    ("LC2CA", Some(7), 0xC2CA),
    ("bank7_code5", Some(7), 0xC2E6),
    ("bank7_code6", Some(7), 0xC31E),
    // Fixed bank: listed, never registered (JMP-only entries).
    ("bank7_PowerON_code", Some(7), 0xC000),
    ("LC388", Some(7), 0xC388),
    // Bank 5: boot/title/file-select/ending entries (data-only).
    ("bank5_PowerON__Reset_Memory", None, 0xA6A0),
    ("bank5_A610", None, 0xA610),
    ("LA6D9", None, 0xA6D9),
    ("bank5_code19", None, 0xA6F0),
    ("bank5_code20", None, 0xA70F),
    ("LA72E", None, 0xA72E),
    ("LA737", None, 0xA737),
    ("LA795", None, 0xA795),
    ("bank5_Intro_Sprites", None, 0xA7C1),
    ("bank5_code21", None, 0xA8C1),
    ("bank5_code_ADE0", None, 0xADE0),
    ("LAF1F", None, 0xAF1F),
    ("LAB6D", None, 0xAB6D),
    ("bank5_code27", None, 0xB960),
    ("bank5_Load_Saved_Games_Data", None, 0xB261),
    ("LB28B", None, 0xB28B),
    ("bank5_filesel_entry", None, 0xB22D),
    // Bank 0: new-game init (data-only; aliases bank 5's window).
    ("startup_init_begin_game", None, 0xAA08),
    ("LAA28", None, 0xAA28),
];

/// Number of fixed-bank boot traps actually registered.
pub const BOOT_TRAP_COUNT: usize = 3;

/// `LC2CA` steady-state cycle cost: `LDY` 2 + 2x `STY` abs 8 +
/// `JSR LD174` 6 + `LD174` same-path 21 (per the central table) +
/// `JSR $D385` 6 + `$D385` body 43 (ditto) = 86. No `RTS` (tail call).
/// Stage-change frames undercount (LD174's change path runs
/// `Remove_All_Sprites`, ~1092 — same caveat as the central table).
pub const LC2CA_CYCLES: u64 = 86;

/// `bank7_code5`/`bank7_code6` steady-state cost: `JSR LD168` 6 +
/// `LD168` same-path 20 + `JSR $D385` 6 + `$D385` body 43 = 75.
/// Mode-change frames undercount (37 vs 20).
pub const CODE5_CYCLES: u64 = 75;
/// See [`CODE5_CYCLES`] (identical shape at `$C31E`).
pub const CODE6_CYCLES: u64 = 75;

// ---------------------------------------------------------------------------
// Shared tail-call helper.
// ---------------------------------------------------------------------------

/// Replicate the `bank7_PullAddrFromTableFollowingThisJSR` (`$D385`) tail
/// call with an explicit `table_base`.
///
/// The trapping `JSR` has already pushed the *outer caller's* return address.
/// The real routine then does an internal `JSR $D385`; that inner return is
/// the word consumed by the trampoline's `PLA/PLA`, while the outer return
/// remains on the stack for the selected table target's eventual `RTS`.
/// Preserve that distinction: the inner frame is never materialised and the
/// `JMP ($0E)` becomes a [`Game::trap_jump`], so the dispatcher lands on the
/// table target (firing its trap when it has one) with only the outer
/// return on the stack. Register/flag/scratch effects match the ASM
/// exactly: `C` from the `ASL` bit 7, `Y = A*2 + 2` (both `INY`s), `A` =
/// target high byte,
/// `$0C/$0D` = the return address the internal `JSR $D385` would have pushed
/// (`table_base - 1`), `$0E/$0F` = target.
fn stage_table_tail(game: &mut Game, table_base: u16) {
    use crate::bank7_common::set_nz;
    use crate::bank7_dispatch::read_jump_table;
    use crate::cpu::FLAG_C;

    let index = game.cpu.a;
    let target = read_jump_table(game, table_base, index);
    // The skipped inner `JSR $D385` would push the address immediately before
    // the inline table. Its `PLA/PLA` result is observable in $0C/$0D, but it
    // is not the outer caller return already on the real stack.
    let tramp_ret = table_base.wrapping_sub(1);
    game.ram[0x0C] = tramp_ret as u8;
    game.ram[0x0D] = (tramp_ret >> 8) as u8;
    // ASL : TAY (C = old bit 7).
    if index & 0x80 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    let doubled = index.wrapping_mul(2);
    game.cpu.y = doubled;
    set_nz(&mut game.cpu.p, doubled);
    // INY.
    let y = doubled.wrapping_add(1);
    game.cpu.y = y;
    set_nz(&mut game.cpu.p, y);
    // LDA ($0C),y : STA $0E : INY : LDA ($0C),y : STA $0F.
    let lo = (target & 0xFF) as u8;
    game.cpu.a = lo;
    set_nz(&mut game.cpu.p, lo);
    game.ram[0x0E] = lo;
    game.cpu.y = y.wrapping_add(1);
    set_nz(&mut game.cpu.p, game.cpu.y);
    let hi = (target >> 8) as u8;
    game.cpu.a = hi;
    set_nz(&mut game.cpu.p, hi);
    game.ram[0x0F] = hi;
    // JMP ($0E): continue at the target once the trap body returns.
    game.trap_jump(target);
}

// ---------------------------------------------------------------------------
// Fixed-bank shims.
// ---------------------------------------------------------------------------

/// `LC2CA` (bank 7 `$C2CA`): NMI-tail stage dispatch.
///
/// Replicates `LDY #$00 : STY $0727 : STY $0729`, the `JSR LD174`
/// stage-change detector (read-only reuse of
/// [`crate::bank7_dispatch::detect_boot_stage_change`]), and the
/// `$D385` tail-call into `bank7_pointer_table2` (`$C2D8`, 7 entries).
/// Stages past the table read trailing bytes as code on hardware (gap:
/// [`read_jump_table`](crate::bank7_dispatch::read_jump_table) reads the
/// same ROM bytes, so the decoded target still matches).
pub fn bt_lc2ca(game: &mut Game) {
    use crate::bank7_common::set_nz;
    use crate::cpu::bus_write;
    // LDY #$00 : STY $0727 : STY $0729.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x0727, 0);
    bus_write(game, 0x0729, 0);
    // JSR LD174.
    crate::bank7_dispatch::detect_boot_stage_change(game);
    // JSR $D385 → bank7_pointer_table2 ($C2D8).
    stage_table_tail(game, 0xC2D8);
}

/// `bank7_code5` (bank 7 `$C2E6`): Boot-Stage-1 dispatch (`JSR LD168`,
/// tail-call `bank7_pointer_table__game_mode`, `$C2EC`, 25 entries).
pub fn bt_code5(game: &mut Game) {
    // JSR LD168.
    crate::bank7_dispatch::detect_game_mode_change(game);
    // JSR $D385 → bank7_pointer_table__game_mode ($C2EC).
    stage_table_tail(game, 0xC2EC);
}

/// `bank7_code6` (bank 7 `$C31E`): Boot-Stage-2 dispatch (`JSR LD168`,
/// tail-call `bank7_pointer_table4`, `$C324`, 12 entries).
pub fn bt_code6(game: &mut Game) {
    // JSR LD168.
    crate::bank7_dispatch::detect_game_mode_change(game);
    // JSR $D385 → bank7_pointer_table4 ($C324).
    stage_table_tail(game, 0xC324);
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every trappable fixed-bank boot trap on `game`.
///
/// Banked (`None`) entries are skipped (aliasing caveat); the two
/// JMP-only `Some(7)` entries (`bank7_PowerON_code`, `LC388`) are skipped
/// by name (a trap's emulated `RTS` would pop unpushed stack). Costs are
/// explicit ([`LC2CA_CYCLES`]/[`CODE5_CYCLES`]/[`CODE6_CYCLES`]) so the
/// boot path does not depend on central-table coverage. Idempotent.
pub fn register_boot_traps(game: &mut Game) {
    game.traps
        .register_with_cycles("LC2CA", Some(7), 0xC2CA, bt_lc2ca, LC2CA_CYCLES);
    game.traps
        .register_with_cycles("bank7_code5", Some(7), 0xC2E6, bt_code5, CODE5_CYCLES);
    game.traps
        .register_with_cycles("bank7_code6", Some(7), 0xC31E, bt_code6, CODE6_CYCLES);
}
