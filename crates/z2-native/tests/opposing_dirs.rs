//! Issue #7: turning around quickly on a keyboard sent Link "zooming across
//! the screen backwards".
//!
//! Cause: key rollover. Pressing Left before Right is released holds
//! Left+Right for a frame or two, which a real NES d-pad can never do. The
//! game's side-view walk code then sees facing `3` and pushes the X speed
//! `$70` by a fixed step each frame — from `+$18` (full walk right) through
//! `$48 $78 $A8` to `$D8` (-40, well past the normal `-$18` walk limit) —
//! and Link keeps sliding left at that speed after the keys are sorted out.
//! One or two overlap frames leave `$70` at `+72`/`+120` instead, so Link
//! faces left while skidding right at 3-5x walking speed.
//!
//! The native frontend now runs live pads through
//! [`z2_native::input::OpposingFilter`] ("last pressed wins"). These tests
//! drive the real game headless: the raw rollover script reproduces the
//! glitch, and the same script through the filter turns around cleanly.
//! ROM-gated (skip without `$Z2_ROM`).

mod common;

use z2_native::app;
use z2_native::input::OpposingFilter;

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const LEFT: u8 = 0x40;
const RIGHT: u8 = 0x80;

const MODE: usize = 0x0736;
const LINK_X: usize = 0x004D;
const LINK_X_SPEED: usize = 0x0070;
const LINK_FACING: usize = 0x009F;

fn step(emu: &mut app::Emu, frames: usize, input: u8) {
    app::step_frames(emu, &vec![input; frames], None);
}

/// Title → name a save → load it → settled side-view gameplay (the
/// `save_flow.rs` edge sequence).
fn sideview(test: &str) -> Option<app::Emu> {
    let raw = common::rom_bytes(test)?;
    let body = z2_assets::rom::strip_ines_header(&raw).to_vec();
    let mut emu = app::emu_from_rom_body(&body, 44_100).expect("build ROM emulator");
    step(&mut emu, 30, 0);
    step(&mut emu, 5, START);
    step(&mut emu, 20, 0);
    step(&mut emu, 5, START);
    step(&mut emu, 20, 0);
    for _ in 0..8 {
        step(&mut emu, 2, A);
        step(&mut emu, 8, 0);
    }
    for _ in 0..3 {
        step(&mut emu, 2, SELECT);
        step(&mut emu, 8, 0);
    }
    step(&mut emu, 5, START);
    step(&mut emu, 30, 0);
    step(&mut emu, 5, START);
    step(&mut emu, 170, 0);
    assert_eq!(emu.game.exec_errors, 0);
    assert_eq!(emu.game.ram[MODE], 0x0B, "should be in side-view gameplay");
    Some(emu)
}

/// Keyboard turn-around with rollover: walk right at full speed, press
/// Left while Right is still down for `overlap` frames, then Left alone.
fn rollover_script(overlap: usize) -> Vec<u8> {
    let mut s = vec![RIGHT; 40];
    s.extend(std::iter::repeat_n(LEFT | RIGHT, overlap));
    s.extend(std::iter::repeat_n(LEFT, 30));
    s
}

#[derive(Debug)]
struct Run {
    /// `$70` read as signed, per frame after the turn starts.
    speeds: Vec<i8>,
    facings: Vec<u8>,
    /// X distance travelled left during the 30 Left-only frames.
    left_travel: i32,
}

fn run(emu: &mut app::Emu, script: &[u8], filter: Option<&mut OpposingFilter>) -> Run {
    let mut f = filter;
    let (mut speeds, mut facings) = (Vec::new(), Vec::new());
    let mut x_at_left_only = None;
    for (i, &raw) in script.iter().enumerate() {
        let pad = match f.as_deref_mut() {
            Some(flt) => flt.apply(raw),
            None => raw,
        };
        if raw == LEFT && x_at_left_only.is_none() {
            x_at_left_only = Some(emu.game.ram[LINK_X] as i32);
        }
        step(emu, 1, pad);
        if i >= 40 {
            speeds.push(emu.game.ram[LINK_X_SPEED] as i8);
            facings.push(emu.game.ram[LINK_FACING]);
        }
    }
    let left_travel = x_at_left_only.expect("script ends on Left") - emu.game.ram[LINK_X] as i32;
    Run {
        speeds,
        facings,
        left_travel,
    }
}

#[test]
fn raw_left_plus_right_rollover_shoves_link_backwards() {
    for overlap in [1usize, 2, 4] {
        let Some(mut emu) = sideview("raw_left_plus_right_rollover_shoves_link_backwards") else {
            return;
        };
        let r = run(&mut emu, &rollover_script(overlap), None);
        eprintln!("raw L+R overlap {overlap}: {r:?}");
        // The game sees the impossible third direction...
        assert!(r.facings[..overlap].iter().all(|&f| f == 3), "{r:?}");
        // ...and even one frame of it throws the X speed outside the
        // +-$18 walking range.
        assert!(
            r.speeds.iter().any(|&v| !(-0x18..=0x18).contains(&v)),
            "{r:?}"
        );
        if overlap == 4 {
            // $70: +$18 -> $48 $78 $A8 $D8 (-40), then keeps growing left.
            assert_eq!(&r.speeds[..4], &[0x48, 0x78, -88, -40], "{r:?}");
            assert!(r.left_travel > 60, "zooms backwards: {r:?}");
        }
    }
}

#[test]
fn filtered_rollover_turns_around_like_a_real_pad() {
    for overlap in [1usize, 2, 4] {
        let Some(mut emu) = sideview("filtered_rollover_turns_around_like_a_real_pad") else {
            return;
        };
        let mut filter = OpposingFilter::new();
        let r = run(&mut emu, &rollover_script(overlap), Some(&mut filter));
        eprintln!("filtered overlap {overlap}: {r:?}");
        assert!(
            r.facings.iter().all(|&f| f == 2),
            "Left (newest) wins at once, never facing 3: {r:?}"
        );
        assert!(
            r.speeds.iter().all(|&v| (-0x18..=0x18).contains(&v)),
            "speed stays inside the walking range: {r:?}"
        );
        // Same as a clean Right→Left turn (no overlap at all).
        let Some(mut clean) = sideview("filtered_rollover_turns_around_like_a_real_pad") else {
            return;
        };
        let mut script = vec![RIGHT; 40];
        script.extend(std::iter::repeat_n(LEFT, 30 + overlap));
        let c = run(&mut clean, &script, None);
        assert_eq!(r.speeds, c.speeds, "overlap {overlap} vs clean turn");
        assert!(r.left_travel < 20, "{r:?}");
    }
}
