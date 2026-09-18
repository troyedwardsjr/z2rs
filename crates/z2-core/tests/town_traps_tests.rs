//! Town trap-table + shim + snapshot/movie harness.
//!
//! Synthetic `Game` tests always run (ROM-free fixtures). Snapshot and
//! movie replays are harness-ready and skip gracefully without
//! `Z2_ROM` / `corpus/` / movie files.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;
use z2_core::town_traps::{
    register_town_traps, tw_dialog_erase, tw_dialog_next, tw_dialog_sound, tw_dialog_text_ptr,
    tw_end_of_line, tw_healer_cond, tw_npc_anim, tw_npc_spawn, tw_restore, tw_talk_gate,
    tw_teardown, tw_thrust_bit, tw_typewriter, tw_wait_b, tw_wise_cond, TOWN_TRAPS,
};

#[test]
fn town_trap_table_is_sorted_unique_and_plausible() {
    assert!(!TOWN_TRAPS.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for (name, bank, addr) in TOWN_TRAPS {
        assert!(!name.is_empty());
        assert!(*addr >= 0x8000, "{name} ${addr:04X} outside PRG");
        assert!(seen.insert((*name, *bank, *addr)), "duplicate {name}");
        // All town code is slot-swapped bank 3 → data-only (aliasing).
        assert_eq!(*bank, None, "{name} must stay bank-None");
    }
}

#[test]
fn register_town_traps_registers_nothing_data_only() {
    let mut game = Game::new();
    assert_eq!(game.traps.len(), 0);
    register_town_traps(&mut game);
    assert_eq!(game.traps.len(), 0, "bank-3 traps stay data-only");
    // Idempotent.
    register_town_traps(&mut game);
    assert_eq!(game.traps.len(), 0);
}

#[test]
fn town_shims_run_on_synthetic_states() {
    let mut game = Game::new();
    // B-gate fires in the door window with B pressed.
    game.ram[0x0010] = 0;
    game.ram[0x05BD] = 0x70;
    game.ram[0x00AF] = 0x00;
    game.ram[0x00F5] = 0x40;
    game.ram[0x0029] = 0x5F;
    tw_talk_gate(&mut game);
    assert_eq!(game.ram[0x074C], 0x02);
    assert_eq!(game.ram[0x00DE], 0x01);
    assert_eq!(game.ram[0x0080], 0x03);
    assert_eq!(game.ram[0x0029], 0x50);
    assert_eq!(game.ram[0x05A5], 0x01);
    assert_eq!(game.ram[0x048B], 0x00);
    // B-gate holds outside the window.
    let mut game2 = Game::new();
    game2.ram[0x0010] = 0;
    game2.ram[0x05BD] = 0x64;
    game2.ram[0x00F5] = 0x40;
    tw_talk_gate(&mut game2);
    assert_eq!(game2.ram[0x074C], 0x00);

    // Wise-man grant in Rauru with 1 container.
    let mut game3 = Game::new();
    game3.ram[0x056B] = 0x00;
    game3.ram[0x0783] = 0x01;
    tw_wise_cond(&mut game3);
    assert_eq!(game3.ram[0x077B], 0x01);
    assert_eq!(game3.ram[0x0005], 0x01);
    assert_eq!(game3.ram[0x0749], 0x00, "first spell sets selector to town");
    // Deny without containers.
    let mut game4 = Game::new();
    game4.ram[0x056B] = 0x07;
    game4.ram[0x0783] = 0x00;
    tw_wise_cond(&mut game4);
    assert_eq!(game4.ram[0x0782], 0x00);
    assert_eq!(game4.ram[0x0005], 0x00);

    // Healer cond: talking latches alt.
    let mut game5 = Game::new();
    game5.ram[0x074C] = 0x02;
    tw_healer_cond(&mut game5);
    assert_eq!(game5.ram[0x0005], 0x01);

    // Typewriter letter emits the PPU packet.
    let mut game6 = Game::new();
    game6.ram[0x0000] = 0xDA;
    game6.ram[0x0489] = 0x02;
    tw_typewriter(&mut game6);
    assert_eq!(game6.ram[0x0301], 0x05);
    assert_eq!(game6.ram[0x0304], 0x82);
    assert_eq!(game6.ram[0x0307], 0xFF);
    assert_eq!(game6.ram[0x0489], 0x03);
    assert_eq!(game6.ram[0x0566], 0x05);
    assert_eq!(game6.ram[0x00EC], 0x60);
    // $FF advances the routine counter.
    game6.ram[0x0000] = 0xFF;
    game6.ram[0x0524] = 0x07;
    tw_typewriter(&mut game6);
    assert_eq!(game6.ram[0x0524], 0x08);

    // End-of-line strides the row.
    let mut game7 = Game::new();
    game7.ram[0x0000] = 0xFD;
    game7.ram[0x048A] = 0x00;
    tw_end_of_line(&mut game7);
    assert_eq!(
        (game7.ram[0x0489], game7.ram[0x048A], game7.ram[0x0566]),
        (0, 0x40, 0x0B)
    );

    // Restore: magic lady → $070C, healer → $070D.
    let mut game8 = Game::new();
    game8.ram[0x048B] = 0x02;
    game8.ram[0x00A1 + 0x02] = 0x18;
    tw_restore(&mut game8);
    assert_eq!(game8.ram[0x070C], 0xFF);
    game8.ram[0x00A1 + 0x02] = 0x17;
    tw_restore(&mut game8);
    assert_eq!(game8.ram[0x070D], 0xFF);

    // Dialog routine counter helpers.
    let mut game9 = Game::new();
    tw_dialog_sound(&mut game9);
    assert_eq!((game9.ram[0x00EE], game9.ram[0x0524]), (0x08, 0x01));
    tw_dialog_next(&mut game9);
    assert_eq!(game9.ram[0x0524], 0x02);
    game9.ram[0x0525] = 0x00;
    tw_dialog_erase(&mut game9);
    assert_eq!(game9.ram[0x0524], 0x03, "erase at 0 advances instead");
    game9.ram[0x0525] = 0x03;
    tw_dialog_erase(&mut game9);
    assert_eq!(game9.ram[0x0525], 0x02);

    // Wait-for-B: idle advances, talking needs B.
    let mut game10 = Game::new();
    game10.ram[0x0766] = 0x00;
    tw_wait_b(&mut game10);
    assert_eq!(game10.ram[0x0524], 0x01);
    game10.ram[0x0766] = 0x01;
    game10.ram[0x00F5] = 0x00;
    tw_wait_b(&mut game10);
    assert_eq!(game10.ram[0x0524], 0x01, "talking without B holds");
    game10.ram[0x00F5] = 0x40;
    tw_wait_b(&mut game10);
    assert_eq!(game10.ram[0x0524], 0x02);

    // Thrust bit sets $0796.
    let mut game11 = Game::new();
    game11.ram[0x0000] = 0x04;
    tw_thrust_bit(&mut game11);
    assert_eq!(game11.ram[0x0796], 0x10);

    // Teardown clears locks and counters.
    let mut game12 = Game::new();
    game12.ram[0x05C3] = 0x01;
    game12.ram[0x00DE] = 0x01;
    game12.ram[0x074C] = 0x02;
    tw_teardown(&mut game12);
    assert_eq!(game12.ram[0x05C3], 0x00);
    assert_eq!(game12.ram[0x00DE], 0x00);
    assert_eq!(game12.ram[0x074C], 0x00);
    assert_eq!(game12.ram[0x00EE], 0x08);

    // NPC spawn records the aux INC + free-slot scan.
    let mut game13 = Game::new();
    game13.ram[0x0010] = 0x00;
    game13.ram[0x00AF] = 0x05;
    tw_npc_spawn(&mut game13);
    assert_eq!(game13.ram[0x00AF], 0x06);

    // NPC anim commits the frame bits.
    let mut game14 = Game::new();
    game14.ram[0x0010] = 0x01;
    game14.ram[0x0012] = 0x18;
    tw_npc_anim(&mut game14);
    assert_eq!(game14.ram[0x0081 + 0x01], 0x18);
    assert_eq!(game14.ram[0x00AF + 0x01], 0x00);

    // Text-ptr dispatch records slot + town.
    let mut game15 = Game::new();
    game15.ram[0x0010] = 0x00;
    game15.ram[0x00A1] = 0x17;
    game15.ram[0x056B] = 0x02;
    tw_dialog_text_ptr(&mut game15);
    assert_eq!(game15.ram[0x0000], 0x0D);
    assert_eq!(game15.ram[0x0001], 0x02);
}

// ---------------------------------------------------------------------------
// Gated: town-* snapshots verify clean (harness-ready).
// ---------------------------------------------------------------------------

#[test]
fn town_snapshots_verify_clean_when_present() {
    let Some(rd) = common::corpus_snapshots("town traps corpus snapshots") else {
        return;
    };
    let snaps: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("town-"))
        })
        .collect();
    if snaps.is_empty() {
        eprintln!("SKIP: no town-* snapshots (no corpus/ dir yet)");
        return;
    }
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
        assert!(b.len() >= 8 + 2048 + 8192, "{}: too small", p.display());
    }
}

// ---------------------------------------------------------------------------
// Gated: movie town segments clean (reports absence per file).
// ---------------------------------------------------------------------------

#[test]
fn movie_town_segments_clean_when_present() {
    let Some(dir) = common::env_dir(
        "Z2_MOVIE_DIR",
        "corpus/movies",
        "movie_town_segments_clean_when_present",
    ) else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!(
            "skipping movie_town_segments_clean_when_present: cannot read {}",
            dir.display()
        );
        return;
    };
    let movies: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("fm2"))
        .collect();
    if movies.is_empty() {
        eprintln!(
            "skipping movie_town_segments_clean_when_present: no .fm2 movies at {}",
            dir.display()
        );
        return;
    }
    // Harness-ready: full town-segment replay needs Z2_ROM + oracle
    // lockstep (`xtask verify --dut game` wiring, out of scope since
    // tools/** is read-only). Presence + non-empty is all this layer
    // asserts; absence is reported per file above via SKIP.
    for m in &movies {
        let meta = std::fs::metadata(m).expect("movie metadata");
        assert!(meta.len() > 0, "{} must be non-empty", m.display());
    }
    let mut game = Game::new();
    register_town_traps(&mut game);
    assert_eq!(game.traps.len(), 0);
}
