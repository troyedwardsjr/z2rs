//! Enemy trap-table + shim tests + gated movie/snapshot replays.
//!
//! Synthetic `Game` tests always run (ROM-free fixtures). Snapshots, boss
//! snapshots and movie replays are harness-ready and skip gracefully
//! without `Z2_ROM` / `corpus/` / movie files.

#![cfg(feature = "interp")]

mod common;

use z2_core::enemy_traps::{
    en_death, en_facing, en_flyer, en_generator, en_kill_all, en_link_collision,
    en_proj_disintegrate, en_shooter, en_spawn_bubble, en_spawn_proj, en_stun_gate, en_walker,
    register_enemy_traps, ENEMY_TRAPS, ENEMY_TRAPS_BANKED, ENEMY_TRAPS_SHARED, ENEMY_TRAP_COUNT,
};
use z2_core::game::Game;

#[test]
fn enemy_trap_table_is_sorted_unique_and_plausible() {
    assert!(!ENEMY_TRAPS.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for (name, bank, addr) in ENEMY_TRAPS {
        assert!(!name.is_empty());
        assert_eq!(*bank, Some(7), "{name} must be fixed-bank");
        assert!(*addr >= 0x8000, "{name} ${addr:04X} outside PRG");
        assert!(seen.insert((*name, *bank, *addr)), "duplicate {name}");
    }
    // Fixed-bank count pins the registered set; banked entries are listed
    // but skipped (aliasing caveat); shared entries stay player-owned.
    let fixed = ENEMY_TRAPS.iter().filter(|(_, b, _)| *b == Some(7)).count();
    assert_eq!(fixed, ENEMY_TRAPS.len());
    assert_eq!(
        ENEMY_TRAP_COUNT,
        ENEMY_TRAPS.len() - ENEMY_TRAPS_SHARED.len()
    );
    for (_, bank, _) in ENEMY_TRAPS_BANKED {
        assert_eq!(*bank, None);
    }
}

#[test]
fn register_enemy_traps_installs_fixed_set() {
    let mut game = Game::new();
    assert_eq!(game.traps.len(), 0);
    register_enemy_traps(&mut game);
    assert_eq!(game.traps.len(), ENEMY_TRAP_COUNT);
    // Spot-check fixed entries; banked entries must NOT be registered.
    assert!(game.traps.is_trapped(0xD6CA));
    assert!(game.traps.is_trapped(0xD6C1));
    assert!(game.traps.is_trapped(0xDA02));
    assert!(game.traps.is_trapped(0xDC91));
    assert!(game.traps.is_trapped(0xE880));
    assert!(game.traps.is_trapped(0xDBCE));
    assert!(!game.traps.is_trapped(0x987E));
    assert!(!game.traps.is_trapped(0xBAC3));
    assert!(!game.traps.is_trapped(0xA359));
    assert!(!game.traps.is_trapped(0x98EB));
    // Shared entries stay player-owned (not registered here).
    assert!(!game.traps.is_trapped(0xE677));
    assert!(!game.traps.is_trapped(0xE558));
    // Idempotent.
    register_enemy_traps(&mut game);
    assert_eq!(game.traps.len(), ENEMY_TRAP_COUNT);
}

/// Stack top as the interpreter's emulated `RTS` would read it (`pop16 + 1`).
fn pending_return(game: &Game) -> u16 {
    let sp = usize::from(game.cpu.sp);
    let lo = u16::from(game.ram[0x0100 + ((sp + 1) & 0xFF)]);
    let hi = u16::from(game.ram[0x0100 + ((sp + 2) & 0xFF)]);
    (lo | (hi << 8)).wrapping_add(1)
}

/// Push a `JSR` frame (`ret - 1`) the way the interpreter does before it
/// fires a trap.
fn push_frame(game: &mut Game, ret: u16) {
    let r = ret.wrapping_sub(1);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = (r >> 8) as u8;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = r as u8;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

#[test]
fn enemy_shims_run_on_synthetic_states() {
    let mut game = Game::new();
    register_enemy_traps(&mut game);
    game.cpu.sp = 0xFD;
    game.cpu.x = 0;
    game.ram[0x0010] = 0;
    // Stun gate on a live enemy: plain return, A = $040E,x (zero).
    push_frame(&mut game, 0xDA0F);
    game.ram[0x040E] = 0x00;
    en_stun_gate(&mut game);
    assert_eq!(game.cpu.a, 0);
    assert_eq!(pending_return(&game), 0xDA0F);
    assert_eq!(game.cpu.sp, 0xFB);
    // Stunned: the JSR frame is popped (A = its high byte) and the tail
    // jump to LDE40 is left for the dispatcher — nothing is pushed back.
    game.ram[0x040E] = 0x30;
    en_stun_gate(&mut game);
    assert_eq!(game.cpu.a, 0xDA);
    assert_eq!(game.take_trap_jump(), Some(0xDE40));
    assert_eq!(game.cpu.sp, 0xFD, "PLA : PLA consumed the frame");
    game.ram[0x040E] = 0x00;
    // Facing: Link right of enemy (BPL) → $60 = 1, Y = 0; left → 2, Y = 1.
    game.ram[0x004D] = 0x90;
    game.ram[0x003B] = 0x01;
    game.ram[0x004E] = 0x10;
    game.ram[0x003C] = 0x01;
    en_facing(&mut game);
    assert_eq!(game.ram[0x0060], 1);
    assert_eq!(game.cpu.y, 0);
    assert_eq!(game.cpu.sp, 0xFD, "PHA/PLA balanced");
    game.ram[0x004D] = 0x10;
    game.ram[0x004E] = 0x90;
    en_facing(&mut game);
    assert_eq!(game.ram[0x0060], 2);
    assert_eq!(game.cpu.y, 1);
    // Frozen gate: $A8 & $10 clear → plain return; set → tail-jump into
    // bank7_Link_Hit_Routine ($E2EF).
    game.ram[0x00A8] = 0x00;
    en_link_collision(&mut game);
    assert_eq!(game.cpu.sp, 0xFD);
    game.ram[0x00A8] = 0x10;
    en_link_collision(&mut game);
    assert_eq!(game.take_trap_jump(), Some(0xE2EF));
    assert_eq!(game.cpu.sp, 0xFD, "tail JMP pushes nothing");
    game.ram[0x00A8] = 0x00;
    // Kill-all from X = 1: live slot 0 reads back 1 (INC-after-remove
    // quirk), slot 1 untouched, X reloaded from $10, stack balanced.
    game.cpu.x = 1;
    game.ram[0x00B6] = 0x01;
    game.ram[0x00B7] = 0x00;
    game.ram[0x00BC] = 0xFF;
    game.ram[0x0010] = 0x03;
    en_kill_all(&mut game);
    assert_eq!(game.ram[0x00B6], 0x01);
    assert_eq!(game.ram[0x00B7], 0x00);
    assert_eq!(game.cpu.x, 0x03);
    assert_eq!(game.cpu.sp, 0xFD);
    game.cpu.x = 0;
    game.ram[0x0010] = 0;
    // Projectile spawn claims the top free slot (Y = 5): type $04, the
    // scan's $20,y/$66,y init, X reloaded from $10, carry clear.
    game.ram[0x004E] = 0x40;
    game.ram[0x003C] = 0x02;
    game.ram[0x002A] = 0x80;
    game.ram[0x0060] = 0x01;
    for i in 0..6 {
        game.ram[0x0087 + i] = 0x00;
    }
    en_spawn_proj(&mut game);
    assert_eq!(game.cpu.y, 5);
    assert_eq!(game.ram[0x0087 + 5], 0x04);
    assert_eq!(game.ram[0x0054 + 5], 0x40);
    assert_eq!(game.ram[0x0042 + 5], 0x02);
    assert_eq!(game.ram[0x0030 + 5], 0x80);
    assert_eq!(game.ram[0x0066 + 5], 0x01);
    assert_eq!(game.ram[0x0020 + 5], 0x01);
    assert_eq!(game.cpu.x, 0);
    assert_eq!(game.cpu.p & 0x01, 0, "CLC on the spawned path");
    assert_eq!(game.cpu.sp, 0xFD);
    // Bubble scan is 3..0 only; a full row sets carry with Y = $FF.
    en_spawn_bubble(&mut game);
    assert_eq!(game.cpu.y, 3);
    assert_eq!(game.ram[0x0020 + 3], 0x01);
    for i in 0..6 {
        game.ram[0x0087 + i] = 0x02;
    }
    en_spawn_bubble(&mut game);
    assert_eq!(game.cpu.y, 0xFF);
    assert_eq!(game.cpu.p & 0x01, 1, "SEC when no slot is free");
    // Disintegrate writes $7D,y = 0, $8D,y = $F2 for the slot in Y.
    game.cpu.y = 0x02;
    game.ram[0x007D + 2] = 0x55;
    en_proj_disintegrate(&mut game);
    assert_eq!(game.ram[0x007D + 2], 0x00);
    assert_eq!(game.ram[0x008D + 2], 0xF2);
    // Family ports run on synthetic states without panicking; apart from a
    // hand-off (the callee's JSR frame, 2 bytes) the stack balances — a
    // tail jump is only a request for the dispatcher.
    game.ram[0x00A8] = 0x04;
    game.ram[0x051B] = 0x00;
    game.ram[0x0012] = 0x00;
    game.ram[0x0071] = 0x08;
    game.ram[0x057E] = 0x00;
    for f in [en_walker, en_flyer, en_generator, en_shooter] {
        game.cpu.sp = 0xFD;
        game.cpu.x = 0;
        f(&mut game);
        assert!([0xFD, 0xFB].contains(&game.cpu.sp));
        let _ = game.take_trap_jump();
    }
    // Death port: rank latch, 6th-kill drop roll arms $414 (ROR), dying
    // timer + kill flag, X reloaded from $10, carry set.
    game.cpu.sp = 0xFD;
    game.cpu.x = 0;
    game.ram[0x00A1] = 0x05;
    game.wram[0x0DD5 + 0x05] = 0x23;
    game.wram[0x0DF9 + 0x05] = 0x42;
    game.ram[0x05DF] = 0x05;
    game.ram[0x051B] = 0x03;
    en_death(&mut game);
    assert_eq!(game.ram[0x0414], 0x81, "rank 3 with the drop bit rolled in");
    assert_eq!(game.ram[0x05DF], 0x00, "6th kill resets the group counter");
    assert_eq!(game.ram[0x0504], 0x25);
    assert_eq!(game.ram[0x043E], 0x00);
    assert_eq!(game.ram[0x00B6], 0x02);
    assert_eq!(game.ram[0x00EF], 0x04);
    assert_eq!(game.cpu.x, 0);
    assert_eq!(game.cpu.a, 0x04);
    assert_eq!(game.cpu.p & 0x01, 1);
    // Not a 6th kill: the counter increments and the rank stays unarmed.
    game.ram[0x05DF] = 0x01;
    en_death(&mut game);
    assert_eq!(game.ram[0x0414], 0x03);
    assert_eq!(game.ram[0x05DF], 0x02);
}

// ---------------------------------------------------------------------------
// Gated: enemy snapshots verify clean (harness-ready).
// ---------------------------------------------------------------------------

#[test]
fn enemy_snapshots_verify_clean_when_present() {
    let Some(rd) = common::corpus_snapshots("enemy traps corpus snapshots") else {
        return;
    };
    let snaps: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("enemy-"))
        })
        .collect();
    if snaps.is_empty() {
        eprintln!("SKIP: no enemy-* snapshots (no corpus/ dir yet)");
        return;
    }
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
        assert!(b.len() >= 8 + 2048 + 8192, "{}: too small", p.display());
    }
}

// ---------------------------------------------------------------------------
// Gated: 100% + warpless movies clean (report absence).
// ---------------------------------------------------------------------------

#[test]
fn warpless_movies_clean_when_present() {
    let Some(root) = common::env_dir(
        "Z2_MOVIES",
        "corpus/movies",
        "warpless_movies_clean_when_present",
    ) else {
        return;
    };
    let names = ["movie-100percent.fm2", "movie-warpless.fm2"];
    let mut absent = Vec::new();
    for n in names {
        let cand = root.join(n);
        if !cand.is_file() {
            absent.push(cand.display().to_string());
        }
    }
    if !absent.is_empty() {
        eprintln!(
            "SKIP: no 100%/warpless movies (report: absent: {})",
            absent.join(", ")
        );
        return;
    }
    // Harness-ready: full input replay needs Z2_ROM + oracle lockstep
    // (`xtask verify --dut game` wiring, out of scope since tools/** is
    // read-only). Presence + non-empty is all this layer asserts.
    for n in names {
        let cand = root.join(n);
        let meta = std::fs::metadata(&cand).expect("movie metadata");
        assert!(
            meta.len() > 0,
            "movie must be non-empty: {}",
            cand.display()
        );
    }
    let mut game = Game::new();
    register_enemy_traps(&mut game);
    assert_eq!(game.traps.len(), ENEMY_TRAP_COUNT);
}
