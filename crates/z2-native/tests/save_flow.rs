//! Native frontend regression for title → save-name → load.
//!
//! This is intentionally headless: it exercises the same native ROM
//! constructor and `app::step_frames` path as the window without opening an
//! audio device. A fault here is what the window's wedge guard turns into an
//! automatic pause.

mod common;

use z2_native::app;

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;

fn rom_emu() -> Option<app::Emu> {
    let raw = common::rom_bytes("native_save_entry_and_load_do_not_wedge_or_pause")?;
    let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
    Some(app::emu_from_rom_body(&body, 44_100).expect("build ROM emulator"))
}

fn step(emu: &mut app::Emu, frames: usize, input: u8) {
    app::step_frames(emu, &vec![input; frames], None);
}

fn enter_named_save(emu: &mut app::Emu) {
    step(emu, 30, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    for _ in 0..8 {
        step(emu, 2, A);
        step(emu, 8, 0);
    }
    for _ in 0..3 {
        step(emu, 2, SELECT);
        step(emu, 8, 0);
    }
    step(emu, 5, START);
    step(emu, 30, 0);
}

#[test]
fn native_save_entry_and_load_do_not_wedge_or_pause() {
    let Some(mut emu) = rom_emu() else {
        return;
    };

    // Same settled edge sequence as the empirical core menu-flow test.
    enter_named_save(&mut emu);
    step(&mut emu, 5, START);
    step(&mut emu, 60, 0);

    assert_eq!(emu.game.exec_errors, 0, "native save-load path wedged");
    assert_eq!(emu.game.ram[0x0700], 3, "save load should restore lives");

    // The first settled side-view frame is the visual regression target. It
    // must contain the loaded room and Link must have landed, rather than
    // remaining at the falling-entry position that the old proxy traps used.
    step(&mut emu, 110, 0);
    assert_eq!(emu.game.exec_errors, 0, "side-view entry path wedged");
    assert_eq!(
        emu.game.ram[0x0736], 0x0B,
        "save load should enter side-view"
    );
    assert!(
        (0xA8..=0xB8).contains(&emu.game.ram[0x0029]),
        "Link should be settled on the room floor after entry, got Y=${:02X}",
        emu.game.ram[0x0029]
    );
    let visible_pixels = emu
        .game
        .frame_indexed()
        .iter()
        .filter(|&&pixel| pixel != 0x0F)
        .count();
    assert!(
        visible_pixels > 30_000,
        "side-view frame should contain room geometry, got {visible_pixels} non-black pixels"
    );

    step(&mut emu, 990, 0);
    assert_eq!(emu.game.exec_errors, 0, "native gameplay path wedged");
    assert_eq!(
        emu.game.ram[0x0736], 0x0B,
        "save load should reach gameplay"
    );
}
