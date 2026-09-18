//! Rollback cost probes for the browser (`z2.ext.perf.rollback()`).
//!
//! wasm has no clock without `js-sys`, so the page times each call with
//! `performance.now()`; these exports only do the work. [`WebEmu::perf_begin`]
//! saves the live game into slot 0 of a private ring, every probe starts from
//! it, and [`WebEmu::perf_end`] loads it back, so a measurement leaves the game
//! where it was (the audio queue is cleared; the sound engine is not rewound).
//! Refused while a netplay session exists.

use wasm_bindgen::prelude::*;
use z2_core::state::GameState;

use crate::{WebEmu, WebError};

/// Slots in the probe ring; a tick at depth `d` uses slots `0..=d`, so depths
/// up to `PERF_SLOTS - 2` can be probed.
pub const PERF_SLOTS: usize = 32;

/// Probe ring (allocated by `perf_begin`, freed by `perf_end`).
pub(crate) struct PerfRing {
    states: Vec<GameState>,
}

#[wasm_bindgen]
impl WebEmu {
    /// Start a measurement: allocate the probe ring and save the live game.
    pub fn perf_begin(&mut self) -> Result<(), String> {
        if self.net_active() {
            return Err("perf probes are disabled during a netplay session".to_string());
        }
        let game = self
            .game
            .as_ref()
            .ok_or_else(|| WebError::NoRom.to_string())?;
        let mut states: Vec<GameState> = (0..PERF_SLOTS).map(|_| GameState::new()).collect();
        game.save_state_into(&mut states[0]);
        self.perf = Some(Box::new(PerfRing { states }));
        Ok(())
    }

    /// One save into `slot` (1..PERF_SLOTS; slot 0 holds the start state).
    pub fn perf_save(&mut self, slot: usize) -> Result<(), String> {
        let (game, ring) = self.perf_parts()?;
        let slot = slot.clamp(1, PERF_SLOTS - 1);
        game.save_state_into(&mut ring.states[slot]);
        Ok(())
    }

    /// One load from `slot` (0 = the start state).
    pub fn perf_load(&mut self, slot: usize) -> Result<(), String> {
        let (game, ring) = self.perf_parts()?;
        game.load_state(&ring.states[slot.min(PERF_SLOTS - 1)]);
        Ok(())
    }

    /// One emulated two-pad frame; `replay` skips the audio path, as a
    /// rollback re-simulation does.
    pub fn perf_step(&mut self, pad1: u8, pad2: u8, replay: bool) -> Result<(), String> {
        let (game, _) = self.perf_parts()?;
        if replay {
            game.step2(pad1, pad2);
            game.apu.log.clear();
        } else {
            self.step_one2(pad1, pad2);
        }
        Ok(())
    }

    /// One worst-case rollback tick at prediction depth `depth`, in the order
    /// the session hands requests over: load the oldest state, re-simulate
    /// `depth` frames silently (saving each but the first), save and simulate
    /// the new frame with audio, checksum that save, and render once.
    pub fn perf_tick(&mut self, depth: usize) -> Result<(), String> {
        let depth = depth.min(PERF_SLOTS - 2);
        {
            let (game, ring) = self.perf_parts()?;
            game.load_state(&ring.states[0]);
            for i in 0..depth {
                if i > 0 {
                    game.save_state_into(&mut ring.states[i]);
                }
                game.step2(0, 0);
                game.apu.log.clear();
            }
            game.save_state_into(&mut ring.states[depth.max(1)]);
            #[cfg(feature = "net")]
            {
                let _ = crate::netroll::rollback_checksum(&ring.states[depth.max(1)]);
            }
        }
        self.step_one2(0, 0);
        self.render_frame()
    }

    /// End a measurement: restore the start state and free the ring.
    pub fn perf_end(&mut self) -> Result<(), String> {
        if let Some(ring) = self.perf.take() {
            if let Some(game) = self.game.as_mut() {
                game.load_state(&ring.states[0]);
            }
        }
        self.audio_fifo.clear();
        Ok(())
    }
}

impl WebEmu {
    fn perf_parts(&mut self) -> Result<(&mut z2_core::game::Game, &mut PerfRing), String> {
        let Self { game, perf, .. } = self;
        let game = game.as_mut().ok_or_else(|| WebError::NoRom.to_string())?;
        let ring = perf
            .as_deref_mut()
            .ok_or_else(|| "call perf_begin first".to_string())?;
        Ok((game, ring))
    }
}
