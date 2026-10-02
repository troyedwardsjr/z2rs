//! `asm_features` module: original 6502 gameplay features.
//!
//! Options read (this module has no option struct of its own):
//!
//! * the resolved Fire option ([`crate::spells::SpellState::fire`], from
//!   `ctx.flags.spells.fire_option`): **Dash spell**;
//! * `ctx.flags.spells.dash_always_on`: **permanent dash speed**;
//! * `ctx.flags.enemies.randomize_knockback`, `ctx.flags.palaces.hard_bosses`
//!   and `ctx.flags.spells.flute_warp`: see README.md for which
//!   of these are implemented.
//!
//! All code here is written for [`crate::asm`] and placed in the vanilla
//! padding of the bank that runs it ([`crate::rom::Rom::alloc_vanilla`]), so
//! no bank switching is needed: the hooks are short `JSR` detours in bank 0
//! (the player-physics bank, which z2rs always runs as ROM code).
//!
//! # Dash (bank 0)
//!
//! Link's horizontal speed (`$70`, signed) is accelerated towards a cap
//! read from a two-entry table (right, left) at two compare sites in the
//! walking routine. Both compares become calls to our routines:
//!
//! * the cap is the dash cap while the Fire bit (`$10`) of the active-spell
//!   byte `$076F` is set (Dash spell), otherwise the vanilla cap;
//! * at the first compare, a speed already past the cap in the cap's
//!   direction is pulled back to the cap. Without this, a fast speed left
//!   over when the spell ends would keep accelerating (the vanilla loop
//!   only stops on an exact match) until it wrapped around;
//! * the walk-animation timer table is indexed by `speed / 8`; dash speeds
//!   would read past its end, so the index is clamped to the last entry.
//!
//! In Dash mode the sword no longer launches fireballs (the "Fire active"
//! test before the beam code never matches). "Dash always on" raises the
//! vanilla caps themselves and installs the same hooks.

use crate::spells::FireMode;
use crate::{asm, Ctx, RandoError};

/// Player-physics bank.
pub const PHYS_BANK: u8 = 0;
/// `CMP cap,Y` before accelerating (`LDA $70` precedes it).
pub const SPEED_CMP_1: u16 = 0x93FF;
/// `CMP cap,Y` after one acceleration step.
pub const SPEED_CMP_2: u16 = 0x940E;
/// Vanilla speed caps (right, left).
pub const SPEED_CAPS: u16 = 0x93B3;
/// `LDA anim,Y` with `Y = |speed| / 8`.
pub const ANIM_LOOKUP: u16 = 0x9458;
/// Walk-animation timer table.
pub const ANIM_TABLE: u16 = 0x93B7;
/// Last safe index into [`ANIM_TABLE`] (vanilla speeds use 0-3).
pub const ANIM_MAX_INDEX: u8 = 4;
/// `AND #$10` in the sword code: Fire active -> fireball instead of beam.
pub const SWORD_FIRE_TEST: u16 = 0x984B;
/// Dash speed caps (right, left): twice the vanilla walking speed.
pub const DASH_CAPS: [u8; 2] = [0x30, 0xD0];

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let dash_spell = ctx.state.spells.fire == FireMode::Dash;
    let dash_always = ctx.flags.spells.dash_always_on;
    let knockback = ctx.flags.enemies.randomize_knockback;
    // Only the known ROM gets these patches (synthetic test images skip
    // them, see `expect`).
    let speed = (dash_spell || dash_always || knockback) && install_speed_hooks(ctx, dash_spell)?;
    let sword = &[0x29, 0x10];
    if speed && dash_spell && expect(ctx, PHYS_BANK, SWORD_FIRE_TEST - 1, sword, "sword")? {
        ctx.rom.write_cpu(PHYS_BANK, SWORD_FIRE_TEST, &[0x00])?;
        ctx.spoiler
            .line("Spells", "Fire is replaced by Dash (double running speed)");
    }
    if speed && dash_always {
        ctx.rom.write_cpu(PHYS_BANK, SPEED_CAPS, &DASH_CAPS)?;
        ctx.spoiler.line("Spells", "Dash speed is always on");
    }
    if speed && knockback {
        install_recoil(ctx)?;
    }
    Ok(())
}

/// See [`crate::spells::site`]: `Ok(false)` skips the patch on an image
/// that is not the known ROM; an error flags a clash with another module.
fn expect(ctx: &Ctx, bank: u8, addr: u16, want: &[u8], what: &str) -> Result<bool, RandoError> {
    crate::spells::site(ctx, bank, addr, want, what)
}

/// Assemble `src` (which must start with `.org`) and write it to `bank`.
fn put(ctx: &mut Ctx, bank: u8, src: &str) -> Result<asm::Assembled, RandoError> {
    let out = asm::assemble(src)?;
    ctx.rom.apply_asm(bank, &out)?;
    Ok(out)
}

/// The dash speed routines (see the module docs).
pub fn dash_source(org: u16, dash_spell: bool) -> String {
    let caps = SPEED_CAPS;
    let dash_test = if dash_spell {
        // Fire bit of the active-spell byte.
        "        LDA $076F\n        AND #$10\n"
    } else {
        // No Dash spell: the dash caps are never used.
        "        LDA #$00\n"
    };
    format!(
        "
        .org ${org:04X}
; In: A = speed ($70), Y = 0 (right) / 1 (left).
; Out: flags of `speed CMP cap`, A = speed; $70 clamped to the cap if it
; was past it in the cap's direction. X is not touched.
speed_cmp_1:
{dash_test}        BEQ normal_1
        LDA dash_caps,Y
        JSR clamp
        LDA $70
        CMP dash_caps,Y
        RTS
normal_1:
        LDA ${caps:04X},Y
        JSR clamp
        LDA $70
        CMP ${caps:04X},Y
        RTS

; In: A = speed after one step, Y as above. Out: flags of `A CMP cap`, A kept.
speed_cmp_2:
        PHA
{dash_test}        BEQ normal_2
        PLA
        CMP dash_caps,Y
        RTS
normal_2:
        PLA
        CMP ${caps:04X},Y
        RTS

; A = cap. If $70 lies beyond it (same sign, larger magnitude) store the cap.
clamp:
        BIT $70
        BMI moving_left
        CMP #$80
        BCS clamp_done      ; cap points left: nothing to clamp
        CMP $70
        BCS clamp_done      ; cap >= speed
        STA $70
        RTS
moving_left:
        CMP #$80
        BCC clamp_done      ; cap points right
        CMP $70
        BCC clamp_done      ; cap < speed (speed nearer zero)
        STA $70
clamp_done:
        RTS

; Walk animation timer for Y = |speed| / 8, clamped to the table.
anim_lookup:
        CPY #${limit:02X}
        BCC anim_ok
        LDY #${max:02X}
anim_ok:
        LDA ${anim:04X},Y
        RTS

dash_caps:
        .byte ${r:02X}, ${l:02X}
",
        limit = ANIM_MAX_INDEX + 1,
        max = ANIM_MAX_INDEX,
        anim = ANIM_TABLE,
        r = DASH_CAPS[0],
        l = DASH_CAPS[1],
    )
}

/// The speed-cap and walk-animation hooks (see the module docs).
fn install_speed_hooks(ctx: &mut Ctx, dash_spell: bool) -> Result<bool, RandoError> {
    let b = PHYS_BANK;
    let mut ok = true;
    ok &= expect(
        ctx,
        b,
        SPEED_CMP_1 - 2,
        &[0xA5, 0x70, 0xD9],
        "speed compare 1",
    )?;
    ok &= expect(
        ctx,
        b,
        SPEED_CMP_1 + 1,
        &SPEED_CAPS.to_le_bytes(),
        "speed compare 1",
    )?;
    ok &= expect(ctx, b, SPEED_CMP_2, &[0xD9], "speed compare 2")?;
    ok &= expect(
        ctx,
        b,
        SPEED_CMP_2 + 1,
        &SPEED_CAPS.to_le_bytes(),
        "speed compare 2",
    )?;
    ok &= expect(ctx, b, ANIM_LOOKUP, &[0xB9], "walk animation lookup")?;
    ok &= expect(
        ctx,
        b,
        ANIM_LOOKUP + 1,
        &ANIM_TABLE.to_le_bytes(),
        "walk animation lookup",
    )?;
    if !ok {
        return Ok(false);
    }

    // Size the routine at a dummy origin, then place it.
    let len = asm::assemble(&dash_source(0x8000, dash_spell))?
        .bytes()
        .len();
    let at = ctx.rom.alloc_vanilla(b, len)?;
    let out = put(ctx, b, &dash_source(at, dash_spell))?;
    let sym = |n: &str| {
        out.symbol(n)
            .ok_or_else(|| RandoError::Asm(format!("missing symbol {n}")))
    };
    let hooks = format!(
        "
        .org ${SPEED_CMP_1:04X}
        JSR ${:04X}
        .org ${SPEED_CMP_2:04X}
        JSR ${:04X}
        .org ${ANIM_LOOKUP:04X}
        JSR ${:04X}
",
        sym("speed_cmp_1")?,
        sym("speed_cmp_2")?,
        sym("anim_lookup")?
    );
    put(ctx, b, &hooks)?;

    Ok(true)
}

// ---------------------------------------------------------------------------
// Chaotic knockback.
// ---------------------------------------------------------------------------

/// Fixed bank: `LDA #$FE / STA $057D`, Link's upward pop when he is hurt.
pub const HURT_POP: u16 = 0xE361;
/// Fixed bank: `LDA table,Y / STA $70`, Link's sideways recoil.
pub const RECOIL_X: u16 = 0xE3B3;
/// Fixed bank: `ASL $057D`, the strong bosses doubling the upward pop.
pub const BOSS_POP_DOUBLE: u16 = 0xE590;
/// Bank switch entry points in the fixed bank.
pub const SWAP_TO_PRG0: u16 = 0xFFC5;
/// See [`SWAP_TO_PRG0`].
pub const SWAP_PRG: u16 = 0xFFCC;
/// Same address in banks 0-6: unused `$FF` bytes after each bank's copy of
/// the reset stub (`$BFE0-$BFF9`). Each bank gets a 6-byte stub that pushes
/// its own number and maps bank 0; bank 0 continues at the same address
/// with the call to the handler and the switch back.
pub const TRAMPOLINE: u16 = 0xBFEC;
/// Enemy ids per area bank in the knockback tables.
pub const KNOCKBACK_IDS: usize = 0x24;
// Seven rows must stay indexable with one byte.
const _: () = assert!(7 * KNOCKBACK_IDS <= 256);

/// Per-enemy knockback for one seed: `[area bank][enemy id]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Knockback {
    /// Sideways speed (1-63; the vanilla hits use 4 and 13).
    pub x: Vec<u8>,
    /// Upward speed as a 16-bit negative number (vanilla about -$200).
    pub y: Vec<u16>,
}

/// Draw the tables: sideways speed uniform in `1..$40`, upward speed
/// uniform in `$50..$700` (tends to throw Link further than vanilla).
pub fn roll_knockback(rng: &mut crate::rng::Rng) -> Knockback {
    let n = 7 * KNOCKBACK_IDS;
    let mut kb = Knockback {
        x: Vec::with_capacity(n),
        y: Vec::with_capacity(n),
    };
    for _ in 0..n {
        kb.x.push(rng.range_u8(1, 0x3F));
        let up = rng.range(0x50, 0x6FF) as u16;
        kb.y.push(up.wrapping_neg());
    }
    kb
}

/// Bank-0 handler. In: X = the enemy (or projectile) slot that hit or was
/// hit, Y = 0 for the upward pop, else the vanilla sideways index (1-4,
/// odd pushes right).
pub fn recoil_source(org: u16) -> String {
    format!(
        "
        .org ${org:04X}
recoil:
        CPY #$00
        BEQ vertical
        TYA
        LSR A               ; C set: push right
        PHP
        JSR kb_index
        LDA kb_x,Y
        PLP
        BCS push_right
        EOR #$FF
        CLC
        ADC #$01
push_right:
        STA $70
        RTS
vertical:
        JSR kb_index
        LDA kb_ylo,Y
        STA $03E6
        LDA kb_yhi,Y
        STA $057D
        LDY $070F           ; what the vanilla path leaves in Y
        RTS
; Y = row of the current area bank + enemy id (ids past the table use 0).
kb_index:
        LDA $0769
        AND #$07
        TAY
        LDA $A1,X
        CMP #${ids:02X}
        BCC id_ok
        LDA #$00
id_ok:
        CLC
        ADC kb_base,Y
        TAY
        RTS
kb_base:
        .byte 0, {r1}, {r2}, {r3}, {r4}, {r5}, {r6}, 0
",
        ids = KNOCKBACK_IDS,
        r1 = KNOCKBACK_IDS,
        r2 = 2 * KNOCKBACK_IDS,
        r3 = 3 * KNOCKBACK_IDS,
        r4 = 4 * KNOCKBACK_IDS,
        r5 = 5 * KNOCKBACK_IDS,
        r6 = 6 * KNOCKBACK_IDS,
    )
}

fn byte_rows(label: &str, v: &[u8]) -> String {
    let mut s = format!("{label}:\n");
    for chunk in v.chunks(16) {
        let row: Vec<String> = chunk.iter().map(|b| format!("${b:02X}")).collect();
        s.push_str(&format!("        .byte {}\n", row.join(", ")));
    }
    s
}

fn install_recoil(ctx: &mut Ctx) -> Result<(), RandoError> {
    let mut ok = true;
    ok &= expect(
        ctx,
        7,
        HURT_POP,
        &[0xA9, 0xFE, 0x8D, 0x7D, 0x05],
        "hurt pop",
    )?;
    ok &= expect(ctx, 7, RECOIL_X, &[0xB9, 0x6C, 0xE3, 0x85, 0x70], "recoil")?;
    ok &= expect(ctx, 7, BOSS_POP_DOUBLE, &[0x0E, 0x7D, 0x05], "boss pop")?;
    for bank in 0..7u8 {
        let len = if bank == 0 { 13 } else { 6 };
        ok &= expect(
            ctx,
            bank,
            TRAMPOLINE,
            &vec![0xFF; len],
            "knockback trampoline space",
        )?;
    }
    if !ok {
        return Ok(());
    }
    let kb = roll_knockback(&mut ctx.rng);
    let lo: Vec<u8> = kb.y.iter().map(|v| *v as u8).collect();
    let hi: Vec<u8> = kb.y.iter().map(|v| (*v >> 8) as u8).collect();
    let tables = format!(
        "{}{}{}",
        byte_rows("kb_x", &kb.x),
        byte_rows("kb_ylo", &lo),
        byte_rows("kb_yhi", &hi)
    );
    let len = asm::assemble(&(recoil_source(0x8000) + &tables))?
        .bytes()
        .len();
    let at = ctx.rom.alloc_vanilla(PHYS_BANK, len)?;
    let out = put(ctx, PHYS_BANK, &(recoil_source(at) + &tables))?;
    let handler = out
        .symbol("recoil")
        .ok_or_else(|| RandoError::Asm("missing symbol recoil".into()))?;
    for bank in 0..7u8 {
        let mut src = format!(
            ".org ${TRAMPOLINE:04X}\n        LDA #${bank:02X}\n        PHA\n        JSR ${SWAP_TO_PRG0:04X}\n"
        );
        if bank == 0 {
            // Execution continues here after the switch, whatever bank
            // the stub ran in.
            src.push_str(&format!(
                "        JSR ${handler:04X}\n        PLA\n        JMP ${SWAP_PRG:04X}\n"
            ));
        }
        put(ctx, bank, &src)?;
    }
    let hooks = format!(
        "
        .org ${HURT_POP:04X}
        LDY #$00
        JSR ${TRAMPOLINE:04X}
        .org ${RECOIL_X:04X}
        JSR ${TRAMPOLINE:04X}
        NOP
        NOP
        .org ${BOSS_POP_DOUBLE:04X}
        NOP
        NOP
        NOP
"
    );
    put(ctx, 7, &hooks)?;
    ctx.spoiler.line(
        "Enemies",
        "Knockback: every enemy throws Link its own distance (chaotic knockback)",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_routines_assemble_with_their_symbols() {
        for spell in [false, true] {
            let out = asm::assemble(&dash_source(0xB000, spell)).unwrap();
            for s in ["speed_cmp_1", "speed_cmp_2", "anim_lookup", "dash_caps"] {
                assert!(out.symbol(s).is_some(), "{s}");
            }
            let bytes = out.bytes();
            assert!(bytes.len() < 120, "{}", bytes.len());
            // The animation clamp reloads the last safe index.
            let src = dash_source(0xB000, spell);
            assert!(src.contains(&format!("LDY #${ANIM_MAX_INDEX:02X}")));
            assert!(src.contains(&format!("CPY #${:02X}", ANIM_MAX_INDEX + 1)));
        }
    }

    #[test]
    fn knockback_tables_and_handler() {
        let mut rng = crate::rng::Rng::new(9);
        let kb = roll_knockback(&mut rng);
        assert_eq!(kb.x.len(), 7 * KNOCKBACK_IDS);
        assert!(kb.x.iter().all(|&x| (1..0x40).contains(&x)));
        assert!(kb
            .y
            .iter()
            .all(|&y| (0x50..0x700).contains(&y.wrapping_neg())));
        let tables = format!(
            "{}{}{}",
            byte_rows("kb_x", &kb.x),
            byte_rows("kb_ylo", &vec![0; kb.y.len()]),
            byte_rows("kb_yhi", &vec![0; kb.y.len()])
        );
        let out = asm::assemble(&(recoil_source(0xA000) + &tables)).unwrap();
        assert_eq!(out.symbol("recoil"), Some(0xA000));
        let x = usize::from(out.symbol("kb_x").unwrap() - 0xA000);
        assert_eq!(&out.bytes()[x..x + kb.x.len()], &kb.x[..]);
    }
}
