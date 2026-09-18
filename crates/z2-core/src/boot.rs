//! Boot-path logic: power-on reset, title-intro sequencing, file-select
//! entry, new-game init.
//!
//! Self-contained pure logic over explicit params (no `Game` dep), following
//! the [`crate::title_flow`]/[`crate::palace`] pattern: callers stage ROM
//! bytes, these fns model the decision/effect halves. Trap entry points are
//! listed in [`crate::boot_traps::BOOT_TRAPS`].
//!
//! | fn / const | label | addr | status |
//! |---|---|---|
//! | [`poweron_e0_span`] / [`POWERON_E0_FILL`] | `bank5_PowerON__Reset_Memory` | bank 5 `$A6A0` | ported |
//! | [`poweron_apu`] | `bank5_PowerON__Reset_Memory` APU span | bank 5 `$A6AE` | ported |
//! | [`poweron_ppu`] | `bank5_PowerON__Reset_Memory` PPU span | bank 5 `$A6B9` | ported |
//! | [`classify_slot`] | `bank5_code27` | bank 5 `$B960` | ported |
//! | [`ADE0_STORES`] | `bank5_code_ADE0` | bank 5 `$ADE0` | ported |
//! | [`code20_flute`] | `bank5_code20` | bank 5 `$A70F` | ported |
//! | [`INTRO_SPRITE_COPY_LEN`] | `bank5_code21` | bank 5 `$A8C1` | ported |
//! | [`sprite0_pass`] / [`ppuctrl_post_wait`] | `LA737` / `LAB6D` spin | bank 5 `$A73D`/`$AB73` | ported |
//! | [`intro_tick`] | `LA737` FC/E8 tail | bank 5 `$A767` | ported |
//! | [`title_start_edge`] / [`TITLE_START_*`] | `LA795` / `LA7AB` | bank 5 `$A795`/`$A7AB` | ported |
//! | [`title_scroll_tick`] | `LAB6D` `$ABA8-$ABE5` | bank 5 `$ABA8` | ported |
//! | [`laf1f_step`] | `LAF1F` | bank 5 `$AF1F` | ported |
//! | [`la6d9_target`] | `LA6D9` | bank 5 `$A6D9` | ported |
//! | [`lc722_step`] / [`lc72d_step`] | `LC722` / `LC72D` | bank 7 `$C722`/`$C72D` | ported |
//! | [`BEGIN_GAME_*`] | `startup_init_begin_game` | bank 0 `$AA08` | ported |
//! | [`code2_advance`] | `bank7_code2` | bank 7 `$C1B6` | ported (descriptor) |
//! | [`FILESEL_ENTRY`] | file-select entry (no label) | bank 5 `$B22D` | descriptor |
//! | [`bank5_A610`] path | `bank5_A610` NMI entry | bank 5 `$A610` | descriptor (interp) |
//!
//! # Preserved quirks
//!
//! * `$A6A8` stores *entry A* (not a constant) across `$E0-$FF`; it reads 0
//!   only via the `$D281`/`$D29C` zeroing chain ([`POWERON_E0_FILL`]).
//! * `bank5_code21` copies a full 256-byte page (the `$A829 $FF`
//!   terminator byte included) and exits with `Y = $FF`.
//! * `LA795` fires only on a *changed + Start-held* edge (releasing Start
//!   does not fire — unlike the game-over `LCA85` handler which fires on
//!   any Start/Select change).
//! * `LA781` runs `LA795` a *second* time after `INC $076C` (the Start edge
//!   is checked twice on the title-take path).
//! * `LAF1F`'s jump table is inline instruction bytes: entry 0 decodes the
//!   `ROR $D2` opcode pair as target `$D266` (erase name tables).
//! * `LC722` *increments* `$0726` while `LC72D` *clears* it (asymmetric).
//! * `startup_init_begin_game` leaves `X = $FF` in `$0708` (the `LAA28`
//!   `DEX`/`BPL` loop exits at `X = $FF`) and its `$A97F → $6957` name copy
//!   runs `Y = $88..1`, skipping index 0 (`BNE` exits at `Y = 0`).
//!
//! # Gaps (honest)
//!
//! * PPU/APU side effects inside these spans (name-table erases, OAM DMA,
//!   `$4015` traffic, `SwapCHR`/`SwapPRG`, sound-engine calls) stay with the
//!   interpreter; only the RAM/decision halves are modelled here.
//! * `LAB6D` past `$ABE5` is raw table bytes under a misaligned
//!   disassembly (`JSR L07AD` decodes the `LDA $2007` stream one byte off;
//!   verified against ROM bytes) — the bytes are data, not code.
//! * `LA6D9` stages `2+` read handler bytes (`AND`/`LDX`/`CPY`/`BRK`, …)
//!   as table entries; only stages 0/1 have known targets.
//! * `bank7_related_to_sound` (`$C1C1`: `SwapPRG` 6 + `JSR $9000` +
//!   `SwapToPRG0`) is sound-engine scope — descriptor only.

// ---------------------------------------------------------------------------
// bank5_PowerON__Reset_Memory ($A6A0).
// ---------------------------------------------------------------------------

/// First/last zero-page address of the `$A6A6` clear loop
/// (`LDY #$1F : STA $E0,y : DEY : BPL`, 32 bytes).
pub const POWERON_E0_SPAN: (u16, u16) = (0x00E0, 0x00FF);

/// Byte range of [`POWERON_E0_SPAN`] as `(base, len)`.
pub const POWERON_E0_BASE: u16 = 0x00E0;
/// Length of the `$E0-$FF` clear (`$1F..$00` = 32 stores).
pub const POWERON_E0_LEN: usize = 0x20;

/// Fill byte the `$A6A8` loop stores: *entry A*, which is 0 on the
/// composed path (`JSR bank5_code27` preserves `A`, then
/// `bank7_Reset_Memory_Ranges` `$D281` exits with `A = 0` via `$D29C`).
pub const POWERON_E0_FILL: u8 = 0x00;

/// Span helper: `(base, len)` of the `$E0-$FF` clear.
pub const fn poweron_e0_span() -> (u16, usize) {
    (POWERON_E0_BASE, POWERON_E0_LEN)
}

/// APU span (`$A6AE-$A6B6`): `STA $4011` (DMC raw, entry A = 0),
/// `LDA #$0F : STA $4015 : STA $076B`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerOnApu {
    /// Stored to `$4011` and across `$E0-$FF` (entry A).
    pub entry_a: u8,
    /// Stored to `$4015` (sound-channel switch).
    pub snd_chn: u8,
    /// Stored to `$076B`.
    pub r76b: u8,
}

/// Build the APU-span effect for an entry `A` (0 on the composed path).
pub const fn poweron_apu(entry_a: u8) -> PowerOnApu {
    PowerOnApu {
        entry_a,
        snd_chn: 0x0F,
        r76b: 0x0F,
    }
}

/// PPU span (`$A6B9-$A6D8`): `PPU_MASK = 0`, `INC $0726`, `$0768 = 6`,
/// `$0100 = $80`, `$FF = PPU_CTRL = $B0` (NMI armed, sprite pattern high).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerOnPpu {
    /// PPU mask byte (`$2001`).
    pub mask: u8,
    /// `$0768` effect byte.
    pub r768: u8,
    /// NMI gate latch (`$0100`): bit 7 set = NMI path, bit 6 clear =
    /// bank-5 handler (`LC077` → `bank5_A610`).
    pub r100: u8,
    /// Sprite-bank mirror (`$FF`) = PPU control byte (`$2000`).
    pub ff: u8,
}

/// Build the PPU-span effect (constant; no inputs).
pub const fn poweron_ppu() -> PowerOnPpu {
    PowerOnPpu {
        mask: 0x00,
        r768: 0x06,
        r100: 0x80,
        ff: 0xB0,
    }
}

// ---------------------------------------------------------------------------
// bank5_code27 ($B960): SRAM slot validation.
// ---------------------------------------------------------------------------

/// Slots validated per reset (`LDX #$02`, `DEX : BPL`, `$B960-$B9A6`).
pub const SRAM_SLOTS: u8 = 3;

/// Fresh-slot stamp (`LDA #$A5 : STA ($0C)`, `$B99D`).
pub const SRAM_FRESH_STAMP: u8 = 0xA5;

/// What `bank5_code27` does with one slot, keyed on its header byte
/// (`LDA ($0C),y : CMP #$A5 / #$5A / #$69`, `$B968-$B974`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotProbe {
    /// Header `$A5`: keep the slot (`LB9A3`, next slot).
    Keep,
    /// Header `$5A`: stamp `$A5` and re-derive (`LB99D` → `LB9A3`).
    InitBackup,
    /// Header `$69`: restore from backup (`LB9A7`).
    RestoreBackup,
    /// Any other header: fresh init from `bank5_Beginning_Values`
    /// (`LB978`: `$32` bytes `($00)` + `($06)→($02)` until
    /// `($06,$07) == ($F5,$BB)`).
    Fresh,
}

/// Classify one slot header byte.
pub const fn classify_slot(header: u8) -> SlotProbe {
    match header {
        0xA5 => SlotProbe::Keep,
        0x5A => SlotProbe::InitBackup,
        0x69 => SlotProbe::RestoreBackup,
        _ => SlotProbe::Fresh,
    }
}

/// Fresh-init copy length (`LDY #$31`, `$B976`: `$31..$00` = 50 bytes).
pub const BEGIN_COPY_LEN: usize = 0x32;

/// Fresh-init backup-copy terminator (`CMP #$F5` / `CMP #$BB`,
/// `$B991-$B999`).
pub const BACKUP_COPY_END: (u8, u8) = (0xF5, 0xBB);

// ---------------------------------------------------------------------------
// bank5_code_ADE0 ($ADE0): intro position init.
// ---------------------------------------------------------------------------

/// (`address`, `value`) stores of `bank5_code_ADE0` (`$ADE0-$AE14`).
pub const ADE0_STORES: &[(u16, u8)] = &[
    (0x0027, 0x2A),
    (0x0029, 0x20),
    (0x002B, 0x28),
    (0x002D, 0x2B),
    (0x002F, 0x23),
    (0x0028, 0x00),
    (0x002A, 0x00),
    (0x002C, 0x00),
    (0x0031, 0x00),
    (0x0032, 0x00),
    (0x0033, 0x00),
    (0x0761, 0x00),
    (0x0747, 0x00),
    (0x00FC, 0x00),
    (0x002E, 0xC0),
    (0x0030, 0xC0),
    (0x0036, 0x02),
];

// ---------------------------------------------------------------------------
// bank5_code20 ($A70F): title-entry setup.
// ---------------------------------------------------------------------------

/// MMC1 control byte installed by `bank5_code20` (`LDA #$0F`, `$A712`).
pub const CODE20_MMC1: u8 = 0x0F;
/// CHR bank installed by `bank5_code20` (`LDA #$00 : JSR SwapCHR`, `$A726`).
pub const CODE20_CHR: u8 = 0x00;
/// Sound-switch value when the flute path is cold (`LDA #$01 : STA $EA`).
pub const CODE20_SOUND_ON: u8 = 0x01;

/// Flute-gate half (`$A717-$A720`): with `$0568 != 0` the `EA` write and
/// the `INC $0568` are both skipped. Returns `(ea_write, r568_next)`.
pub const fn code20_flute(r568: u8) -> (Option<u8>, u8) {
    if r568 != 0 {
        (None, r568)
    } else {
        (Some(CODE20_SOUND_ON), r568.wrapping_add(1))
    }
}

// ---------------------------------------------------------------------------
// bank5_code21 ($A8C1): intro-sprite page copy.
// ---------------------------------------------------------------------------

/// Bytes copied `$A7C1 → $0200` (`LDY #$FF` wrap loop, `$A8C1-$A8CE`):
/// the full page, `$A829 $FF` terminator included.
pub const INTRO_SPRITE_COPY_LEN: usize = 256;

/// `Y` on loop exit (wraps `$00 → $FF`, `CPY #$FF` equal).
pub const INTRO_SPRITE_EXIT_Y: u8 = 0xFF;

// ---------------------------------------------------------------------------
// LA737 ($A737) / LAB6D ($AB6D): sprite-0 wait + PPUCTRL update.
// ---------------------------------------------------------------------------

/// Vblank/spin-wait pass predicate (`BIT $2002 : BVC`, `$A73D`/`$AB73`):
/// the wait exits iff bit 6 (sprite-0 hit) is set.
pub const fn sprite0_pass(status: u8) -> bool {
    status & 0x40 != 0
}

/// Post-wait PPU control merge (`LDA $FF : AND #$FC : ORA $36`,
/// `$A74C`/`$AB82`).
pub const fn ppuctrl_post_wait(ff: u8, r36: u8) -> u8 {
    (ff & 0xFC) | r36
}

/// Pre-spin delay iterations (`LDY #$00 : NOP : DEY : BNE`, `$A737`).
pub const SPIN_DELAY_A: u32 = 256;
/// Post-spin delay iterations (`LDY #$00 : DEY : BNE`, `$A742`).
pub const SPIN_DELAY_B: u32 = 256;
/// Short delay iterations (`LDY #$4A : DEY : BNE`, `$A747`).
pub const SPIN_DELAY_C: u32 = 0x4A;

/// Intro-anim tick out of `LA737`'s `$A767-$A794` tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntroTick {
    /// Next `$FC` (scroll-Y driver).
    pub fc: u8,
    /// Whether `$0736` increments (title take, `LA781`).
    pub mode_inc: bool,
    /// Whether control jumps to `bank5_code_ABF7` (skips the second
    /// `LA795` check).
    pub to_abf7: bool,
    /// Whether `$0504` is forced to `$80` (`$FC == $60` arm).
    pub timer504: Option<u8>,
}

/// Model the `$A767-$A794` tail: `LDA $FC : CMP #$60` selects the
/// `$0504`/`$E8` arm, else `$E8 == 2 && frame & 3 == 0` creeps `$FC`.
/// (`LDA $2002` / `LDA $2007` dummy reads are side-effect-only PPU
/// traffic and leave no RAM state.)
pub const fn intro_tick(fc: u8, e8: u8, frame: u8) -> IntroTick {
    if fc == 0x60 {
        // `LDA #$80 : STA $0504 : JSR LA795`, then `$E8 == 8 → LA781`.
        if e8 == 0x08 {
            IntroTick {
                fc,
                mode_inc: true,
                to_abf7: false,
                timer504: Some(0x80),
            }
        } else {
            IntroTick {
                fc,
                mode_inc: false,
                to_abf7: true,
                timer504: Some(0x80),
            }
        }
    } else if e8 == 0x02 && frame & 0x03 == 0 {
        IntroTick {
            fc: fc.wrapping_add(1),
            mode_inc: false,
            to_abf7: false,
            timer504: None,
        }
    } else {
        IntroTick {
            fc,
            mode_inc: false,
            to_abf7: false,
            timer504: None,
        }
    }
}

// ---------------------------------------------------------------------------
// LA795 ($A795) / LA7AB ($A7AB): title Start edge.
// ---------------------------------------------------------------------------

/// Start-button mask tested by `LA795` (`AND #$10`).
pub const TITLE_START_MASK: u8 = 0x10;

/// Title Start edge (`LDA $F7 : CMP $0744 : BEQ skip : AND #$10 :
/// BEQ skip`, `$A795-$A79E`): fires on a *changed* held set that
/// includes Start. Releasing Start (`held = 0`) never fires.
pub const fn title_start_edge(held: u8, prev: u8) -> bool {
    held != prev && held & TITLE_START_MASK != 0
}

/// Sound-switch value installed on title take (`LDA #$80 : STA $EA`).
pub const TITLE_TAKE_SOUND: u8 = 0x80;

/// Bytes cleared on title take (`STA $0727/$0761/$0747/$0568/$073E`).
pub const TITLE_TAKE_CLEARS: [u16; 5] = [0x0727, 0x0761, 0x0747, 0x0568, 0x073E];

// ---------------------------------------------------------------------------
// LAB6D ($ABA8-$ABE5): title scroll/state machine.
// ---------------------------------------------------------------------------

/// Title-screen tick out of `LAB6D`'s tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleScroll {
    /// `$0504 != 0`: jump to `bank5_code_ABF7` (`$ABA8`).
    ToAbf7,
    /// Settle at `LAC06` (no state change on this path).
    Settle,
    /// Continue at `LABE5` (table-driven sprite work — interp-only gap).
    Continue,
    /// `$FC` (and maybe `$0761`/`$0747`) stepped; then settle or continue
    /// per `$FC & 7`.
    Tick {
        /// Next `$FC`.
        fc: u8,
        /// Next `$0761` (`5` when the `$60` trip fires, else unchanged).
        r761: u8,
        /// Next `$0747` (flipped `^ $02` on `$F0` wrap, else unchanged).
        r747: u8,
        /// Whether the walk reaches `LABE5` (`$FC & 7 == 0` or the
        /// `$0761`/`$31`/`$60` arms) rather than settling.
        cont: bool,
    },
}

/// Model `$ABA8-$ABE5`: `$0504 → ABF7`; `$3F → LABE5`; `frame & 3 → settle`;
/// else `INC $FC` with `$F0`-wrap (`$FC = 0`, `$747 ^= 2`); `$0761 → LABE5`;
/// `$31 → LABDF`; `$FC == $60` latches `$0761 = 5`; `$FC & 7 → settle`.
pub const fn title_scroll_tick(
    r504: u8,
    r3f: u8,
    r31: u8,
    fc: u8,
    frame: u8,
    r761: u8,
    r747: u8,
) -> TitleScroll {
    if r504 != 0 {
        return TitleScroll::ToAbf7;
    }
    if r3f != 0 {
        return TitleScroll::Continue;
    }
    if frame & 0x03 != 0 {
        return TitleScroll::Settle;
    }
    let (fc2, r747b) = if fc.wrapping_add(1) == 0xF0 {
        (0x00, r747 ^ 0x02)
    } else {
        (fc.wrapping_add(1), r747)
    };
    if r761 != 0 {
        return TitleScroll::Continue;
    }
    if r31 != 0 {
        return if fc2 & 0x07 == 0 {
            TitleScroll::Tick {
                fc: fc2,
                r761,
                r747: r747b,
                cont: true,
            }
        } else {
            TitleScroll::Tick {
                fc: fc2,
                r761,
                r747: r747b,
                cont: false,
            }
        };
    }
    if fc2 != 0x60 {
        return if fc2 & 0x07 == 0 {
            TitleScroll::Tick {
                fc: fc2,
                r761,
                r747: r747b,
                cont: true,
            }
        } else {
            TitleScroll::Tick {
                fc: fc2,
                r761,
                r747: r747b,
                cont: false,
            }
        };
    }
    // `$FC == $60` trip: `$0761 = 5`, then the `$FC & 7` test
    // (`$60 & 7 == 0` → continue).
    TitleScroll::Tick {
        fc: fc2,
        r761: 0x05,
        r747: r747b,
        cont: true,
    }
}

// ---------------------------------------------------------------------------
// LAF1F ($AF1F): story-screen $073D stepper.
// ---------------------------------------------------------------------------

/// One `LAF1F` step (`LDA $073D` through the inline table, `$AF1F-$AF4C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Laf1fStep {
    /// `$073D == 0`: erase name tables (`$D266` — entry 0 decodes the
    /// `ROR $D2` opcode bytes `$66 $D2` as the target address).
    EraseNt,
    /// `$073D == 1`: `$0725 = 1`, `$073D = 2` (`$AF2B`).
    Macro1,
    /// `$073D == 2`: `$0725 = 4`, `$0736++` (`$AF34`).
    Macro4ModeInc,
    /// Past the 3-entry table: hardware reads trailing bytes as code
    /// (gap — modelled as no-op rather than executing data).
    PastTable,
}

/// Step the story-screen state machine on `$073D`.
pub const fn laf1f_step(r73d: u8) -> Laf1fStep {
    match r73d {
        0 => Laf1fStep::EraseNt,
        1 => Laf1fStep::Macro1,
        2 => Laf1fStep::Macro4ModeInc,
        _ => Laf1fStep::PastTable,
    }
}

/// PPU-macro byte stored by the `$AF2B` arm.
pub const LAF1F_MACRO_1: u8 = 0x01;
/// PPU-macro byte stored by the `$AF34` arm.
pub const LAF1F_MACRO_4: u8 = 0x04;

// ---------------------------------------------------------------------------
// LA6D9 ($A6D9): boot-stage entry dispatch.
// ---------------------------------------------------------------------------

/// `bank5_code19` entry (dispatched when `$076C == 0`: the inline table's
/// first two bytes are the `BEQ LA687` opcode pair `$F0 $A6`, which
/// decodes as address `$A6F0`).
pub const LA6D9_STAGE0_TARGET: u16 = 0xA6F0;

/// File-select entry (dispatched when `$076C == 1`: bytes `$A6E1-$A6E2`
/// are the `AND LE5B2` opcode pair `$2D $B2`, decoding as `$B22D`).
pub const LA6D9_STAGE1_TARGET: u16 = 0xB22D;

/// Resolve the `LA6D9` inline-table target for a Boot Stage byte.
/// Stages 2+ index handler bytes (`LDX`/`CPY`/`BRK`/operands) whose
/// decoded targets are not code entries (gap: `None`).
pub const fn la6d9_target(stage: u8) -> Option<u16> {
    match stage {
        0 => Some(LA6D9_STAGE0_TARGET),
        1 => Some(LA6D9_STAGE1_TARGET),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// LC722 ($C722) / LC72D ($C72D): mode-advance tails.
// ---------------------------------------------------------------------------

/// `LC722` (`LDA #$00 : STA $073D : INC $0726 : JMP LCF05`): zero the
/// routine index, bump the dialog-hide counter, advance the game mode.
/// Returns `(r73d, r726_next, mode_next)`.
pub const fn lc722_step(r726: u8, mode: u8) -> (u8, u8, u8) {
    (0x00, r726.wrapping_add(1), mode.wrapping_add(1))
}

/// `LC72D` (`LDA #$00 : STA $0726 : JMP LCF05`): clear the dialog-hide
/// counter and advance the game mode. Returns `(r726, mode_next)`.
/// Note the asymmetry with [`lc722_step`]: clear here, increment there.
pub const fn lc72d_step(mode: u8) -> (u8, u8) {
    (0x00, mode.wrapping_add(1))
}

// ---------------------------------------------------------------------------
// startup_init_begin_game (bank 0 $AA08): new-game init.
// ---------------------------------------------------------------------------

/// Zero-page/RAM bytes the entry `A` is stored to (`STA $0738/$0706 /
/// $0707/$0561/$0748/$073F/$075C/$0701`, `$AA08-$AA1D`).
pub const BEGIN_GAME_A_STORES: [u16; 8] = [
    0x0738, 0x0706, 0x0707, 0x0561, 0x0748, 0x073F, 0x075C, 0x0701,
];

/// `X` init (`LDX #$01 : STX $075D : STX $075F`, `$AA20-$AA25`).
pub const BEGIN_GAME_X_INIT: u8 = 0x01;

/// `$0708` value (`STX $0708` after the `LAA28` loop exits with
/// `X = $FF`, `$AA2E`).
pub const BEGIN_GAME_0708: u8 = 0xFF;

/// Name-table copy top (`LDY #$88`, `$AA34`): copies `Y = $88..1`
/// (`$A97F → $6957`), skipping index 0.
pub const BEGIN_GAME_NAME_TOP: u8 = 0x88;

/// Source/dest of the name copy (`LDA LA97F,y : STA $6957,y`, `$AA36`).
pub const BEGIN_GAME_NAME_SRC: u16 = 0xA97F;
/// Name copy destination base.
pub const BEGIN_GAME_NAME_DST: u16 = 0x6957;

// ---------------------------------------------------------------------------
// bank7_code2 ($C1B6): sound-trigger accumulator (descriptor).
// ---------------------------------------------------------------------------

/// `bank7_code2` (`LDA $07AB : CLC : ADC #$D8 : STA $07AB : BCC LC1CC`,
/// `$C1B6-$C1BF`): accumulates `$D8` into `$07AB`; on carry falls into
/// `bank7_related_to_sound` (`$C1C1`: `SwapPRG` 6 + `JSR $9000` +
/// `SwapToPRG0` — sound-engine scope).
/// Returns `(r7ab_next, sound_triggered)`.
pub const fn code2_advance(r7ab: u8) -> (u8, bool) {
    let (next, carry) = r7ab.overflowing_add(0xD8);
    (next, carry)
}

// ---------------------------------------------------------------------------
// File-select entry (bank 5 $B22D) + bank5_A610 ($A610): descriptors.
// ---------------------------------------------------------------------------

/// File-select entry point (reached via `LA6D9` stage 1; no disassembly
/// label — the bytes sit past the `$B1DA` text table):
/// `JSR Remove_All_Sprites : JSR LD168 : JSR $D385` into
/// `bank5_table_B236` (`$B236`: entries `$B242`/`$B3CF`/`$B3FA`).
pub const FILESEL_ENTRY: u16 = 0xB22D;

/// File-select `$073B` table (`bank5_table_B236`, `$B236`, 3 entries).
pub const FILESEL_TABLE: [u16; 3] = [0xB242, 0xB3CF, 0xB3FA];

/// `bank5_A610` NMI bank-5 entry (`JMP bank5_A610` from `LC077`, `$C078`):
/// full-frame handler (PPU setup mirror, `$0725`-macro drain, bank-6
/// sound frame, input latch, `INC $12`, `JSR LA6D9`, `$2002`/`$FF|$80`
/// epilogue + `RTI`). Interpreted, not ported (PPU/sound-bound).
pub const BANK5_NMI_ENTRY: u16 = 0xA610;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poweron_spans_match_asm() {
        assert_eq!(poweron_e0_span(), (0x00E0, 32));
        assert_eq!(POWERON_E0_FILL, 0x00);
        let apu = poweron_apu(0);
        assert_eq!((apu.snd_chn, apu.r76b), (0x0F, 0x0F));
        let ppu = poweron_ppu();
        assert_eq!(
            (ppu.mask, ppu.r768, ppu.r100, ppu.ff),
            (0x00, 0x06, 0x80, 0xB0)
        );
    }

    #[test]
    fn slot_probe_headers() {
        assert_eq!(classify_slot(0xA5), SlotProbe::Keep);
        assert_eq!(classify_slot(0x5A), SlotProbe::InitBackup);
        assert_eq!(classify_slot(0x69), SlotProbe::RestoreBackup);
        assert_eq!(classify_slot(0x00), SlotProbe::Fresh);
        assert_eq!(SRAM_SLOTS, 3);
    }

    #[test]
    fn flute_gate_skips_when_set() {
        assert_eq!(code20_flute(1), (None, 1));
        assert_eq!(code20_flute(0), (Some(1), 1));
    }

    #[test]
    fn sprite0_predicate_is_bit6() {
        assert!(sprite0_pass(0x40));
        assert!(sprite0_pass(0xE0));
        assert!(!sprite0_pass(0x80));
        assert!(!sprite0_pass(0x00));
        assert_eq!(ppuctrl_post_wait(0xB0, 0x02), 0xB2);
        assert_eq!(ppuctrl_post_wait(0xB3, 0x02), 0xB2);
    }

    #[test]
    fn start_edge_needs_change_plus_held() {
        assert!(title_start_edge(0x10, 0x00));
        assert!(!title_start_edge(0x10, 0x10));
        assert!(!title_start_edge(0x00, 0x10));
        assert!(!title_start_edge(0x08, 0x00));
    }
}
