//! Lag-frame map.
//!
//! `std`-only: compiles standalone (`rustc --test lag.rs`) and later as
//! `z2_verify::lag`.
//!
//! ## Background
//!
//! The NES draws every raw (emulator) frame, but the game only runs its
//! logic on non-lag frames; on lag frames the oracle "skips game logic"
//! (input is still polled, PPU still renders). Zelda II TAS movies therefore
//! contain more raw frames than logic frames. The corpus stores snapshots
//! against **both** indices ([`Snapshot`](crate::snapshot::Snapshot)
//! carries `frame_raw` + `frame_logic`) and ships, per movie, a [`LagMap`]
//! plus the derived **logic-frame input track** alongside the raw track.
//!
//! The oracle owns lag *detection* (e.g. the frame-skip / NMI-run
//! flag it observes per frame). This module owns lag *bookkeeping*: it turns
//! the oracle's per-raw-frame `skipped_logic: &[bool]` feed into maps and
//! filtered tracks. It is deliberately generic over `u8` pad bytes so the
//! `.fm2` and `.bk2` tracks plug in unchanged.

use std::error::Error;
use std::fmt;

/// Per-movie lag map: raw (emulator) frames ↔ logic (game-step) frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LagMap {
    /// Total raw frames in the movie.
    pub raw_frames: u64,
    /// Total logic frames (`raw_frames - skipped.len()`).
    pub logic_frames: u64,
    /// Sorted raw indices where the oracle skipped game logic.
    pub skipped_raw: Vec<u32>,
    /// `logic_to_raw[l] == raw index of logic frame l` (len = logic_frames).
    pub logic_to_raw: Vec<u32>,
}

/// Lag-map build failure (inputs inconsistent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LagError(pub String);

impl fmt::Display for LagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lag: {}", self.0)
    }
}

impl Error for LagError {}

impl LagMap {
    /// Build a map from the oracle feed: `skipped[i] == true` when raw
    /// frame `i` skipped game logic.
    pub fn build(skipped: &[bool]) -> Self {
        let mut skipped_raw = Vec::new();
        let mut logic_to_raw = Vec::with_capacity(skipped.len());
        for (i, &lag) in skipped.iter().enumerate() {
            if lag {
                skipped_raw.push(i as u32);
            } else {
                logic_to_raw.push(i as u32);
            }
        }
        LagMap {
            raw_frames: skipped.len() as u64,
            logic_frames: logic_to_raw.len() as u64,
            skipped_raw,
            logic_to_raw,
        }
    }

    /// Was raw frame `raw` a lag frame?
    pub fn is_lag(&self, raw: u64) -> Option<bool> {
        if raw >= self.raw_frames {
            return None;
        }
        Some(self.skipped_raw.binary_search(&(raw as u32)).is_ok())
    }

    /// Logic index of raw frame `raw` (`None` for lag / out-of-range frames).
    pub fn raw_to_logic(&self, raw: u64) -> Option<u64> {
        if raw >= self.raw_frames || self.is_lag(raw)? {
            return None;
        }
        self.logic_to_raw
            .binary_search(&(raw as u32))
            .ok()
            .map(|l| l as u64)
    }

    /// Raw index of logic frame `logic` (`None` when out of range).
    pub fn logic_to_raw_frame(&self, logic: u64) -> Option<u64> {
        self.logic_to_raw.get(logic as usize).map(|&r| r as u64)
    }

    /// Fraction of raw frames that were lag (0.0–1.0; 0.0 when empty).
    pub fn lag_ratio(&self) -> f64 {
        if self.raw_frames == 0 {
            0.0
        } else {
            self.skipped_raw.len() as f64 / self.raw_frames as f64
        }
    }

    /// One-line corpus-log summary, e.g. `raw=125000 logic=119832 lag=5168 (4.13%)`.
    pub fn summary(&self) -> String {
        format!(
            "raw={} logic={} lag={} ({:.2}%)",
            self.raw_frames,
            self.logic_frames,
            self.skipped_raw.len(),
            self.lag_ratio() * 100.0
        )
    }
}

/// Derive the logic-frame input track: `raw_inputs` with lag frames removed.
///
/// Errors when `raw_inputs.len() != map.raw_frames` (caller mixed movies).
/// The returned track is always `<=` the raw track, strictly shorter
/// whenever the movie contains at least one lag frame.
pub fn logic_track(raw_inputs: &[u8], map: &LagMap) -> Result<Vec<u8>, LagError> {
    if raw_inputs.len() as u64 != map.raw_frames {
        return Err(LagError(format!(
            "track len {} != map raw_frames {}",
            raw_inputs.len(),
            map.raw_frames
        )));
    }
    Ok(map
        .logic_to_raw
        .iter()
        .map(|&r| raw_inputs[r as usize])
        .collect())
}

/// Expand a logic-frame track back onto raw frames, holding `fill` on lags.
/// Inverse of [`logic_track`] up to the (unobservable) lag-frame inputs.
pub fn expand_track(logic_inputs: &[u8], map: &LagMap, fill: u8) -> Result<Vec<u8>, LagError> {
    if logic_inputs.len() as u64 != map.logic_frames {
        return Err(LagError(format!(
            "logic track len {} != map logic_frames {}",
            logic_inputs.len(),
            map.logic_frames
        )));
    }
    let mut raw = vec![fill; map.raw_frames as usize];
    for (l, &r) in map.logic_to_raw.iter().enumerate() {
        raw[r as usize] = logic_inputs[l];
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// skipped pattern: frames 1, 4, 5 lag out of 8 raw.
    fn sample() -> (Vec<u8>, LagMap) {
        let skipped = [false, true, false, false, true, true, false, false];
        let raw: Vec<u8> = vec![0x01, 0xFF, 0x02, 0x04, 0xFF, 0xFF, 0x08, 0x10];
        (raw, LagMap::build(&skipped))
    }

    #[test]
    fn logic_track_shorter_than_raw() {
        let (raw, map) = sample();
        let logic = logic_track(&raw, &map).unwrap();
        // Acceptance probe: the logic-frame track is strictly shorter.
        assert!(logic.len() < raw.len());
        assert_eq!(logic.len() as u64, map.logic_frames);
        assert_eq!(logic, vec![0x01, 0x02, 0x04, 0x08, 0x10]);
    }

    #[test]
    fn maps_roundtrip() {
        let (_, map) = sample();
        assert_eq!(map.raw_frames, 8);
        assert_eq!(map.logic_frames, 5);
        assert_eq!(map.skipped_raw, vec![1, 4, 5]);
        assert_eq!(map.raw_to_logic(0), Some(0));
        assert_eq!(map.raw_to_logic(1), None); // lag
        assert_eq!(map.raw_to_logic(2), Some(1));
        assert_eq!(map.logic_to_raw_frame(1), Some(2));
        assert_eq!(map.logic_to_raw_frame(5), None);
        assert_eq!(map.is_lag(4), Some(true));
        assert_eq!(map.is_lag(8), None);
    }

    #[test]
    fn lag_free_movie_is_identity() {
        let map = LagMap::build(&[false; 4]);
        let raw = vec![1, 2, 3, 4];
        assert_eq!(logic_track(&raw, &map).unwrap(), raw);
        assert_eq!(map.lag_ratio(), 0.0);
    }

    #[test]
    fn mismatched_track_errors() {
        let (_, map) = sample();
        assert!(logic_track(&[1, 2, 3], &map).is_err());
        assert!(expand_track(&[1, 2], &map, 0x00).is_err());
    }

    #[test]
    fn expand_inverts_logic_track() {
        let (raw, map) = sample();
        let logic = logic_track(&raw, &map).unwrap();
        let back = expand_track(&logic, &map, 0x00).unwrap();
        assert_eq!(back.len(), raw.len());
        for &r in &map.logic_to_raw {
            assert_eq!(back[r as usize], raw[r as usize]);
        }
    }
}
