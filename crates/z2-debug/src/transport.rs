//! Frame transport: snapshot-history ring + movie transport.
//!
//! Two cooperating pieces, both plain data over read-only snapshots:
//!
//! * [`HistoryRing`] — a bounded ring of per-frame RAM snapshots. The
//!   frontend pushes one [`HistoryEntry`] per emulated frame; the overlay's
//!   step/rewind controls move a cursor over it. Rewind *displays* an older
//!   snapshot; actually resuming emulation from it is the frontend's job
//!   (`restore_ram` hands back the bytes; the frontend loads them into its
//!   `Game`/oracle and truncates the future — see the embed sequence in the
//!   [crate root](crate)).
//! * [`Transport`] — movie transport over a pad-byte track (`&[u8]`, one NES
//!   byte per frame, LSB-first `A B Sel Sta U D L R`). Frontends parse
//!   `.fm2`/`.bk2` with the `z2_verify` parsers and hand the overlay
//!   the player-1 track (`Fm2Movie::pad1_track`); the overlay never parses
//!   movies itself, keeping it parser-free and wasm-light.

use std::collections::VecDeque;

use z2_core::facts::{Game, GameFacts};
use z2_core::ram::Ram;

/// One history entry: full RAM image + exported facts + frame index.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Emulated frame index at capture.
    pub frame: u64,
    /// Full 2 KiB RAM image.
    pub ram: [u8; Ram::LEN],
    /// Facts exported from that image.
    pub facts: GameFacts,
}

impl HistoryEntry {
    /// Capture from live RAM (frontend-side constructor).
    #[must_use]
    pub fn capture(frame: u64, ram: &Ram) -> Self {
        Self {
            frame,
            ram: *ram.as_slice(),
            facts: Game::new(ram.clone()).facts(),
        }
    }
}

/// Bounded snapshot-history ring with a display cursor.
#[derive(Debug, Clone)]
pub struct HistoryRing {
    entries: VecDeque<HistoryEntry>,
    /// Maximum entries retained (oldest evicted first).
    pub capacity: usize,
    /// Display cursor: index into `entries` (`None` = live tail).
    cursor: Option<usize>,
}

/// Default ring capacity (frames of rewindable history).
pub const DEFAULT_HISTORY_CAPACITY: usize = 600;

impl Default for HistoryRing {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_CAPACITY)
    }
}

impl HistoryRing {
    /// Empty ring holding at most `capacity` entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            cursor: None,
        }
    }

    /// Push one frame; evicts the oldest past capacity. Push always returns
    /// the cursor to the live tail (new input supersedes a rewind view).
    pub fn push(&mut self, entry: HistoryEntry) {
        while self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        self.cursor = None;
    }

    /// Entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Oldest frame held, if any.
    #[must_use]
    pub fn oldest_frame(&self) -> Option<u64> {
        self.entries.front().map(|e| e.frame)
    }

    /// Newest (live) frame held, if any.
    #[must_use]
    pub fn newest_frame(&self) -> Option<u64> {
        self.entries.back().map(|e| e.frame)
    }

    /// The entry under the cursor (`None` cursor = live tail).
    #[must_use]
    pub fn selected(&self) -> Option<&HistoryEntry> {
        match self.cursor {
            None => self.entries.back(),
            Some(i) => self.entries.get(i),
        }
    }

    /// True when viewing history rather than the live tail.
    #[must_use]
    pub fn is_rewound(&self) -> bool {
        self.cursor.is_some()
    }

    /// Step one frame back (stays clamped at the oldest entry).
    pub fn step_back(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let i = match self.cursor {
            None => self.entries.len().saturating_sub(2),
            Some(0) => 0,
            Some(i) => i.saturating_sub(1),
        };
        self.cursor = Some(i);
    }

    /// Step one frame forward; reaching the tail clears the cursor (live).
    pub fn step_forward(&mut self) {
        match self.cursor {
            None => {}
            Some(i) if i + 1 >= self.entries.len() => self.cursor = None,
            Some(i) => self.cursor = Some(i + 1),
        }
    }

    /// Jump to an absolute frame (nearest entry at or below it).
    pub fn jump_to_frame(&mut self, frame: u64) {
        let mut best = None;
        for (i, e) in self.entries.iter().enumerate() {
            if e.frame <= frame {
                best = Some(i);
            } else {
                break;
            }
        }
        self.cursor = match best {
            None if self.entries.is_empty() => None,
            None => Some(0),
            Some(i) if i + 1 == self.entries.len() => None,
            Some(i) => Some(i),
        };
    }

    /// Return to the live tail.
    pub fn go_live(&mut self) {
        self.cursor = None;
    }

    /// RAM bytes under the cursor (for frontend restore on rewind-resume).
    #[must_use]
    pub fn restore_ram(&self) -> Option<([u8; Ram::LEN], u64)> {
        self.selected().map(|e| (e.ram, e.frame))
    }

    /// Render step/rewind controls. Returns nothing; the frontend reads
    /// [`HistoryRing::selected`] afterwards (same-snapshot discipline: the
    /// overlay never touches live state).
    pub fn show_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("|◀ oldest").clicked() {
                self.cursor = if self.entries.is_empty() {
                    None
                } else {
                    Some(0)
                };
            }
            if ui.button("◀ step").clicked() {
                self.step_back();
            }
            if ui.button("step ▶").clicked() {
                self.step_forward();
            }
            if ui.button("live ▶|").clicked() {
                self.go_live();
            }
            match self.selected() {
                None => {
                    ui.label("history: empty");
                }
                Some(e) => {
                    let mark = if self.is_rewound() { "REWOUND" } else { "live" };
                    ui.label(format!("frame {} ({mark})", e.frame));
                }
            }
        });
    }
}

/// Movie transport over a pad-byte track.
#[derive(Debug, Clone, Default)]
pub struct Transport {
    /// Player-1 pad bytes, one per frame (e.g. `Fm2Movie::pad1_track`).
    track: Vec<u8>,
    /// Source label (movie file name, for the panel header).
    pub source: String,
    /// Frame cursor (index into `track`; may equal `track.len()` = end).
    pub cursor: usize,
    /// Playing flag (frontend advances while set).
    pub playing: bool,
}

impl Transport {
    /// Empty transport.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a track (resets cursor, pauses). Returns frames loaded.
    pub fn load_track(&mut self, source: impl Into<String>, track: Vec<u8>) -> usize {
        let n = track.len();
        self.track = track;
        self.source = source.into();
        self.cursor = 0;
        self.playing = false;
        n
    }

    /// Track length in frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.track.len()
    }

    /// True when no track is loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.track.is_empty()
    }

    /// Input byte under the cursor (`0x00` past the end).
    #[must_use]
    pub fn current_input(&self) -> u8 {
        self.track.get(self.cursor).copied().unwrap_or(0x00)
    }

    /// Advance one frame (clamped at the end; stops playing there).
    pub fn advance(&mut self) {
        if self.cursor < self.track.len() {
            self.cursor += 1;
        }
        if self.cursor >= self.track.len() {
            self.playing = false;
        }
    }

    /// Seek (clamped).
    pub fn seek(&mut self, frame: usize) {
        self.cursor = frame.min(self.track.len());
    }

    /// Render transport controls.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = if self.playing {
                "⏸ pause"
            } else {
                "▶ play"
            };
            if ui.button(label).clicked() {
                self.playing = !self.playing && !self.is_empty();
            }
            if ui.button("⏮ restart").clicked() {
                self.seek(0);
            }
            if ui.button("◀ -1").clicked() {
                self.seek(self.cursor.saturating_sub(1));
            }
            if ui.button("+1 ▶").clicked() {
                self.seek(self.cursor + 1);
            }
        });
        if self.is_empty() {
            ui.label("no movie loaded (frontend: parse .fm2/.bk2 via z2_verify, call load_track)");
        } else {
            ui.label(format!(
                "movie: {} ({} frames)",
                self.source,
                self.track.len()
            ));
            let mut cursor = self.cursor;
            ui.horizontal(|ui| {
                ui.label("frame:");
                ui.add(egui::DragValue::new(&mut cursor).range(0..=self.track.len()));
            });
            // Clamp after the drag (the drag may run while a track loads).
            self.seek(cursor);
            ui.monospace(format!("input=${:02X}", self.current_input()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring_with(n: u64) -> HistoryRing {
        let mut ring = HistoryRing::new(8);
        for f in 0..n {
            let mut ram = Ram::new();
            ram.set_frame_counter(f as u8);
            ring.push(HistoryEntry::capture(f, &ram));
        }
        ring
    }

    #[test]
    fn push_evicts_oldest_and_returns_to_live() {
        let ring = ring_with(10);
        assert_eq!(ring.len(), 8);
        assert_eq!(ring.oldest_frame(), Some(2));
        assert_eq!(ring.newest_frame(), Some(9));
        assert!(!ring.is_rewound());
    }

    #[test]
    fn step_back_forward_and_jump() {
        let mut ring = ring_with(5);
        ring.step_back();
        assert!(ring.is_rewound());
        assert_eq!(ring.selected().unwrap().frame, 3);
        ring.step_back();
        ring.step_back();
        ring.step_back();
        ring.step_back(); // clamps at oldest
        assert_eq!(ring.selected().unwrap().frame, 0);
        ring.step_forward();
        assert_eq!(ring.selected().unwrap().frame, 1);
        ring.jump_to_frame(3);
        assert_eq!(ring.selected().unwrap().frame, 3);
        ring.jump_to_frame(4); // newest => live tail
        assert!(!ring.is_rewound());
        ring.jump_to_frame(99); // past end => live tail
        assert!(!ring.is_rewound());
        ring.jump_to_frame(0);
        ring.go_live();
        assert!(!ring.is_rewound());
    }

    #[test]
    fn restore_hands_back_exact_ram() {
        let mut ring = ring_with(4);
        ring.jump_to_frame(1);
        let (bytes, frame) = ring.restore_ram().unwrap();
        assert_eq!(frame, 1);
        let ram = Ram::from_slice(&bytes).unwrap();
        assert_eq!(ram.frame_counter(), 1);
    }

    #[test]
    fn transport_load_play_seek() {
        let mut t = Transport::new();
        assert!(t.is_empty());
        assert_eq!(t.load_track("m.fm2", vec![0x01, 0x80, 0x00]), 3);
        assert_eq!(t.current_input(), 0x01);
        t.playing = true;
        t.advance();
        assert_eq!(t.current_input(), 0x80);
        t.seek(99);
        assert_eq!(t.cursor, 3);
        assert_eq!(t.current_input(), 0x00); // past-end pads zero
        t.advance(); // clamped, stops
        assert!(!t.playing);
    }

    #[test]
    fn headless_transport_renders_without_panic() {
        let mut ring = ring_with(3);
        let mut t = Transport::new();
        t.load_track("m.fm2", vec![0xFF; 60]);
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ring.show_controls(ui);
                t.show(ui);
            });
        });
    }
}
