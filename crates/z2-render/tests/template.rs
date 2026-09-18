//! Template sheets from (synthetic) CHR round-trip through the pack loader.

mod common;

use common::{contains, synthetic_chr};
use z2_render::chr::{chr_page, upscale_tile_rgba};
use z2_render::template::{MARKER_FILE, TEMPLATE_TEXT_KEY};
use z2_render::{
    decode_png_rgba, template_from_chr, tile_subpixels, HdPack, MasterPalette, TemplateError,
    TemplateOptions,
};

fn opts<'a>(pages: &'a [u8], scale: u32) -> TemplateOptions<'a> {
    TemplateOptions {
        name: "synthetic",
        scale,
        pages,
        colors: None,
    }
}

#[test]
fn template_round_trips_through_pack_loader() {
    let chr = synthetic_chr(32);
    let pages = [0u8, 5, 31];
    for scale in [1, 2, 3] {
        let out = template_from_chr(&chr, &opts(&pages, scale)).unwrap();
        assert_eq!(out.sheets.len(), 3);
        let pack = HdPack::from_files(&out.into_files()).unwrap();
        assert_eq!(pack.scale(), scale);
        assert_eq!(pack.tile_count(), 3 * 256);
        assert_eq!(pack.variant_count(), 0);
        for &page in &pages {
            let bytes = chr_page(&chr, page).unwrap();
            for t in 0..=255u8 {
                let cell = pack
                    .lookup(page, t, [0x0F, 0x16, 0x30])
                    .unwrap_or_else(|| panic!("page {page} tile {t} unresolved"));
                let want = upscale_tile_rgba(
                    &tile_subpixels(bytes, t),
                    z2_render::template::GRAY_RAMP,
                    scale,
                );
                assert_eq!(
                    pack.cell_pixels(cell).to_vec(),
                    want,
                    "page {page} tile {t} scale {scale}"
                );
            }
        }
        assert_eq!(pack.lookup(1, 0, [0; 3]), None, "page not templated");
    }
}

#[test]
fn template_files_carry_legal_markers() {
    let chr = synthetic_chr(2);
    let out = template_from_chr(&chr, &opts(&[1, 0, 1], 2)).unwrap();
    let names: Vec<&str> = out.sheets.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["sheets/page00.png", "sheets/page01.png"]);
    let manifest: serde_json::Value = serde_json::from_str(&out.pack_json).unwrap();
    assert_eq!(manifest["format"], "z2rs-hdpack");
    assert_eq!(manifest["scale"], 2);
    for (name, png) in &out.sheets {
        assert!(contains(png, TEMPLATE_TEXT_KEY.as_bytes()), "{name}");
        let img = decode_png_rgba(png).unwrap();
        assert_eq!((img.width, img.height), (256, 256));
    }
    let files = out.into_files();
    assert_eq!(files[0].0, "pack.json");
    assert_eq!(files.last().unwrap().0, MARKER_FILE);
}

#[test]
fn explicit_colors_use_nes_palette() {
    let chr = synthetic_chr(1);
    let colors = [0x16, 0x27, 0x30];
    let mut o = opts(&[0], 1);
    o.colors = Some(colors);
    let pack = HdPack::from_files(&template_from_chr(&chr, &o).unwrap().into_files()).unwrap();
    let sub = tile_subpixels(&chr, 0);
    let cell = pack.cell_pixels(pack.lookup(0, 0, [0; 3]).unwrap());
    for (i, &s) in sub.iter().enumerate() {
        let px = cell.pixel(i as u32 % 8, i as u32 / 8);
        if s == 0 {
            assert_eq!(px[3], 0);
        } else {
            let [r, g, b] = MasterPalette::NES.rgb(colors[usize::from(s) - 1]);
            assert_eq!(px, [r, g, b, 255]);
        }
    }
}

#[test]
fn blank_chr_tiles_stay_unregistered() {
    let mut chr = synthetic_chr(1);
    chr[16 * 9..16 * 10].fill(0);
    let pack = HdPack::from_files(
        &template_from_chr(&chr, &opts(&[0], 1))
            .unwrap()
            .into_files(),
    )
    .unwrap();
    assert_eq!(pack.tile_count(), 255);
    assert_eq!(pack.lookup(0, 9, [0; 3]), None);
}

#[test]
fn template_errors() {
    let chr = synthetic_chr(1);
    assert_eq!(
        template_from_chr(&chr, &opts(&[0], 0)),
        Err(TemplateError::Scale(0))
    );
    assert_eq!(
        template_from_chr(&chr, &opts(&[0], 9)),
        Err(TemplateError::Scale(9))
    );
    assert_eq!(
        template_from_chr(&chr, &opts(&[], 1)),
        Err(TemplateError::NoPages)
    );
    assert_eq!(
        template_from_chr(&chr, &opts(&[32], 1)),
        Err(TemplateError::Page(32))
    );
    assert_eq!(
        template_from_chr(&chr, &opts(&[1], 1)),
        Err(TemplateError::ChrTooShort { page: 1, len: 4096 })
    );
}
