//! Palace trap-table + shim + snapshot/movie harness.
//!
//! Synthetic `Game` tests always run (ROM-free fixtures). Snapshot and
//! movie replays are harness-ready and skip gracefully without
//! `Z2_ROM` / `corpus/` / movie files.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;
use z2_core::palace_traps::{
    pc_barrier_gate, pc_crystal_flight, pc_crystal_place, pc_crystal_refill, pc_darklink_phase,
    pc_darklink_setup, pc_ending, pc_item_grant, pc_key_pickup, pc_locked_door, pc_palace_entry,
    pc_stone_stamp, pc_thunder_door, pc_thunder_wake, pc_triforce, register_palace_traps,
    PALACE_TRAPS, PALACE_TRAP_COUNT,
};

#[test]
fn palace_trap_table_is_sorted_unique_and_plausible() {
    assert!(!PALACE_TRAPS.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for (name, bank, addr) in PALACE_TRAPS {
        assert!(!name.is_empty());
        assert!(*addr >= 0x8000, "{name} ${addr:04X} outside PRG");
        assert!(seen.insert((*name, *bank, *addr)), "duplicate {name}");
        // All palace code is slot-swapped bank 4/5 → data-only (aliasing).
        assert_eq!(*bank, None, "{name} must stay bank-None");
    }
    // No fixed-bank entries today; count pins the registration set.
    assert_eq!(PALACE_TRAP_COUNT, 0);
    let fixed = PALACE_TRAPS.iter().filter(|(_, b, _)| b.is_some()).count();
    assert_eq!(fixed, PALACE_TRAP_COUNT);
}

#[test]
fn register_palace_traps_registers_nothing_data_only() {
    let mut game = Game::new();
    assert_eq!(game.traps.len(), 0);
    register_palace_traps(&mut game);
    assert_eq!(game.traps.len(), 0, "bank-4/5 traps stay data-only");
    // Idempotent.
    register_palace_traps(&mut game);
    assert_eq!(game.traps.len(), 0);
}

#[test]
fn palace_entry_shim_selects_world_byte() {
    let mut game = Game::new();
    game.ram[0x056C] = 1;
    pc_palace_entry(&mut game);
    assert_eq!(game.ram[0x0000], 3);
    game.ram[0x056C] = 4;
    pc_palace_entry(&mut game);
    assert_eq!(game.ram[0x0000], 4);
    // Unknown codes hold.
    game.ram[0x0000] = 0xAA;
    game.ram[0x056C] = 9;
    pc_palace_entry(&mut game);
    assert_eq!(game.ram[0x0000], 0xAA);
}

#[test]
fn key_and_door_shims_follow_the_ledger() {
    let mut game = Game::new();
    game.ram[0x0793] = 0x03;
    pc_key_pickup(&mut game);
    assert_eq!(game.ram[0x0793], 0x04);
    assert_eq!(game.ram[0x00EB], 0x02);
    // Boss-lock clears on pickup.
    game.ram[0x0728] = 1;
    pc_key_pickup(&mut game);
    assert_eq!(game.ram[0x0728], 0);
    // Door consumes.
    game.ram[0x0010] = 0;
    game.ram[0x078C] = 0;
    game.ram[0x0793] = 0x02;
    pc_locked_door(&mut game);
    assert_eq!(game.ram[0x0793], 0x01);
    assert_eq!(game.ram[0x00AF], 1);
    // Empty-handed without the magic key holds.
    game.ram[0x0793] = 0;
    game.ram[0x00AF] = 0;
    pc_locked_door(&mut game);
    assert_eq!(game.ram[0x00AF], 0);
    // Magic key bypasses without decrementing.
    game.ram[0x078C] = 1;
    pc_locked_door(&mut game);
    assert_eq!(game.ram[0x00AF], 1);
    assert_eq!(game.ram[0x0793], 0);
}

#[test]
fn item_grant_shim_sets_flag_row_and_keys() {
    let mut game = Game::new();
    game.ram[0x0010] = 0;
    // Raft (palace 3 item) lands in the flag row.
    game.ram[0x00AF] = 0x02;
    pc_item_grant(&mut game);
    assert_eq!(game.ram[0x0787], 0x01);
    // Key code bumps $0793.
    game.ram[0x00AF] = 0x08;
    game.ram[0x0793] = 0x01;
    pc_item_grant(&mut game);
    assert_eq!(game.ram[0x0793], 0x02);
}

#[test]
fn crystal_shims_place_fly_and_refill() {
    let mut game = Game::new();
    game.ram[0x0010] = 0;
    // Placement gate: decor idle, crystals left, touch + grounded.
    game.ram[0x00C9] = 0;
    game.ram[0x0794] = 6;
    game.ram[0x00A8] = 0x10;
    game.ram[0x00A7] = 0x04;
    game.ram[0x0706] = 0;
    game.ram[0x056C] = 1;
    pc_crystal_place(&mut game);
    assert_eq!(game.ram[0x0794], 5);
    assert_eq!(game.ram[0x002A], 0xA0);
    assert_eq!(game.ram[0x0080], 0x03);
    // slot = 0 + 1 + 1 = 2 → $078E holds 2.
    assert_eq!(game.ram[0x078E], 2);
    // Blocked without touch.
    game.ram[0x00A8] = 0x00;
    game.ram[0x0794] = 5;
    pc_crystal_place(&mut game);
    assert_eq!(game.ram[0x0794], 5);
    // Flight seats on $62.
    game.ram[0x002A] = 0x63;
    pc_crystal_flight(&mut game);
    assert_eq!(game.ram[0x002A], 0x62);
    assert_eq!(game.ram[0x0767], 0x62);
    assert_eq!(game.ram[0x00EB], 0x40);
    // Non-seating rise just moves.
    game.ram[0x002A] = 0xA0;
    game.ram[0x00EB] = 0x00;
    pc_crystal_flight(&mut game);
    assert_eq!(game.ram[0x002A], 0x9F);
    assert_eq!(game.ram[0x00EB], 0x00);
    // Refill stages $FF when the boss key is untaken.
    game.ram[0x07FB] = 0;
    pc_crystal_refill(&mut game);
    assert_eq!(game.ram[0x070C], 0xFF);
    assert_eq!(game.ram[0x070D], 0xFF);
}

#[test]
fn stone_stamp_swaps_palace_tiles_in_staged_window() {
    let mut game = Game::new();
    game.wram[0x1C00] = 0x60;
    game.wram[0x1C01] = 0x63;
    game.wram[0x1C02] = 0x6D;
    pc_stone_stamp(&mut game);
    assert_eq!(game.wram[0x1C00], 0x56);
    assert_eq!(game.wram[0x1C01], 0x59);
    assert_eq!(game.wram[0x1C02], 0x6D);
    assert_eq!(game.ram[0x0002], 2);
}

#[test]
fn barrier_and_thunder_shims_gate_on_context() {
    let mut game = Game::new();
    game.ram[0x0010] = 0;
    // Barrier opens with all crystals placed, page 0, x >= $C0.
    game.ram[0x00AF] = 0;
    game.ram[0x0794] = 0;
    game.ram[0x00C9] = 0;
    game.ram[0x003B] = 0;
    game.ram[0x004D] = 0xC0;
    pc_barrier_gate(&mut game);
    assert_eq!(game.ram[0x00AF], 3);
    assert_eq!(game.ram[0x0080], 3);
    // One crystal left holds.
    game.ram[0x00AF] = 0;
    game.ram[0x0794] = 1;
    pc_barrier_gate(&mut game);
    assert_eq!(game.ram[0x00AF], 0);
    // Thunder doors slam off page 0.
    game.ram[0x0728] = 0;
    game.ram[0x072A] = 2;
    pc_thunder_door(&mut game);
    assert_eq!(game.ram[0x0728], 1);
    assert_eq!(game.ram[0x0504], 0x90);
    // Page 0 holds.
    game.ram[0x0728] = 0;
    game.ram[0x072A] = 0;
    pc_thunder_door(&mut game);
    assert_eq!(game.ram[0x0728], 0);
    // Wake latches through the kernel on the Thunder strike.
    game.ram[0x0000] = 0;
    game.ram[0x0001] = 0x80; // negative = $6E3F struck
    pc_thunder_wake(&mut game);
    assert_eq!(game.ram[0x0000], 1);
    assert_eq!(game.ram[0x00D9], 0x0A);
}

#[test]
fn darklink_triforce_ending_shims_chain() {
    let mut game = Game::new();
    game.ram[0x0010] = 0;
    // Setup plants a grounded Link on later pages.
    game.ram[0x072A] = 2;
    game.ram[0x00A7] = 0x04;
    pc_darklink_setup(&mut game);
    assert_eq!(game.ram[0x0728], 1);
    assert_eq!(game.ram[0x0029], 0xA0);
    // Phase dispatch records the class; flash + $81 selects macro $0D.
    game.ram[0x0063] = 2;
    game.ram[0x074B] = 0x81;
    pc_darklink_phase(&mut game);
    assert_eq!(game.ram[0x0002], 2);
    assert_eq!(game.ram[0x0725], 0x0D);
    // Triforce + ending: 1 → 3 → 4.
    pc_triforce(&mut game);
    assert_eq!((game.ram[0x0002], game.ram[0x0003]), (0xD2, 0x01));
    game.ram[0x076C] = 1;
    pc_ending(&mut game);
    assert_eq!(game.ram[0x076C], 3);
    pc_ending(&mut game);
    assert_eq!(game.ram[0x076C], 4);
}

// ---------------------------------------------------------------------------
// Gated: palace-entrance / item-room / boss snapshots verify clean.
// ---------------------------------------------------------------------------

#[test]
fn palace_snapshots_verify_clean_when_present() {
    let Some(rd) = common::corpus_snapshots("palace corpus snapshots") else {
        return;
    };
    let snaps: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("palace-entrance-")
                    || n.starts_with("palace-item-room-")
                    || n.starts_with("palace-boss-")
            })
        })
        .collect();
    if snaps.is_empty() {
        eprintln!("SKIP: no palace-entrance/item-room/boss snapshots (need corpus/ dir)");
        return;
    }
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
        assert!(b.len() >= 8 + 2048 + 8192, "{}: too small", p.display());
    }
}

// ---------------------------------------------------------------------------
// Gated: ROM-oracle diffs skip gracefully without Z2_ROM.
// ---------------------------------------------------------------------------

#[test]
fn palace_rom_oracle_diffs_when_present() {
    let Some(rom) = common::rom_path("palace rom image") else {
        return;
    };
    let b = std::fs::read(&rom).expect("read ROM");
    assert!(!b.is_empty(), "Z2_ROM must be non-empty");
}

// ---------------------------------------------------------------------------
// Gated: any% + warpless to credits (report absence of movie files).
// ---------------------------------------------------------------------------

#[test]
fn any_percent_to_credits_when_present() {
    let Some(movie) = common::env_path_file(
        "Z2_ANY_PERCENT_MOVIE",
        "corpus/movies/any-percent.fm2",
        "palace any% movie",
    ) else {
        return;
    };
    let meta = std::fs::metadata(&movie).expect("movie metadata");
    assert!(meta.len() > 0, "any% movie must be non-empty");
    let mut game = Game::new();
    register_palace_traps(&mut game);
    assert_eq!(game.traps.len(), PALACE_TRAP_COUNT);
}

#[test]
fn warpless_to_credits_when_present() {
    let Some(movie) = common::env_path_file(
        "Z2_WARPLESS_MOVIE",
        "corpus/movies/warpless.fm2",
        "palace warpless movie",
    ) else {
        return;
    };
    let meta = std::fs::metadata(&movie).expect("movie metadata");
    assert!(meta.len() > 0, "warpless movie must be non-empty");
    let mut game = Game::new();
    register_palace_traps(&mut game);
    assert_eq!(game.traps.len(), PALACE_TRAP_COUNT);
}
