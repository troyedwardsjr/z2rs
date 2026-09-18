//! Title / file-select / name-entry / death / ending tests.
//!
//! ROM-free fixtures throughout; ROM-gated descriptor checks take
//! caller-staged bytes via `Z2_ROM` and skip gracefully without it.
//! The power-on→credits movie check is gated on an out-of-tree movie
//! file and reports absence (no `corpus/` in this checkout).

mod common;

use z2_core::palace;
use z2_core::save;
use z2_core::title;
use z2_core::title_flow;

// ---------------------------------------------------------------------------
// File select: advance / settle / start.
// ---------------------------------------------------------------------------

#[test]
fn file_advance_wraps_mod_5() {
    assert_eq!(title_flow::file_advance(0), 1);
    assert_eq!(title_flow::file_advance(3), 4);
    assert_eq!(title_flow::file_advance(4), 0);
}

#[test]
fn file_settle_skips_occupied_slots() {
    // LB319 $B319: empty slot 0 parks at $40.
    assert_eq!(title_flow::file_settle(0, 0x00), (0, 0x40));
    // Occupied slot 0 advances to slot 1 ($58) when free.
    assert_eq!(title_flow::file_settle(0, 0x01), (1, 0x58));
    // Slots 0-1 occupied → slot 2 ($70).
    assert_eq!(title_flow::file_settle(0, 0x03), (2, 0x70));
    // All occupied → REGISTER ($90), since presence != 0 (LB35F $B35F).
    assert_eq!(title_flow::file_settle(0, 0x07), (3, 0x90));
    // Cursor already past slots is left alone…
    assert_eq!(title_flow::file_settle(4, 0x00), (4, 0xA8));
    // …except all-full parks back at REGISTER (LB375 $B375: DEC $19).
    assert_eq!(title_flow::file_settle(4, 0x07), (3, 0x90));
    assert_eq!(title_flow::file_settle(3, 0x00), (4, 0xA8));
}

#[test]
fn file_start_dispatch_matches_lb28b() {
    assert_eq!(title_flow::file_start(3), title_flow::FileStart::Register);
    assert_eq!(
        title_flow::file_start(4),
        title_flow::FileStart::Elimination
    );
    assert_eq!(
        title_flow::file_start(0),
        title_flow::FileStart::LoadSlot(0)
    );
    assert_eq!(
        title_flow::file_start(2),
        title_flow::FileStart::LoadSlot(2)
    );
}

// ---------------------------------------------------------------------------
// Register screen.
// ---------------------------------------------------------------------------

#[test]
fn register_advance_wraps_mod_4_and_start_dispatch() {
    assert_eq!(title_flow::register_advance(2), 3);
    assert_eq!(title_flow::register_advance(3), 0);
    assert_eq!(
        title_flow::register_start(3),
        title_flow::RegisterStart::Back
    );
    assert_eq!(
        title_flow::register_start(1),
        title_flow::RegisterStart::EnterName(1)
    );
}

#[test]
fn register_settle_parks_on_first_occupied_or_end() {
    // LB6C6 $B6C6: presence arrives with the $08 latch already ORed.
    // No saves → END ($78).
    assert_eq!(title_flow::register_settle(0, 0x08), (3, 0x78));
    // Slot 1 named → park there ($48).
    assert_eq!(title_flow::register_settle(0, 0x08 | 0x02), (1, 0x48));
    // Slot 0 named → park at 0 ($30).
    assert_eq!(title_flow::register_settle(0, 0x08 | 0x01), (0, 0x30));
    // END always parks.
    assert_eq!(title_flow::register_settle(3, 0x08), (3, 0x78));
}

// ---------------------------------------------------------------------------
// Elimination screen.
// ---------------------------------------------------------------------------

#[test]
fn elim_start_and_advance() {
    assert_eq!(
        title_flow::elim_start(3),
        title_flow::ElimStart::BackToRegister
    );
    assert_eq!(
        title_flow::elim_start(2),
        title_flow::ElimStart::Eliminate(2)
    );
    assert_eq!(title_flow::elim_advance(3), 0);
}

#[test]
fn elim_settle_parks_on_empty_or_end() {
    // LB4BC $B4BC: empty slot 0 parks ($30).
    assert_eq!(title_flow::elim_settle(0, 0x00), (0, Some(0x30)));
    // Occupied 0 → slot 1.
    assert_eq!(title_flow::elim_settle(0, 0x01), (1, Some(0x48)));
    // All occupied → END (doubled bit 8 always misses code26 presence).
    assert_eq!(title_flow::elim_settle(0, 0x07), (3, Some(0x78)));
}

#[test]
fn eliminate_slot_writes_backup_only() {
    let mut sram = vec![0xFFu8; z2_core::save_format::SRAM_LEN];
    // Seed main with a marker so we can prove it is untouched.
    for b in sram.iter_mut() {
        *b = 0x11;
    }
    let beginning = [0x22u8; z2_core::save_format::PART1_LEN];
    let items = [0x33u8; z2_core::save_format::PART2_LEN];
    assert!(title_flow::eliminate_slot(&mut sram, 1, &beginning, &items));
    // Backup regions match the init images (bank5_elimination_mode $B462).
    let p = z2_core::save_format::slot_pointers(1).unwrap();
    let b1 = z2_core::save_format::sram_index(p.bak1).unwrap();
    let b2 = z2_core::save_format::sram_index(p.bak2).unwrap();
    assert!(sram[b1..b1 + 50].iter().all(|&b| b == 0x22));
    assert!(sram[b2..b2 + 224].iter().all(|&b| b == 0x33));
    // Main untouched (propagation is the later commit-all, $B3DF).
    let m1 = z2_core::save_format::sram_index(p.part1).unwrap();
    assert!(sram[m1..m1 + 50].iter().all(|&b| b == 0x11));
    assert!(!title_flow::eliminate_slot(
        &mut sram, 3, &beginning, &items
    ));
}

// ---------------------------------------------------------------------------
// Name entry grid.
// ---------------------------------------------------------------------------

#[test]
fn grid_left_right_up_down_vectors() {
    use z2_core::save::{grid_down, grid_left, grid_right, grid_up};
    // Borrowing into row 2 caps at col 5 ($B7DB-$B7E1).
    assert_eq!(grid_left(0, 3), (5, 2));
    assert_eq!(grid_left(0, 2), (10, 1));
    assert_eq!(grid_left(0, 0), (10, 3));
    assert_eq!(grid_left(5, 1), (4, 1));
    assert_eq!(grid_up(3, 0), (3, 3));
    assert_eq!(grid_up(7, 2), (7, 1));
    assert_eq!(grid_up(2, 2), (2, 1));
    assert_eq!(grid_down(7, 2), (7, 3));
    assert_eq!(grid_down(2, 3), (2, 0));
    assert_eq!(grid_right(10, 3), (0, 0));
    assert_eq!(grid_right(10, 0), (0, 1));
    assert_eq!(grid_right(5, 2), (0, 3));
    assert_eq!(grid_right(4, 2), (5, 2));
}

#[test]
fn name_store_index_and_letter_pos_match_lb8ae() {
    // $1E == 0 writes slot 7, else pos-1 ($B8A8-$B8AE).
    assert_eq!(save::name_store_index(0), 7);
    assert_eq!(save::name_store_index(1), 0);
    assert_eq!(save::name_store_index(8), 7);
    assert_eq!(save::letter_pos_advance(7), 0);
    assert_eq!(save::letter_pos_advance(3), 4);
    // letter_index = col + row*11 ($B89C).
    assert_eq!(save::letter_index(0, 0), 0);
    assert_eq!(save::letter_index(3, 1), 14);
    assert_eq!(save::letter_index(10, 3), 43);
    // 43-byte table: index 43 (row 3, col 10) is past the end —
    // hardware reads $BDA1 $FF padding (gap: None = blank).
    let letters = [0xDAu8; 43];
    assert_eq!(save::letter_tile(&letters, 0), Some(0xDA));
    assert_eq!(save::letter_tile(&letters, 43), None);
}

// ---------------------------------------------------------------------------
// Title Start, game-over wait, lives screen, story skip, credits, ending.
// ---------------------------------------------------------------------------

#[test]
fn title_start_press_increments_state() {
    // LA7AB $A7BD: title 0 → 1.
    assert_eq!(title_flow::title_start_press(0), 1);
    assert_eq!(title_flow::TITLE_START_CLEARS, [0x0727, 0x0761]);
}

#[test]
fn gameover_wait_advance_matches_lca72() {
    assert!(title_flow::gameover_wait_advance(true, 0x50));
    assert!(title_flow::gameover_wait_advance(false, 0));
    assert!(!title_flow::gameover_wait_advance(false, 0x50));
}

#[test]
fn lives_digit_tile_adds_d0() {
    assert_eq!(title_flow::lives_digit_tile(3), 0xD3);
    assert_eq!(title_flow::LIVES_SCREEN_TIMER, 0x70);
}

#[test]
fn story_skip_ticks_on_held_start() {
    assert_eq!(title_flow::story_skip_tick(true, 5), 6);
    assert_eq!(title_flow::story_skip_tick(false, 5), 5);
}

#[test]
fn credits_walk_is_18_then_done() {
    assert_eq!(title::CREDITS_COUNT, 18);
    assert_eq!(title_flow::credits_next(17), None);
    let mut page = 0u8;
    for expect in 1..18u8 {
        page = title_flow::credits_next(page).unwrap();
        assert_eq!(page, expect);
    }
    assert_eq!(title_flow::credits_next(page), None);
}

#[test]
fn ending_chain_reuses_palace_states() {
    // $76C = 3 wake (STA $A6EC) → 4 credits (INC $9244).
    assert_eq!(title_flow::wake_zelda_state(), palace::STATE_WAKE_ZELDA);
    assert_eq!(title_flow::wake_zelda_state(), 3);
    assert_eq!(
        title_flow::credits_state_from_wake(3),
        palace::STATE_CREDITS
    );
    assert!(title_flow::ending_chain(3));
    assert!(title_flow::ending_chain(4));
    assert!(!title_flow::ending_chain(1));
}

#[test]
fn seq_tables_cite_ledger_addrs() {
    assert_eq!(title_flow::TITLE_SEQ[4], ("LAB6D", 0xAB6D));
    assert_eq!(title_flow::FILE_SEQ[2], ("LB678", 0xB678));
    assert_eq!(title_flow::FILE_SEQ[3], ("bank5_code23", 0xB3DF));
    assert_eq!(title_flow::ELIM_SEQ[2], ("bank5_code25", 0xB425));
    assert_eq!(title::FILE_CURSOR_Y, [0x40, 0x58, 0x70, 0x90, 0xA8]);
    assert_eq!(title::REG_CURSOR_Y, [0x30, 0x48, 0x60, 0x78]);
}

// ---------------------------------------------------------------------------
// ROM-gated descriptor checks (skip gracefully without Z2_ROM).
// ---------------------------------------------------------------------------

/// Read the PRG bytes for `(bank, cpu_addr, len)` from `Z2_ROM`, or `None`
/// (skip) without it. iNES: 16-byte header + 16 KiB banks; bank `b` maps
/// `$8000-$BFFF` at `16 + b*0x4000 + (addr - 0x8000)`.
fn prg_slice(bank: u8, addr: u16, len: u16) -> Option<Vec<u8>> {
    let img = common::rom_bytes("title_tests prg_slice")?;
    if img.len() < 16 || img[0..4] != *b"NES\x1A" {
        eprintln!("SKIP title ROM check: bad iNES magic");
        return None;
    }
    let off = 16 + bank as usize * 0x4000 + (addr as usize - 0x8000);
    if img.len() < off + len as usize {
        eprintln!("SKIP title ROM check: ROM too short");
        return None;
    }
    Some(img[off..off + len as usize].to_vec())
}

#[test]
fn rom_selection_text_and_letters_match_descriptors() {
    let Some(head) = prg_slice(5, title::SELECT_TEXT.addr, 4) else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // `20 6A 0B` + `EC` (SELECT…) — bank5_Tables_for_Selection_Screen_Text_.
    assert_eq!(head, vec![0x20, 0x6A, 0x0B, 0xEC]);
    let letters = prg_slice(5, title::LETTERS.addr, 4).unwrap();
    assert_eq!(letters, vec![0xDA, 0xDB, 0xDC, 0xDD]);
    let over = prg_slice(0, title::GAMEOVER_TEXT.addr, 3).unwrap();
    assert_eq!(over, vec![0x21, 0x6B, 0x0A]);
    let pal = prg_slice(0, title::GANON_PAL.addr, 4).unwrap();
    assert_eq!(pal, vec![0x3F, 0x00, 0x08, 0x16]);
    let cred = prg_slice(5, title::CREDITS_TABLE.addr, 2).unwrap();
    // First word = bank5_End_Credits $927D (little-endian).
    assert_eq!(cred, vec![0x7D, 0x92]);
}

#[test]
fn rom_beginning_values_match_documented_scalars() {
    let Some(vals) = prg_slice(5, 0xBAE3, 42) else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    assert_eq!(vals[0x777 - 0x777], save::BEGIN_ATK_MAG_LIFE); // atk
    assert_eq!(vals[0x778 - 0x777], save::BEGIN_ATK_MAG_LIFE); // magic
    assert_eq!(vals[0x779 - 0x777], save::BEGIN_ATK_MAG_LIFE); // life
    assert_eq!(vals[0x783 - 0x777], save::BEGIN_CONTAINERS);
    assert_eq!(vals[0x784 - 0x777], save::BEGIN_CONTAINERS);
    assert_eq!(vals[0x794 - 0x777], save::BEGIN_CRYSTALS);
    // Thrust/magic-key/quest words start 0.
    assert_eq!(vals[0x796 - 0x777], 0);
    assert_eq!(vals[0x7A0 - 0x777], 0);
}

// ---------------------------------------------------------------------------
// Gated power-on→credits movie check (reports absence).
// ---------------------------------------------------------------------------

#[test]
fn gated_movie_power_on_to_credits() {
    // Full-movie replay needs the out-of-tree TAS corpus (`corpus/movies/`,
    // gitignored) plus the wired oracle harness.
    // Report and skip.
    for dir in ["corpus/movies", "../corpus/movies", "corpus"] {
        if std::path::Path::new(dir).exists() {
            let n = std::fs::read_dir(dir).map(|r| r.count()).unwrap_or(0);
            eprintln!("movie dir {dir} present ({n} entries) — full power-on→credits replay runs under `xtask verify`, skipping");
            return;
        }
    }
    eprintln!("SKIP movie power-on→credits: no corpus/movies in this checkout (the corpus is out-of-tree and gitignored)");
}
