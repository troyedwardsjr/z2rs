//! Process-global input and control mailboxes for a host that is not winit.
//!
//! On Android the on-screen touch pad, hardware gamepads and the app's pause /
//! save-state buttons all live on the Kotlin side (`com.z2rs.game.NativeBridge`
//! in `crates/z2-android`), which calls into Rust over JNI from the UI thread.
//! Those calls only ever touch the atomics here; the windowed loop in
//! [`crate::app`] reads them once per emulated frame (pads) or once per loop
//! iteration (requests), on its own thread, so no lock is shared with the UI
//! thread and a JNI call can never stall a frame.
//!
//! Compiled on every target. On desktop nothing writes these, so every mask
//! reads 0 and every request reads "none" — ORing a zero into the pad bytes
//! changes nothing, which keeps desktop, headless and movie runs bit-identical.
//!
//! Pad bytes use the shared NES contract (LSB-first): A=0, B=1, Select=2,
//! Start=3, Up=4, Down=5, Left=6, Right=7 (see [`crate::input`]).

use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

/// On-screen (touch) pad, always player 1.
static TOUCH_MASK: AtomicU8 = AtomicU8::new(0);
/// Hardware gamepads reported by the host, indexed by player (0 = P1, 1 = P2).
static HW_MASK: [AtomicU8; 2] = [AtomicU8::new(0), AtomicU8::new(0)];
/// Pending pause change: [`PAUSE_NONE`], [`PAUSE_ON`] or [`PAUSE_OFF`].
static PAUSE_REQUEST: AtomicU8 = AtomicU8::new(PAUSE_NONE);
/// Pending save-state slot + 1 (0 = none), so slot 0 is representable.
static SAVE_REQUEST: AtomicU16 = AtomicU16::new(0);
/// Pending load-state slot + 1 (0 = none).
static LOAD_REQUEST: AtomicU16 = AtomicU16::new(0);

const PAUSE_NONE: u8 = 0;
const PAUSE_ON: u8 = 1;
const PAUSE_OFF: u8 = 2;

/// Set the on-screen pad's held buttons (replaces the previous mask).
pub fn set_touch_mask(mask: u8) {
    TOUCH_MASK.store(mask, Ordering::Relaxed);
}

/// Set a hardware gamepad's held buttons for `player` (0 or 1). Other player
/// indices are ignored — the game has two ports.
pub fn set_hw_mask(player: usize, mask: u8) {
    if let Some(slot) = HW_MASK.get(player) {
        slot.store(mask, Ordering::Relaxed);
    }
}

/// Release every externally held button (e.g. when the surface goes away, so
/// a finger that was down at suspend does not come back as a stuck button).
pub fn clear_masks() {
    TOUCH_MASK.store(0, Ordering::Relaxed);
    for slot in &HW_MASK {
        slot.store(0, Ordering::Relaxed);
    }
}

/// Player 1's external bits: touch pad OR hardware pad 1.
#[must_use]
pub fn p1_mask() -> u8 {
    TOUCH_MASK.load(Ordering::Relaxed) | HW_MASK[0].load(Ordering::Relaxed)
}

/// Player 2's external bits: hardware pad 2.
#[must_use]
pub fn p2_mask() -> u8 {
    HW_MASK[1].load(Ordering::Relaxed)
}

/// Ask the loop to pause (`true`) or resume (`false`). The latest request
/// wins if several arrive between two loop iterations.
pub fn request_pause(paused: bool) {
    PAUSE_REQUEST.store(if paused { PAUSE_ON } else { PAUSE_OFF }, Ordering::Relaxed);
}

/// Consume a pending pause request (`Some(paused)`), if any.
#[must_use]
pub fn take_pause_request() -> Option<bool> {
    match PAUSE_REQUEST.swap(PAUSE_NONE, Ordering::Relaxed) {
        PAUSE_ON => Some(true),
        PAUSE_OFF => Some(false),
        _ => None,
    }
}

/// Ask the loop to write save-state `slot`.
pub fn request_save_state(slot: u8) {
    SAVE_REQUEST.store(u16::from(slot) + 1, Ordering::Relaxed);
}

/// Ask the loop to load save-state `slot`.
pub fn request_load_state(slot: u8) {
    LOAD_REQUEST.store(u16::from(slot) + 1, Ordering::Relaxed);
}

/// Consume a pending save-state request (the slot), if any.
#[must_use]
pub fn take_save_request() -> Option<u8> {
    take_slot(&SAVE_REQUEST)
}

/// Consume a pending load-state request (the slot), if any.
#[must_use]
pub fn take_load_request() -> Option<u8> {
    take_slot(&LOAD_REQUEST)
}

fn take_slot(cell: &AtomicU16) -> Option<u8> {
    match cell.swap(0, Ordering::Relaxed) {
        0 => None,
        n => u8::try_from(n - 1).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test touches the globals so parallel test threads cannot race it.
    #[test]
    fn masks_and_requests_round_trip_and_are_consumed_once() {
        assert_eq!((p1_mask(), p2_mask()), (0, 0));
        set_touch_mask(0b0000_0001);
        set_hw_mask(0, 0b1000_0000);
        set_hw_mask(1, 0b0000_1000);
        set_hw_mask(7, 0xFF); // out of range: ignored
        assert_eq!(p1_mask(), 0b1000_0001);
        assert_eq!(p2_mask(), 0b0000_1000);
        clear_masks();
        assert_eq!((p1_mask(), p2_mask()), (0, 0));

        assert_eq!(take_pause_request(), None);
        request_pause(true);
        request_pause(false);
        assert_eq!(take_pause_request(), Some(false));
        assert_eq!(take_pause_request(), None);

        request_save_state(0);
        request_load_state(9);
        assert_eq!(take_save_request(), Some(0));
        assert_eq!(take_save_request(), None);
        assert_eq!(take_load_request(), Some(9));
        assert_eq!(take_load_request(), None);
    }
}
