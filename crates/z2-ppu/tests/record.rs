//! ROM-free tests for the opt-in render record and the wide indexed
//! renderer, on synthetic PPU scenes (hand-made CHR, nametables, OAM).

use z2_ppu::{
    chr_sub, diff_indexed, render_wide_indexed, AccessKind, BgTileId, FrameRecord, IndexedFrame,
    MarginFill, Margins, Mirroring, OamEntry, Ppu, SpriteLimit, SpriteRef, WideFrame, CHR_BANK_LEN,
    PPUCTRL_BG_TABLE, PPUCTRL_TALL_SPRITES, WIDTH,
};

const PAGES: usize = 4;

/// Synthetic CHR image: every page/tile/row gets distinct-ish planes.
fn chr_image() -> Vec<u8> {
    let mut chr = vec![0u8; PAGES * CHR_BANK_LEN];
    for p in 0..PAGES {
        for t in 0..256usize {
            for r in 0..8usize {
                let base = p * CHR_BANK_LEN + t * 16 + r;
                chr[base] = (t as u8) ^ ((r as u8).wrapping_mul(37)) ^ ((p as u8) * 11);
                chr[base + 8] = (t as u8).rotate_left(3) ^ (r as u8) ^ (p as u8).wrapping_mul(71);
            }
        }
    }
    chr
}

fn load_page(p: &mut Ppu, chr: &[u8], slot: usize, page: usize) {
    p.load_chr_4k(
        slot,
        page as u8,
        &chr[page * CHR_BANK_LEN..(page + 1) * CHR_BANK_LEN],
    )
    .unwrap();
}

/// Scene: vertical mirroring, patterned nametables/attributes, distinct
/// palette, rendering on (left-8 shown), no sprites.
fn scene(chr: &[u8]) -> Ppu {
    let mut p = Ppu::new();
    p.set_mirroring(Mirroring::Vertical);
    load_page(&mut p, chr, 0, 1);
    load_page(&mut p, chr, 1, 2);
    for nt in 0..2u8 {
        for row in 0..30u8 {
            for col in 0..32u8 {
                let t = col
                    .wrapping_mul(7)
                    .wrapping_add(row.wrapping_mul(13))
                    .wrapping_add(nt * 101);
                p.set_tile(nt, col, row, t);
            }
        }
        for ar in 0..8u8 {
            for ac in 0..8u8 {
                for q in 0..4u8 {
                    p.set_attr_quad(nt, ac, ar, q & 1, q >> 1, (ac + ar + q + nt) & 3);
                }
            }
        }
    }
    for i in 0..32usize {
        p.set_palette(i, (i as u8 * 5 + 3) & 0x3F);
    }
    p.write_mask(0x1E);
    p
}

fn add_sprites(p: &mut Ppu) {
    for i in 0..12usize {
        p.set_oam_entry(
            i,
            OamEntry {
                y: 60 + (i as u8 % 3),
                tile: 0x30 + i as u8,
                attr: (i as u8) & 0xE3,
                x: 20 + 9 * i as u8,
            },
        );
    }
}

/// One game-like frame with mid-frame effects: a line-boundary scroll/CHR
/// switch, a mid-line `$2005`, a mid-line palette write and a mask blank.
fn drive_frame(p: &mut Ppu, chr: &[u8], n: u8) -> IndexedFrame {
    p.begin_frame();
    let _ = p.read_status();
    p.write_ctrl(0);
    p.write_scroll(n.wrapping_mul(3));
    p.write_scroll(0);
    p.set_beam(-1, 340);
    p.set_beam(20, 10);
    let _ = p.read_status();
    // Line-boundary split: scroll + bg table + CHR page from line 41.
    p.set_beam_access(40, 300, AccessKind::Fetch);
    p.write_scroll(77);
    p.write_scroll(0);
    p.set_beam_access(40, 310, AccessKind::Fetch);
    p.write_ctrl(PPUCTRL_BG_TABLE);
    load_page(p, chr, 1, 3);
    // Mid-line `$2005` on line 90.
    p.set_beam_access(90, 60, AccessKind::Fetch);
    let _ = p.read_status();
    p.write_scroll(19);
    // Mid-line palette write on line 120.
    p.set_beam_access(120, 100, AccessKind::Pixel);
    p.set_palette(3, 0x2A);
    // Blank lines from 200 on, then back on (next frame).
    p.set_beam_access(199, 300, AccessKind::Pixel);
    p.write_mask(0);
    let f = p.finish_frame();
    p.end_frame();
    // Restore for the next frame.
    p.write_mask(0x1E);
    load_page(p, chr, 1, 2);
    p.set_palette(3, 3 * 5 + 3);
    f
}

#[test]
fn record_off_by_default_and_toggle() {
    let mut p = Ppu::new();
    assert!(!p.record_enabled());
    assert!(p.frame_record().is_none());
    p.set_record(true);
    assert!(p.record_enabled());
    assert!(p.frame_record().is_some());
    let _ = p.render_frame();
    assert_eq!(p.frame_record().unwrap().lines_done, 240);
    p.set_record(false);
    assert!(p.frame_record().is_none());
}

#[test]
fn record_on_does_not_change_pixels_or_flags() {
    let chr = chr_image();
    let mut off = scene(&chr);
    add_sprites(&mut off);
    // Sprite 0 over opaque background for a real hit.
    off.set_oam_entry(
        0,
        OamEntry {
            y: 30,
            tile: 0xFF,
            attr: 0,
            x: 100,
        },
    );
    let mut on = off.clone();
    on.set_record(true);
    for limit in [SpriteLimit::Faithful8, SpriteLimit::Unlimited] {
        off.set_sprite_limit(limit);
        on.set_sprite_limit(limit);
        for n in 0..4u8 {
            let fa = drive_frame(&mut off, &chr, n);
            let fb = drive_frame(&mut on, &chr, n);
            let d = diff_indexed(&fa, &fb);
            assert!(d.is_clean(), "frame {n}: {} px differ", d.count);
            assert_eq!(off.sprite0_hit_at(), on.sprite0_hit_at());
            assert_eq!(off.status(), on.status());
            assert_eq!(off.sprite_overflow(), on.sprite_overflow());
            assert_eq!(
                (off.v(), off.t(), off.fine_x()),
                (on.v(), on.t(), on.fine_x())
            );
            assert_eq!(on.frame_record().unwrap().lines_done, 240);
        }
    }
    // Plain whole-frame renders too.
    let fa = off.render_frame();
    let fb = on.render_frame();
    assert!(diff_indexed(&fa, &fb).is_clean());
}

#[test]
fn record_captures_bg_identity_from_nametable() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_tile(0, 3, 2, 0xA7);
    p.set_attr_quad(0, 0, 0, 1, 1, 2);
    p.set_record(true);
    p.write_scroll(13); // coarse_x 1, fine_x 5
    p.write_scroll(0);
    let _ = p.render_frame();
    let rec = p.frame_record().unwrap();
    for y in 16..24usize {
        let l = rec.line(y);
        assert!(l.valid && !l.split && !l.pixel_split);
        assert_eq!(
            l.tiles[2],
            BgTileId {
                page: 1,
                tile: 0xA7,
                pal: 2,
                fine_y: (y & 7) as u8,
                nt: 0,
                coarse_x: 3,
                coarse_y: 2,
                fetched: true,
            },
            "line {y}"
        );
        assert_eq!(l.v_start & 0x1F, 1);
        assert_eq!(l.fine_x, 5);
        assert_eq!(FrameRecord::ring_x(l), 13);
        assert_eq!(FrameRecord::coarse_y(l), 2);
        assert_eq!(FrameRecord::bg_page(l), 1);
        assert!(FrameRecord::show_bg(l));
    }
    // Every fetch of every line matches the nametable through the `v` walk.
    for y in 0..240usize {
        let l = rec.line(y);
        let row = y / 8;
        for (k, id) in l.tiles.iter().enumerate() {
            let x = 1 + k;
            let (nt, col) = ((x / 32) % 2, x % 32);
            let addr = 0x2000 + (nt << 10) + row * 32 + col;
            assert_eq!(id.tile, p.nt_read(addr as u16), "y {y} k {k}");
            assert_eq!(usize::from(id.nt), nt);
            assert_eq!(usize::from(id.coarse_x), col);
            assert_eq!(l.palette, *p.palette());
        }
    }
}

/// Rebuild every non-split, sprite-free, bg-on line from the record's tile
/// identities + the CHR image and compare with the rendered frame (the HD
/// contract: identity alone reproduces the pixels).
#[test]
fn record_reconstructs_background_across_scroll_and_chr_splits() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_record(true);
    let mut checked = 0usize;
    for n in 0..3u8 {
        let frame = drive_frame(&mut p, &chr, n);
        let rec = p.frame_record().unwrap();
        let mut pages = std::collections::BTreeSet::new();
        for y in 0..240usize {
            let l = rec.line(y);
            assert!(l.valid);
            if l.split || l.pixel_split || !FrameRecord::show_bg(l) {
                continue;
            }
            assert!(rec.sprites_on(y).is_empty());
            pages.insert(FrameRecord::bg_page(l));
            for x in 0..WIDTH {
                let k = (x + usize::from(l.fine_x)) >> 3;
                let sub_x = ((x + usize::from(l.fine_x)) & 7) as u8;
                let id = l.tiles[k];
                assert_eq!(id.page, FrameRecord::bg_page(l), "y {y} x {x}");
                let want = match chr_sub(&chr, id, sub_x).unwrap() {
                    0 => l.backdrop,
                    s => l.palette[usize::from(id.pal) * 4 + usize::from(s)],
                };
                assert_eq!(frame[y * WIDTH + x], want, "frame {n} y {y} x {x}");
            }
            checked += 1;
        }
        assert_eq!(pages.len(), 2, "both CHR pages seen in frame {n}");
    }
    assert!(checked > 3 * 150, "checked {checked} lines");
}

#[test]
fn record_marks_mid_line_splits_and_blank_lines() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_record(true);
    let _ = drive_frame(&mut p, &chr, 0);
    let rec = p.frame_record().unwrap();
    // `$2005` at (90, 60): fine_x changes mid-line.
    let l90 = rec.line(90);
    assert!(l90.split);
    assert_ne!(l90.fine_x, l90.fine_x_end);
    assert_eq!(l90.fine_x_end, 19 & 7);
    assert_eq!(rec.line(91).fine_x, 19 & 7);
    assert!(!rec.line(91).split);
    // Line-boundary writes (dot >= 256) never split a line.
    assert!(!rec.line(40).split && !rec.line(41).split);
    assert_eq!(FrameRecord::bg_page(rec.line(40)), 1);
    assert_eq!(FrameRecord::bg_page(rec.line(41)), 3);
    // Palette write mid-line 120.
    assert!(rec.line(120).pixel_split);
    assert!(!rec.line(121).pixel_split);
    assert_eq!(rec.line(121).palette[3], 0x2A);
    // Rendering off from line 200.
    let l = rec.line(200);
    assert!(!FrameRecord::show_bg(l));
    assert!(l.tiles.iter().all(|t| !t.fetched));
    assert!(FrameRecord::show_bg(rec.line(199)));
}

#[test]
fn record_captures_chosen_sprites_8x16() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.write_ctrl(PPUCTRL_TALL_SPRITES);
    p.set_oam_entry(
        5,
        OamEntry {
            y: 50,
            tile: 0xC4,
            attr: 0xC1,
            x: 60,
        },
    );
    p.set_record(true);
    let _ = p.render_frame();
    let rec = p.frame_record().unwrap();
    assert!(rec.sprites_on(50).is_empty());
    assert!(rec.sprites_on(67).is_empty());
    for y in 51..=66usize {
        let row = (y - 51) as u8;
        let r = 15 - row;
        let got = rec.sprites_on(y);
        assert_eq!(
            got,
            &[SpriteRef {
                oam_index: 5,
                x: 60,
                page: p.chr_page_no(0),
                tile: if r < 8 { 0xC4 } else { 0xC5 },
                fine_row: r & 7,
                row_in_sprite: row,
                pal: 1,
                flip_h: true,
                flip_v: true,
                behind: false,
                tall: true,
            }],
            "line {y}"
        );
    }
}

#[test]
fn record_sprite_list_follows_eight_sprite_dropout() {
    let chr = chr_image();
    let mut p = scene(&chr);
    for i in 0..9usize {
        p.set_oam_entry(
            i,
            OamEntry {
                y: 99,
                tile: 0x10 + i as u8,
                attr: 0x20,
                x: 10 * i as u8,
            },
        );
    }
    p.set_record(true);
    let _ = p.render_frame();
    let rec = p.frame_record().unwrap();
    let s = rec.sprites_on(100);
    assert_eq!(s.len(), 8);
    assert!(s
        .iter()
        .enumerate()
        .all(|(i, e)| usize::from(e.oam_index) == i));
    assert!(s.iter().all(|e| e.behind && !e.tall && e.page == 1));
    assert!(rec.line(100).sprite_overflow);
    assert_eq!(s[3].tile, 0x13);
    assert_eq!(s[3].fine_row, 0);

    p.begin_frame();
    p.set_sprite_limit(SpriteLimit::Unlimited);
    let _ = p.render_frame();
    let rec = p.frame_record().unwrap();
    assert_eq!(rec.sprites_on(100).len(), 9);
    assert!(!rec.line(100).sprite_overflow);
    assert_eq!(rec.sprites_on(107).len(), 9);
    assert!(rec.sprites_on(108).is_empty());
}

#[test]
fn record_enabled_mid_frame_marks_earlier_lines_invalid() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_beam(-1, 340);
    p.set_beam(100, 0);
    p.set_record(true);
    let _ = p.finish_frame();
    let rec = p.frame_record().unwrap();
    assert!(!rec.line(99).valid);
    assert!(rec.line(100).valid);
    assert_eq!(rec.lines_done, 140);
}

#[test]
fn wide_render_of_recorded_frame_keeps_centre_and_paints_margins() {
    let chr = chr_image();
    let mut p = scene(&chr);
    p.set_record(true);
    p.write_scroll(0);
    p.write_scroll(0);
    let frame = p.render_frame();
    let rec = p.frame_record().unwrap();
    let tiles = 11u8;
    let mut m = Margins::new(tiles);
    for (y, ml) in m.lines.iter_mut().enumerate() {
        ml.fill = MarginFill::Tiles;
        // Continue the line with the record's own geometry: slot -1-j and
        // slot 32+j repeat tiles 31-j / j of the same row (a tiling world).
        let l = rec.line(y);
        for j in 0..=usize::from(tiles) {
            ml.left[j] = l.tiles[(31 - j) % 32];
            ml.right[j] = l.tiles[j % 32];
        }
    }
    let mut out = WideFrame::new(0);
    render_wide_indexed(&frame, rec, &m, &chr, &mut out);
    assert_eq!(out.width, 432);
    let mp = out.margin_px();
    for y in 0..240usize {
        let row = out.row(y);
        assert_eq!(&row[mp..mp + WIDTH], &frame[y * WIDTH..(y + 1) * WIDTH]);
        // With scroll 0 the world tiles every 256 px here, so the right
        // margin repeats the frame's left columns and vice versa.
        for x in 0..mp {
            assert_eq!(
                row[mp + WIDTH + x],
                frame[y * WIDTH + x],
                "right y {y} x {x}"
            );
            assert_eq!(
                row[x],
                frame[y * WIDTH + WIDTH - mp + x],
                "left y {y} x {x}"
            );
        }
    }
}
