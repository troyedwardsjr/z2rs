//! Recorder bookkeeping, determinism and palette-specific template packs.

mod common;

use common::{contains, synthetic_chr};
use z2_render::chr::{chr_page, upscale_tile_rgba};
use z2_render::recorder::{SeenStats, SEEN_FILE};
use z2_render::template::{MARKER_FILE, TEMPLATE_TEXT_KEY};
use z2_render::{
    decode_png_rgba, tile_subpixels, HdPack, MasterPalette, Recorder, SeenAs, SeenKey,
    TemplateError,
};

fn key(page: u8, tile: u8, colors: [u8; 3]) -> SeenKey {
    SeenKey { page, tile, colors }
}

const A: [u8; 3] = [0x16, 0x27, 0x30];
const B: [u8; 3] = [0x0F, 0x12, 0x21];

fn observations() -> Vec<(SeenKey, SeenAs)> {
    vec![
        (key(3, 7, A), SeenAs::Background),
        (key(3, 7, A), SeenAs::Background),
        (key(3, 7, B), SeenAs::Sprite),
        (key(3, 9, B), SeenAs::Sprite),
        (key(3, 9, B), SeenAs::Background),
        (key(1, 0, A), SeenAs::Background),
    ]
}

#[test]
fn insertion_order_does_not_matter() {
    let chr = synthetic_chr(4);
    let mut fwd = Recorder::new();
    for (k, a) in observations() {
        fwd.insert(k, a);
    }
    let mut rev = Recorder::new();
    for (k, a) in observations().into_iter().rev() {
        rev.insert(k, a);
    }
    assert_eq!(fwd, rev);
    let order: Vec<SeenKey> = fwd.iter().map(|(k, _)| *k).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
    assert_eq!(
        fwd.write_pack(&chr, 2, "rec").unwrap(),
        rev.write_pack(&chr, 2, "rec").unwrap()
    );
}

#[test]
fn counts_layers_and_merge() {
    let mut r = Recorder::new();
    for (k, a) in observations() {
        r.insert(k, a);
    }
    r.end_frame();
    assert_eq!(
        (r.distinct_keys(), r.distinct_tiles(), r.frames()),
        (4, 3, 1)
    );
    let stats: Vec<SeenStats> = r.iter().map(|(_, s)| *s).collect();
    assert_eq!(
        stats[3],
        SeenStats {
            count: 2,
            seen_as: SeenAs::Both
        },
        "3/9/B seen on both layers"
    );

    let obs = observations();
    let (left, right) = obs.split_at(3);
    let mut a = Recorder::new();
    left.iter().for_each(|(k, s)| a.insert(*k, *s));
    a.end_frame();
    let mut b = Recorder::new();
    right.iter().for_each(|(k, s)| b.insert(*k, *s));
    a.merge(&b);
    assert_eq!(a, r);

    // Colours are masked to 6 bits.
    let mut m = Recorder::new();
    m.insert(key(0, 0, [0x7F, 0x40, 0x01]), SeenAs::Sprite);
    assert_eq!(m.iter().next().unwrap().0.colors, [0x3F, 0x00, 0x01]);
}

#[test]
fn default_colors_prefer_frequency_then_lowest_triple() {
    let mut r = Recorder::new();
    r.insert_count(key(0, 1, B), SeenAs::Background, 5);
    r.insert_count(key(0, 1, A), SeenAs::Background, 9);
    assert_eq!(r.default_colors(0, 1), Some(A));
    r.insert_count(key(0, 1, B), SeenAs::Background, 4);
    assert_eq!(r.default_colors(0, 1), Some(B), "tie 9/9 -> lowest triple");
    assert_eq!(r.default_colors(0, 2), None);
}

#[test]
fn write_pack_emits_palette_variants_that_load() {
    let chr = synthetic_chr(4);
    let mut r = Recorder::new();
    for (k, a) in observations() {
        r.insert(k, a);
    }
    let files = r.write_pack(&chr, 2, "rec").unwrap();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "pack.json",
            "sheets/page01.png",
            "sheets/page03.png",
            "sheets/page03.c0F-12-21.png",
            SEEN_FILE,
            MARKER_FILE,
        ]
    );
    for (name, bytes) in files.iter().filter(|(n, _)| n.ends_with(".png")) {
        assert!(contains(bytes, TEMPLATE_TEXT_KEY.as_bytes()), "{name}");
    }

    // Unseen cells are fully transparent.
    let page3 = decode_png_rgba(&files[2].1).unwrap();
    assert_eq!((page3.width, page3.height), (256, 256));
    let cell0_alpha: Vec<u8> = (0..16)
        .flat_map(|y| (0..16).map(move |x| (x, y)))
        .map(|(x, y)| page3.pixel(x, y)[3])
        .collect();
    assert!(cell0_alpha.iter().all(|&a| a == 0));

    let pack = HdPack::from_files(&files).unwrap();
    assert_eq!((pack.tile_count(), pack.variant_count()), (3, 1));
    let bytes3 = chr_page(&chr, 3).unwrap();
    let expect = |tile: u8, colors: [u8; 3]| {
        upscale_tile_rgba(
            &tile_subpixels(bytes3, tile),
            colors.map(|c| MasterPalette::NES.rgb(c)),
            2,
        )
    };
    let px = |tile, colors| {
        pack.cell_pixels(pack.lookup(3, tile, colors).unwrap())
            .to_vec()
    };
    // 3/7: A seen twice (default), B once (variant).
    assert_eq!(px(7, A), expect(7, A));
    assert_eq!(px(7, B), expect(7, B));
    assert_eq!(px(7, [1, 2, 3]), expect(7, A), "unseen triple -> default");
    assert_eq!(px(9, B), expect(9, B));
    assert_eq!(pack.lookup(3, 8, A), None);

    let seen: serde_json::Value = serde_json::from_slice(&files[4].1).unwrap();
    let rows = seen.as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["page"], 1);
    assert_eq!(rows[3]["as"], "both");
}

#[test]
fn write_pack_errors() {
    let chr = synthetic_chr(1);
    assert_eq!(
        Recorder::new().write_pack(&chr, 1, "x"),
        Err(TemplateError::NoPages)
    );
    let mut r = Recorder::new();
    r.insert(key(2, 0, A), SeenAs::Sprite);
    assert_eq!(r.write_pack(&chr, 9, "x"), Err(TemplateError::Scale(9)));
    assert_eq!(
        r.write_pack(&chr, 1, "x"),
        Err(TemplateError::ChrTooShort { page: 2, len: 4096 })
    );
}
