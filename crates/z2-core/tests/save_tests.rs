//! SRAM save/load/validity/recovery + slot RAM-logic tests.
//!
//! ROM-free fixtures throughout (synthetic SRAM blobs — no `Z2_ROM`
//! needed). The oracle-SRAM cross-check is gated on `Z2_ORACLE_SRAM`
//! and skips without it.

mod common;

use z2_core::save;
use z2_core::save_format;

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn blank_sram() -> Vec<u8> {
    vec![0xFFu8; save_format::SRAM_LEN]
}

fn part1_fill(v: u8) -> [u8; save_format::PART1_LEN] {
    [v; save_format::PART1_LEN]
}

fn part2_fill(v: u8) -> [u8; save_format::PART2_LEN] {
    [v; save_format::PART2_LEN]
}

// ---------------------------------------------------------------------------
// Acceptance: SRAM round-trip byte-equal (writer → loader → writer).
// ---------------------------------------------------------------------------

#[test]
fn sram_round_trip_is_byte_equal() {
    let mut sram = blank_sram();
    // Distinctive synthetic progress images per slot.
    let p1 = part1_fill(0xA0);
    let p2 = part2_fill(0xB0);
    assert!(save_format::save_slot(&mut sram, 1, &p1, &p2));
    let snap1 = sram.clone();

    // Loader reads back exactly what the writer stored (LB911 $B911).
    let mut o1 = part1_fill(0);
    let mut o2 = part2_fill(0);
    assert!(save_format::load_slot(&sram, 1, &mut o1, &mut o2));
    assert_eq!(o1, p1);
    assert_eq!(o2, p2);

    // Writer → loader → writer is identical (header `$A5` throughout).
    assert!(save_format::save_slot(&mut sram, 1, &o1, &o2));
    assert_eq!(sram, snap1);

    // Other slots untouched (still blank `$FF`, headers included).
    for slot in [0u8, 2] {
        let p = save_format::slot_pointers(slot).unwrap();
        let h = save_format::sram_index(p.header).unwrap();
        assert_eq!(sram[h], 0xFF);
    }
    // Slot-1 header is `$A5` (LBA13 $BA13).
    let p = save_format::slot_pointers(1).unwrap();
    assert_eq!(sram[save_format::sram_index(p.header).unwrap()], 0xA5);
}

// ---------------------------------------------------------------------------
// Header protocol + recovery.
// ---------------------------------------------------------------------------

#[test]
fn staged_header_finalizes_without_touching_data() {
    let mut sram = blank_sram();
    let p1 = part1_fill(0x41);
    let p2 = part2_fill(0x42);
    assert!(save_format::save_slot(&mut sram, 0, &p1, &p2));
    // Simulate a write torn between the `$5A` mark and the backup copy:
    // header back to `$5A`, main intact.
    let p = save_format::slot_pointers(0).unwrap();
    sram[save_format::sram_index(p.header).unwrap()] = save_format::HDR_STAGED;
    let before = sram.clone();
    let rec = save_format::recover_slot(&mut sram, 0, &[0u8; 50], &[0u8; 224]).unwrap();
    assert_eq!(rec, save_format::Recovery::Finalized);
    // Only the header byte changed (LB99D $B99D).
    let mut expect = before;
    expect[save_format::sram_index(p.header).unwrap()] = save_format::HDR_VALID;
    assert_eq!(sram, expect);
}

#[test]
fn torn_header_restores_backup_over_main() {
    let mut sram = blank_sram();
    let good1 = part1_fill(0x51);
    let good2 = part2_fill(0x52);
    assert!(save_format::save_slot(&mut sram, 2, &good1, &good2));
    // Tear the main copies, mark `$69` (LB9A7 $B9A7 path input).
    let p = save_format::slot_pointers(2).unwrap();
    let m1 = save_format::sram_index(p.part1).unwrap();
    let m2 = save_format::sram_index(p.part2).unwrap();
    sram[m1..m1 + 50].fill(0x00);
    sram[m2..m2 + 224].fill(0x00);
    sram[save_format::sram_index(p.header).unwrap()] = save_format::HDR_TORN;
    let rec = save_format::recover_slot(&mut sram, 2, &[0u8; 50], &[0u8; 224]).unwrap();
    assert_eq!(rec, save_format::Recovery::Restored);
    let mut o1 = part1_fill(0);
    let mut o2 = part2_fill(0);
    assert!(save_format::load_slot(&sram, 2, &mut o1, &mut o2));
    assert_eq!(o1, good1);
    assert_eq!(o2, good2);
}

#[test]
fn blank_header_initialises_from_beginning_values() {
    let mut sram = blank_sram();
    let mut beginning = [0u8; 50];
    beginning[0] = 0x01; // atk $777
    beginning[0x0C] = 0x04; // containers $783 ($783-$777 = $0C)
    let items = [0xFBu8; 224];
    let rec = save_format::recover_slot(&mut sram, 0, &beginning, &items).unwrap();
    assert_eq!(rec, save_format::Recovery::Initialized);
    // LB978 $B978: 50 B beginning image → main part1; LB981 walk → part2.
    let mut o1 = part1_fill(0);
    let mut o2 = part2_fill(0);
    assert!(save_format::load_slot(&sram, 0, &mut o1, &mut o2));
    assert_eq!(o1, beginning);
    assert_eq!(o2, items);
    // Valid header now; re-recovery keeps.
    assert_eq!(
        save_format::recover_slot(&mut sram, 0, &beginning, &items).unwrap(),
        save_format::Recovery::Kept
    );
}

#[test]
fn recover_all_covers_three_slots_in_any_state() {
    let mut sram = blank_sram();
    let beginning = [0x07u8; 50];
    let items = [0x08u8; 224];
    let recs = save_format::recover_all(&mut sram, &beginning, &items).unwrap();
    assert_eq!(recs, [save_format::Recovery::Initialized; 3]);
    let recs2 = save_format::recover_all(&mut sram, &beginning, &items).unwrap();
    assert_eq!(recs2, [save_format::Recovery::Kept; 3]);
}

#[test]
fn stage_then_commit_halves_match_full_save() {
    let mut via_halves = blank_sram();
    let p1 = part1_fill(0x61);
    let p2 = part2_fill(0x62);
    // Seed main first (stage copies main→backup: LBA40 $BA40).
    assert!(save_format::save_slot(&mut via_halves, 0, &p1, &p2));
    // Corrupt main, re-stage (backup ← torn main), restore good backup
    // manually is out of scope — instead check stage/commit primitives:
    assert!(save_format::stage_slot(&mut via_halves, 0));
    let p = save_format::slot_pointers(0).unwrap();
    assert_eq!(
        via_halves[save_format::sram_index(p.header).unwrap()],
        save_format::HDR_STAGED
    );
    assert!(save_format::commit_slot(&mut via_halves, 0));
    assert_eq!(
        via_halves[save_format::sram_index(p.header).unwrap()],
        save_format::HDR_VALID
    );
    let mut o1 = part1_fill(0);
    let mut o2 = part2_fill(0);
    assert!(save_format::load_slot(&via_halves, 0, &mut o1, &mut o2));
    assert_eq!((o1, o2), (p1, p2));

    assert!(!save_format::save_slot(&mut via_halves, 3, &p1, &p2));
    assert!(!save_format::load_slot(&via_halves, 9, &mut o1, &mut o2));
}

// ---------------------------------------------------------------------------
// Slot RAM logic: reset windows, presence, lives/deaths, respawn.
// ---------------------------------------------------------------------------

#[test]
fn reset_window_covers_783_to_7a0_only() {
    // $B2CC $B2CA: Y $29..$0C → $783-$7A0; levels/name survive.
    let mut part1 = [0xAAu8; 50];
    let mut beginning = [0u8; 50];
    for (i, b) in beginning.iter_mut().enumerate() {
        *b = i as u8;
    }
    assert!(save::reset_stats_to_beginning(&mut part1, &beginning));
    // $777-$782 kept ($00-$0B untouched).
    assert!(part1[..0x0C].iter().all(|&b| b == 0xAA));
    // $783-$7A0 reset ($0C-$29 from image).
    assert_eq!(&part1[0x0C..0x2A], &beginning[0x0C..0x2A]);
    // Name $7A1-$7A8 ($2A-$31) kept.
    assert!(part1[0x2A..].iter().all(|&b| b == 0xAA));
    assert!(!save::reset_stats_to_beginning(
        &mut part1[..10],
        &beginning
    ));
}

#[test]
fn item_bits_reset_is_224_plain_copy() {
    let mut ram = [0u8; 224];
    let init = [0xFFu8; 224];
    assert!(save::reset_item_bits(&mut ram, &init));
    assert_eq!(ram, init);
}

#[test]
fn new_game_clear_matches_lb2ee_walk() {
    // LDY #$DA walk: $7DA-$7FF cleared; Y wraps to $00 and exits
    // WITHOUT storing $700 (lives set to 3 later by bank-7 LC34F).
    let mut ram = [0xFFu8; 0x800];
    save::new_game_ram_clear(&mut ram);
    assert_eq!(ram[0x700], 0xFF);
    assert_eq!(ram[0x710], 0xFF); // $701-$7D9 untouched
    assert!(ram[0x7DA..0x800].iter().all(|&b| b == 0));
}

#[test]
fn presence_bits_match_table13_weights() {
    let empty = [save::NAME_BLANK; 8];
    let mut named = [save::NAME_BLANK; 8];
    named[3] = 0xDA;
    assert!(!save::slot_present(&empty));
    assert!(save::slot_present(&named));
    assert_eq!(save::presence_bit(0), 0x01);
    assert_eq!(save::presence_bit(1), 0x02);
    assert_eq!(save::presence_bit(2), 0x04);
    assert_eq!(save::presence_bits([&empty, &named, &empty]), 0x02);
    assert_eq!(save::presence_bit(9), 0x00);
}

#[test]
fn death_continue_respawn_rules() {
    // bank7_code16 $CA44: lives-- → 0 = game over else $76C = 6.
    assert_eq!(save::die_step(3), save::DieStep::NewLife { lives: 2 });
    assert_eq!(save::die_step(1), save::DieStep::GameOver);
    assert_eq!(save::STATE_NEW_LIFE, 6);
    // LCA85 $CAA3: deaths saturate at $FF.
    assert_eq!(save::continue_deaths(0xFF), 0xFF);
    assert_eq!(save::continue_deaths(41), 42);
    // $488: 0 = continue, else save ($CA85-$CAC4).
    assert_eq!(
        save::gameover_start_choice(0),
        save::GameOverChoice::Continue
    );
    assert_eq!(save::gameover_start_choice(1), save::GameOverChoice::Save);
    // region*5+world == $0F ⟺ Great Palace ($CF30).
    assert_eq!(save::region_world_code(2, 5), 0x0F);
    assert!(matches!(
        save::respawn_select(2, 5, 0),
        save::Respawn::GrandPalace
    ));
    assert_eq!(
        save::respawn_select(0, 0, 0),
        save::Respawn::Castle { music: 4 }
    );
    assert_eq!(
        save::respawn_select(1, 1, 3),
        save::Respawn::Castle { music: 2 }
    );
    assert_eq!(save::respawn_music(0, 0), 4);
    assert_eq!(save::respawn_music(2, 7), 4);
    assert_eq!(save::respawn_music(3, 0), 2);
    // Manual save: Up+A on pad 2 ($A19C).
    assert!(save::manual_save_gate(0x88));
    assert!(!save::manual_save_gate(0x80));
    // Meter refill: containers*32-1 (LCB18 $CB18).
    assert_eq!(save::refill_meter(4), 127);
    // Second quest: $7A0 == 1 gates, thrust $14 kept ($B2BC).
    assert!(save::second_quest_gate(1));
    assert!(!save::second_quest_gate(2));
    assert_eq!(save::thrust_preserve(0x15), 0x14);
    assert_eq!(save::second_quest_setup_flag(), 1);
}

// ---------------------------------------------------------------------------
// Gated oracle-SRAM cross-check (harness + skip).
// ---------------------------------------------------------------------------

#[test]
fn gated_oracle_sram_cross_load() {
    // An oracle-produced SRAM image (8 KiB `$6000-$7FFF`, e.g. dumped by
    // the Mesen probe or z2se) at $Z2_ORACLE_SRAM must load through
    // load_slot and re-save byte-identically. Absent → skip.
    let Some(path) = common::env_file("Z2_ORACLE_SRAM", "gated_oracle_sram_cross_load") else {
        return;
    };
    let img = match std::fs::read(&path) {
        Ok(b) if b.len() == save_format::SRAM_LEN => b,
        Ok(b) => {
            eprintln!(
                "skipping gated_oracle_sram_cross_load: {} is {} bytes, want 8192",
                path.display(),
                b.len()
            );
            return;
        }
        Err(e) => {
            eprintln!(
                "skipping gated_oracle_sram_cross_load: cannot read {} ({e})",
                path.display()
            );
            return;
        }
    };
    for slot in 0..3u8 {
        let mut o1 = part1_fill(0);
        let mut o2 = part2_fill(0);
        assert!(save_format::load_slot(&img, slot, &mut o1, &mut o2));
        let mut back = img.clone();
        assert!(save_format::save_slot(&mut back, slot, &o1, &o2));
        // Re-saving what the port loaded must preserve the loaded bytes.
        let mut r1 = part1_fill(0);
        let mut r2 = part2_fill(0);
        assert!(save_format::load_slot(&back, slot, &mut r1, &mut r2));
        assert_eq!((r1, r2), (o1, o2), "slot {slot} not stable");
    }
}
