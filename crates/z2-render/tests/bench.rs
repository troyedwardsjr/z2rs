//! Compositor cost per frame, printed for the record. `#[ignore]`d: run with
//!
//! ```text
//! cargo test -p z2-render --release --test bench -- --ignored --nocapture
//! ```
//!
//! The assertion is a loose sanity floor, not a benchmark gate: it only fails
//! if composing has become wildly slower than a 60 Hz budget allows.

use std::time::Instant;

use z2_ppu::record::{BgTileId, FrameRecord};
use z2_ppu::wide::{render_wide_indexed, MarginFill, Margins, WideFrame, MARGIN_SLOTS};
use z2_ppu::{Mirroring, OamEntry, Ppu, CHR_BANK_LEN, PPUCTRL_SPRITE_TABLE};
use z2_render::chr::paint_page_sheet;
use z2_render::compositor::{bg_colors, sprite_colors, IndexedView};
use z2_render::{encode_png_rgba, ComposeInput, Compositor, HdPack, MasterPalette};

const BG_PAGE: u8 = 1;
const SPR_PAGE: u8 = 2;
const FRAMES: u32 = 100;

/// Dense CHR: every tile has opaque pixels, so no slot takes a cheap path.
fn chr_image() -> Vec<u8> {
    let mut chr = vec![0u8; 4 * CHR_BANK_LEN];
    for (i, b) in chr.iter_mut().enumerate() {
        *b = ((i * 37) % 251) as u8 | 0x11;
    }
    chr
}

fn scene(chr: &[u8]) -> Ppu {
    let mut p = Ppu::new();
    p.set_mirroring(Mirroring::Vertical);
    p.load_chr_4k(
        0,
        BG_PAGE,
        &chr[usize::from(BG_PAGE) * CHR_BANK_LEN..(usize::from(BG_PAGE) + 1) * CHR_BANK_LEN],
    )
    .unwrap();
    p.load_chr_4k(
        1,
        SPR_PAGE,
        &chr[usize::from(SPR_PAGE) * CHR_BANK_LEN..(usize::from(SPR_PAGE) + 1) * CHR_BANK_LEN],
    )
    .unwrap();
    for nt in 0..2u8 {
        for row in 0..30u8 {
            for col in 0..32u8 {
                p.set_tile(nt, col, row, col.wrapping_mul(7).wrapping_add(row));
            }
        }
    }
    for i in 0..32usize {
        p.set_palette(i, ((i as u8) * 3 + 1) & 0x3F);
    }
    // 8 sprites on most lines: the worst-case sprite replay.
    for i in 0..64usize {
        p.set_oam_entry(
            i,
            OamEntry {
                y: ((i * 3) % 230) as u8,
                tile: (i * 5) as u8,
                attr: (i % 4) as u8,
                x: ((i * 29) % 240) as u8,
            },
        );
    }
    p.write_ctrl(PPUCTRL_SPRITE_TABLE);
    p.write_mask(0x1E);
    p
}

/// Page sheets covering every tile of both pages in the line's own colours.
fn full_pack_files(chr: &[u8], rec: &FrameRecord, scale: u32) -> Vec<(String, Vec<u8>)> {
    let line = rec.line(120);
    let bg3 = bg_colors(line, 0).map(|c| MasterPalette::NES.rgb(c));
    let spr3 = sprite_colors(line, 0).map(|c| MasterPalette::NES.rgb(c));
    let bg = paint_page_sheet(&chr[usize::from(BG_PAGE) * CHR_BANK_LEN..], scale, |_| {
        Some(bg3)
    });
    let spr = paint_page_sheet(&chr[usize::from(SPR_PAGE) * CHR_BANK_LEN..], scale, |_| {
        Some(spr3)
    });
    vec![
        (
            "bg.png".to_string(),
            encode_png_rgba(bg.width, bg.height, &bg.rgba, &[]).unwrap(),
        ),
        (
            "spr.png".to_string(),
            encode_png_rgba(spr.width, spr.height, &spr.rgba, &[]).unwrap(),
        ),
    ]
}

/// Pack covering every tile of both pages in the line's own colours.
fn full_pack(chr: &[u8], rec: &FrameRecord, scale: u32) -> HdPack {
    let json = format!(
        "{{\"version\":1,\"name\":\"bench\",\"scale\":{scale},\"sheets\":[\
         {{\"file\":\"bg.png\",\"page\":{BG_PAGE}}},{{\"file\":\"spr.png\",\"page\":{SPR_PAGE}}}]}}"
    );
    let mut files = vec![("pack.json".to_string(), json.into_bytes())];
    files.extend(full_pack_files(chr, rec, scale));
    HdPack::from_files(&files).unwrap()
}

/// [`full_pack`] plus the three layers a whole-scene paint-over uses: a far
/// backdrop at 20% scroll, a repeating ground strip over one tile and a
/// full-width painting.
fn layer_pack(chr: &[u8], rec: &FrameRecord, scale: u32) -> HdPack {
    let w = 432 * scale;
    let px = |a: u8| {
        let rgba: Vec<u8> = (0..w * 240 * scale)
            .flat_map(|i| [a, 0, 0, (i % 3) as u8 * 127])
            .collect();
        encode_png_rgba(w, 240 * scale, &rgba, &[]).unwrap()
    };
    let json = format!(
        "{{\"version\":1,\"name\":\"layers\",\"scale\":{scale},\"sprite_alpha\":\"art\",\
         \"sheets\":[{{\"file\":\"bg.png\",\"page\":{BG_PAGE}}},{{\"file\":\"spr.png\",\"page\":{SPR_PAGE}}}],\
         \"layers\":[\
         {{\"file\":\"far.png\",\"x\":-66,\"scroll\":20,\"repeat_x\":true}},\
         {{\"file\":\"ground.png\",\"depth\":\"front\",\"x\":22,\"y\":208,\"repeat_x\":true,\
           \"over_tiles\":[{{\"page\":{BG_PAGE},\"tile\":66}}]}},\
         {{\"file\":\"paint.png\",\"depth\":\"front\",\"x\":22,\"y\":32}}]}}"
    );
    let base = full_pack_files(chr, rec, scale);
    let mut files = vec![("pack.json".to_string(), json.into_bytes())];
    files.extend(base);
    files.push(("far.png".to_string(), px(10)));
    files.push(("ground.png".to_string(), px(20)));
    files.push(("paint.png".to_string(), px(30)));
    HdPack::from_files(&files).unwrap()
}

#[test]
#[ignore = "timing report; run with --release --ignored --nocapture"]
fn compose_cost_per_frame() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_record(true);
    p.begin_frame();
    let frame = Box::new(p.finish_frame());
    p.end_frame();
    let rec = p.frame_record().unwrap().clone();

    let mut margins = Margins::new(8);
    let id = BgTileId {
        page: BG_PAGE,
        tile: 0x42,
        pal: 0,
        fine_y: 0,
        fetched: true,
        ..BgTileId::NONE
    };
    for l in margins.lines.iter_mut() {
        l.fill = MarginFill::Tiles;
        l.left = [id; MARGIN_SLOTS];
        l.right = [id; MARGIN_SLOTS];
    }
    let mut wide = WideFrame::new(8);
    render_wide_indexed(&frame, &rec, &margins, &chr, &mut wide);

    println!("\ncompositor cost per frame ({FRAMES} frames each, release build):");
    let mut worst_ms = 0.0f64;
    for scale in [1u32, 2, 4] {
        let pack = full_pack(&chr, &rec, scale);
        let layered = layer_pack(&chr, &rec, scale);
        for (label, use_pack) in [
            ("no pack  ", None),
            ("full pack", Some(&pack)),
            ("+ layers ", Some(&layered)),
        ] {
            for (geom, tiles) in [("256", 0u8), ("wide 8", 8)] {
                let mut c = Compositor::with_margins(scale, tiles).unwrap();
                let mut out = vec![0u8; c.rgba_len()];
                let indexed = if tiles == 0 {
                    IndexedView::frame(&frame)
                } else {
                    IndexedView::wide(&wide)
                };
                let input = ComposeInput {
                    indexed,
                    record: &rec,
                    chr_rom: &chr,
                    pack: use_pack,
                    margins: if tiles == 0 { None } else { Some(&margins) },
                    palette: &MasterPalette::NES,
                    scene: Some(z2_render::SceneView {
                        world: 1,
                        region: 0,
                        scene: 7,
                        camera_x: 123,
                    }),
                };
                // Warm up caches, then time.
                c.compose(input, &mut out).unwrap();
                let t0 = Instant::now();
                for _ in 0..FRAMES {
                    c.compose(input, &mut out).unwrap();
                }
                let ms = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
                println!(
                    "  scale {scale} {label} {geom:6} {:4}x{:<4} {ms:7.3} ms/frame",
                    c.width(),
                    c.height()
                );
                worst_ms = worst_ms.max(ms);
            }
        }
    }
    assert!(
        worst_ms < 50.0,
        "composing got far slower than the frame budget: {worst_ms:.1} ms/frame"
    );
}
