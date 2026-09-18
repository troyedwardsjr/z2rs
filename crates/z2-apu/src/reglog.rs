//! `$4000-$4017` write-log recorder + oracle comparison.
//!
//! Register-log verification is the acceptance path for the sound engine: the engine
//! records every APU register write it emits per frame ([`RegLog`]), and
//! [`compare`] diffs that log against the oracle's log (the `z2-verify`
//! lockstep oracle) over a warpless movie. The full-movie check
//! is **gated on the oracle**; until then, [`compare`] is covered by
//! tests against synthetic logs below.
//!
//! Log entries are plain data (`frame`, `cycle`, `addr`, `value`) so both
//! sides can serialise them without sharing code.

/// Lowest / highest loggable APU register.
pub const REG_FIRST: u16 = 0x4000;
/// Lowest / highest loggable APU register.
pub const REG_LAST: u16 = 0x4017;

/// One APU register write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegWrite {
    /// Video frame the write belongs to (emulator frame index).
    pub frame: u32,
    /// CPU cycle of the write (for ordering within a frame).
    pub cycle: u64,
    /// Register address (`$4000-$4017`).
    pub addr: u16,
    /// Value written.
    pub value: u8,
}

/// Ordered record of APU register writes.
#[derive(Debug, Clone, Default)]
pub struct RegLog {
    entries: Vec<RegWrite>,
}

impl RegLog {
    /// Create an empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one write. Out-of-range addresses (`<$4000` / `>$4017`) are
    /// still stored (the engine must be able to log first, filter later)
    /// and reported by [`RegLog::validate`].
    pub fn record(&mut self, frame: u32, cycle: u64, addr: u16, value: u8) {
        self.entries.push(RegWrite {
            frame,
            cycle,
            addr,
            value,
        });
    }

    /// Number of recorded writes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over entries in record order.
    pub fn iter(&self) -> std::slice::Iter<'_, RegWrite> {
        self.entries.iter()
    }

    /// Drop all entries (frame-boundary reuse without reallocation).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Indices of entries outside `$4000-$4017` (wiring bugs, not game data).
    #[must_use]
    pub fn validate(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.addr < REG_FIRST || e.addr > REG_LAST)
            .map(|(i, _)| i)
            .collect()
    }
}

/// First divergence between two logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch {
    /// Entry index of the divergence.
    pub index: usize,
    /// Expected (oracle) entry; `None` = oracle log ended here.
    pub want: Option<RegWrite>,
    /// Actual (engine) entry; `None` = engine log ended here.
    pub got: Option<RegWrite>,
}

/// Result of [`compare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegCompare {
    /// Entries compared (length of the longer log).
    pub total: usize,
    /// Leading entries that matched exactly.
    pub matched: usize,
    /// First divergence, if any.
    pub first_mismatch: Option<Mismatch>,
}

impl RegCompare {
    /// True when both logs are identical.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.first_mismatch.is_none()
    }
}

impl std::fmt::Display for RegCompare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.first_mismatch {
            None => write!(f, "reglog match: {} entries", self.total),
            Some(m) => {
                write!(
                    f,
                    "reglog mismatch at entry {} (matched {}/{}): want {:?}, got {:?}",
                    m.index, self.matched, self.total, m.want, m.got
                )
            }
        }
    }
}

/// Compare an engine log against the oracle log entry-by-entry.
///
/// `want` is the oracle; `got` is ours. Comparison is
/// exact: same frame, cycle, address and value. Cycle-exactness is the
/// goal; if the oracle turns out to quantise to frame granularity, both
/// sides can mask `cycle` before calling (not here — silent masking
/// would hide real drift).
#[must_use]
pub fn compare(want: &RegLog, got: &RegLog) -> RegCompare {
    let total = want.len().max(got.len());
    for i in 0..total {
        let w = want.entries.get(i).copied();
        let g = got.entries.get(i).copied();
        if w != g {
            return RegCompare {
                total,
                matched: i,
                first_mismatch: Some(Mismatch {
                    index: i,
                    want: w,
                    got: g,
                }),
            };
        }
    }
    RegCompare {
        total,
        matched: total,
        first_mismatch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_with(pairs: &[(u16, u8)]) -> RegLog {
        let mut l = RegLog::new();
        for (i, &(a, v)) in pairs.iter().enumerate() {
            l.record(7, 100 + i as u64, a, v);
        }
        l
    }

    #[test]
    fn identical_logs_match() {
        let a = log_with(&[(0x4000, 0x1F), (0x4002, 0xFF), (0x4003, 0xF8)]);
        let b = a.clone();
        let r = compare(&a, &b);
        assert!(r.matches());
        assert_eq!((r.total, r.matched), (3, 3));
        assert!(a.validate().is_empty());
    }

    #[test]
    fn value_divergence_reports_index_and_sides() {
        let want = log_with(&[(0x4000, 0x1F), (0x4002, 0xFF)]);
        let got = log_with(&[(0x4000, 0x1F), (0x4002, 0xFE)]);
        let r = compare(&want, &got);
        assert!(!r.matches());
        let m = r.first_mismatch.unwrap();
        assert_eq!(m.index, 1);
        assert_eq!(m.want.unwrap().value, 0xFF);
        assert_eq!(m.got.unwrap().value, 0xFE);
        assert_eq!(r.matched, 1);
    }

    #[test]
    fn length_mismatch_reports_truncation() {
        let want = log_with(&[(0x4015, 0x0F)]);
        let got = RegLog::new();
        let r = compare(&want, &got);
        assert!(!r.matches());
        assert_eq!(r.matched, 0);
        let m = r.first_mismatch.unwrap();
        assert!(m.want.is_some() && m.got.is_none());
    }

    #[test]
    fn frame_or_cycle_skew_counts_as_mismatch() {
        let mut a = RegLog::new();
        let mut b = RegLog::new();
        a.record(7, 100, 0x4000, 0x1F);
        b.record(7, 101, 0x4000, 0x1F); // one cycle late
        assert!(!compare(&a, &b).matches());
    }

    #[test]
    fn validate_flags_out_of_range_addresses() {
        let mut l = RegLog::new();
        l.record(0, 0, 0x4000, 0);
        l.record(0, 1, 0x4014, 0); // OAMDMA: in range ($4000-$4017)
        l.record(0, 2, 0x5000, 0); // out of range
        assert_eq!(l.validate(), vec![2]);
    }

    #[test]
    fn empty_logs_match() {
        let r = compare(&RegLog::new(), &RegLog::new());
        assert!(r.matches());
        assert_eq!(r.total, 0);
    }

    #[test]
    fn display_is_human_readable() {
        let a = log_with(&[(0x4000, 1)]);
        let ok = compare(&a, &a.clone()).to_string();
        assert!(ok.contains("match") && ok.contains('1'));
        let bad = compare(&a, &RegLog::new()).to_string();
        assert!(bad.contains("mismatch at entry 0"));
    }
}
