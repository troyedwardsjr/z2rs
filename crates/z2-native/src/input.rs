//! Keyboard + gamepad → NES pad-byte mapping.
//!
//! Shared contract (see `z2_core::game`): input bits LSB-first
//! `A,B,Select,Start,Up,Down,Left,Right = 0..7`
//! ([`z2_core::game::INPUT_A`]..`INPUT_RIGHT`, masks `BTN_A`..`BTN_RIGHT`).
//! One logic frame is `Game::step(input: u8)`.
//!
//! This module is pure: string-keyed tables (mirroring [`crate::config`])
//! plus tiny `u8` combinators. The `winit`/`gilrs` adapters take already-
//! resolved names / button values so unit tests never open a window or
//! touch a gamepad device.
//!
//! # Input latency note (acceptance)
//!
//! The windowed loop polls keyboard + gamepad **once per emulated frame**,
//! immediately before `Game::step`: worst-case added latency is one frame
//! (~16.64 ms at 60.0988 Hz) plus OS/display queue depth. There is no input
//! queue across frames — a press that lands mid-accumulator applies to the
//! next stepped frame, never to a stale one. Fast-forward steps each frame
//! with the *current* held state (no repeat/dedupe).

use z2_core::game::{BTN_A, BTN_B, BTN_DOWN, BTN_LEFT, BTN_RIGHT, BTN_SELECT, BTN_START, BTN_UP};

/// Button bit indices in shared-contract order.
pub const BUTTON_BITS: [(&str, u8); 8] = [
    ("A", 0),
    ("B", 1),
    ("Select", 2),
    ("Start", 3),
    ("Up", 4),
    ("Down", 5),
    ("Left", 6),
    ("Right", 7),
];

/// Masks in shared-contract order (index = bit).
pub const BUTTON_MASKS: [u8; 8] = [
    BTN_A, BTN_B, BTN_SELECT, BTN_START, BTN_UP, BTN_DOWN, BTN_LEFT, BTN_RIGHT,
];

/// Combine keyboard + gamepad bytes (bitwise OR — either source holds).
#[must_use]
pub fn combine_inputs(keyboard: u8, gamepad: u8) -> u8 {
    keyboard | gamepad
}

/// Set (`pressed=true`) or clear a button bit.
#[must_use]
pub fn with_button(mut pad: u8, bit: u8, pressed: bool) -> u8 {
    if bit >= 8 {
        return pad;
    }
    if pressed {
        pad |= 1 << bit;
    } else {
        pad &= !(1 << bit);
    }
    pad
}

/// Map a `winit` key-code debug name (e.g. `"KeyZ"`, `"ArrowUp"`) through a
/// string table (usually [`crate::config::KeyBindings`]).
pub fn key_name_to_bit(
    table: &std::collections::HashMap<String, u8>,
    key_name: &str,
) -> Option<u8> {
    table.get(key_name).copied().filter(|&b| b < 8)
}

/// SDL mapping overrides applied on top of the bundled gilrs database.
///
/// `0079:0011` "Retrolink SNES Controller" (also sold as kiwitata and other
/// clones): the bundled macOS entry maps the dpad as half-axes
/// (`dpup:-a4,dpdown:+a4`), which gilrs folds into a single 0..1 button per
/// axis — Up and Left never read as pressed and Down reads as Up. Mapping
/// the dpad axes to the left stick instead yields a proper -1..1 axis that
/// [`axis_to_bits`] folds into dpad bits.
pub const GILRS_MAPPING_OVERRIDES: &str = "\
03000000790000001100000006010000,Retrolink SNES Controller,a:b2,b:b1,back:b8,leftx:a3,lefty:a4,leftshoulder:b4,rightshoulder:b5,start:b9,x:b3,y:b0,platform:Mac OS X,
03000000790000001100000000000000,Retrolink SNES Controller,a:b2,b:b1,back:b8,leftx:a3,lefty:a4,leftshoulder:b4,rightshoulder:b5,start:b9,x:b3,y:b0,platform:Mac OS X,
";

/// Build a `Gilrs` context with [`GILRS_MAPPING_OVERRIDES`] applied.
///
/// `GilrsBuilder::add_mappings` cannot override a bundled entry: `build()`
/// loads the bundled database after it, replacing same-UUID mappings. Only
/// `SDL_GAMECONTROLLERCONFIG` loads last, so the overrides are appended to
/// that variable (keeping any user-provided mappings, which still win when
/// they come later in the value) before building.
// `gilrs::Error` is a third-party type returned once, at startup; boxing it
// would only move the allocation without changing any caller.
#[allow(clippy::result_large_err)]
pub fn new_gilrs(default_filters: bool) -> Result<gilrs::Gilrs, gilrs::Error> {
    const VAR: &str = "SDL_GAMECONTROLLERCONFIG";
    let user = std::env::var(VAR).unwrap_or_default();
    std::env::set_var(VAR, format!("{GILRS_MAPPING_OVERRIDES}{user}"));
    gilrs::GilrsBuilder::new()
        .with_default_filters(default_filters)
        .build()
}

/// Map a `gilrs` button to its NES bit via the default layout.
///
/// Pure over the button value — never touches a `Gilrs` context or opens a
/// device:
///
/// * East → A(0), South → B(1), Select → Select(2), Start → Start(3),
/// * DPadUp/Down/Left/Right → 4/5/6/7; left-stick directions mirror the dpad.
///
/// Positional, like the NES pad: B sits left/below A, so the bottom face
/// button (Xbox A, SNES B) is NES B and the right one (Xbox B, SNES A) is NES A.
pub fn gilrs_button_to_bit(button: gilrs::Button) -> Option<u8> {
    use gilrs::Button;
    match button {
        Button::East => Some(0),
        Button::South => Some(1),
        Button::Select => Some(2),
        Button::Start => Some(3),
        Button::DPadUp => Some(4),
        Button::DPadDown => Some(5),
        Button::DPadLeft => Some(6),
        Button::DPadRight => Some(7),
        _ => None,
    }
}

/// Map a gamepad-button display name (e.g. `"South"`, `"DPadUp"`) through a
/// string table (usually [`crate::config::GamepadBindings`]).
pub fn gamepad_name_to_bit(
    table: &std::collections::HashMap<String, u8>,
    button_name: &str,
) -> Option<u8> {
    table.get(button_name).copied().filter(|&b| b < 8)
}

/// The eight `gilrs` buttons the config tables can name, in bit order.
///
/// This is the bridge that makes `config.gamepad` / `config.gamepad_p2`
/// actually do something: the windowed loop asks `gilrs` whether each of
/// these is pressed and folds the bit the *table* gives it, instead of the
/// old hard-coded layout. The list is the domain of the binding tables, not
/// a layout — [`crate::config::GamepadBindings::default`] still supplies the
/// same default mapping as before.
pub const BINDABLE_BUTTONS: [(&str, gilrs::Button); 8] = [
    ("South", gilrs::Button::South),
    ("East", gilrs::Button::East),
    ("West", gilrs::Button::West),
    ("North", gilrs::Button::North),
    ("Select", gilrs::Button::Select),
    ("Start", gilrs::Button::Start),
    ("DPadUp", gilrs::Button::DPadUp),
    ("DPadDown", gilrs::Button::DPadDown),
];

/// Extra d-pad buttons (kept out of [`BINDABLE_BUTTONS`] only to keep that
/// array at the eight most commonly rebound entries); the loop polls both.
pub const BINDABLE_DPAD: [(&str, gilrs::Button); 2] = [
    ("DPadLeft", gilrs::Button::DPadLeft),
    ("DPadRight", gilrs::Button::DPadRight),
];

/// Fold a set of pressed button *names* through a binding table into a pad
/// byte. Pure over the names, so it is testable without a gamepad.
#[must_use]
pub fn gamepad_bits<'a>(
    table: &std::collections::HashMap<String, u8>,
    pressed: impl Iterator<Item = &'a str>,
) -> u8 {
    let mut pad = 0u8;
    for name in pressed {
        if let Some(b) = gamepad_name_to_bit(table, name) {
            pad |= 1 << b;
        }
    }
    pad
}

/// Fold `gilrs` axis values into dpad bits (called once per frame).
///
/// Dead-zone default 0.5: `lx < -dz` → Left, `lx > dz` → Right, and likewise
/// for `ly` (note: `gilrs` Y-up is negated here so push-up = Up).
#[must_use]
pub fn axis_to_bits(lx: f32, ly: f32, deadzone: f32) -> u8 {
    let mut pad = 0u8;
    if lx < -deadzone {
        pad |= BTN_LEFT;
    } else if lx > deadzone {
        pad |= BTN_RIGHT;
    }
    if ly > deadzone {
        pad |= BTN_UP;
    } else if ly < -deadzone {
        pad |= BTN_DOWN;
    }
    pad
}

/// Live keyboard state tracker (pressed-key set → pad byte).
///
/// The windowed loop feeds `winit` key names in; the emulator reads
/// [`KeyboardState::pad`] once per frame. Pure — no `winit` import.
#[derive(Debug, Clone, Default)]
pub struct KeyboardState {
    held: std::collections::HashSet<String>,
}

impl KeyboardState {
    /// Empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record press/release of a `winit` key-code debug name.
    pub fn set(&mut self, key_name: &str, pressed: bool) {
        if pressed {
            self.held.insert(key_name.to_string());
        } else {
            self.held.remove(key_name);
        }
    }

    /// Current pad byte under `table`.
    pub fn pad(&self, table: &std::collections::HashMap<String, u8>) -> u8 {
        let mut pad = 0u8;
        for k in &self.held {
            if let Some(b) = key_name_to_bit(table, k) {
                pad |= 1 << b;
            }
        }
        pad
    }

    /// Clear all held keys (e.g. on focus loss when pausing).
    pub fn clear(&mut self) {
        self.held.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn table() -> HashMap<String, u8> {
        crate::config::KeyBindings::default().map
    }

    #[test]
    fn shared_contract_bits_are_0_through_7() {
        assert_eq!(
            BUTTON_MASKS,
            [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]
        );
        for (i, (_, bit)) in BUTTON_BITS.iter().enumerate() {
            assert_eq!(*bit as usize, i);
        }
    }

    #[test]
    fn default_keys_cover_all_buttons() {
        let t = table();
        let mut seen = [false; 8];
        for b in t.values() {
            seen[*b as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "every bit 0..7 bound: {t:?}");
    }

    #[test]
    fn combine_is_or() {
        assert_eq!(combine_inputs(BTN_A, BTN_B), BTN_A | BTN_B);
        assert_eq!(combine_inputs(0xFF, 0x00), 0xFF);
    }

    #[test]
    fn with_button_sets_and_clears() {
        assert_eq!(with_button(0, 0, true), BTN_A);
        assert_eq!(with_button(BTN_A, 0, false), 0);
        assert_eq!(with_button(0, 9, true), 0, "out-of-range bit ignored");
    }

    #[test]
    fn keyboard_state_folds_held_keys() {
        let t = table();
        let mut k = KeyboardState::new();
        k.set("KeyZ", true);
        k.set("ArrowUp", true);
        assert_eq!(k.pad(&t), BTN_A | BTN_UP);
        k.set("KeyZ", false);
        assert_eq!(k.pad(&t), BTN_UP);
        k.clear();
        assert_eq!(k.pad(&t), 0);
    }

    #[test]
    fn gilrs_default_layout_matches_contract() {
        use gilrs::Button;
        assert_eq!(gilrs_button_to_bit(Button::East), Some(0));
        assert_eq!(gilrs_button_to_bit(Button::South), Some(1));
        assert_eq!(gilrs_button_to_bit(Button::Select), Some(2));
        assert_eq!(gilrs_button_to_bit(Button::Start), Some(3));
        assert_eq!(gilrs_button_to_bit(Button::DPadUp), Some(4));
        assert_eq!(gilrs_button_to_bit(Button::DPadDown), Some(5));
        assert_eq!(gilrs_button_to_bit(Button::DPadLeft), Some(6));
        assert_eq!(gilrs_button_to_bit(Button::DPadRight), Some(7));
        assert_eq!(gilrs_button_to_bit(Button::North), None);
    }

    #[test]
    fn config_gamepad_table_reproduces_the_default_layout() {
        // The default table must agree with `gilrs_button_to_bit`: the
        // positional NES layout, right face button A and bottom face button B.
        let t = crate::config::GamepadBindings::default().map;
        assert_eq!(gamepad_bits(&t, ["East"].into_iter()), BTN_A);
        assert_eq!(gamepad_bits(&t, ["South"].into_iter()), BTN_B);
        assert_eq!(gamepad_bits(&t, ["Select"].into_iter()), BTN_SELECT);
        assert_eq!(gamepad_bits(&t, ["Start"].into_iter()), BTN_START);
        assert_eq!(gamepad_bits(&t, ["DPadUp"].into_iter()), BTN_UP);
        assert_eq!(gamepad_bits(&t, ["DPadDown"].into_iter()), BTN_DOWN);
        assert_eq!(gamepad_bits(&t, ["DPadLeft"].into_iter()), BTN_LEFT);
        assert_eq!(gamepad_bits(&t, ["DPadRight"].into_iter()), BTN_RIGHT);
        // Unbound buttons contribute nothing; combinations OR.
        assert_eq!(gamepad_bits(&t, ["North", "West"].into_iter()), 0);
        assert_eq!(
            gamepad_bits(&t, ["East", "DPadLeft"].into_iter()),
            BTN_A | BTN_LEFT
        );
    }

    #[test]
    fn rebinding_the_gamepad_table_is_honoured() {
        // Swap A and B away from the default: the fold must follow the table,
        // not the button name.
        let mut t = crate::config::GamepadBindings::default().map;
        t.insert("South".into(), 0);
        t.insert("East".into(), 1);
        assert_eq!(gamepad_bits(&t, ["South"].into_iter()), BTN_A);
        assert_eq!(gamepad_bits(&t, ["East"].into_iter()), BTN_B);
        // Out-of-range bits are rejected rather than shifting off the byte.
        t.insert("North".into(), 9);
        assert_eq!(gamepad_bits(&t, ["North"].into_iter()), 0);
    }

    #[test]
    fn every_bindable_button_has_a_stable_name() {
        for (name, button) in BINDABLE_BUTTONS.iter().chain(BINDABLE_DPAD.iter()) {
            // The name the config uses must be the gilrs Debug name, so a
            // user can read a button name off the connect log and bind it.
            assert_eq!(&format!("{button:?}"), name);
        }
    }

    #[test]
    fn second_player_default_keys_are_disjoint_from_player_one() {
        let p1 = crate::config::KeyBindings::default();
        let p2 = crate::config::KeyBindings::default_p2();
        for k in p2.map.keys() {
            assert!(
                !p1.map.contains_key(k),
                "P2 key {k} collides with a P1 binding"
            );
        }
        // P2 covers every button, on the documented WASD+GFRT layout.
        assert_eq!(p2.bit_for("KeyG"), Some(0), "G = A");
        assert_eq!(p2.bit_for("KeyF"), Some(1), "F = B");
        assert_eq!(p2.bit_for("KeyR"), Some(2), "R = Select");
        assert_eq!(p2.bit_for("KeyT"), Some(3), "T = Start");
        assert_eq!(p2.bit_for("KeyW"), Some(4));
        assert_eq!(p2.bit_for("KeyS"), Some(5));
        assert_eq!(p2.bit_for("KeyA"), Some(6));
        assert_eq!(p2.bit_for("KeyD"), Some(7));
        let mut seen = [false; 8];
        for b in p2.map.values() {
            seen[*b as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "P2 binds all eight buttons");
    }

    #[test]
    fn two_keyboard_tables_read_one_held_set_independently() {
        // One KeyboardState, two tables: this is exactly how local co-op
        // splits a single keyboard between the two players.
        let p1 = crate::config::KeyBindings::default().map;
        let p2 = crate::config::KeyBindings::default_p2().map;
        let mut k = KeyboardState::new();
        k.set("KeyZ", true); // P1 A
        k.set("KeyW", true); // P2 Up
        k.set("KeyG", true); // P2 A
        assert_eq!(k.pad(&p1), BTN_A, "P1 sees only its own keys");
        assert_eq!(k.pad(&p2), BTN_UP | BTN_A, "P2 sees only its own keys");
        k.set("KeyZ", false);
        assert_eq!(k.pad(&p1), 0);
        assert_eq!(k.pad(&p2), BTN_UP | BTN_A, "releasing P1's key leaves P2");
    }

    #[test]
    fn axis_deadzone_maps_to_dpad() {
        assert_eq!(axis_to_bits(-0.9, 0.0, 0.5), BTN_LEFT);
        assert_eq!(axis_to_bits(0.9, 0.0, 0.5), BTN_RIGHT);
        assert_eq!(axis_to_bits(0.0, 0.9, 0.5), BTN_UP);
        assert_eq!(axis_to_bits(0.0, -0.9, 0.5), BTN_DOWN);
        assert_eq!(axis_to_bits(0.1, 0.1, 0.5), 0);
    }
}
