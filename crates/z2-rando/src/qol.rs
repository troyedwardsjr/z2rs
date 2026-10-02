//! `qol` module: fixed quality-of-life patches. Never changes the generated
//! world, the seed or the hash code.
//!
//! Options: [`crate::flags::QolFlags`] (`ctx.flags.qol`). Every patch here is
//! either a data/operand edit or a short original 6502 hook assembled with
//! [`crate::asm`]; nothing is copied from another randomizer. Each patch
//! first checks that the bytes it replaces still hold the vanilla code (an
//! earlier module may have moved things); if not, the patch is skipped and
//! the reason goes to the developer log instead of corrupting code.
//!
//! | Option | What changes | Where |
//! |---|---|---|
//! | `fast_text` | The dialog typewriter delays (box opening, per letter, after a line) become 0 | bank 3 `$B615`, `$B74E`, `$B657` operands |
//! | `fast_spell_casting` | Casting no longer sets the "already cast" latch `$074A`, so Select casts the selected spell again without reopening the pause pane | bank 0 `$8E05` (`STY $074A` becomes `NOP`s) |
//! | `beep_threshold` | Life-bar level below which the low-health beep plays | fixed bank `$D4D5` operand |
//! | `beep_frequency` | Beep repeat period (Normal `$30`, half `$60`, quarter `$C0` frames); Off makes the threshold test always fail | bank 6 `$93B2` operand; Off: fixed bank `$D4D4` |
//! | `remove_flashing` | The death flash and the game-over flash use one steady colour | fixed bank `$C9EA` table and `$CA09`; bank 0 `$A9F1` table |
//! | `up_a_on_controller_1` | Up+Select held on controller 1 also opens the save-and-quit prompt (vanilla: Up+A on controller 2 only, which still works) | bank 0 `$A19F`, `$A1DD` + hook |
//! | `darken_thunderbird` | While Thunderbird is in the room the Thunder spell's screen flash is skipped (background and Link keep their colours) | bank 0 `$9272` + hook |
//! | `bug_fixes` | The 300-experience enemies give 300 (vanilla 301); killing Horsehead only clears the enemy slots below his own, so an item held in a higher slot survives | fixed bank `$DDCC`; bank 4 `$BEB1` |
//!
//! Not done yet (the options are accepted and ignored, see
//! README.md): `updated_hud` (needs a new HUD layout inside the
//! ported status-bar code) and `disable_hud_lag` (needs the NMI rework the
//! ported NMI path does not have).
//!
//! # Traps
//!
//! The fixed-bank edits are outside every ported routine except the death
//! flash table (`$C9EA`), which the trap ledger lumps in with the ported
//! item-spawn routine before it; it is listed in
//! [`crate::trap_policy::TRAP_HONORED_BYTES`] because only unported ROM code
//! reads it. The 300-experience fix edits the experience table the ports
//! read through the bus. So none of these patches turns a trap off.

use crate::asm::Assembler;
use crate::flags::{BeepFrequency, BeepThreshold};
use crate::{Ctx, RandoError};

/// Bank of the vanilla fixed bank.
const FIXED: u8 = 7;

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let q = ctx.flags.qol.clone();
    if q.fast_text {
        fast_text(ctx)?;
    }
    if q.fast_spell_casting {
        fast_spell_casting(ctx)?;
    }
    if q.beep_frequency == BeepFrequency::Off {
        // `CMP #$20` -> `SEC : NOP`: the "below threshold" branch is never
        // taken, so the beep request is never made.
        patch(ctx, "beep off", FIXED, 0xD4D4, &[0xC9, 0x20], &[0x38, 0xEA])?;
    } else {
        if q.beep_threshold != BeepThreshold::Normal {
            let v = beep_threshold_value(q.beep_threshold);
            patch(
                ctx,
                "beep threshold",
                FIXED,
                0xD4D4,
                &[0xC9, 0x20],
                &[0xC9, v],
            )?;
        }
        if q.beep_frequency != BeepFrequency::Normal {
            let v = beep_period_value(q.beep_frequency);
            patch(ctx, "beep frequency", 6, 0x93B1, &[0xA9, 0x30], &[0xA9, v])?;
        }
    }
    if q.remove_flashing {
        remove_flashing(ctx)?;
    }
    if q.up_a_on_controller_1 {
        up_a_on_controller_1(ctx)?;
    }
    if q.darken_thunderbird {
        darken_thunderbird(ctx)?;
    }
    if q.bug_fixes {
        bug_fixes(ctx)?;
    }
    if q.updated_hud {
        ctx.log("qol: updated HUD is not implemented yet; option ignored");
    }
    if q.disable_hud_lag {
        ctx.log("qol: steady HUD on lag is not implemented yet; option ignored");
    }
    Ok(())
}

/// `CMP` operand for the low-health beep: the beep plays while the life
/// bar value (`$0565`) is below it.
#[must_use]
pub fn beep_threshold_value(t: BeepThreshold) -> u8 {
    match t {
        BeepThreshold::Normal => 0x20,
        BeepThreshold::HalfBar => 0x10,
        BeepThreshold::QuarterBar => 0x08,
        BeepThreshold::TwoBars => 0x40,
    }
}

/// Beep timer reload (frames between beeps). `Off` is handled separately.
#[must_use]
pub fn beep_period_value(f: BeepFrequency) -> u8 {
    match f {
        BeepFrequency::Normal | BeepFrequency::Off => 0x30,
        BeepFrequency::HalfSpeed => 0x60,
        BeepFrequency::QuarterSpeed => 0xC0,
    }
}

/// Replace `expect` at `(bank, addr)` with `new`. When the bytes there are
/// not `expect` (an earlier module changed them) nothing is written and the
/// skip is logged. Returns whether the patch was applied.
pub(crate) fn patch(
    ctx: &mut Ctx,
    what: &str,
    bank: u8,
    addr: u16,
    expect: &[u8],
    new: &[u8],
) -> Result<bool, RandoError> {
    debug_assert_eq!(expect.len(), new.len());
    let off = ctx.rom.cpu_offset(bank, addr)?;
    let have = ctx.rom.read_slice(off, expect.len())?.to_vec();
    if have != expect {
        ctx.log(format!(
            "qol: {what} skipped: bank {bank} ${addr:04X} holds {have:02X?}, expected {expect:02X?}"
        ));
        return Ok(false);
    }
    ctx.rom.write(off, new)?;
    Ok(true)
}

/// Dialog delays: box-opening wait (`$2A`), per-letter wait (`$05`) and
/// end-of-line wait (`$0B`) all become 0. The printer reads `$0566` with
/// `LDA : BNE` before decrementing, so 0 means "print now".
fn fast_text(ctx: &mut Ctx) -> Result<(), RandoError> {
    patch(
        ctx,
        "fast text (open)",
        3,
        0xB614,
        &[0xA9, 0x2A],
        &[0xA9, 0],
    )?;
    patch(
        ctx,
        "fast text (letter)",
        3,
        0xB74D,
        &[0xA9, 0x05],
        &[0xA9, 0],
    )?;
    patch(
        ctx,
        "fast text (line)",
        3,
        0xB656,
        &[0xA0, 0x0B],
        &[0xA0, 0],
    )?;
    Ok(())
}

/// `STY $074A` in `Spell_Casting_Routine` latches "a spell was cast" until
/// the pause pane clears it; dropping the store lets Select cast again.
fn fast_spell_casting(ctx: &mut Ctx) -> Result<(), RandoError> {
    patch(
        ctx,
        "fast spell casting",
        0,
        0x8E05,
        &[0x8C, 0x4A, 0x07],
        &[0xEA, 0xEA, 0xEA],
    )?;
    Ok(())
}

/// The death flash cycles the backdrop through a 4-entry table (fixed bank
/// `$C9EA`) and ends on a black frame (`LDA #$0F` at `$CA08`); the game-over
/// screen has its own 4-entry cycle (bank 0 `$A9F0`). Each table becomes
/// one colour (its first entry), and the final frame uses that colour too.
fn remove_flashing(ctx: &mut Ctx) -> Result<(), RandoError> {
    let death = ctx.rom.read_cpu(FIXED, 0xC9EA)?;
    let old = ctx
        .rom
        .read_slice(ctx.rom.cpu_offset(FIXED, 0xC9EA)?, 4)?
        .to_vec();
    patch(ctx, "death flash", FIXED, 0xC9EA, &old, &[death; 4])?;
    patch(
        ctx,
        "death flash end",
        FIXED,
        0xCA08,
        &[0xA9, 0x0F],
        &[0xA9, death],
    )?;
    let over = ctx.rom.read_cpu(0, 0xA9F0)?;
    let old = ctx
        .rom
        .read_slice(ctx.rom.cpu_offset(0, 0xA9F0)?, 4)?
        .to_vec();
    patch(ctx, "game-over flash", 0, 0xA9F0, &old, &[over; 4])?;
    Ok(())
}

/// Vanilla tests `$F8 == $88` (controller 2 holds Up+A) at two places in
/// the pause pane. Both tests become `JSR up_a_check`, which also accepts
/// controller 1 holding exactly Up+Select (`$F7 == $28`).
fn up_a_on_controller_1(ctx: &mut Ctx) -> Result<(), RandoError> {
    let site_a = [0xA5, 0xF8, 0xC9, 0x88, 0xD0, 0x37];
    let site_b = [0xA5, 0xF8, 0xC9, 0x88, 0xF0, 0xC2];
    if ctx.rom.read_slice(ctx.rom.cpu_offset(0, 0xA19F)?, 6)? != site_a
        || ctx.rom.read_slice(ctx.rom.cpu_offset(0, 0xA1DD)?, 6)? != site_b
    {
        ctx.log("qol: Up+Select on controller 1 skipped: pause-pane code moved");
        return Ok(());
    }
    let hook_src = "
        ; Z set when controller 2 holds Up+A or controller 1 holds Up+Select.
        up_a_check:
            LDA $F8
            CMP #$88
            BEQ done
            LDA $F7
            CMP #$28
        done:
            RTS
    ";
    let len = crate::asm::assemble_at(0, hook_src)?.len();
    let hook = ctx.rom.alloc_vanilla(0, len)?;
    let mut a = Assembler::new();
    a.define("HOOK", hook);
    let out = a.assemble(&format!(
        ".org HOOK\n{hook_src}
        .org $A19F
            JSR up_a_check
            BNE $A1DC
            NOP
        .org $A1DD
            JSR up_a_check
            BEQ $A1A5
            NOP
        "
    ))?;
    ctx.rom.apply_asm(0, &out)?;
    Ok(())
}

/// Thunder's flash: bank 0 `$9272` loads the flash timer `$074B` and picks
/// the flashing palette row from its low bits. The hook returns 0 instead
/// (no flash) while Thunderbird (Great Palace, world 5, enemy `$20`) is in
/// an enemy slot; everywhere else it returns `$074B` unchanged.
fn darken_thunderbird(ctx: &mut Ctx) -> Result<(), RandoError> {
    if ctx.rom.read_slice(ctx.rom.cpu_offset(0, 0x9272)?, 3)? != [0xAD, 0x4B, 0x07] {
        ctx.log("qol: dark Thunderbird room skipped: flash code moved");
        return Ok(());
    }
    let hook_src = "
        WORLD = $0707
        FLASH = $074B
        ENEMY_STATE = $B6
        ENEMY_ID = $A1
        tb_flash:
            LDA WORLD
            CMP #$05
            BNE plain
            TXA
            PHA
            LDX #$05
        scan:
            LDA ENEMY_STATE,X
            BEQ next
            LDA ENEMY_ID,X
            CMP #$20
            BEQ dark
        next:
            DEX
            BPL scan
            PLA
            TAX
        plain:
            LDA a:FLASH
            RTS
        dark:
            PLA
            TAX
            LDA #$00
            RTS
    ";
    let len = crate::asm::assemble_at(0, hook_src)?.len();
    let hook = ctx.rom.alloc_vanilla(0, len)?;
    let mut a = Assembler::new();
    a.define("HOOK", hook);
    let out = a.assemble(&format!(
        ".org HOOK\n{hook_src}
        .org $9272
            JSR tb_flash
        "
    ))?;
    ctx.rom.apply_asm(0, &out)?;
    Ok(())
}

/// Small vanilla bugs that matter once enemies and items move around.
fn bug_fixes(ctx: &mut Ctx) -> Result<(), RandoError> {
    // Experience table entry 12 (low bytes from `$DDC0`, high bytes from
    // `$DDDC`, so `$DDCC` / `$DDE8`) is $012D = 301; make it 300. Skipped when the stats module
    // already rewrote it.
    patch(ctx, "300 experience", FIXED, 0xDDCC, &[0x2D], &[0x2C])?;
    // Horsehead's death clears enemy slots 5..0 (`LDX #$05`), wiping an
    // item that sits in a higher slot. Start from his own slot instead
    // (`$10`, already in A): `TAX : NOP`.
    patch(
        ctx,
        "Horsehead slot clear",
        4,
        0xBEB1,
        &[0xA2, 0x05],
        &[0xAA, 0xEA],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Flags;
    use crate::rng::Rng;
    use crate::rom::{Rom, VANILLA_BODY_LEN};
    use crate::spoiler::Spoiler;
    use crate::{Extras, State};

    /// A synthetic image carrying the vanilla bytes at every patch site.
    fn image() -> Vec<u8> {
        let mut body = vec![0u8; VANILLA_BODY_LEN];
        let mut put = |bank: u8, addr: u16, b: &[u8]| {
            let off = if addr >= 0xC000 {
                7 * 0x4000 + usize::from(addr - 0xC000)
            } else {
                usize::from(bank) * 0x4000 + usize::from(addr - 0x8000)
            };
            body[off..off + b.len()].copy_from_slice(b);
        };
        put(3, 0xB614, &[0xA9, 0x2A]);
        put(3, 0xB74D, &[0xA9, 0x05]);
        put(3, 0xB656, &[0xA0, 0x0B]);
        put(0, 0x8E05, &[0x8C, 0x4A, 0x07]);
        put(7, 0xD4D4, &[0xC9, 0x20]);
        put(6, 0x93B1, &[0xA9, 0x30]);
        put(7, 0xC9EA, &[0x12, 0x16, 0x2A, 0x16]);
        put(7, 0xCA08, &[0xA9, 0x0F]);
        put(0, 0xA9F0, &[0x12, 0x16, 0x2A, 0x16]);
        put(0, 0xA19F, &[0xA5, 0xF8, 0xC9, 0x88, 0xD0, 0x37]);
        put(0, 0xA1DD, &[0xA5, 0xF8, 0xC9, 0x88, 0xF0, 0xC2]);
        put(0, 0x9272, &[0xAD, 0x4B, 0x07]);
        put(7, 0xDDCC, &[0x2D]);
        put(4, 0xBEB1, &[0xA2, 0x05]);
        body
    }

    fn ctx(flags: Flags) -> Ctx {
        let rom = Rom::from_body(&image()).unwrap();
        Ctx {
            vanilla: rom.clone(),
            rom,
            rng: Rng::new(1),
            flags,
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        }
    }

    fn rd(c: &Ctx, bank: u8, addr: u16, n: usize) -> Vec<u8> {
        let off = c.rom.cpu_offset(bank, addr).unwrap();
        c.rom.read_slice(off, n).unwrap().to_vec()
    }

    #[test]
    fn default_flags_change_nothing() {
        let mut c = ctx(Flags::default());
        apply(&mut c).unwrap();
        assert_eq!(c.rom.body(), image());
    }

    #[test]
    fn every_option_patches_its_sites() {
        let mut f = Flags::default();
        f.qol.fast_text = true;
        f.qol.fast_spell_casting = true;
        f.qol.beep_threshold = BeepThreshold::TwoBars;
        f.qol.beep_frequency = BeepFrequency::QuarterSpeed;
        f.qol.remove_flashing = true;
        f.qol.up_a_on_controller_1 = true;
        f.qol.darken_thunderbird = true;
        f.qol.bug_fixes = true;
        let mut c = ctx(f);
        apply(&mut c).unwrap();
        assert_eq!(rd(&c, 3, 0xB615, 1), [0]);
        assert_eq!(rd(&c, 3, 0xB74E, 1), [0]);
        assert_eq!(rd(&c, 3, 0xB657, 1), [0]);
        assert_eq!(rd(&c, 0, 0x8E05, 3), [0xEA; 3]);
        assert_eq!(rd(&c, 7, 0xD4D4, 2), [0xC9, 0x40]);
        assert_eq!(rd(&c, 6, 0x93B2, 1), [0xC0]);
        assert_eq!(rd(&c, 7, 0xC9EA, 4), [0x12; 4]);
        assert_eq!(rd(&c, 7, 0xCA08, 2), [0xA9, 0x12]);
        assert_eq!(rd(&c, 0, 0xA9F0, 4), [0x12; 4]);
        assert_eq!(rd(&c, 7, 0xDDCC, 1), [0x2C]);
        assert_eq!(rd(&c, 4, 0xBEB1, 2), [0xAA, 0xEA]);
        // Up+A: JSR into vanilla free space in bank 0, branch kept.
        let a = rd(&c, 0, 0xA19F, 6);
        assert_eq!(a[0], 0x20);
        let hook = u16::from_le_bytes([a[1], a[2]]);
        assert!((0xAA40..0xBF70).contains(&hook), "{hook:04X}");
        assert_eq!(&a[3..], [0xD0, 0x38, 0xEA]); // BNE $A1DC from $A1A4
        let b = rd(&c, 0, 0xA1DD, 6);
        assert_eq!(&b[..3], &a[..3]);
        assert_eq!(&b[3..], [0xF0, 0xC3, 0xEA]); // BEQ $A1A5 from $A1E2
        assert_eq!(
            rd(&c, 0, hook, 11),
            [0xA5, 0xF8, 0xC9, 0x88, 0xF0, 0x04, 0xA5, 0xF7, 0xC9, 0x28, 0x60]
        );
        // Thunderbird: the flash load is a JSR to a different hook.
        let t = rd(&c, 0, 0x9272, 3);
        assert_eq!(t[0], 0x20);
        let tb = u16::from_le_bytes([t[1], t[2]]);
        assert_ne!(tb, hook);
        assert_eq!(rd(&c, 0, tb, 5), [0xAD, 0x07, 0x07, 0xC9, 0x05]);
    }

    #[test]
    fn beep_off_wins_over_the_threshold() {
        let mut f = Flags::default();
        f.qol.beep_threshold = BeepThreshold::HalfBar;
        f.qol.beep_frequency = BeepFrequency::Off;
        let mut c = ctx(f);
        apply(&mut c).unwrap();
        assert_eq!(rd(&c, 7, 0xD4D4, 2), [0x38, 0xEA]);
        assert_eq!(rd(&c, 6, 0x93B2, 1), [0x30]);
    }

    #[test]
    fn a_moved_site_is_skipped_and_logged() {
        let mut f = Flags::default();
        f.qol.bug_fixes = true;
        let mut c = ctx(f);
        c.rom.write_cpu(7, 0xDDCC, &[0x99]).unwrap();
        apply(&mut c).unwrap();
        assert_eq!(rd(&c, 7, 0xDDCC, 1), [0x99]);
        assert!(c.log.iter().any(|l| l.contains("300 experience")));
    }

    #[test]
    fn beep_values() {
        assert_eq!(beep_threshold_value(BeepThreshold::Normal), 0x20);
        assert_eq!(beep_threshold_value(BeepThreshold::QuarterBar), 0x08);
        assert_eq!(beep_period_value(BeepFrequency::HalfSpeed), 0x60);
    }

    /// ROM-gated: the patch sites hold the vanilla bytes the module expects,
    /// every option applies on several seeds, and only the intended fixed
    /// bank bytes change (none inside a ported routine).
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_all_qol_options() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let mut f = Flags::default();
        f.qol.fast_text = true;
        f.qol.fast_spell_casting = true;
        f.qol.beep_threshold = BeepThreshold::QuarterBar;
        f.qol.beep_frequency = BeepFrequency::HalfSpeed;
        f.qol.remove_flashing = true;
        f.qol.up_a_on_controller_1 = true;
        f.qol.darken_thunderbird = true;
        f.qol.bug_fixes = true;
        for seed in ["1", "qol", "another seed"] {
            let out = crate::randomize(&body, seed, &f).unwrap();
            assert!(
                !out.log.iter().any(|l| l.contains("skipped")),
                "{:?}",
                out.log
            );
            assert_eq!(out.prg_units, 8, "no PRG expansion needed");
            let want: Vec<u16> = vec![0xC9EB, 0xC9EC, 0xC9ED, 0xCA09, 0xD4D5, 0xDDCC];
            assert_eq!(out.fixed_bank_changes, want);
            let a = randomize_body_byte(&out.body, 0, 0x8E05);
            assert_eq!(a, 0xEA);
        }
        let a = crate::randomize(&body, "x", &f).unwrap();
        let b = crate::randomize(&body, "x", &f).unwrap();
        assert_eq!(a.body, b.body);
    }

    fn randomize_body_byte(body: &[u8], bank: u8, addr: u16) -> u8 {
        body[usize::from(bank) * 0x4000 + usize::from(addr - 0x8000)]
    }
}
