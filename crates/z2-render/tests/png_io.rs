//! PNG wrapper: every colour type / depth decodes to RGBA8; encoder checks.

mod common;

use z2_render::{decode_png_rgba, encode_png_rgba, PngError, MAX_SHEET_DIM};

fn raw_png(
    w: u32,
    h: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
    palette: Option<(Vec<u8>, Vec<u8>)>,
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(depth);
        if let Some((pal, trns)) = palette {
            e.set_palette(pal);
            e.set_trns(trns);
        }
        let mut wr = e.write_header().unwrap();
        wr.write_image_data(data).unwrap();
    }
    out
}

#[test]
fn rgba_round_trip_keeps_alpha() {
    let rgba = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 9, 9, 9];
    let bytes = encode_png_rgba(2, 2, &rgba, &[]).unwrap();
    let img = decode_png_rgba(&bytes).unwrap();
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(img.rgba, rgba);
}

#[test]
fn grayscale_8bit_widens_to_rgba() {
    let bytes = raw_png(
        2,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &[10, 200],
        None,
    );
    let img = decode_png_rgba(&bytes).unwrap();
    assert_eq!(img.rgba, [10, 10, 10, 255, 200, 200, 200, 255]);
}

#[test]
fn grayscale_alpha_widens_to_rgba() {
    let bytes = raw_png(
        2,
        1,
        png::ColorType::GrayscaleAlpha,
        png::BitDepth::Eight,
        &[10, 0, 200, 128],
        None,
    );
    assert_eq!(
        decode_png_rgba(&bytes).unwrap().rgba,
        [10, 10, 10, 0, 200, 200, 200, 128]
    );
}

#[test]
fn low_depth_grayscale_expands() {
    let bytes = raw_png(
        4,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Two,
        &[0b0001_1011],
        None,
    );
    let img = decode_png_rgba(&bytes).unwrap();
    let greys: Vec<u8> = img.rgba.chunks(4).map(|p| p[0]).collect();
    assert_eq!(greys, [0, 85, 170, 255]);
    assert!(img.rgba.chunks(4).all(|p| p[3] == 255));
}

#[test]
fn sixteen_bit_is_stripped_to_eight() {
    let bytes = raw_png(
        1,
        1,
        png::ColorType::Rgba,
        png::BitDepth::Sixteen,
        &[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xFF, 0xFF],
        None,
    );
    assert_eq!(
        decode_png_rgba(&bytes).unwrap().rgba,
        [0x12, 0x56, 0x9A, 0xFF]
    );
    let grey16 = raw_png(
        1,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Sixteen,
        &[0x80, 0x01],
        None,
    );
    assert_eq!(
        decode_png_rgba(&grey16).unwrap().rgba,
        [0x80, 0x80, 0x80, 0xFF]
    );
}

#[test]
fn rgb_and_palette_expand_with_trns() {
    let rgb = raw_png(
        1,
        1,
        png::ColorType::Rgb,
        png::BitDepth::Eight,
        &[1, 2, 3],
        None,
    );
    assert_eq!(decode_png_rgba(&rgb).unwrap().rgba, [1, 2, 3, 255]);
    let indexed = raw_png(
        2,
        1,
        png::ColorType::Indexed,
        png::BitDepth::Eight,
        &[0, 1],
        Some((vec![255, 0, 0, 0, 255, 0], vec![0])),
    );
    assert_eq!(
        decode_png_rgba(&indexed).unwrap().rgba,
        [255, 0, 0, 0, 0, 255, 0, 255]
    );
}

#[test]
fn oversized_and_garbage_are_rejected() {
    let w = MAX_SHEET_DIM + 1;
    let wide = raw_png(
        w,
        1,
        png::ColorType::Grayscale,
        png::BitDepth::Eight,
        &vec![0; w as usize],
        None,
    );
    assert_eq!(
        decode_png_rgba(&wide),
        Err(PngError::TooLarge {
            width: w,
            height: 1
        })
    );
    assert!(matches!(
        decode_png_rgba(b"definitely not a png"),
        Err(PngError::Decode(_))
    ));
}

#[test]
fn encoder_checks_buffer_and_writes_text() {
    assert_eq!(
        encode_png_rgba(2, 2, &[0; 15], &[]),
        Err(PngError::BadBuffer { want: 16, got: 15 })
    );
    let bytes = encode_png_rgba(1, 1, &[0; 4], &[("z2rs-template", "marker")]).unwrap();
    assert!(common::contains(&bytes, b"z2rs-template"));
}
