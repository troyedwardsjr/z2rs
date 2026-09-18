//! ROM-free codec tests for `assets.bin`.
//!
//! These tests use only synthetic sections, so they run anywhere — host,
//! `wasm32-unknown-unknown` builds, and the headless-browser suite.
//! (No `Z2_ROM`, no filesystem, no `std`-only APIs under test.)

use z2_assets::assets_bin::{self, DecodeError};
use z2_assets::extract::RawSection;
use z2_assets::extract_tables::SectionDef;

fn fake_def(id: u16, name: &'static str, file_off: u32, len: u32) -> SectionDef {
    SectionDef {
        id,
        name,
        file_off,
        len,
        label: "synthetic (test only)",
    }
}

#[test]
fn codec_roundtrips_synthetic_sections() {
    let a = fake_def(0x0100, "a", 0x20010, 4);
    let b = fake_def(0x0300, "b", 0x506C, 0);
    let big = fake_def(0x04F0, "big", 0x4010, 65536);
    let big_bytes = vec![0xABu8; 65536];
    let sections = [
        RawSection {
            def: &a,
            bytes: &[1, 2, 3, 4],
        },
        RawSection {
            def: &b,
            bytes: &[],
        },
        RawSection {
            def: &big,
            bytes: &big_bytes,
        },
    ];
    let image = assets_bin::encode(&sections, 0x1234_5678, [0x5Au8; 20]);
    let assets = assets_bin::decode(&image).expect("decode synthetic bundle");

    assert_eq!(assets.section_count(), 3);
    assert_eq!(assets.body_crc32(), 0x1234_5678);
    assert_eq!(assets.body_sha1(), [0x5Au8; 20]);
    assert_eq!(assets.get(0x0100), Some(&[1u8, 2, 3, 4][..]));
    assert_eq!(assets.get(0x0300), Some(&[][..]));
    assert_eq!(assets.get(0x04F0).expect("big").len(), 65536);
    assert!(assets.get(0x9999).is_none());
    assert_eq!(
        assets.records(),
        vec![
            (0x0100, 0x20010, 4),
            (0x0300, 0x506C, 0),
            (0x04F0, 0x4010, 65536)
        ]
    );
}

#[test]
fn codec_rejects_corrupt_images() {
    let a = fake_def(0x0100, "a", 0x20010, 2);
    let sections = [RawSection {
        def: &a,
        bytes: &[9, 9],
    }];
    let image = assets_bin::encode(&sections, 0, [0u8; 20]);

    assert_eq!(assets_bin::decode(&[]), Err(DecodeError::TruncatedHeader));
    let mut bad_magic = image.clone();
    bad_magic[0] = b'X';
    assert_eq!(assets_bin::decode(&bad_magic), Err(DecodeError::BadMagic));
    let mut bad_version = image.clone();
    bad_version[4] = 0xFF;
    assert!(matches!(
        assets_bin::decode(&bad_version),
        Err(DecodeError::UnsupportedVersion { .. })
    ));
    assert_eq!(
        assets_bin::decode(&image[..image.len() - 1]),
        Err(DecodeError::BlobOutOfBounds)
    );
    let mut trailing = image.clone();
    trailing.push(0);
    assert_eq!(
        assets_bin::decode(&trailing),
        Err(DecodeError::TrailingBytes)
    );
    // Corrupting the record's data_off breaks the contiguous-blob layout.
    let mut bad_off = image.clone();
    bad_off[44] = bad_off[44].wrapping_add(1);
    assert_eq!(
        assets_bin::decode(&bad_off),
        Err(DecodeError::BadBlobLayout)
    );
}
