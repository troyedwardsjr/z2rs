//! Divergence view.
//!
//! Read-only reuse of the `z2-verify` lockstep vocabulary and the `z2-ppu`
//! [`diff_indexed`](z2_ppu::diff_indexed): the frontend runs the real
//! `z2_verify::Lockstep` (oracle vs port), converts the resulting
//! [`Divergence`](z2_verify::lockstep::Divergence) field-for-field with
//! [`DivergenceReport::from_lockstep_parts`], and hands the overlay the
//! report plus both sides' RAM/frames. The overlay then shows the
//! oracle-vs-port RAM side-by-side (mismatch highlighted, `last_writer`
//! attributed) and the framebuffer diff summary. The overlay never steps
//! either side itself (see the determinism pattern in the
//! [crate root](crate)).
//!
//! `z2_verify` is intentionally NOT a library dependency (it would drag
//! `tetanes-core` into both frontends' wasm builds); the field-for-field
//! adapter plus the dev-dependency cross-check in `tests/divergence_reuse.rs`
//! is the reuse mechanism.

use z2_ppu::{diff_indexed, IndexedFrame};

/// First-divergence report, mirroring `z2_verify::lockstep::Divergence`
/// field for field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergenceReport {
    /// 0-based frame index of the first mismatch.
    pub frame: u64,
    /// Region short name (`Region::as_str`: `zp+stack`, `ram`, `wram`,
    /// `oam`, `palette`, `frame`).
    pub region: &'static str,
    /// Byte address (CPU address for RAM/WRAM, byte index for OAM/frame).
    pub addr: u32,
    /// Oracle (reference) byte.
    pub expected: u8,
    /// Port (device-under-test) byte.
    pub actual: u8,
    /// Producing write (`Game::last_writer` attribution), if known.
    pub last_writer: Option<String>,
}

impl DivergenceReport {
    /// Field-for-field adapter from a real lockstep divergence. Call as
    /// `DivergenceReport::from_lockstep_parts(d.frame, d.region.as_str(),
    /// d.addr, d.expected, d.actual, d.last_writer.clone())`.
    #[must_use]
    pub fn from_lockstep_parts(
        frame: u64,
        region: &'static str,
        addr: u32,
        expected: u8,
        actual: u8,
        last_writer: Option<String>,
    ) -> Self {
        Self {
            frame,
            region,
            addr,
            expected,
            actual,
            last_writer,
        }
    }

    /// One-line greppable form (same shape as `Divergence::to_string`).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut s = format!(
            "frame {}: {} {:#06X} expected {:#04X} actual {:#04X}",
            self.frame, self.region, self.addr, self.expected, self.actual,
        );
        if let Some(w) = &self.last_writer {
            s.push_str(&format!(" last_writer={w}"));
        }
        s
    }
}

/// RAM-only first-difference scan in lockstep compare order (zero page +
/// stack `$0000-$01FF` first, then `$0200-$07FF`). This is the overlay-side
/// fast path for the side-by-side view; the authoritative report still
/// comes from the frontend's `Lockstep::run` (which also covers WRAM, OAM,
/// palette and the framebuffer).
#[must_use]
pub fn find_first_ram_diff(
    oracle_ram: &[u8; 2048],
    dut_ram: &[u8; 2048],
    frame: u64,
    last_writer: Option<String>,
) -> Option<DivergenceReport> {
    for (i, (&e, &a)) in oracle_ram.iter().zip(dut_ram.iter()).enumerate() {
        if e != a {
            return Some(DivergenceReport {
                frame,
                region: if i < 0x200 { "zp+stack" } else { "ram" },
                addr: i as u32,
                expected: e,
                actual: a,
                last_writer,
            });
        }
    }
    None
}

/// Divergence view state: the loaded report, both sides' RAM windows and
/// optional frame pair for the pixel diff.
#[derive(Debug, Clone, Default)]
pub struct DivergenceView {
    /// Loaded report (None = no divergence loaded).
    pub report: Option<DivergenceReport>,
    /// Oracle CPU RAM (`$0000-$07FF`).
    pub oracle_ram: Vec<u8>,
    /// Port CPU RAM (`$0000-$07FF`).
    pub dut_ram: Vec<u8>,
    /// Oracle indexed frame at the divergence frame.
    pub oracle_frame: Option<IndexedFrame>,
    /// Port indexed frame at the divergence frame.
    pub dut_frame: Option<IndexedFrame>,
    /// Frame the overlay wants displayed (`jump to frame` sets this; the
    /// frontend drives its history/transport cursors from it).
    pub wanted_frame: Option<u64>,
}

impl DivergenceView {
    /// Empty view.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a report plus both RAM sides (frames optional; set them for the
    /// pixel-diff pane). Arms `wanted_frame` to the report frame.
    pub fn load_report(
        &mut self,
        report: DivergenceReport,
        oracle_ram: Vec<u8>,
        dut_ram: Vec<u8>,
        oracle_frame: Option<IndexedFrame>,
        dut_frame: Option<IndexedFrame>,
    ) {
        self.wanted_frame = Some(report.frame);
        self.report = Some(report);
        self.oracle_ram = oracle_ram;
        self.dut_ram = dut_ram;
        self.oracle_frame = oracle_frame;
        self.dut_frame = dut_frame;
    }

    /// Clear the view.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Pixel diff of the loaded frame pair (None unless both are present).
    /// Direct reuse of `z2_ppu::diff_indexed`.
    #[must_use]
    pub fn frame_diff(&self) -> Option<z2_ppu::FrameDiff> {
        match (&self.oracle_frame, &self.dut_frame) {
            (Some(a), Some(b)) => Some(diff_indexed(a, b)),
            _ => None,
        }
    }

    /// Render the view: report header, jump control, RAM side-by-side with
    /// the mismatch highlighted, and the framebuffer diff summary.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let Some(rep) = self.report.clone() else {
            ui.label("no divergence loaded (frontend: Lockstep::run → from_lockstep_parts → load_report)");
            return;
        };
        ui.monospace(rep.summary());
        ui.horizontal(|ui| {
            if ui.button(format!("jump to frame {}", rep.frame)).clicked() {
                self.wanted_frame = Some(rep.frame);
            }
            if ui.button("clear").clicked() {
                self.clear();
            }
        });
        if self.report.is_none() {
            return; // cleared above — nothing left to show
        }
        if let Some(w) = &rep.last_writer {
            ui.colored_label(egui::Color32::YELLOW, format!("last writer: {w}"));
        }
        ui.separator();
        ui.heading("RAM oracle-vs-port");
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                ui.monospace(ram_side_by_side(&self.oracle_ram, &self.dut_ram, rep.addr));
            });
        ui.separator();
        ui.heading("framebuffer diff");
        match self.frame_diff() {
            None => {
                ui.label("frame pair not loaded");
            }
            Some(d) => {
                ui.monospace(frame_diff_summary(&rep, &d));
            }
        }
    }
}

/// Side-by-side hex around `addr` (8 rows x 16 bytes, `addr` row first when
/// in range). The mismatching byte (when both sides have it) is wrapped in
/// `>> <<` markers — text highlight that survives headless dumps and both
/// frontends without rich-text plumbing.
#[must_use]
pub fn ram_side_by_side(oracle: &[u8], dut: &[u8], addr: u32) -> String {
    let mut out = String::from("addr   : oracle             port               ^ = mismatch\n");
    let base = (addr & !0xF) as usize;
    for row in 0..8usize {
        let a = base + row * 16;
        if a >= oracle.len().max(dut.len()) && row > 0 {
            break;
        }
        out.push_str(&format!("${a:04X} : "));
        for i in 0..16usize {
            let idx = a + i;
            let o = oracle.get(idx).copied().unwrap_or(0);
            out.push_str(&format!("{o:02X}"));
            out.push(if idx as u32 == addr { '>' } else { ' ' });
        }
        out.push_str("  ");
        for i in 0..16usize {
            let idx = a + i;
            let d = dut.get(idx).copied().unwrap_or(0);
            let o = oracle.get(idx).copied().unwrap_or(0);
            let mark = if idx as u32 == addr {
                '>'
            } else if d != o {
                '^'
            } else {
                ' '
            };
            out.push_str(&format!("{d:02X}"));
            out.push(mark);
        }
        out.push('\n');
    }
    out
}

/// One-screen framebuffer diff summary: count + first coordinates.
#[must_use]
pub fn frame_diff_summary(rep: &DivergenceReport, diff: &z2_ppu::FrameDiff) -> String {
    if diff.is_clean() {
        return format!(
            "frame {}: pixels identical (divergence is in {})",
            rep.frame, rep.region
        );
    }
    let mut out = format!(
        "frame {}: {} differing pixels; first:",
        rep.frame, diff.count
    );
    for i in 0..diff.first_len {
        let (x, y) = diff.first[i];
        out.push_str(&format!(" ({x},{y})"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> DivergenceReport {
        DivergenceReport::from_lockstep_parts(12, "ram", 0x456, 0x01, 0x02, None)
    }

    #[test]
    fn summary_shape_is_greppable() {
        let s = report().summary();
        assert!(s.contains("frame 12") && s.contains("ram") && s.contains("0x0456"));
        let w = DivergenceReport::from_lockstep_parts(
            3,
            "ram",
            0x10,
            0,
            1,
            Some("trap@$C123".to_string()),
        );
        assert!(w.summary().contains("last_writer=trap@$C123"));
    }

    #[test]
    fn ram_scan_follows_lockstep_region_order() {
        let mut o = [0u8; 2048];
        let mut d = [0u8; 2048];
        assert!(find_first_ram_diff(&o, &d, 0, None).is_none());
        d[0x456] = 0xA5;
        d[0x42] = 0x7E; // zp fault sorts before the ram fault
        let r = find_first_ram_diff(&o, &d, 7, None).unwrap();
        assert_eq!((r.region, r.addr, r.frame), ("zp+stack", 0x42, 7));
        assert_eq!((r.expected, r.actual), (o[0x42], 0x7E));
        o[0x42] = 0x7E;
        let r = find_first_ram_diff(&o, &d, 7, None).unwrap();
        assert_eq!((r.region, r.addr), ("ram", 0x456));
    }

    #[test]
    fn side_by_side_marks_the_mismatch() {
        let o = [0u8; 2048];
        let mut d = [0u8; 2048];
        d[0x456] = 0xA5;
        let s = ram_side_by_side(&o, &d, 0x456);
        assert!(s.contains("A5>"), "dut mismatch marked:\n{s}");
        assert!(s.contains("$0450"), "addr row present:\n{s}");
    }

    #[test]
    fn frame_diff_needs_both_sides() {
        let mut v = DivergenceView::new();
        assert!(v.frame_diff().is_none());
        v.oracle_frame = Some([0x0F; 256 * 240]);
        assert!(v.frame_diff().is_none());
        let mut dut = [0x0Fu8; 256 * 240];
        dut[100] = 0x01;
        v.dut_frame = Some(dut);
        let diff = v.frame_diff().unwrap();
        assert_eq!(diff.count, 1);
        let s = frame_diff_summary(&report(), &diff);
        assert!(s.contains('1'), "{s}");
    }

    #[test]
    fn headless_view_renders_without_panic() {
        let mut v = DivergenceView::new();
        let o = vec![0u8; 2048];
        let mut d = vec![0u8; 2048];
        d[0x456] = 0x02;
        v.load_report(report(), o, d, None, None);
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                v.show(ui);
            });
        });
        assert_eq!(v.wanted_frame, Some(12));
    }
}
