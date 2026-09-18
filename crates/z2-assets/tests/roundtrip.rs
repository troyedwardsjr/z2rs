//! Round-trip + determinism tests for the asset extractor.
//!
//! These tests read the user's ROM at test time via [`z2_assets::rom::open`]
//! (the `Z2_ROM` env var, ROM hash gate). No ROM bytes are committed:
//! every expected value is sliced from the ROM file during the test run,
//! and generated artifacts (`assets.bin`) are gitignored (see root
//! `.gitignore`).
//!
//! Run: `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-assets`.

use z2_assets::assets_bin;
use z2_assets::extract;
use z2_assets::extract_tables::{section_id, SECTION_TABLE};
use z2_assets::rom;

/// Open the reference ROM through the hash gate, or `None` when `Z2_ROM` is
/// unset so public CI (`cargo test` without a ROM) stays green — ROM-gated
/// coverage runs with `Z2_ROM` set (see LEGAL.md).
fn rom_body() -> Option<Vec<u8>> {
    match rom::open() {
        Ok(body) => Some(body),
        Err(e) => {
            eprintln!("skipping ROM-gated test: {e}");
            None
        }
    }
}

/// Body-relative index for a headered-file offset.
fn body_idx(file_off: u32) -> usize {
    file_off as usize - rom::INES_HEADER_LEN
}

#[test]
fn extraction_is_deterministic_and_hashed() {
    let Some(body) = rom_body() else { return };
    let first = extract::extract(&body).expect("extract reference ROM");
    let second = extract::extract(&body).expect("extract reference ROM again");

    assert_eq!(first, second, "same ROM must give identical extraction");

    let image_a = assets_bin::encode(&first.sections, first.body_crc32, first.body_sha1);
    let image_b = assets_bin::encode(&second.sections, second.body_crc32, second.body_sha1);
    assert_eq!(image_a, image_b, "same ROM must give identical assets.bin");

    let fingerprint = rom::sha1_hex(&image_a);
    assert_eq!(image_a.len(), image_b.len());
    // Provenance: the bundle records the exact source ROM hashes.
    assert_eq!(first.body_crc32, rom::EXPECTED_BODY_CRC32);
    assert_eq!(rom::sha1_hex(&body), rom::EXPECTED_BODY_SHA1_HEX);
    println!("assets.bin sha1: {fingerprint} ({} bytes)", image_a.len());
}

#[test]
fn roundtrip_rereads_every_section() {
    let Some(body) = rom_body() else { return };
    let extracted = extract::extract(&body).expect("extract reference ROM");
    assert_eq!(
        extracted.sections.len(),
        SECTION_TABLE.len(),
        "every table row extracted exactly once, in order"
    );

    let image = assets_bin::encode(
        &extracted.sections,
        extracted.body_crc32,
        extracted.body_sha1,
    );
    let assets = assets_bin::decode(&image).expect("decode what we encoded");

    assert_eq!(assets.section_count(), SECTION_TABLE.len());
    assert_eq!(assets.body_crc32(), rom::EXPECTED_BODY_CRC32);
    assert_eq!(rom::sha1_hex(&body), rom::EXPECTED_BODY_SHA1_HEX);

    // Every section: id lookup hits, bytes equal an independent ROM slice,
    // record header echoes the table's (id, file offset, length).
    let records = assets.records();
    assert_eq!(records.len(), SECTION_TABLE.len());
    for (def, (id, rom_off, len)) in SECTION_TABLE.iter().zip(records.iter()) {
        assert_eq!(*id, def.id, "record id for {}", def.name);
        assert_eq!(*rom_off, def.file_off, "record offset for {}", def.name);
        assert_eq!(
            *len as usize, def.len as usize,
            "record len for {}",
            def.name
        );

        let bytes = assets
            .get(def.id)
            .unwrap_or_else(|| panic!("missing section {}", def.name));
        assert_eq!(bytes.len(), def.len as usize, "size: {}", def.name);
        let want = &body[body_idx(def.file_off)..body_idx(def.file_off) + def.len as usize];
        assert_eq!(bytes, want, "bytes: {}", def.name);
    }

    // CHR paging view: 32 x 4 KiB pages covering the whole CHR section.
    let chr = assets.get(section_id::CHR_ROM).expect("chr_rom");
    assert_eq!(chr.len(), 128 * 1024);
    for page in [0, 1, 31] {
        let p = assets.chr_page(page).expect("chr page");
        assert_eq!(p.len(), 4096);
        assert_eq!(p, &chr[page * 4096..(page + 1) * 4096]);
    }
    assert!(assets.chr_page(32).is_none());
}

#[test]
fn section_spot_values_from_rom() {
    let Some(body) = rom_body() else { return };
    let extracted = extract::extract(&body).expect("extract reference ROM");
    let at = |file_off: u32, len: usize| -> &[u8] {
        &body[body_idx(file_off)..body_idx(file_off) + len]
    };

    // First RLE byte of West Hyrule comes straight from the ROM (a
    // known example value), generated here at test time.
    let west = extracted.get(section_id::MAP_WEST).expect("map_west");
    assert_eq!(west.len(), 801);
    assert_eq!(west[0], at(0x506C, 1)[0]);

    // Death Mountain and Maze Island blobs are byte-identical in the ROM.
    assert_eq!(
        extracted.get(section_id::MAP_DM).expect("map_dm"),
        extracted.get(section_id::MAP_MAZE).expect("map_maze"),
    );

    // Overworld map pointer tables resolve to the map records they index.
    let ow1 = extracted.get(section_id::OWPTR_B1).expect("owptr_b1");
    let west_ptr = u16::from_le_bytes([ow1[0], ow1[1]]);
    assert_eq!(0x4010 + u32::from(west_ptr - 0x8000), 0x506C);

    // Dialog block is non-trivial TBL text with end-of-talk terminators.
    let dlg = extracted.get(section_id::DIALOG).expect("dialog");
    assert_eq!(dlg.len(), 3133);
    assert!(dlg.contains(&0xFF), "dialog has end-of-talk markers");

    // Palettes are PPU indexes; enemy blocks cover every referenced list;
    // Great Palace has no behind maps (14 zero bytes, still extracted).
    for id in [
        section_id::PAL_EXTERIOR,
        section_id::PAL_INTERIOR,
        section_id::PAL_OVERWORLD,
        section_id::PAL_OVERWORLD_SPR,
    ] {
        assert!(extracted
            .get(id)
            .expect("palette")
            .iter()
            .all(|&b| b <= 0x3F));
    }
    assert_eq!(
        extracted.get(section_id::SB5_BEHIND).expect("sb5_behind"),
        &[0u8; 14]
    );

    // Bank-6 song complexes are non-empty and pairwise distinct blobs.
    let songs = [
        section_id::MUSIC_SONG0,
        section_id::MUSIC_SONG1,
        section_id::MUSIC_SONG2,
        section_id::MUSIC_SONG3,
        section_id::MUSIC_SONG4,
    ];
    for id in songs {
        assert!(!extracted.get(id).expect("song").is_empty());
    }
    assert_ne!(
        extracted.get(section_id::MUSIC_SONG0).expect("song0"),
        extracted.get(section_id::MUSIC_SONG1).expect("song1"),
    );

    // Save signature: 17 bytes of trailing-PR G data, all printable ASCII
    // (the SRAM init marker; exact bytes compared against the ROM slice in
    // the round-trip test above, never hardcoded here).
    let sig = extracted.get(section_id::SAVE_SIG).expect("save_sig");
    assert_eq!(sig.len(), 17);
    assert!(
        sig.iter().all(|b| (0x20..=0x7Eu8).contains(b)),
        "save signature is printable ASCII"
    );
}

#[test]
fn io_helper_writes_readable_bundle() {
    // Absent = unset, empty, or not an existing file (public CI used to
    // export an empty `Z2_ROM`).
    let rom_path = match std::env::var(rom::ROM_ENV_VAR) {
        Ok(p) if std::path::Path::new(&p).is_file() => p,
        _ => {
            eprintln!(
                "skipping io_helper_writes_readable_bundle: Z2_ROM not set to an existing file"
            );
            return;
        }
    };
    let dir = std::env::temp_dir().join("z2rs-assets-roundtrip");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let out = dir.join("roundtrip-assets.bin");

    let summary = z2_assets::assets_bin_io::run(std::path::Path::new(&rom_path), &out)
        .expect("extract to temp file");
    assert_eq!(summary.sections, SECTION_TABLE.len());

    let image = std::fs::read(&out).expect("read back bundle");
    assert_eq!(rom::sha1_hex(&image), summary.assets_sha1_hex);
    let assets = assets_bin::decode(&image).expect("decode file bundle");
    assert_eq!(assets.section_count(), SECTION_TABLE.len());
    std::fs::remove_file(&out).ok();
}
