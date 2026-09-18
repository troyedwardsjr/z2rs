//! macOS GameController-framework gamepad reader.
//!
//! Pads bound to Apple's own drivers (e.g. the USB/Bluetooth Xbox One S
//! controller, `XboxOneSGamepad` user class) enumerate through IOKit HID —
//! so `gilrs` lists them — but never deliver input values there; their input
//! only reaches `GCController`. This reads every connected extended gamepad
//! once per frame and folds it into a NES pad byte with the same positional
//! layout as the `gilrs` path (see [`crate::input::gilrs_button_to_bit`]).
//!
//! Controller discovery notifications are delivered on the main run loop,
//! which the `winit` event loop pumps, so plain polling of
//! `GCController::controllers()` stays current.

use objc2_game_controller::{GCController, GCControllerButtonInput, GCControllerDirectionPad};
use z2_core::game::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_SELECT, BTN_START, BTN_UP};

fn pressed(button: &GCControllerButtonInput) -> bool {
    // SAFETY: plain property read on a live framework object.
    unsafe { button.isPressed() }
}

fn dpad_bits(dpad: &GCControllerDirectionPad) -> u8 {
    let mut pad = 0u8;
    // SAFETY: plain property reads on a live framework object.
    unsafe {
        if pressed(&dpad.up()) {
            pad |= BTN_UP;
        }
        if pressed(&dpad.down()) {
            pad |= BTN_DOWN;
        }
        if pressed(&dpad.left()) {
            pad |= BTN_LEFT;
        }
        if pressed(&dpad.right()) {
            pad |= BTN_RIGHT;
        }
    }
    pad
}

/// OR of every connected GameController extended gamepad.
///
/// Unlike raw HID adapter ports, GameController only lists pads that are
/// actually present, so combining them cannot inject ghost input.
pub fn poll() -> u8 {
    let mut pad = 0u8;
    // SAFETY: class/property reads on the main thread (the winit loop).
    unsafe {
        for controller in GCController::controllers().iter() {
            let Some(gp) = controller.extendedGamepad() else {
                continue;
            };
            if pressed(&gp.buttonB()) {
                pad |= BTN_A;
            }
            if pressed(&gp.buttonA()) {
                pad |= BTN_B;
            }
            if let Some(options) = gp.buttonOptions() {
                if pressed(&options) {
                    pad |= BTN_SELECT;
                }
            }
            if pressed(&gp.buttonMenu()) {
                pad |= BTN_START;
            }
            pad |= dpad_bits(&gp.dpad());
            pad |= dpad_bits(&gp.leftThumbstick());
        }
    }
    pad
}
