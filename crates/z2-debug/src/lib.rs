//! `z2-debug`: egui debug overlay.
//!
//! Pure `egui::Ui` / `egui::Context` calls — no window, no GPU, no platform
//! code — so the same [`DebugOverlay`] embeds in both `z2-native` (eframe or
//! any egui backend) and `z2-web` (egui web runner), and renders headless in
//! tests with a viewport-less [`egui::Context`].
//!
//! # Determinism guard (read this before embedding)
//!
//! The overlay **never touches live emulation state**. Per frame the
//! frontend snapshots what the overlay may read and passes shared borrows
//! in one [`OverlayFrame`]:
//!
//! * `ram: &Ram` — 2 KiB image the frontend copied out of its `Game`/oracle
//!   after stepping (or the [`HistoryRing`](transport::HistoryRing) entry
//!   under the rewind cursor — never the live `Game`).
//! * `facts: &GameFacts` — exported from that same image.
//! * `frame: &IndexedFrame` — the indexed pixels for that frame.
//! * `ppu: &Ppu` — the PPU model state for that frame.
//!
//! Writes go the other way as data: the RAM watch panel stages
//! [`PendingEdit`](watch::PendingEdit)s; the frontend applies them with
//! [`apply_pending`](watch::apply_pending) **between** frames, then steps.
//! Rewind works the same way: [`HistoryRing`](transport::HistoryRing) hands
//! back bytes; the frontend restores them into its own state. Because the
//! overlay only ever reads snapshots and queues data, opening, closing or
//! wiggling it cannot perturb the trajectory — proven by
//! `tests/overlay_determinism.rs` (overlay-open == overlay-closed over a
//! synthetic trajectory, plus a `facts_diff`-empty check).
//!
//! # Embed API (exact frontend call sequence, both frontends)
//!
//! ```ignore
//! use z2_debug::{DebugOverlay, OverlayFrame};
//! use z2_debug::transport::HistoryEntry;
//!
//! // 1. Own one overlay alongside the emulator.
//! let mut overlay = DebugOverlay::new();
//!
//! // 2. Per frame, AFTER stepping the Game/oracle:
//! //    a. snapshot RAM + facts + indexed frame (+ &Ppu if available),
//! history.push(HistoryEntry::capture(frame_no, &ram));
//! //    b. feed the movie transport when playing:
//! if overlay.transport.playing { game.step(overlay.transport.current_input()); overlay.transport.advance(); }
//!    //    c. show the overlay (F1 toggles; also `overlay.toggle()`):
//! overlay.show(&ctx, &OverlayFrame { ram: &ram, facts: &facts, frame: &frame, ppu: &ppu });
//! //    d. between frames, apply staged RAM edits + rewind restores:
//! z2_debug::watch::apply_pending(game_ram_mut, &overlay.drain_edits());
//! if overlay.wants_rewind() { /* load overlay.rewind_bytes() into Game/oracle, truncate history */ }
//! ```
//!
//! # Headless reuse (`--dump-frame` / `window.z2.screenshot()`)
//!
//! Overlay-side note only (frontends own the pixels): the indexed frame the
//! overlay reads is the same [`IndexedFrame`] the headless `--dump-frame`
//! path (native) and `window.z2.screenshot()` (web) must serve —
//! `z2_ppu::indexed_to_rgba` converts it to display bytes and
//! `z2_ppu::write_diff_ppm` dumps oracle-vs-port diffs. This crate only
//! needs the `&IndexedFrame` borrow, so all three consumers (overlay,
//! headless dump, web screenshot) share one render call with no extra work.
//!
//! # Reuse map (read-only; nothing copied)
//!
//! * `z2-core`: [`Ram`](z2_core::ram::Ram) accessors ([`watch`]), `GameFacts`
//!   + `facts_diff` ([`entities`], determinism test).
//! * `z2-verify`: `Lockstep`/`Divergence` vocabulary via
//!   [`DivergenceReport`](divergence::DivergenceReport)`::from_lockstep_parts`
//!   ([`divergence`]); `z2_verify` stays a dev-dependency so `tetanes-core`
//!   never enters the wasm builds.
//! * `z2-ppu`: [`IndexedFrame`](z2_ppu::IndexedFrame), [`diff_indexed`](z2_ppu::diff_indexed),
//!   [`Ppu`](z2_ppu::Ppu) ([`divergence`], [`ppu_view`]).
//! * z2-verify Snapshot codec + movie parsers: consumed frontend-side; the
//!   overlay accepts their outputs (RAM bytes, pad tracks) — see
//!   [`transport::Transport::load_track`].

pub mod divergence;
pub mod entities;
pub mod hitbox;
pub mod ppu_view;
pub mod transport;
pub mod watch;

use z2_core::facts::GameFacts;
use z2_core::ram::Ram;
use z2_ppu::{IndexedFrame, Ppu};

pub use divergence::{DivergenceReport, DivergenceView};
pub use entities::{EnemyRow, EntitiesPanel, ProjectileRow};
pub use hitbox::{BoxKind, HitboxLayer, OverlayRect};
pub use ppu_view::PpuView;
pub use transport::{HistoryRing, Transport};
pub use watch::{PendingEdit, WatchPanel};

/// Read-only per-frame snapshot the frontend hands the overlay. All borrows
/// must describe the SAME frame (live post-step image, or the history entry
/// under the rewind cursor). The overlay keeps none of them.
#[derive(Debug)]
pub struct OverlayFrame<'a> {
    /// RAM image for this frame.
    pub ram: &'a Ram,
    /// Facts exported from `ram`.
    pub facts: &'a GameFacts,
    /// Indexed framebuffer for this frame.
    pub frame: &'a IndexedFrame,
    /// PPU model state for this frame.
    pub ppu: &'a Ppu,
    /// Link's sprite byte `$0080` (for the sword gate; `ram.link_sprite()`).
    pub link_sprite: u8,
    /// Screen scroll X (for hitbox world→screen mapping; `0` if unknown).
    pub scroll_x: u8,
}

/// Which overlay tab is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayTab {
    /// RAM watch + entity tables.
    #[default]
    State,
    /// Hitbox canvas.
    Hitboxes,
    /// PPU nametables + OAM.
    Ppu,
    /// History step/rewind + movie transport.
    Transport,
    /// Divergence report.
    Divergence,
}

/// The composed debug overlay. Owns only UI state (visibility, tabs,
/// panels, history ring, transport, divergence view) — never emulation
/// state.
#[derive(Debug, Default)]
pub struct DebugOverlay {
    /// Whether the window is open (F1 toggles in [`DebugOverlay::show`]).
    pub open: bool,
    /// Active tab.
    pub tab: OverlayTab,
    /// RAM watch panel.
    pub watch: WatchPanel,
    /// Entity tables panel.
    pub entities: EntitiesPanel,
    /// Hitbox layer toggles.
    pub hitboxes: HitboxLayer,
    /// PPU viewer.
    pub ppu_view: PpuView,
    /// Snapshot-history ring (frontend pushes; overlay moves the cursor).
    pub history: HistoryRing,
    /// Movie transport.
    pub transport: Transport,
    /// Divergence view.
    pub divergence: DivergenceView,
}

impl DebugOverlay {
    /// Closed overlay with default panels.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open overlay (for frontends wiring their own hotkey).
    #[must_use]
    pub fn opened() -> Self {
        Self {
            open: true,
            ..Self::default()
        }
    }

    /// Flip [`DebugOverlay::open`].
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Take staged RAM edits out of the watch panel (frontend applies them
    /// via [`watch::apply_pending`] between frames).
    pub fn drain_edits(&mut self) -> Vec<PendingEdit> {
        std::mem::take(&mut self.watch.pending)
    }

    /// True when the history cursor sits on an older entry (frontend should
    /// offer restore-from-rewind).
    #[must_use]
    pub fn wants_rewind(&self) -> bool {
        self.history.is_rewound()
    }

    /// RAM bytes + frame under the rewind cursor (frontend restore source).
    #[must_use]
    pub fn rewind_bytes(&self) -> Option<([u8; Ram::LEN], u64)> {
        self.history.restore_ram()
    }

    /// Show the overlay for `snap` (hotkey handled first so F1 works even
    /// when the window is closed). When rewound, panels read the history
    /// entry under the cursor instead of the live snapshot.
    pub fn show(&mut self, ctx: &egui::Context, snap: &OverlayFrame<'_>) {
        // F1 with no modifiers toggles (matched on the event so Shift+F1
        // etc. keep working in the host app).
        let f1 = ctx.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => *key == egui::Key::F1 && *modifiers == egui::Modifiers::NONE,
                _ => false,
            })
        });
        if f1 {
            self.toggle();
        }
        if !self.open {
            return;
        }
        // Rewind discipline: display the snapshot under the cursor when the
        // user stepped back; live input otherwise. The entry is cloned out
        // of the ring (2 KiB + facts) so panels can take `&mut self` below;
        // the clone happens only while rewound. Pixel/PPU history is NOT
        // stored — frame/ppu stay live (documented gap, see README note in
        // RETURN).
        let rewound: Option<transport::HistoryEntry> = self
            .history
            .selected()
            .filter(|_| self.history.is_rewound())
            .cloned();
        // `Ram` has no zero-copy view over stored bytes (`from_slice`
        // copies); keep the owned copy in a local for the window closure.
        let rewound_ram;
        let (ram, facts): (&Ram, &GameFacts) = match &rewound {
            Some(e) => {
                rewound_ram = Ram::from_slice(&e.ram).expect("history stores full RAM images");
                (&rewound_ram, &e.facts)
            }
            None => (snap.ram, snap.facts),
        };

        // `Window::open` needs `&mut bool` while the body needs `&mut self`:
        // bounce through a local (also lets the X button close the overlay).
        let mut open = self.open;
        egui::Window::new("z2 debug (F1)")
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, OverlayTab::State, "state");
                    ui.selectable_value(&mut self.tab, OverlayTab::Hitboxes, "hitboxes");
                    ui.selectable_value(&mut self.tab, OverlayTab::Ppu, "ppu");
                    ui.selectable_value(&mut self.tab, OverlayTab::Transport, "transport");
                    ui.selectable_value(&mut self.tab, OverlayTab::Divergence, "divergence");
                });
                ui.separator();
                match self.tab {
                    OverlayTab::State => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.heading("RAM watch");
                            self.watch.show(ui, ram);
                            ui.separator();
                            self.entities.show(ui, facts);
                        });
                    }
                    OverlayTab::Hitboxes => {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.hitboxes.show_player, "player");
                            ui.checkbox(&mut self.hitboxes.show_enemies, "enemies");
                        });
                        let mut rects = Vec::new();
                        if self.hitboxes.show_player {
                            rects.extend(hitbox::player_boxes(
                                facts,
                                snap.link_sprite,
                                snap.scroll_x,
                                0,
                                8,
                            ));
                        }
                        if self.hitboxes.show_enemies {
                            rects.extend(
                                hitbox::enemy_markers(facts, snap.scroll_x)
                                    .into_iter()
                                    .map(|(_, r)| r),
                            );
                        }
                        HitboxLayer::show_layer(ui, &rects);
                        let _ = snap.frame; // Pixels ride with the frontend's own frame draw (see headless note).
                    }
                    OverlayTab::Ppu => {
                        self.ppu_view.show(ui, snap.ppu);
                    }
                    OverlayTab::Transport => {
                        ui.heading("frame history");
                        self.history.show_controls(ui);
                        if self.wants_rewind() {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "viewing history — frontend restore resumes from here",
                            );
                        }
                        ui.separator();
                        ui.heading("movie");
                        self.transport.show(ui);
                    }
                    OverlayTab::Divergence => {
                        self.divergence.show(ui);
                    }
                }
            });
        self.open = open;
    }
}

/// Run one headless pass, discarding the font-atlas texture delta.
///
/// egui panics if a pass's `TexturesDelta` is dropped unhandled; real
/// frontends upload it to the GPU, so tests clear it explicitly. Unit tests
/// call this as `crate::run_headless`; integration tests carry their own
/// copy (they cannot see `pub(crate)` items).
#[cfg(test)]
pub(crate) fn run_headless(ctx: &egui::Context, mut f: impl FnMut(&mut egui::Ui)) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| f(ui));
    out.textures_delta.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::HistoryEntry;

    fn snapshot() -> (Ram, GameFacts, IndexedFrame, Ppu) {
        let mut ram = Ram::new();
        ram.set_link_hp(8);
        ram.set_link_x(100);
        let facts = z2_core::facts::Game::new(ram.clone()).facts();
        (ram, facts, [0x0F; 256 * 240], Ppu::new())
    }

    #[test]
    fn closed_overlay_skips_panels_but_handles_hotkey() {
        let (ram, facts, frame, ppu) = snapshot();
        let snap = OverlayFrame {
            ram: &ram,
            facts: &facts,
            frame: &frame,
            ppu: &ppu,
            link_sprite: 0,
            scroll_x: 0,
        };
        let mut overlay = DebugOverlay::new();
        assert!(!overlay.open);
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            overlay.show(ui.ctx(), &snap);
        });
        assert!(!overlay.open);
        overlay.toggle();
        // Every tab renders headless without panic.
        for tab in [
            OverlayTab::State,
            OverlayTab::Hitboxes,
            OverlayTab::Ppu,
            OverlayTab::Transport,
            OverlayTab::Divergence,
        ] {
            overlay.tab = tab;
            let ctx = egui::Context::default();
            crate::run_headless(&ctx, |ui| {
                overlay.show(ui.ctx(), &snap);
            });
        }
    }

    #[test]
    fn rewind_view_reads_history_not_live() {
        let (ram, facts, frame, ppu) = snapshot();
        let mut overlay = DebugOverlay::opened();
        let mut old = Ram::new();
        old.set_link_hp(3);
        overlay.history.push(HistoryEntry::capture(0, &old));
        overlay.history.push(HistoryEntry::capture(1, &ram));
        overlay.history.step_back();
        assert!(overlay.wants_rewind());
        let (bytes, f) = overlay.rewind_bytes().unwrap();
        assert_eq!(f, 0);
        assert_eq!(Ram::from_slice(&bytes).unwrap().link_hp(), 3);
        let snap = OverlayFrame {
            ram: &ram,
            facts: &facts,
            frame: &frame,
            ppu: &ppu,
            link_sprite: 0,
            scroll_x: 0,
        };
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            overlay.show(ui.ctx(), &snap);
        });
    }

    #[test]
    fn drain_edits_empties_the_queue() {
        let mut overlay = DebugOverlay::new();
        overlay.watch.pending.push(PendingEdit {
            name: "link_hp",
            addr: 0x0774,
            width: 1,
            value: 5,
        });
        let edits = overlay.drain_edits();
        assert_eq!(edits.len(), 1);
        assert!(overlay.watch.pending.is_empty());
    }
}
