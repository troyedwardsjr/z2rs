//! Determinism guard: overlay-open == overlay-closed.
//!
//! Synthetic trajectory (no ROM, no oracle): a tiny wrapping-add sim stands
//! in for the Game. The overlay renders every frame through a headless
//! viewport-less `egui::Context` in the "open" run; the "closed" run never
//! touches it. Both runs must produce identical per-frame hashes, an empty
//! `facts_diff` between their final snapshots, and byte-identical inputs
//! before/after every `show` (the overlay must not mutate what it reads, nor
//! stage edits on its own).

use z2_core::facts::{facts_diff, Game};
use z2_core::ram::Ram;
use z2_debug::transport::{HistoryEntry, HistoryRing};
use z2_debug::{DebugOverlay, OverlayFrame};
use z2_ppu::{IndexedFrame, Ppu};

/// Tiny deterministic stand-in for the Game: 64 bytes advanced by a
/// wrapping add of the input + frame counter each step.
struct Sim {
    state: [u8; 64],
    frame: u64,
}

impl Sim {
    fn new() -> Self {
        Self {
            state: [0; 64],
            frame: 0,
        }
    }

    fn step(&mut self, input: u8) {
        self.frame += 1;
        let m = input.wrapping_add(self.frame as u8).wrapping_add(1);
        for b in &mut self.state {
            *b = b.wrapping_add(m);
        }
    }

    fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in &self.state {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= self.frame;
        h = h.wrapping_mul(0x100000001b3);
        h
    }

    /// Project into overlay inputs: RAM carries the state in the low bytes,
    /// the indexed frame is a deterministic projection of it.
    fn snapshot(&self) -> (Ram, IndexedFrame) {
        let mut ram = Ram::new();
        for (i, &b) in self.state.iter().enumerate() {
            ram.write(i as u16, b);
        }
        ram.write(0x0012, self.frame as u8);
        let mut frame: IndexedFrame = [0; 256 * 240];
        for (i, px) in frame.iter_mut().enumerate() {
            *px = self.state[i % 64] & 0x3F;
        }
        (ram, frame)
    }
}

fn track() -> Vec<u8> {
    (0..120u16)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(11))
        .collect()
}

/// Headless pass that discards the font-atlas delta (see
/// `z2_debug` docs: frontends upload it to the GPU instead).
fn run_headless(ctx: &egui::Context, mut f: impl FnMut(&mut egui::Ui)) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| f(ui));
    out.textures_delta.clear();
}

/// Drive one trajectory; when `with_overlay` render the full overlay every
/// frame through a headless context and exercise every panel.
fn drive(with_overlay: bool) -> (Vec<u64>, Ram) {
    let inputs = track();
    let mut sim = Sim::new();
    let mut overlay = if with_overlay {
        DebugOverlay::opened()
    } else {
        DebugOverlay::new()
    };
    let mut history = HistoryRing::new(64);
    let mut hashes = Vec::with_capacity(inputs.len());
    // Show every tab at least once in the open run.
    let tabs = [
        z2_debug::OverlayTab::State,
        z2_debug::OverlayTab::Hitboxes,
        z2_debug::OverlayTab::Ppu,
        z2_debug::OverlayTab::Transport,
        z2_debug::OverlayTab::Divergence,
    ];
    for (i, &input) in inputs.iter().enumerate() {
        sim.step(input);
        hashes.push(sim.hash());
        if with_overlay {
            let (ram, frame) = sim.snapshot();
            let facts = Game::new(ram.clone()).facts();
            let ppu = Ppu::new();
            let before = *ram.as_slice();
            overlay.tab = tabs[i % tabs.len()];
            history.push(HistoryEntry::capture(sim.frame, &ram));
            let snap = OverlayFrame {
                ram: &ram,
                facts: &facts,
                frame: &frame,
                ppu: &ppu,
                link_sprite: ram.link_sprite(),
                scroll_x: 0,
            };
            let ctx = egui::Context::default();
            run_headless(&ctx, |ui| {
                overlay.show(ui.ctx(), &snap);
            });
            // Guard 1: the overlay never mutates the snapshot it read.
            assert_eq!(
                *ram.as_slice(),
                before,
                "overlay mutated its input at frame {i}"
            );
            // Guard 2: merely rendering stages no edits (edits need clicks).
            assert!(
                overlay.watch.pending.is_empty(),
                "overlay staged edits unprompted at frame {i}"
            );
            // Drain path stays empty too.
            assert!(overlay.drain_edits().is_empty());
        }
    }
    let (ram, _) = sim.snapshot();
    (hashes, ram)
}

#[test]
fn overlay_open_matches_overlay_closed() {
    let (closed_hashes, closed_ram) = drive(false);
    let (open_hashes, open_ram) = drive(true);
    assert_eq!(
        open_hashes, closed_hashes,
        "trajectories diverge with overlay open"
    );
    let a = Game::new(closed_ram).facts();
    let b = Game::new(open_ram).facts();
    assert!(
        facts_diff(&a, &b).is_empty(),
        "final facts differ: {:?}",
        facts_diff(&a, &b)
    );
}

#[test]
fn history_rewind_view_does_not_perturb_live() {
    // Rewind-display + go-live roundtrip leaves the live trajectory intact.
    let inputs = track();
    let mut sim = Sim::new();
    let mut overlay = DebugOverlay::opened();
    for &input in &inputs {
        sim.step(input);
        let (ram, _) = sim.snapshot();
        overlay.history.push(HistoryEntry::capture(sim.frame, &ram));
    }
    overlay.history.jump_to_frame(10);
    assert!(overlay.wants_rewind());
    let live_hash = sim.hash();
    // Render the rewound view headlessly (this is what a frontend does while
    // paused on history).
    let (ram, frame) = sim.snapshot();
    let facts = Game::new(ram.clone()).facts();
    let ppu = Ppu::new();
    let snap = OverlayFrame {
        ram: &ram,
        facts: &facts,
        frame: &frame,
        ppu: &ppu,
        link_sprite: 0,
        scroll_x: 0,
    };
    let ctx = egui::Context::default();
    run_headless(&ctx, |ui| {
        overlay.show(ui.ctx(), &snap);
    });
    overlay.history.go_live();
    assert!(!overlay.wants_rewind());
    assert_eq!(
        sim.hash(),
        live_hash,
        "viewing history perturbed live state"
    );
}
