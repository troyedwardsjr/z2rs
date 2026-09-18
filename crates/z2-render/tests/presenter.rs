//! Presenter: sizing, effective scale, and the per-frame call sequence.

use z2_ppu::record::FrameRecord;
use z2_ppu::wide::{wide_width, MarginFill};
use z2_ppu::{indexed_to_rgba, IndexedFrame, HEIGHT, WIDTH};
use z2_render::presenter::{PresentConfig, Presenter};
use z2_render::{ComposeError, HdPack};

fn blank_frame() -> Box<IndexedFrame> {
    let mut f = Box::new([0u8; WIDTH * HEIGHT]);
    for (i, px) in f.iter_mut().enumerate() {
        *px = ((i * 13) % 64) as u8;
    }
    f
}

/// One-cell pack at `scale` (page 0, tile 0).
fn pack(scale: u32) -> HdPack {
    let cell = 8 * scale;
    let edge = 16 * cell;
    let rgba: Vec<u8> = (0..edge * edge)
        .flat_map(|i| {
            let (x, y) = (i % edge, i / edge);
            if x < cell && y < cell {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    let png = z2_render::encode_png_rgba(edge, edge, &rgba, &[]).unwrap();
    let json = format!(
        "{{\"version\":1,\"name\":\"p\",\"scale\":{scale},\"sheets\":[{{\"file\":\"s.png\",\"page\":0}}]}}"
    );
    HdPack::from_files(&[
        ("pack.json".to_string(), json.into_bytes()),
        ("s.png".to_string(), png),
    ])
    .unwrap()
}

#[test]
fn sizes_follow_scale_and_margins() {
    let mut p = Presenter::new(PresentConfig {
        scale: 2,
        ..PresentConfig::default()
    })
    .unwrap();
    assert_eq!((p.width(), p.height()), (512, 480));
    assert!(!p.is_wide() && !p.needs_record());
    p.set_config(PresentConfig {
        scale: 2,
        margin_tiles: 8,
        fill_left_clip: true,
        fill_right_clip: true,
    })
    .unwrap();
    assert_eq!(p.width(), wide_width(8) * 2);
    assert_eq!(p.height(), 480);
    assert!(p.is_wide() && p.needs_record());
    assert!(p.margins().fill_left_clip && p.margins().fill_right_clip);
    assert_eq!(p.margins().tiles, 8);
    assert_eq!(
        Presenter::new(PresentConfig {
            scale: 9,
            ..PresentConfig::default()
        })
        .unwrap_err(),
        ComposeError::BadScale(9)
    );
}

#[test]
fn pack_scale_that_does_not_divide_wins() {
    let mut p = Presenter::new(PresentConfig {
        scale: 3,
        ..PresentConfig::default()
    })
    .unwrap();
    assert_eq!(p.effective_scale(), 3);
    // 4 is not divisible by 3: the pack's own scale is used instead.
    p.set_pack(Some(pack(4))).unwrap();
    assert_eq!(p.effective_scale(), 4);
    assert_eq!((p.width(), p.height()), (WIDTH * 4, HEIGHT * 4));
    assert!(p.needs_record());
    // 2 divides 4: the requested scale is honoured by point-sampling.
    p.set_config(PresentConfig {
        scale: 2,
        ..PresentConfig::default()
    })
    .unwrap();
    assert_eq!(p.effective_scale(), 2);
    assert_eq!(p.width(), WIDTH * 2);
    p.set_pack(None).unwrap();
    assert_eq!(p.effective_scale(), 2);
}

#[test]
fn present_returns_the_upscaled_frame_without_a_pack() {
    let frame = blank_frame();
    let record = FrameRecord::new();
    let mut p = Presenter::new(PresentConfig {
        scale: 2,
        ..PresentConfig::default()
    })
    .unwrap();
    let rgba = p.present(&frame, &record, &[]).unwrap().to_vec();
    assert_eq!(rgba.len(), WIDTH * 2 * HEIGHT * 2 * 4);
    let want = indexed_to_rgba(frame[0]);
    assert_eq!(&rgba[..4], want.as_slice());
    assert_eq!(p.rgba().len(), rgba.len());
    // Widescreen with all-backdrop margins still composes (margins default to
    // Backdrop until a decoder fills them).
    p.set_config(PresentConfig {
        scale: 1,
        margin_tiles: 2,
        fill_left_clip: false,
        fill_right_clip: false,
    })
    .unwrap();
    assert_eq!(p.margins_mut().lines[0].fill, MarginFill::Backdrop);
    let rgba = p.present(&frame, &record, &[]).unwrap();
    assert_eq!(rgba.len(), wide_width(2) * HEIGHT * 4);
}
