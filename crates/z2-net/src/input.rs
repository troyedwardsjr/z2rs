//! Per-player input log: frame-indexed pads with a sliding base.

use std::collections::VecDeque;

/// Frames accepted beyond the base (bounds memory against a misbehaving peer).
pub(crate) const MAX_WINDOW: u32 = 1024;

/// Result of [`InputLog::insert`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Insert {
    New,
    Same,
    Conflict,
    Stale,
    TooFar,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InputLog {
    /// Frame of `slots[0]`.
    base: u32,
    slots: VecDeque<Option<u8>>,
    /// Every frame `< contiguous` is known.
    contiguous: u32,
}

impl InputLog {
    pub(crate) fn get(&self, frame: u32) -> Option<u8> {
        let idx = frame.checked_sub(self.base)?;
        self.slots.get(idx as usize).copied().flatten()
    }

    pub(crate) fn insert(&mut self, frame: u32, pad: u8) -> Insert {
        let Some(off) = frame.checked_sub(self.base) else {
            return Insert::Stale;
        };
        if off >= MAX_WINDOW {
            return Insert::TooFar;
        }
        let idx = off as usize;
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        match self.slots[idx] {
            Some(p) if p == pad => Insert::Same,
            Some(_) => Insert::Conflict,
            None => {
                self.slots[idx] = Some(pad);
                while self.get(self.contiguous).is_some() {
                    self.contiguous += 1;
                }
                Insert::New
            }
        }
    }

    pub(crate) fn contiguous(&self) -> u32 {
        self.contiguous
    }

    /// Forget frames `< frame` (never beyond the contiguous prefix).
    pub(crate) fn prune_below(&mut self, frame: u32) {
        let target = frame.min(self.contiguous);
        while self.base < target && self.slots.pop_front().is_some() {
            self.base += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_prune() {
        let mut l = InputLog::default();
        assert_eq!(l.insert(1, 5), Insert::New);
        assert_eq!(l.contiguous(), 0);
        assert_eq!(l.insert(0, 4), Insert::New);
        assert_eq!(l.contiguous(), 2);
        assert_eq!(l.insert(1, 5), Insert::Same);
        assert_eq!(l.insert(1, 6), Insert::Conflict);
        assert_eq!(l.insert(MAX_WINDOW, 1), Insert::TooFar);
        l.prune_below(10);
        assert_eq!(l.get(0), None);
        assert_eq!(l.insert(1, 9), Insert::Stale);
        assert_eq!(l.insert(2, 7), Insert::New);
        assert_eq!(l.get(2), Some(7));
        assert_eq!(l.contiguous(), 3);
    }
}
