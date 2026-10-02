//! ROM-gated: the randomizer seam boots exactly like the vanilla path.
//!
//! * vanilla flags through `--seed` / `--rando-flags` give the same game,
//!   frame for frame, as no randomizer at all;
//! * the expanded PRG layout (256 KiB, fixed bank moved to bank 15) boots and
//!   runs identically to the vanilla layout;
//! * every preset boots through the title, file registration and load into
//!   side-view gameplay without an interpreter fault.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --test
//! rando_boot -- --ignored`.

use z2_core::game::{BTN_A, BTN_RIGHT, BTN_SELECT, BTN_START};
use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;

/// `$0736`: game mode; `$0B` is side-view gameplay.
const MODE: usize = 0x0736;
const MODE_SIDEVIEW: u8 = 0x0B;

fn rom_body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

fn hold(v: &mut Vec<u8>, frames: usize, pad: u8) {
    v.extend(std::iter::repeat_n(pad, frames));
}

/// Title -> register a name -> load it -> settled in the first room, then
/// walk right for a while (the settled menu sequence from `save_flow.rs`).
/// Identical for every emulator compared.
fn inputs() -> Vec<u8> {
    let mut v = Vec::new();
    hold(&mut v, 30, 0);
    hold(&mut v, 5, BTN_START);
    hold(&mut v, 20, 0);
    hold(&mut v, 5, BTN_START);
    hold(&mut v, 20, 0);
    for _ in 0..8 {
        hold(&mut v, 2, BTN_A);
        hold(&mut v, 8, 0);
    }
    for _ in 0..3 {
        hold(&mut v, 2, BTN_SELECT);
        hold(&mut v, 8, 0);
    }
    hold(&mut v, 5, BTN_START);
    hold(&mut v, 30, 0);
    hold(&mut v, 5, BTN_START);
    hold(&mut v, 170, 0);
    v
}

/// Run the boot script; asserts the run reached side-view gameplay, then
/// walks right for `walk` frames.
fn run(emu: &mut Emu, walk: usize, what: &str) {
    app::step_frames(emu, &inputs(), None);
    assert_eq!(emu.game.exec_errors, 0, "{what}: menu/load path faulted");
    assert_eq!(
        emu.game.ram[MODE], MODE_SIDEVIEW,
        "{what}: reached gameplay"
    );
    app::step_frames(emu, &vec![BTN_RIGHT; walk], None);
    assert_eq!(emu.game.exec_errors, 0, "{what}: gameplay faulted");
}

fn assert_same(a: &Emu, b: &Emu, what: &str) {
    assert_eq!(a.game.ram[..], b.game.ram[..], "{what}: CPU RAM");
    assert_eq!(a.game.wram[..], b.game.wram[..], "{what}: WRAM");
    assert_eq!(
        a.game.frame_indexed()[..],
        b.game.frame_indexed()[..],
        "{what}: frame"
    );
    assert_eq!(a.game.cpu.cycles, b.game.cpu.cycles, "{what}: cycles");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_flags_through_the_randomizer_boot_identically() {
    let body = rom_body();
    let spec = RandoSpec::from_cli(Some("test"), Some("1"), None, None)
        .unwrap()
        .unwrap()
        .leak();
    let feats = Features {
        rando: Some(spec),
        ..Features::default()
    };
    let mut plain = app::emu_from_rom_body(&body, 44_100).unwrap();
    let mut rando = app::emu_from_rom_body_with(&body, 44_100, feats).unwrap();
    assert_eq!(rando.rom.body_crc32, z2_assets::rom::EXPECTED_BODY_CRC32);
    assert!(rando.rom.hash_code.as_deref().is_some_and(|h| h.len() == 6));
    assert!(rando.rom.untrapped.is_empty());
    assert_eq!(
        rando.trapset_id, plain.trapset_id,
        "netplay identity unchanged"
    );
    run(&mut plain, 600, "plain");
    run(&mut rando, 600, "vanilla flags");
    assert_same(&plain, &rando, "vanilla flags");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn expanded_prg_layout_boots_identically() {
    let body = rom_body();
    let mut rom = z2_rando::rom::Rom::from_body(&body).unwrap();
    rom.expand_prg();
    let expanded = rom.body();
    assert_eq!(expanded.len(), z2_rando::rom::EXPANDED_BODY_LEN);
    let mut plain = app::emu_from_rom_body(&body, 44_100).unwrap();
    let mut big =
        app::emu_from_trusted_body_with(&expanded, Some(&body), None, 44_100, Features::default())
            .unwrap();
    assert_eq!(big.game.prg.len(), 256 * 1024);
    assert!(big.rom.untrapped.is_empty(), "fixed bank content unchanged");
    assert_eq!(big.trapset_id, plain.trapset_id);
    run(&mut plain, 600, "plain");
    run(&mut big, 600, "expanded");
    assert_same(&plain, &big, "expanded layout");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn patched_fixed_bank_code_untraps_its_routine() {
    let body = rom_body();
    let build = |patched: &[u8]| {
        app::emu_from_trusted_body_with(patched, Some(&body), None, 44_100, Features::default())
            .unwrap()
    };
    let vanilla = build(&body);
    assert!(vanilla.rom.untrapped.is_empty());

    // The starting-lives operand (`LDA #$03` at `$C358`) is read by the
    // port itself, so patching it keeps the trap.
    let mut rom = z2_rando::rom::Rom::from_body(&body).unwrap();
    rom.write_cpu(0, 0xC359, &[5]).unwrap();
    let lives = build(&rom.body());
    assert!(lives.rom.untrapped.is_empty(), "{:X?}", lives.rom.untrapped);
    assert_eq!(lives.trapset_id, vanilla.trapset_id);

    // Any other change inside the routine (`STA $0700` -> `STA $0701`) makes
    // it run as patched ROM code, and the netplay identity records that.
    let mut rom = z2_rando::rom::Rom::from_body(&body).unwrap();
    rom.write_cpu(0, 0xC35B, &[0x01]).unwrap();
    let patched = build(&rom.body());
    assert!(
        patched.rom.untrapped.contains(&0xC358),
        "{:X?}",
        patched.rom.untrapped
    );
    assert!(!patched.game.traps.is_trapped(0xC358));
    assert_ne!(patched.trapset_id, vanilla.trapset_id);
}

#[test]
#[ignore = "needs Z2_ROM"]
fn every_preset_reaches_gameplay() {
    let body = rom_body();
    for p in z2_rando::flags::Preset::ALL {
        let flags = p.flags().to_flag_string();
        for seed in ["boot-1", "boot-2"] {
            let spec = RandoSpec::from_cli(Some(seed), Some(&flags), None, None)
                .unwrap()
                .unwrap()
                .leak();
            let feats = Features {
                rando: Some(spec),
                ..Features::default()
            };
            let mut emu = app::emu_from_rom_body_with(&body, 44_100, feats).unwrap();
            run(&mut emu, 900, &format!("{p:?} {seed}"));
        }
    }
}

/// Enhancement group R (the light runtime randomizer) on top of a ROM
/// randomizer seed: the scalers and the palette randomizer patch the
/// randomized tables (in the fixed bank wherever the layout put it), the
/// start loadout still finds the new file against the seed's own beginning
/// values, the item shuffle is skipped (it assumes the vanilla locations),
/// and the game boots into gameplay without a fault.
#[test]
#[ignore = "needs Z2_ROM"]
fn enhancement_randomizer_runs_on_top_of_a_rom_seed() {
    use z2_core::enh::{rando::DAMAGE_ADDR, rando::GET_ITEM_ADDR, Enhancements, RandoOpts};
    let body = rom_body();
    let enh = Enhancements {
        rando: RandoOpts {
            seed: 77,
            start_attack: 3,
            start_life: 5,
            start_spells: 0b0000_0011,
            start_containers_heart: 6,
            enemy_hp_pct: 20,
            enemy_dmg_pct: -20,
            xp_pct: 10,
            level_cost_pct: -10,
            spell_cost_pct: -10,
            palette_rando: 2,
            item_shuffle: true,
            ..RandoOpts::default()
        },
        ..Enhancements::default()
    };
    // Control: on the vanilla ROM this seed's item shuffle hooks the pickup.
    let plain = app::emu_from_rom_body_with(
        &body,
        44_100,
        Features {
            enhancements: enh,
            ..Features::default()
        },
    )
    .unwrap();
    assert!(plain.game.enhancements().rando.item_shuffle);
    assert_eq!(
        plain.game.traps.get(GET_ITEM_ADDR).map(|t| t.name),
        Some("enh_rando_get_item")
    );
    for p in [
        z2_rando::flags::Preset::Standard,
        z2_rando::flags::Preset::MaxRando,
    ] {
        let flags = p.flags().to_flag_string();
        let spec = RandoSpec::from_cli(Some("enh-r"), Some(&flags), None, None)
            .unwrap()
            .unwrap()
            .leak();
        let randomized = spec.run(&body).unwrap().body;
        let feats = Features {
            rando: Some(spec),
            enhancements: enh,
            ..Features::default()
        };
        let mut emu = app::emu_from_rom_body_with(&body, 44_100, feats).unwrap();
        let what = format!("{p:?} + group R");
        assert!(emu.rom.randomized, "{what}");
        // Item shuffle skipped; everything else in the group still on.
        let running = *emu.game.enhancements();
        assert!(!running.rando.item_shuffle, "{what}: item shuffle skipped");
        assert_eq!(running, enh.for_rom(true));
        assert_ne!(
            emu.game.traps.get(GET_ITEM_ADDR).map(|t| t.name),
            Some("enh_rando_get_item"),
            "{what}: no item-shuffle hook"
        );
        // The damage scaler patched the fixed bank of this layout.
        let fixed = z2_core::enh::bank_offset(emu.game.prg.len(), 7, DAMAGE_ADDR);
        assert_ne!(
            emu.game.prg[fixed..fixed + 56],
            randomized[fixed..fixed + 56],
            "{what}: damage table scaled"
        );
        if emu.game.prg.len() > 128 * 1024 {
            // Expanded layout: bank 7 is an ordinary bank and stays as the
            // randomizer wrote it.
            let b7 = 7 * 0x4000 + usize::from(DAMAGE_ADDR & 0x3FFF);
            assert_eq!(emu.game.prg[b7..b7 + 56], randomized[b7..b7 + 56]);
        }
        run(&mut emu, 900, &what);
        // The loadout reached the new file.
        assert_eq!(emu.game.ram[0x0777], 3, "{what}: start attack");
        assert_eq!(emu.game.ram[0x0779], 5, "{what}: start life");
        assert_eq!(emu.game.ram[0x077B], 1, "{what}: SHIELD known");
        assert_eq!(emu.game.ram[0x077C], 1, "{what}: JUMP known");
        assert_eq!(emu.game.ram[0x0784], 6, "{what}: heart containers");
    }
}
