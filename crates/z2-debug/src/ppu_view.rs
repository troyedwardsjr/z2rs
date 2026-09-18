//! PPU nametable / OAM text viewer.
//!
//! Hex + decoded summary over a shared [`Ppu`](z2_ppu::Ppu) borrow (the `z2-ppu`
//! model). Text-only: no textures, no GPU calls, so the viewer runs
//! headless and on wasm unchanged. Frontends pass the same `&Ppu` they
//! render the frame from (see the determinism pattern in the
//! [crate root](crate)).

use z2_ppu::Ppu;

/// Which nametable page the viewer shows (`0=$2000 .. 3=$2C00` logical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NametableSel {
    /// `$2000`.
    #[default]
    Nt0,
    /// `$2400`.
    Nt1,
    /// `$2800`.
    Nt2,
    /// `$2C00`.
    Nt3,
}

impl NametableSel {
    /// Logical slot number.
    #[must_use]
    pub fn slot(self) -> u8 {
        match self {
            NametableSel::Nt0 => 0,
            NametableSel::Nt1 => 1,
            NametableSel::Nt2 => 2,
            NametableSel::Nt3 => 3,
        }
    }

    /// Cycle to the next page.
    pub fn advance(&mut self) {
        *self = match self {
            NametableSel::Nt0 => NametableSel::Nt1,
            NametableSel::Nt1 => NametableSel::Nt2,
            NametableSel::Nt2 => NametableSel::Nt3,
            NametableSel::Nt3 => NametableSel::Nt0,
        };
    }
}

/// Viewer state (page selector + hex-pane scroll are display-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PpuView {
    /// Selected nametable page.
    pub nt: NametableSel,
    /// Show the full 960-byte tile hex (off = first 4 rows + summary only).
    pub full_hex: bool,
}

impl PpuView {
    /// Default viewer (NT0, summary mode).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Render the nametable pane (hex + decoded summary) and the OAM pane.
    pub fn show(&mut self, ui: &mut egui::Ui, ppu: &Ppu) {
        ui.horizontal(|ui| {
            ui.label(format!(
                "nametable ${:04X}",
                0x2000 + u16::from(self.nt.slot()) * 0x400
            ));
            if ui.button("next NT").clicked() {
                self.nt.advance();
            }
            ui.checkbox(&mut self.full_hex, "full hex");
        });
        ui.monospace(nametable_summary(ppu, self.nt.slot()));
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                ui.monospace(nametable_hex(ppu, self.nt.slot(), self.full_hex));
            });
        ui.separator();
        ui.heading("OAM (decoded)");
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                ui.monospace(oam_summary(ppu));
            });
        ui.separator();
        ui.heading("registers");
        ui.monospace(ppu_summary(ppu));
    }
}

/// One-line decoded summary: mirroring, scroll, `v`/`t`, control/mask, sprite-0 hit, CHR
/// pages, plus the most common tile on the page (blank-page detection).
#[must_use]
pub fn nametable_summary(ppu: &Ppu, slot: u8) -> String {
    let base = 0x2000 + u16::from(slot & 3) * 0x400;
    let mut counts = [0u32; 256];
    for i in 0..960usize {
        counts[ppu.nt_read(base + i as u16) as usize] += 1;
    }
    let (top_tile, top_n) = counts
        .iter()
        .enumerate()
        .max_by_key(|(_, &n)| n)
        .map(|(t, &n)| (t, n))
        .unwrap_or((0, 0));
    let (sx, sy) = ppu.scroll();
    let hit = match ppu.sprite0_hit_at() {
        Some((line, x)) => format!("{line}@{x}"),
        None => "none".to_string(),
    };
    format!(
        "mirror={:?} scroll=({sx},{sy}) v=${:04X} t=${:04X} ctrl=${:02X} mask=${:02X} hit={hit} chr_pages=[{},{}] top_tile=${top_tile:02X}x{top_n}",
        ppu.mirroring(),
        ppu.v(),
        ppu.t(),
        ppu.ctrl(),
        ppu.mask(),
        ppu.chr_page_no(0),
        ppu.chr_page_no(1),
    )
}

/// Hex dump of the 30x32 tile plane (plus attribute bytes in full mode).
/// Summary mode shows the first 4 rows; full mode shows all 30 rows + the
/// 64 attribute bytes.
#[must_use]
pub fn nametable_hex(ppu: &Ppu, slot: u8, full: bool) -> String {
    let base = 0x2000 + u16::from(slot & 3) * 0x400;
    let rows = if full { 30 } else { 4 };
    let mut out = String::new();
    for row in 0..rows {
        out.push_str(&format!("r{row:02}:"));
        for col in 0..32 {
            out.push_str(&format!(" {:02X}", ppu.nt_read(base + row * 32 + col)));
        }
        out.push('\n');
    }
    if full {
        out.push_str("attr:");
        for i in 0..64u16 {
            if i % 16 == 0 {
                out.push_str(&format!("\na{i:02X}:"));
            }
            out.push_str(&format!(" {:02X}", ppu.nt_read(base + 0x3C0 + i)));
        }
        out.push('\n');
    } else {
        out.push_str("… (enable full hex for all 30 rows + attributes)\n");
    }
    out
}

/// Decoded OAM: one line per sprite (`i: x,y tile attr` + parked flag).
/// Parked sprites (`y=$FF`, top edge 256 = fully off-screen) are marked.
#[must_use]
pub fn oam_summary(ppu: &Ppu) -> String {
    let mut out = String::new();
    for i in 0..64usize {
        let e = ppu.oam_entry(i);
        let parked = if e.y == 0xFF { " parked" } else { "" };
        out.push_str(&format!(
            "{i:02}: x=${:02X} y=${:02X} tile=${:02X} attr=${:02X}{parked}\n",
            e.x, e.y, e.tile, e.attr
        ));
    }
    out
}

/// Control/mask/status + palette-RAM first bytes, one screen.
#[must_use]
pub fn ppu_summary(ppu: &Ppu) -> String {
    let mut pal = String::new();
    for (i, b) in ppu.palette().iter().enumerate().take(16) {
        if i % 8 == 0 {
            pal.push_str(&format!("\n  ${:04X}:", 0x3F00 + i));
        }
        pal.push_str(&format!(" {b:02X}"));
    }
    format!(
        "ctrl=${:02X} mask=${:02X} status=${:02X} vblank={} s0hit={} overflow={} palette:{pal}",
        ppu.ctrl(),
        ppu.mask(),
        ppu.status(),
        ppu.vblank(),
        ppu.sprite0_hit(),
        ppu.sprite_overflow(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_ppu::OamEntry;

    fn sample_ppu() -> Ppu {
        let mut p = Ppu::new();
        p.set_tile(0, 3, 2, 0x42);
        p.set_palette(1, 0x16);
        p.write_scroll(10);
        p.write_scroll(20);
        p.set_oam_entry(
            0,
            OamEntry {
                y: 0xFF,
                tile: 0,
                attr: 0,
                x: 0,
            },
        );
        p.set_oam_entry(
            1,
            OamEntry {
                y: 50,
                tile: 7,
                attr: 0x21,
                x: 90,
            },
        );
        p
    }

    #[test]
    fn hex_shows_written_tile_and_oam_decodes() {
        let p = sample_ppu();
        let hex = nametable_hex(&p, 0, false);
        assert!(hex.contains("42"), "written tile visible:\n{hex}");
        let oam = oam_summary(&p);
        assert!(oam.contains("parked"), "sprite 0 parked:\n{oam}");
        assert!(oam.contains("tile=$07"), "sprite 1 tile:\n{oam}");
        let sum = nametable_summary(&p, 0);
        assert!(sum.contains("top_tile=$00"), "blank page top tile:\n{sum}");
        assert!(sum.contains("scroll=(10,20)"), "scroll:\n{sum}");
    }

    #[test]
    fn full_hex_covers_all_rows_and_attributes() {
        let p = sample_ppu();
        let hex = nametable_hex(&p, 0, true);
        assert!(hex.contains("r29:"), "30 rows");
        assert!(hex.contains("attr:"), "attribute plane");
    }

    #[test]
    fn headless_viewer_renders_without_panic() {
        let p = sample_ppu();
        let mut view = PpuView::new();
        view.full_hex = true;
        let ctx = egui::Context::default();
        crate::run_headless(&ctx, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                view.show(ui, &p);
            });
        });
    }
}
