//! Boot-path pure-logic tests.
//!
//! ROM-free throughout: every assertion pins a [`z2_core::boot`] predicate
//! against its disassembly span (label + address in the test name).

use z2_core::boot::{
    self, Laf1fStep, SlotProbe, TitleScroll, BEGIN_COPY_LEN, BEGIN_GAME_0708, BEGIN_GAME_A_STORES,
    BEGIN_GAME_NAME_DST, BEGIN_GAME_NAME_SRC, BEGIN_GAME_NAME_TOP, BEGIN_GAME_X_INIT, CODE20_CHR,
    CODE20_MMC1, INTRO_SPRITE_COPY_LEN, INTRO_SPRITE_EXIT_Y, LAF1F_MACRO_1, LAF1F_MACRO_4,
    POWERON_E0_FILL, SPIN_DELAY_A, SPIN_DELAY_B, SPIN_DELAY_C, SRAM_FRESH_STAMP, SRAM_SLOTS,
    TITLE_START_MASK, TITLE_TAKE_CLEARS, TITLE_TAKE_SOUND,
};

// ---------------------------------------------------------------------------
// bank5_PowerON__Reset_Memory ($A6A0).
// ---------------------------------------------------------------------------

#[test]
fn poweron_e0_span_is_32_bytes() {
    assert_eq!(boot::poweron_e0_span(), (0x00E0, 32));
    assert_eq!(boot::POWERON_E0_SPAN, (0x00E0, 0x00FF));
    // Entry A (0 via the $D281/$D29C chain) is what `$A6A8` stores.
    assert_eq!(POWERON_E0_FILL, 0x00);
}

#[test]
fn poweron_apu_span_values() {
    // `$A6AE-$A6B6`: STA $4011 (entry A), LDA #$0F : STA $4015 : STA $076B.
    let apu = boot::poweron_apu(0);
    assert_eq!(apu.entry_a, 0x00);
    assert_eq!(apu.snd_chn, 0x0F);
    assert_eq!(apu.r76b, 0x0F);
}

#[test]
fn poweron_ppu_span_values() {
    // `$A6B9-$A6D8`: mask 0, INC $0726, $0768 = 6, $0100 = $80, $FF = $B0.
    let ppu = boot::poweron_ppu();
    assert_eq!(ppu.mask, 0x00);
    assert_eq!(ppu.r768, 0x06);
    assert_eq!(ppu.r100, 0x80);
    assert_eq!(ppu.ff, 0xB0);
}

// ---------------------------------------------------------------------------
// bank5_code27 ($B960).
// ---------------------------------------------------------------------------

#[test]
fn slot_probe_headers() {
    assert_eq!(boot::classify_slot(0xA5), SlotProbe::Keep);
    assert_eq!(boot::classify_slot(0x5A), SlotProbe::InitBackup);
    assert_eq!(boot::classify_slot(0x69), SlotProbe::RestoreBackup);
    for h in [0x00, 0x01, 0xA4, 0xFF] {
        assert_eq!(boot::classify_slot(h), SlotProbe::Fresh, "header {h:02X}");
    }
    assert_eq!(SRAM_SLOTS, 3);
    assert_eq!(SRAM_FRESH_STAMP, 0xA5);
    assert_eq!(BEGIN_COPY_LEN, 0x32);
    assert_eq!(boot::BACKUP_COPY_END, (0xF5, 0xBB));
}

// ---------------------------------------------------------------------------
// bank5_code_ADE0 ($ADE0) / bank5_code20 ($A70F) / bank5_code21 ($A8C1).
// ---------------------------------------------------------------------------

#[test]
fn ade0_stores_cover_all_17() {
    assert_eq!(boot::ADE0_STORES.len(), 17);
    assert!(boot::ADE0_STORES.contains(&(0x0027, 0x2A)));
    assert!(boot::ADE0_STORES.contains(&(0x0029, 0x20)));
    assert!(boot::ADE0_STORES.contains(&(0x00FC, 0x00)));
    assert!(boot::ADE0_STORES.contains(&(0x0036, 0x02)));
    assert!(boot::ADE0_STORES.contains(&(0x0761, 0x00)));
}

#[test]
fn code20_flute_gate() {
    // `$A717-$A720`: cold flute byte writes EA = 1 and bumps $0568.
    assert_eq!(boot::code20_flute(0), (Some(1), 1));
    // Warm path skips both.
    assert_eq!(boot::code20_flute(7), (None, 7));
    assert_eq!(CODE20_MMC1, 0x0F);
    assert_eq!(CODE20_CHR, 0x00);
}

#[test]
fn code21_copies_full_page() {
    // `$A8C1-$A8CE`: LDY #$FF wrap loop = 256 bytes, exits Y = $FF.
    assert_eq!(INTRO_SPRITE_COPY_LEN, 256);
    assert_eq!(INTRO_SPRITE_EXIT_Y, 0xFF);
}

// ---------------------------------------------------------------------------
// LA737 ($A737) / LAB6D ($AB6D).
// ---------------------------------------------------------------------------

#[test]
fn sprite0_wait_predicate_is_bit6() {
    // `BIT $2002 : BVC` ($A73D/$AB73): exit iff bit 6 set.
    assert!(boot::sprite0_pass(0x40));
    assert!(boot::sprite0_pass(0xFF));
    assert!(!boot::sprite0_pass(0x80));
    assert!(!boot::sprite0_pass(0x00));
    // Post-wait merge keeps low 2 bits from $36 only.
    assert_eq!(boot::ppuctrl_post_wait(0xB0, 0x02), 0xB2);
    assert_eq!(boot::ppuctrl_post_wait(0xFF, 0x00), 0xFC);
    assert_eq!((SPIN_DELAY_A, SPIN_DELAY_B, SPIN_DELAY_C), (256, 256, 0x4A));
}

#[test]
fn intro_tick_fc60_arm() {
    // `$A76A-$A77C`: FC == $60 forces $0504 = $80; E8 == 8 takes title.
    let t = boot::intro_tick(0x60, 0x08, 0);
    assert_eq!(t.timer504, Some(0x80));
    assert!(t.mode_inc);
    assert!(!t.to_abf7);
    // Any other E8 jumps to bank5_code_ABF7 instead.
    let t = boot::intro_tick(0x60, 0x01, 0);
    assert!(t.to_abf7);
    assert!(!t.mode_inc);
}

#[test]
fn intro_tick_fc_creep() {
    // `$A787-$A794`: E8 == 2 and frame & 3 == 0 creeps FC.
    let t = boot::intro_tick(0x10, 0x02, 4);
    assert_eq!(t.fc, 0x11);
    assert!(!t.mode_inc && !t.to_abf7 && t.timer504.is_none());
    // Wrong E8 or frame phase: no creep.
    assert_eq!(boot::intro_tick(0x10, 0x02, 5).fc, 0x10);
    assert_eq!(boot::intro_tick(0x10, 0x01, 4).fc, 0x10);
    assert_eq!(boot::intro_tick(0x10, 0x02, 4).fc, 0x11);
}

// ---------------------------------------------------------------------------
// LA795 ($A795) / LA7AB ($A7AB).
// ---------------------------------------------------------------------------

#[test]
fn title_start_edge_semantics() {
    assert_eq!(TITLE_START_MASK, 0x10);
    // Changed + Start held fires.
    assert!(boot::title_start_edge(0x10, 0x00));
    assert!(boot::title_start_edge(0x19, 0x09));
    // Steady hold does not re-fire; release never fires.
    assert!(!boot::title_start_edge(0x10, 0x10));
    assert!(!boot::title_start_edge(0x00, 0x10));
    assert!(!boot::title_start_edge(0x08, 0x00));
    assert_eq!(TITLE_TAKE_SOUND, 0x80);
    assert_eq!(TITLE_TAKE_CLEARS, [0x0727, 0x0761, 0x0747, 0x0568, 0x073E]);
}

// ---------------------------------------------------------------------------
// LAB6D title scroll ($ABA8-$ABE5).
// ---------------------------------------------------------------------------

#[test]
fn title_scroll_arms() {
    // `$ABA8`: $0504 nonzero → ABF7.
    assert_eq!(
        boot::title_scroll_tick(1, 0, 0, 0, 0, 0, 0),
        TitleScroll::ToAbf7
    );
    // `$ABAD`: $3F nonzero → LABE5.
    assert_eq!(
        boot::title_scroll_tick(0, 1, 0, 0, 0, 0, 0),
        TitleScroll::Continue
    );
    // `$ABB1`: frame & 3 nonzero → settle.
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0, 2, 0, 0),
        TitleScroll::Settle
    );
}

#[test]
fn title_scroll_fc_walk() {
    // Plain creep then settle (`$FC & 7 != 0 → LAC06`).
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0x10, 0, 0, 0),
        TitleScroll::Tick {
            fc: 0x11,
            r761: 0,
            r747: 0,
            cont: false
        }
    );
    // `$FC & 7 == 0` continues at LABE5.
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0x17, 0, 0, 0),
        TitleScroll::Tick {
            fc: 0x18,
            r761: 0,
            r747: 0,
            cont: true
        }
    );
    // `$F0` wrap clears FC and flips `$747 ^ $02`.
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0xEF, 0, 0, 0x02),
        TitleScroll::Tick {
            fc: 0x00,
            r761: 0,
            r747: 0x00,
            cont: true
        }
    );
    // `$60` trip latches `$0761 = 5` (`$60 & 7 == 0` → continue).
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0x5F, 0, 0, 0),
        TitleScroll::Tick {
            fc: 0x60,
            r761: 0x05,
            r747: 0,
            cont: true
        }
    );
    // `$0761` set forces the LABE5 arm (checked before the trip).
    assert_eq!(
        boot::title_scroll_tick(0, 0, 0, 0x10, 0, 3, 0),
        TitleScroll::Continue
    );
}

// ---------------------------------------------------------------------------
// LAF1F ($AF1F) / LA6D9 ($A6D9).
// ---------------------------------------------------------------------------

#[test]
fn laf1f_steps() {
    assert_eq!(boot::laf1f_step(0), Laf1fStep::EraseNt);
    assert_eq!(boot::laf1f_step(1), Laf1fStep::Macro1);
    assert_eq!(boot::laf1f_step(2), Laf1fStep::Macro4ModeInc);
    assert_eq!(boot::laf1f_step(3), Laf1fStep::PastTable);
    assert_eq!((LAF1F_MACRO_1, LAF1F_MACRO_4), (0x01, 0x04));
}

#[test]
fn la6d9_targets() {
    // Stage 0 → bank5_code19 ($A6F0: BEQ bytes F0 A6).
    assert_eq!(boot::la6d9_target(0), Some(0xA6F0));
    // Stage 1 → file-select entry ($B22D: AND bytes 2D B2).
    assert_eq!(boot::la6d9_target(1), Some(0xB22D));
    assert_eq!(boot::la6d9_target(2), None);
}

// ---------------------------------------------------------------------------
// LC722 ($C722) / LC72D ($C72D).
// ---------------------------------------------------------------------------

#[test]
fn lc722_increments_lc72d_clears() {
    // LC722: zero $073D, INC $0726, INC mode.
    assert_eq!(boot::lc722_step(1, 0), (0x00, 2, 1));
    // LC72D: clear $0726, INC mode (asymmetric by design).
    assert_eq!(boot::lc72d_step(2), (0x00, 3));
    assert_eq!(boot::lc722_step(0xFF, 0xFF), (0x00, 0x00, 0x00));
}

// ---------------------------------------------------------------------------
// startup_init_begin_game (bank 0 $AA08).
// ---------------------------------------------------------------------------

#[test]
fn begin_game_shape() {
    // Entry A fans out to 8 bytes; X = 1; $0708 ends $FF (loop-exit X).
    assert_eq!(BEGIN_GAME_A_STORES.len(), 8);
    assert!(BEGIN_GAME_A_STORES.contains(&0x0738));
    assert!(BEGIN_GAME_A_STORES.contains(&0x0701));
    assert_eq!(BEGIN_GAME_X_INIT, 0x01);
    assert_eq!(BEGIN_GAME_0708, 0xFF);
    // Name copy: $88..1 ($A97F → $6957), index 0 skipped.
    assert_eq!(BEGIN_GAME_NAME_TOP, 0x88);
    assert_eq!(BEGIN_GAME_NAME_SRC, 0xA97F);
    assert_eq!(BEGIN_GAME_NAME_DST, 0x6957);
}

// ---------------------------------------------------------------------------
// bank7_code2 ($C1B6) + file-select/bank5 descriptors.
// ---------------------------------------------------------------------------

#[test]
fn code2_advance_carry_gates_sound() {
    // `$07AB + $D8` without carry: no sound frame.
    assert_eq!(boot::code2_advance(0x00), (0xD8, false));
    // Carry (e.g. $7AB = $30) falls into bank7_related_to_sound.
    assert_eq!(boot::code2_advance(0x30), (0x08, true));
}

#[test]
fn filesel_and_nmi_descriptors() {
    assert_eq!(boot::FILESEL_ENTRY, 0xB22D);
    assert_eq!(boot::FILESEL_TABLE, [0xB242, 0xB3CF, 0xB3FA]);
    assert_eq!(boot::BANK5_NMI_ENTRY, 0xA610);
}
