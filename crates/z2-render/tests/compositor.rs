//! Compositor: NES equivalence, HD substitution, priority, margins.
//! Synthetic PPU scenes only (no ROM bytes).

mod common;

use z2_ppu::record::{BgTileId, FrameRecord};
use z2_ppu::wide::{
    render_wide_indexed, right_edge_masked, MarginFill, Margins, WideFrame, MARGIN_SLOTS,
};
use z2_ppu::{
    indexed_to_rgba, Mirroring, OamEntry, Ppu, CHR_BANK_LEN, HEIGHT, PPUCTRL_SPRITE_TABLE,
    PPUMASK_GRAYSCALE, WIDTH,
};
use z2_render::chr::paint_page_sheet;
use z2_render::compositor::{bg_colors, sprite_colors, IndexedView};
use z2_render::{encode_png_rgba, ComposeError, ComposeInput, Compositor, HdPack, MasterPalette};

const BG_PAGE: u8 = 1;
const SPR_PAGE: u8 = 2;
/// Background tile that is solid sub-palette 3 (always opaque).
const TILE_SOLID: u8 = 0x10;
/// Background tile that is solid sub-palette 1.
const TILE_FLAT1: u8 = 0x42;
/// Fully transparent background tile.
const TILE_CLEAR: u8 = 0x20;
/// Sprite tile that is solid sub-palette 2.
const SPR_SOLID: u8 = 0x33;
/// Sprite tile with one opaque column (column 0) for flip tests.
const SPR_EDGE: u8 = 0x44;

fn chr_image() -> Vec<u8> {
    let mut chr = vec![0u8; 4 * CHR_BANK_LEN];
    let mut set = |page: usize, tile: u8, lo: u8, hi: u8| {
        for r in 0..8 {
            let base = page * CHR_BANK_LEN + usize::from(tile) * 16 + r;
            chr[base] = lo;
            chr[base + 8] = hi;
        }
    };
    set(BG_PAGE as usize, TILE_SOLID, 0xFF, 0xFF);
    set(BG_PAGE as usize, TILE_FLAT1, 0xFF, 0x00);
    set(SPR_PAGE as usize, SPR_SOLID, 0x00, 0xFF);
    set(SPR_PAGE as usize, SPR_EDGE, 0x80, 0x00);
    chr
}

/// Uniform background of `TILE_SOLID`, palette group 0 everywhere, both
/// layers on with the left 8 columns shown, no scroll (fine_x 0).
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
                p.set_tile(nt, col, row, TILE_SOLID);
            }
        }
    }
    for i in 0..32usize {
        p.set_palette(i, ((i as u8) * 3 + 1) & 0x3F);
    }
    p.write_ctrl(PPUCTRL_SPRITE_TABLE);
    p.write_mask(0x1E);
    p
}

fn frame_and_record(p: &mut Ppu) -> (Box<z2_ppu::IndexedFrame>, FrameRecord) {
    p.set_record(true);
    p.begin_frame();
    let f = Box::new(p.finish_frame());
    p.end_frame();
    let r = p.frame_record().expect("record armed").clone();
    (f, r)
}

/// Nearest upscale of the indexed frame through the NES palette.
fn upscale(src: &[u8], width: usize, scale: usize) -> Vec<u8> {
    let out_w = width * scale;
    let mut out = vec![0u8; out_w * HEIGHT * scale * 4];
    for y in 0..HEIGHT {
        for x in 0..width {
            let rgba = indexed_to_rgba(src[y * width + x]);
            for fy in 0..scale {
                for fx in 0..scale {
                    let d = ((y * scale + fy) * out_w + x * scale + fx) * 4;
                    out[d..d + 4].copy_from_slice(&rgba);
                }
            }
        }
    }
    out
}

/// Build a pack from page sheets: `(page, colors, painter)` per sheet.
type SheetPainter = Box<dyn Fn(u8, u32, u32) -> [u8; 4]>;
type SheetSpec = (u8, Option<[u8; 3]>, SheetPainter);

fn build_pack(scale: u32, sheets: Vec<SheetSpec>) -> HdPack {
    let mut entries = Vec::new();
    let mut files = Vec::new();
    for (i, (page, colors, paint)) in sheets.into_iter().enumerate() {
        let name = format!("s{i}.png");
        let colors_json = match colors {
            Some([a, b, c]) => format!(",\"colors\":[{a},{b},{c}]"),
            None => String::new(),
        };
        entries.push(format!(
            "{{\"file\":\"{name}\",\"page\":{page}{colors_json}}}"
        ));
        files.push((name, common::page_sheet_png(scale, paint)));
    }
    let json = format!(
        "{{\"version\":1,\"name\":\"t\",\"scale\":{scale},\"sheets\":[{}]}}",
        entries.join(",")
    );
    let mut list = vec![("pack.json".to_string(), json.into_bytes())];
    list.extend(files);
    HdPack::from_files(&list).expect("pack loads")
}

fn compose(
    scale: u32,
    margin_tiles: u8,
    indexed: IndexedView<'_>,
    record: &FrameRecord,
    chr: &[u8],
    pack: Option<&HdPack>,
    margins: Option<&Margins>,
) -> Vec<u8> {
    let mut c = Compositor::with_margins(scale, margin_tiles).unwrap();
    let mut out = vec![0u8; c.rgba_len()];
    c.compose(
        ComposeInput {
            indexed,
            record,
            chr_rom: chr,
            pack,
            margins,
            palette: &MasterPalette::NES,
            scene: None,
        },
        &mut out,
    )
    .expect("compose");
    out
}

#[test]
fn no_pack_output_is_the_upscaled_frame() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_oam_entry(
        0,
        OamEntry {
            y: 99,
            tile: SPR_SOLID,
            attr: 0,
            x: 64,
        },
    );
    let (frame, rec) = frame_and_record(&mut p);
    for scale in [1u32, 2, 3] {
        let got = compose(scale, 0, IndexedView::frame(&frame), &rec, &chr, None, None);
        assert_eq!(
            got,
            upscale(frame.as_slice(), WIDTH, scale as usize),
            "scale {scale} must equal indexed_to_rgba upscaled"
        );
    }
}

/// A pack whose cells reproduce the original CHR art in the line's own
/// colours must leave the image unchanged — this exercises the whole HD
/// path (background substitution plus the sprite replay) against the PPU's
/// own output.
#[test]
fn pack_reproducing_nes_art_is_pixel_identical() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_tile(0, 5, 3, TILE_FLAT1);
    p.set_tile(0, 6, 3, TILE_CLEAR);
    for (i, (y, x, attr, tile)) in [
        (99u8, 64u8, 0u8, SPR_SOLID),
        (99, 80, 0x20, SPR_SOLID),
        (99, 96, 0x40, SPR_EDGE),
    ]
    .into_iter()
    .enumerate()
    {
        p.set_oam_entry(i, OamEntry { y, tile, attr, x });
    }
    let (frame, rec) = frame_and_record(&mut p);
    let line = rec.line(100);
    let bg3 = bg_colors(line, 0).map(|c| MasterPalette::NES.rgb(c));
    let spr3 = sprite_colors(line, 0).map(|c| MasterPalette::NES.rgb(c));
    let chr_bg = chr[usize::from(BG_PAGE) * CHR_BANK_LEN..].to_vec();
    let chr_spr = chr[usize::from(SPR_PAGE) * CHR_BANK_LEN..].to_vec();

    // The plain pack takes the original sprite pass; `"sprite_alpha": "art"`
    // and a layer that draws nothing each send every line through the shaped
    // pass instead, which must reproduce the same hardware mux.
    let variants = [
        "",
        ",\"sprite_alpha\":\"art\"",
        ",\"layers\":[{\"file\":\"none.png\"}]",
    ];
    for (scale, extra) in [1u32, 2].into_iter().flat_map(|s| variants.map(|v| (s, v))) {
        let bg_sheet = paint_page_sheet(&chr_bg, scale, |_| Some(bg3));
        let spr_sheet = paint_page_sheet(&chr_spr, scale, |_| Some(spr3));
        let json = format!(
            "{{\"version\":1,\"name\":\"nes\",\"scale\":{scale},\"sheets\":[\
             {{\"file\":\"bg.png\",\"page\":{BG_PAGE}}},{{\"file\":\"spr.png\",\"page\":{SPR_PAGE}}}]{extra}}}"
        );
        let pack = HdPack::from_files(&[
            ("pack.json".to_string(), json.into_bytes()),
            (
                "none.png".to_string(),
                common::solid_png(4, 4, [0, 0, 0, 0]),
            ),
            (
                "bg.png".to_string(),
                encode_png_rgba(bg_sheet.width, bg_sheet.height, &bg_sheet.rgba, &[]).unwrap(),
            ),
            (
                "spr.png".to_string(),
                encode_png_rgba(spr_sheet.width, spr_sheet.height, &spr_sheet.rgba, &[]).unwrap(),
            ),
        ])
        .unwrap();
        let got = compose(
            scale,
            0,
            IndexedView::frame(&frame),
            &rec,
            &chr,
            Some(&pack),
            None,
        );
        let want = upscale(frame.as_slice(), WIDTH, scale as usize);
        assert_eq!(got.len(), want.len());
        let bad = got
            .chunks_exact(4)
            .zip(want.chunks_exact(4))
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, _)| (i % (WIDTH * scale as usize), i / (WIDTH * scale as usize)));
        assert_eq!(
            bad, None,
            "scale {scale}, pack extra '{extra}': first differing pixel (x, y)"
        );
    }
}

#[test]
fn replaced_tile_lands_on_the_right_pixels() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_tile(0, 5, 3, TILE_FLAT1);
    let (frame, rec) = frame_and_record(&mut p);
    const RED: [u8; 4] = [255, 0, 0, 255];
    let pack = build_pack(
        1,
        vec![(
            BG_PAGE,
            None,
            Box::new(|t, _, _| if t == TILE_FLAT1 { RED } else { [0; 4] }),
        )],
    );
    let got = compose(
        1,
        0,
        IndexedView::frame(&frame),
        &rec,
        &chr,
        Some(&pack),
        None,
    );
    let at = |x: usize, y: usize| &got[(y * WIDTH + x) * 4..(y * WIDTH + x) * 4 + 4];
    // Tile (col 5, row 3) with fine_x 0 covers x 40..48, y 24..32.
    for y in 24..32 {
        for x in 40..48 {
            assert_eq!(at(x, y), RED, "({x}, {y}) should be HD red");
        }
    }
    assert_ne!(at(39, 24), RED, "left neighbour untouched");
    assert_ne!(at(48, 24), RED, "right neighbour untouched");
    assert_ne!(at(40, 23), RED, "row above untouched");
}

#[test]
fn palette_variant_beats_default() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_tile(0, 5, 3, TILE_FLAT1);
    let (frame, rec) = frame_and_record(&mut p);
    let live = bg_colors(rec.line(24), 0);
    let other = [live[0], live[1], live[2] ^ 0x01];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    let with = |variant: [u8; 3]| {
        build_pack(
            1,
            vec![
                (
                    BG_PAGE,
                    None,
                    Box::new(|t, _, _| if t == TILE_FLAT1 { GREEN } else { [0; 4] }),
                ),
                (
                    BG_PAGE,
                    Some(variant),
                    Box::new(|t, _, _| if t == TILE_FLAT1 { BLUE } else { [0; 4] }),
                ),
            ],
        )
    };
    let px = |pack: &HdPack| {
        let got = compose(
            1,
            0,
            IndexedView::frame(&frame),
            &rec,
            &chr,
            Some(pack),
            None,
        );
        let i = (24 * WIDTH + 40) * 4;
        [got[i], got[i + 1], got[i + 2], got[i + 3]]
    };
    assert_eq!(px(&with(live)), BLUE, "matching variant wins");
    assert_eq!(px(&with(other)), GREEN, "non-matching variant ignored");
}

#[test]
fn nes_priority_survives_hd_background() {
    let chr = chr_image();
    const RED: [u8; 4] = [255, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    // Sprite 0 front at x 64, sprite 1 behind at x 80, over opaque bg;
    // sprite 2 behind at x 96 over a transparent bg tile (col 12 = x 96).
    let mut p = scene(&chr);
    p.set_tile(0, 12, 12, TILE_CLEAR);
    p.set_tile(0, 13, 12, TILE_CLEAR);
    p.set_oam_entry(
        0,
        OamEntry {
            y: 99,
            tile: SPR_SOLID,
            attr: 0,
            x: 64,
        },
    );
    p.set_oam_entry(
        1,
        OamEntry {
            y: 99,
            tile: SPR_SOLID,
            attr: 0x20,
            x: 80,
        },
    );
    p.set_oam_entry(
        2,
        OamEntry {
            y: 99,
            tile: SPR_SOLID,
            attr: 0x20,
            x: 96,
        },
    );
    let (frame, rec) = frame_and_record(&mut p);
    let pack = build_pack(
        1,
        vec![
            (BG_PAGE, None, Box::new(|_, _, _| RED)),
            (SPR_PAGE, None, Box::new(|_, _, _| WHITE)),
        ],
    );
    let got = compose(
        1,
        0,
        IndexedView::frame(&frame),
        &rec,
        &chr,
        Some(&pack),
        None,
    );
    let at = |x: usize| {
        let i = (100 * WIDTH + x) * 4;
        [got[i], got[i + 1], got[i + 2], got[i + 3]]
    };
    assert_eq!(at(64), WHITE, "front sprite draws HD art over HD bg");
    assert_eq!(
        at(80),
        RED,
        "behind sprite stays hidden by opaque background"
    );
    // Line 100 is inside tile row 12 (y 96..104); bg there is transparent, so
    // the behind sprite shows — over the HD background art.
    assert_eq!(at(96), WHITE, "behind sprite shows over transparent bg");
}

#[test]
fn sprite_flips_mirror_hd_subpixels() {
    let chr = chr_image();
    let mut p = scene(&chr);
    // SPR_EDGE is opaque only in column 0.
    p.set_oam_entry(
        0,
        OamEntry {
            y: 99,
            tile: SPR_EDGE,
            attr: 0,
            x: 64,
        },
    );
    p.set_oam_entry(
        1,
        OamEntry {
            y: 99,
            tile: SPR_EDGE,
            attr: 0x40,
            x: 96,
        },
    );
    let (frame, rec) = frame_and_record(&mut p);
    // Scale 2: HD cell column 0 is light, column 1 dark, so a flip swaps them.
    const A: [u8; 4] = [10, 10, 10, 255];
    const B: [u8; 4] = [200, 200, 200, 255];
    let pack = build_pack(
        2,
        vec![(
            SPR_PAGE,
            None,
            Box::new(|_, x, _| if x % 2 == 0 { A } else { B }),
        )],
    );
    let got = compose(
        2,
        0,
        IndexedView::frame(&frame),
        &rec,
        &chr,
        Some(&pack),
        None,
    );
    let w = WIDTH * 2;
    let at = |x: usize| {
        let i = (200 * w + x) * 4;
        [got[i], got[i + 1], got[i + 2], got[i + 3]]
    };
    // Unflipped sprite: NES column 0 at x=64 -> output x 128,129 = A,B.
    assert_eq!((at(128), at(129)), (A, B), "unflipped HD sub-pixels");
    // Flipped sprite: opaque NES pixel moves to dx=7 (x=103) and the HD
    // sub-pixels mirror -> B,A.
    assert_eq!((at(206), at(207)), (B, A), "flip_h mirrors HD sub-pixels");
}

#[test]
fn greyscale_and_split_lines_keep_nes_art() {
    let chr = chr_image();
    let mut p = scene(&chr);
    let (frame, rec) = frame_and_record(&mut p);
    let mut grey = rec.clone();
    grey.lines[50].mask |= PPUMASK_GRAYSCALE;
    // A split only costs the line its HD art when fine X moved with it.
    let mut split = rec.clone();
    split.lines[51].split = true;
    split.lines[51].fine_x_end = split.lines[51].fine_x ^ 3;
    split.lines[52].split = true;
    const RED: [u8; 4] = [255, 0, 0, 255];
    let pack = build_pack(1, vec![(BG_PAGE, None, Box::new(|_, _, _| RED))]);
    for (label, record) in [("greyscale", &grey), ("split", &split)] {
        let got = compose(
            1,
            0,
            IndexedView::frame(&frame),
            record,
            &chr,
            Some(&pack),
            None,
        );
        let y = if label == "greyscale" { 50 } else { 51 };
        let i = (y * WIDTH + 100) * 4;
        assert_ne!(
            &got[i..i + 4],
            RED.as_slice(),
            "{label} line must keep NES art"
        );
        let j = (60 * WIDTH + 100) * 4;
        assert_eq!(&got[j..j + 4], RED.as_slice(), "other lines still get HD");
        if label == "split" {
            let k = (52 * WIDTH + 100) * 4;
            assert_eq!(
                &got[k..k + 4],
                RED.as_slice(),
                "a split that leaves fine X alone still gets HD"
            );
        }
    }
}

#[test]
fn widescreen_centre_matches_and_margins_use_the_pack() {
    let chr = chr_image();
    let mut p = scene(&chr);
    let (frame, rec) = frame_and_record(&mut p);
    let tiles = 4u8;
    let mut margins = Margins::new(tiles);
    let margin_id = BgTileId {
        page: BG_PAGE,
        tile: TILE_FLAT1,
        pal: 0,
        fine_y: 0,
        fetched: true,
        ..BgTileId::NONE
    };
    for l in margins.lines.iter_mut() {
        l.fill = MarginFill::Tiles;
        l.left = [margin_id; MARGIN_SLOTS];
        l.right = [margin_id; MARGIN_SLOTS];
    }
    let mut wide = WideFrame::new(tiles);
    render_wide_indexed(&frame, &rec, &margins, &chr, &mut wide);
    const RED: [u8; 4] = [255, 0, 0, 255];
    let pack = build_pack(
        1,
        vec![(
            BG_PAGE,
            None,
            Box::new(|t, _, _| if t == TILE_FLAT1 { RED } else { [0; 4] }),
        )],
    );
    let wide_out = compose(
        1,
        tiles,
        IndexedView::wide(&wide),
        &rec,
        &chr,
        Some(&pack),
        Some(&margins),
    );
    let plain = compose(
        1,
        0,
        IndexedView::frame(&frame),
        &rec,
        &chr,
        Some(&pack),
        None,
    );
    let w = wide.width;
    let mp = wide.margin_px();
    for y in 0..HEIGHT {
        let got = &wide_out[(y * w + mp) * 4..(y * w + mp + WIDTH) * 4];
        let want = &plain[y * WIDTH * 4..(y + 1) * WIDTH * 4];
        assert_eq!(
            got, want,
            "centre must match the non-wide composite, row {y}"
        );
    }
    // Margin pixels show the HD replacement of the margin tile.
    let i = (10 * w + 3) * 4;
    assert_eq!(&wide_out[i..i + 4], RED.as_slice(), "left margin is HD art");
    let j = (10 * w + mp + WIDTH + 3) * 4;
    assert_eq!(
        &wide_out[j..j + 4],
        RED.as_slice(),
        "right margin is HD art"
    );
}

#[test]
fn geometry_and_scale_errors() {
    let chr = chr_image();
    let mut p = scene(&chr);
    let (frame, rec) = frame_and_record(&mut p);
    let pack = build_pack(4, vec![(BG_PAGE, None, Box::new(|_, _, _| [1, 2, 3, 255]))]);
    assert!(Compositor::pack_supports_scale(&pack, 2));
    assert!(!Compositor::pack_supports_scale(&pack, 3));
    assert_eq!(Compositor::new(0).unwrap_err(), ComposeError::BadScale(0));
    assert_eq!(Compositor::new(9).unwrap_err(), ComposeError::BadScale(9));

    let mut c = Compositor::new(3).unwrap();
    let mut out = vec![0u8; c.rgba_len()];
    let input = ComposeInput {
        indexed: IndexedView::frame(&frame),
        record: &rec,
        chr_rom: &chr,
        pack: Some(&pack),
        margins: None,
        palette: &MasterPalette::NES,
        scene: None,
    };
    assert_eq!(
        c.compose(input, &mut out).unwrap_err(),
        ComposeError::ScaleMismatch { pack: 4, output: 3 }
    );
    let mut short = vec![0u8; 16];
    assert!(matches!(
        c.compose(
            ComposeInput {
                pack: None,
                ..input
            },
            &mut short
        )
        .unwrap_err(),
        ComposeError::BadOutLen { .. }
    ));
    // A wide source handed to a margin-less compositor is rejected.
    let wide = WideFrame::new(4);
    let mut c0 = Compositor::new(1).unwrap();
    let mut out0 = vec![0u8; c0.rgba_len()];
    assert!(matches!(
        c0.compose(
            ComposeInput {
                indexed: IndexedView::wide(&wide),
                pack: None,
                ..input
            },
            &mut out0
        )
        .unwrap_err(),
        ComposeError::SourceWidth { .. }
    ));
}

/// Regression: the ROM blanks the window's right 8 columns with an opaque
/// sprite column at x 248, and `Margins::fill_right_clip` repaints them from
/// the line's own tile. The HD composite has to honour that too: it used to
/// paint the pack's art there and then replay the mask sprite straight back
/// over it, so widescreen + a pack showed the black seam again.
#[test]
fn widescreen_right_edge_recovers_under_the_mask_sprite() {
    const HD: [u8; 4] = [0, 255, 0, 255];
    let chr = chr_image();
    let mut p = scene(&chr);
    // Nametable column 31 is window x 248..255. Row 12 (y 96..103) gets a
    // tile the pack does NOT replace, so that band exercises the fallback.
    for nt in 0..2u8 {
        p.set_tile(nt, 31, 12, TILE_FLAT1);
    }
    // Two opaque, in-front sprites parked at x 248, one over each band.
    for (i, y) in [95u8, 111].into_iter().enumerate() {
        p.set_oam_entry(
            i,
            OamEntry {
                y,
                tile: SPR_SOLID,
                attr: 0,
                x: (WIDTH - 8) as u8,
            },
        );
    }
    let (frame, rec) = frame_and_record(&mut p);

    let masked: Vec<usize> = (0..HEIGHT)
        .filter(|&y| right_edge_masked(&rec, y, &chr))
        .collect();
    assert!(!masked.is_empty(), "scene must produce an edge mask");
    let hd_rows: Vec<usize> = masked
        .iter()
        .copied()
        .filter(|&y| rec.line(y).tiles[31].tile == TILE_SOLID)
        .collect();
    let plain_rows: Vec<usize> = masked
        .iter()
        .copied()
        .filter(|&y| rec.line(y).tiles[31].tile == TILE_FLAT1)
        .collect();
    assert!(
        !hd_rows.is_empty() && !plain_rows.is_empty(),
        "need both a pack-covered and an uncovered tile under the mask"
    );

    let tiles = 4u8;
    let mut margins = Margins::new(tiles);
    let margin_id = BgTileId {
        page: BG_PAGE,
        tile: TILE_FLAT1,
        pal: 0,
        fine_y: 0,
        fetched: true,
        ..BgTileId::NONE
    };
    for l in margins.lines.iter_mut() {
        l.fill = MarginFill::Tiles;
        l.left = [margin_id; MARGIN_SLOTS];
        l.right = [margin_id; MARGIN_SLOTS];
    }
    // The pack replaces only TILE_SOLID, on the background page.
    let pack = build_pack(
        1,
        vec![(
            BG_PAGE,
            None,
            Box::new(|t, _, _| if t == TILE_SOLID { HD } else { [0; 4] }),
        )],
    );
    let mp = 8 * usize::from(tiles);
    let w = mp * 2 + WIDTH;
    let mut wide = WideFrame::new(tiles);

    // Flag off: NES behaviour, the mask sprite still covers the strip.
    let mut off = margins.clone();
    off.fill_right_clip = false;
    render_wide_indexed(&frame, &rec, &off, &chr, &mut wide);
    let out_off = compose(
        1,
        tiles,
        IndexedView::wide(&wide),
        &rec,
        &chr,
        Some(&pack),
        Some(&off),
    );
    let mask_rgba = MasterPalette::NES.rgba(rec.line(masked[0]).palette_entry(0x10 + 2));
    for &y in &masked {
        for x in WIDTH - 8..WIDTH {
            let i = (y * w + mp + x) * 4;
            assert_eq!(
                &out_off[i..i + 4],
                mask_rgba.as_slice(),
                "flag off: row {y} col {x} keeps the ROM's mask"
            );
        }
    }

    // Flag on: the strip shows the background again -- pack art where the
    // pack covers the tile, upscaled original art where it does not.
    margins.fill_right_clip = true;
    render_wide_indexed(&frame, &rec, &margins, &chr, &mut wide);
    let out_on = compose(
        1,
        tiles,
        IndexedView::wide(&wide),
        &rec,
        &chr,
        Some(&pack),
        Some(&margins),
    );
    for &y in &hd_rows {
        for x in WIDTH - 8..WIDTH {
            let i = (y * w + mp + x) * 4;
            assert_eq!(
                &out_on[i..i + 4],
                HD.as_slice(),
                "flag on: row {y} col {x} must be HD art, not the mask"
            );
        }
    }
    for &y in &plain_rows {
        for x in WIDTH - 8..WIDTH {
            let i = (y * w + mp + x) * 4;
            let want = indexed_to_rgba(wide.row(y)[mp + x]);
            assert_eq!(
                &out_on[i..i + 4],
                want.as_slice(),
                "flag on: row {y} col {x} must be the recovered original art"
            );
            assert_ne!(
                &out_on[i..i + 4],
                mask_rgba.as_slice(),
                "row {y}: mask back"
            );
        }
    }
    // Unmasked rows and every column left of the strip are untouched.
    for y in 0..HEIGHT {
        let limit = if masked.contains(&y) {
            WIDTH - 8
        } else {
            WIDTH
        };
        assert_eq!(
            &out_on[(y * w + mp) * 4..(y * w + mp + limit) * 4],
            &out_off[(y * w + mp) * 4..(y * w + mp + limit) * 4],
            "row {y}: the fill changed something outside the masked strip"
        );
    }

    // Centre parity with the fills off, in both flag states of the
    // compositor: a margin-less composite of the same frame.
    let plain = compose(
        1,
        0,
        IndexedView::frame(&frame),
        &rec,
        &chr,
        Some(&pack),
        None,
    );
    for y in 0..HEIGHT {
        assert_eq!(
            &out_off[(y * w + mp) * 4..(y * w + mp + WIDTH) * 4],
            &plain[y * WIDTH * 4..(y + 1) * WIDTH * 4],
            "fills off: centre must equal the non-wide composite, row {y}"
        );
    }
}

/// Regression: with `fine_x != 0`, fetch slot 0 covers window pixels
/// `-fine_x .. 8 - fine_x`. Its negative columns are drawn by
/// `render_wide_indexed` from `record.tiles[0]`, so the HD pass must
/// substitute them from the same tile. They used to be skipped entirely
/// (the centre band clipped to `0..WIDTH`, and the left band only ran for
/// `k < 0`, sourcing `margins.left`), leaving a 1-7 px strip of original
/// art at the left edge of the play field.
#[test]
fn widescreen_left_edge_is_hd_with_fine_x() {
    const CENTRE_HD: [u8; 4] = [0, 255, 0, 255];
    const MARGIN_HD: [u8; 4] = [255, 0, 0, 255];
    let chr = chr_image();
    for fine_x in 1u8..8 {
        let mut p = scene(&chr);
        p.write_scroll(fine_x); // low 3 bits are fine X
        p.write_scroll(0);
        let (frame, rec) = frame_and_record(&mut p);
        assert_eq!(rec.line(100).fine_x & 7, fine_x, "scene sets fine_x");

        let tiles = 4u8;
        let mut margins = Margins::new(tiles);
        let margin_id = BgTileId {
            page: BG_PAGE,
            tile: TILE_FLAT1,
            pal: 0,
            fine_y: 0,
            fetched: true,
            ..BgTileId::NONE
        };
        for l in margins.lines.iter_mut() {
            l.fill = MarginFill::Tiles;
            l.left = [margin_id; MARGIN_SLOTS];
            l.right = [margin_id; MARGIN_SLOTS];
        }
        let mut wide = WideFrame::new(tiles);
        render_wide_indexed(&frame, &rec, &margins, &chr, &mut wide);
        // The window background is TILE_SOLID and the margins TILE_FLAT1;
        // the pack replaces each with its own flat colour, so a pixel's
        // colour says which tile fed it.
        let pack = build_pack(
            1,
            vec![(
                BG_PAGE,
                None,
                Box::new(|t, _, _| match t {
                    TILE_SOLID => CENTRE_HD,
                    TILE_FLAT1 => MARGIN_HD,
                    _ => [0; 4],
                }),
            )],
        );
        let wide_out = compose(
            1,
            tiles,
            IndexedView::wide(&wide),
            &rec,
            &chr,
            Some(&pack),
            Some(&margins),
        );
        let w = wide.width;
        let mp = wide.margin_px();
        let row = 100usize;
        let at = |wx: i32| {
            let i = (row * w + (wx + mp as i32) as usize) * 4;
            &wide_out[i..i + 4]
        };
        // Slot 0's leading columns: HD art of the record's own tile.
        for wx in -i32::from(fine_x)..0 {
            assert_eq!(
                at(wx),
                CENTRE_HD.as_slice(),
                "fine_x {fine_x}: window pixel {wx} must be HD art of tiles[0]"
            );
        }
        // The pixel just left of slot 0 belongs to margin slot -1.
        assert_eq!(
            at(-i32::from(fine_x) - 1),
            MARGIN_HD.as_slice(),
            "fine_x {fine_x}: slot -1 stays margin HD art"
        );
        assert_eq!(at(0), CENTRE_HD.as_slice(), "fine_x {fine_x}: window");
        assert_eq!(
            at(WIDTH as i32 + 3),
            MARGIN_HD.as_slice(),
            "fine_x {fine_x}: right margin"
        );

        // Parity: the centre 256 columns still equal the non-wide composite.
        let plain = compose(
            1,
            0,
            IndexedView::frame(&frame),
            &rec,
            &chr,
            Some(&pack),
            None,
        );
        for y in 0..HEIGHT {
            assert_eq!(
                &wide_out[(y * w + mp) * 4..(y * w + mp + WIDTH) * 4],
                &plain[y * WIDTH * 4..(y + 1) * WIDTH * 4],
                "fine_x {fine_x}: centre row {y}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pack extensions: art-shaped sprites, bleed, layers
// ---------------------------------------------------------------------------

const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const PINK: [u8; 4] = [255, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// A `w` x `h` PNG painted per pixel.
fn png(w: u32, h: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let rgba: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .flat_map(|(x, y)| paint(x, y))
        .collect();
    encode_png_rgba(w, h, &rgba, &[]).unwrap()
}

/// A scale-1 pack from a manifest body (everything after `"scale":1,`).
fn pack_from(body: &str, files: Vec<(&str, Vec<u8>)>) -> HdPack {
    let json = format!("{{\"version\":1,\"name\":\"t\",\"scale\":1,{body}}}");
    let mut list = vec![("pack.json".to_string(), json.into_bytes())];
    list.extend(files.into_iter().map(|(n, b)| (n.to_string(), b)));
    HdPack::from_files(&list).expect("pack loads")
}

fn compose_scene(
    frame: &z2_ppu::IndexedFrame,
    record: &FrameRecord,
    chr: &[u8],
    pack: &HdPack,
    scene: Option<z2_render::SceneView>,
) -> Vec<u8> {
    let mut c = Compositor::new(1).unwrap();
    let mut out = vec![0u8; c.rgba_len()];
    c.compose(
        ComposeInput {
            indexed: IndexedView::frame(frame),
            record,
            chr_rom: chr,
            pack: Some(pack),
            margins: None,
            palette: &MasterPalette::NES,
            scene,
        },
        &mut out,
    )
    .expect("compose");
    out
}

fn px(img: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * WIDTH + x) * 4;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

fn saria(camera_x: i32) -> Option<z2_render::SceneView> {
    Some(z2_render::SceneView {
        world: 1,
        region: 0,
        scene: 7,
        camera_x,
    })
}

/// `"sprite_alpha": "art"`: the cell's alpha is the silhouette. Art may
/// cover NES-transparent pixels, and where the art is clear over a NES-opaque
/// pixel the true background shows, HD cell included.
#[test]
fn art_alpha_lets_the_cell_shape_the_sprite() {
    let chr = chr_image();
    let mut p = scene(&chr);
    // SPR_EDGE is opaque in column 0 only.
    p.set_oam_entry(
        0,
        OamEntry {
            y: 99,
            tile: SPR_EDGE,
            attr: 0,
            x: 100,
        },
    );
    let (frame, rec) = frame_and_record(&mut p);
    // Art: column 0 clear, columns 1 and 2 green. The background is red.
    let sprite = png(
        8,
        8,
        |x, _| if (1..=2).contains(&x) { GREEN } else { CLEAR },
    );
    let files = |s: &[u8]| {
        vec![
            (
                "bg.png",
                common::page_sheet_png(1, |_, _, _| [255, 0, 0, 255]),
            ),
            ("spr.png", s.to_vec()),
        ]
    };
    let body = |alpha: &str| {
        format!(
            "{alpha}\"sheets\":[{{\"file\":\"bg.png\",\"page\":{BG_PAGE}}},{{\"file\":\"spr.png\"}}],\
             \"tiles\":[{{\"page\":{SPR_PAGE},\"tile\":{SPR_EDGE},\"sheet\":\"spr.png\",\"x\":0,\"y\":0}}]"
        )
    };
    let red = [255, 0, 0, 255];

    let art = pack_from(&body("\"sprite_alpha\":\"art\","), files(&sprite));
    assert!(art.sprite_alpha_art());
    let got = compose_scene(&frame, &rec, &chr, &art, None);
    assert_eq!(
        px(&got, 100, 100),
        red,
        "clear art over a NES pixel shows the HD background"
    );
    assert_eq!(
        px(&got, 101, 100),
        GREEN,
        "art covers a NES-transparent pixel"
    );
    assert_eq!(px(&got, 102, 107), GREEN);
    assert_eq!(px(&got, 103, 100), red);
    assert_eq!(
        px(&got, 101, 99),
        red,
        "nothing above the box without bleed"
    );

    let nes = pack_from(&body(""), files(&sprite));
    assert!(!nes.sprite_alpha_art());
    let got = compose_scene(&frame, &rec, &chr, &nes, None);
    assert_eq!(
        px(&got, 101, 100),
        red,
        "default: art stays inside the NES silhouette"
    );
}

/// `bleed`: art around the 8x8 box, mirrored with the sprite, and never over
/// a pixel another sprite's core claimed.
#[test]
fn bleed_draws_around_the_box() {
    let chr = chr_image();
    // Core green at (2, 1) of a 10x9 sheet; two columns left and one row
    // above it are blue.
    let sheet = png(10, 9, |x, y| if x >= 2 && y >= 1 { GREEN } else { BLUE });
    let body = format!(
        "\"sprite_alpha\":\"art\",\"sheets\":[{{\"file\":\"spr.png\"}}],\
         \"tiles\":[{{\"page\":{SPR_PAGE},\"tile\":{SPR_SOLID},\"sheet\":0,\"x\":2,\"y\":1,\
         \"bleed\":[2,1,0,0]}}]"
    );
    let pack = pack_from(&body, vec![("spr.png", sheet)]);
    assert_eq!(pack.max_bleed_v(), 1);

    let shot = |attr: u8, second: Option<u8>| {
        let mut p = scene(&chr);
        p.set_oam_entry(
            0,
            OamEntry {
                y: 99,
                tile: SPR_SOLID,
                attr,
                x: 100,
            },
        );
        if let Some(x) = second {
            // Lower priority, plain NES art (SPR_EDGE has no cell).
            p.set_oam_entry(
                1,
                OamEntry {
                    y: 99,
                    tile: SPR_EDGE,
                    attr: 0,
                    x,
                },
            );
        }
        let (frame, rec) = frame_and_record(&mut p);
        let bg = px(&upscale(frame.as_slice(), WIDTH, 1), 50, 100);
        (compose_scene(&frame, &rec, &chr, &pack, None), bg)
    };

    let (got, bg) = shot(0, None);
    assert_eq!(px(&got, 100, 100), GREEN);
    assert_eq!(px(&got, 107, 107), GREEN);
    assert_eq!(
        (px(&got, 98, 100), px(&got, 99, 107)),
        (BLUE, BLUE),
        "left bleed"
    );
    assert_eq!(px(&got, 97, 100), bg);
    assert_eq!(px(&got, 108, 100), bg, "no right bleed");
    assert_eq!(
        (px(&got, 98, 99), px(&got, 107, 99)),
        (BLUE, BLUE),
        "top bleed row"
    );
    assert_eq!(px(&got, 100, 98), bg, "one row only");
    assert_eq!(px(&got, 100, 108), bg, "no bottom bleed");

    let (got, bg) = shot(0x40, None);
    assert_eq!(
        (px(&got, 108, 100), px(&got, 109, 100)),
        (BLUE, BLUE),
        "flip_h moves it right"
    );
    assert_eq!(px(&got, 99, 100), bg);

    let (got, bg) = shot(0xC0, None);
    assert_eq!(
        px(&got, 100, 108),
        BLUE,
        "flip_v moves the top bleed under the box"
    );
    assert_eq!(px(&got, 100, 99), bg, "and off the row above it");

    // The second sprite's opaque column sits at x = 99, inside the bleed.
    let (got, bg) = shot(0, Some(99));
    assert_ne!(
        px(&got, 99, 100),
        BLUE,
        "a core beats another sprite's bleed"
    );
    assert_ne!(px(&got, 99, 100), bg);
    assert_eq!(px(&got, 98, 100), BLUE);
}

/// Layers: back shows through transparent background only, front covers
/// tiles but not sprites, `over_tiles` restricts a front layer, and the
/// camera, `scroll`, `repeat_x` and `when` place and gate them.
#[test]
fn layers_sit_behind_and_over_the_background() {
    let chr = chr_image();
    let mut p = scene(&chr);
    // Transparent background over x 80..104; one TILE_FLAT1 at x 112..120.
    for row in 0..30u8 {
        for col in 10..13u8 {
            p.set_tile(0, col, row, TILE_CLEAR);
        }
    }
    p.set_tile(0, 14, 12, TILE_FLAT1);
    p.set_oam_entry(
        0,
        OamEntry {
            y: 99,
            tile: SPR_SOLID,
            attr: 0,
            x: 110,
        },
    );
    let (frame, rec) = frame_and_record(&mut p);
    let flat = upscale(frame.as_slice(), WIDTH, 1);
    let (solid, backdrop, sprite) = (px(&flat, 50, 50), px(&flat, 90, 50), px(&flat, 112, 102));
    assert_ne!(solid, backdrop);

    let files = || {
        vec![
            ("back.png", common::solid_png(32, 240, YELLOW)),
            ("front.png", common::solid_png(16, 8, PINK)),
            (
                "stripes.png",
                png(4, 240, |x, _| if x < 2 { BLUE } else { CLEAR }),
            ),
        ]
    };
    let when = "\"when\":{\"world\":1,\"scene\":7}";
    let back = |extra: &str| {
        pack_from(
            &format!("\"layers\":[{{\"file\":\"back.png\",\"x\":80,{when}{extra}}}]"),
            files(),
        )
    };

    // Back layer, locked to the level.
    let pack = back("");
    let got = compose_scene(&frame, &rec, &chr, &pack, saria(0));
    assert_eq!(
        px(&got, 90, 50),
        YELLOW,
        "shows through transparent background"
    );
    assert_eq!(px(&got, 108, 50), solid, "not over an opaque tile");
    assert_eq!(px(&got, 79, 50), solid);
    let got = compose_scene(&frame, &rec, &chr, &pack, saria(8));
    assert_eq!(
        px(&got, 103, 50),
        YELLOW,
        "the camera moved it 8 px left: x 72..104"
    );
    let moved = back(",\"scroll\":50");
    let got = compose_scene(&frame, &rec, &chr, &moved, saria(-40));
    assert_eq!(px(&got, 99, 50), backdrop, "half speed: x 100..132");
    assert_eq!(px(&got, 100, 50), YELLOW);

    // `when` gates it.
    let other = Some(z2_render::SceneView {
        scene: 8,
        ..saria(0).unwrap()
    });
    for scene in [other, None] {
        let got = compose_scene(&frame, &rec, &chr, &pack, scene);
        assert_eq!(
            px(&got, 90, 50),
            backdrop,
            "{scene:?} is not this layer's scene"
        );
    }
    let always = pack_from("\"layers\":[{\"file\":\"back.png\",\"x\":80}]", files());
    let got = compose_scene(&frame, &rec, &chr, &always, None);
    assert_eq!(
        px(&got, 90, 50),
        YELLOW,
        "no `when`: shown with no scene known"
    );

    // repeat_x tiles a 4 px image; its clear half leaves the backdrop.
    let stripes = pack_from(
        "\"layers\":[{\"file\":\"stripes.png\",\"repeat_x\":true}]",
        files(),
    );
    let got = compose_scene(&frame, &rec, &chr, &stripes, None);
    assert_eq!((px(&got, 84, 50), px(&got, 85, 50)), (BLUE, BLUE));
    assert_eq!((px(&got, 86, 50), px(&got, 87, 50)), (backdrop, backdrop));
    assert_eq!(px(&got, 88, 50), BLUE);

    // Front layer over x 104..120, y 96..104: covers tiles, not the sprite.
    let front = |extra: &str| {
        pack_from(
            &format!(
                "\"layers\":[{{\"file\":\"front.png\",\"depth\":\"front\",\"x\":104,\"y\":96{extra}}}]"
            ),
            files(),
        )
    };
    let got = compose_scene(&frame, &rec, &chr, &front(""), None);
    assert_eq!(px(&got, 105, 97), PINK, "covers an opaque tile");
    assert_eq!(px(&got, 112, 102), sprite, "sprites stay on top");
    assert_eq!(px(&got, 105, 104), solid, "8 px tall");
    let over = format!(",\"over_tiles\":[{{\"page\":{BG_PAGE},\"tile\":{TILE_FLAT1}}}]");
    let got = compose_scene(&frame, &rec, &chr, &front(&over), None);
    assert_eq!(px(&got, 105, 97), solid, "not over other tiles");
    assert_eq!(px(&got, 119, 97), PINK, "over the named tile");
}

/// A split line that moved fine X has no tile identities to trust: a back
/// layer still shows where the indexed pixel is the backdrop.
#[test]
fn back_layer_reaches_split_lines_by_colour() {
    let chr = chr_image();
    let mut p = scene(&chr);
    for row in 0..30u8 {
        p.set_tile(0, 10, row, TILE_CLEAR);
    }
    let (frame, mut rec) = frame_and_record(&mut p);
    rec.lines[60].split = true;
    rec.lines[60].fine_x_end = rec.lines[60].fine_x ^ 5;
    let pack = pack_from(
        "\"layers\":[{\"file\":\"back.png\"}]",
        vec![("back.png", common::solid_png(256, 240, YELLOW))],
    );
    let got = compose_scene(&frame, &rec, &chr, &pack, None);
    let flat = upscale(frame.as_slice(), WIDTH, 1);
    assert_eq!(px(&got, 84, 60), YELLOW);
    assert_eq!(px(&got, 50, 60), px(&flat, 50, 60));
}
