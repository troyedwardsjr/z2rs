//! Integration tests for the `.fm2` parser.
//!
//! Wiring: `z2-verify` lib target with `mod movie_fm2;`.
//! Run: `cargo test -p z2-verify --test movie_fm2` (no ROM, no network).

use z2_verify::movie_fm2::{parse_fm2, FM2_COL_BITS, FM2_COL_MNEMONIC};

const TINY: &str = "version 3\n\
     emuVersion 20501\n\
     rerecordCount 7\n\
     palFlag 0\n\
     romFilename Zelda II - The Adventure of Link (USA).nes\n\
     guid 12345678-1234-1234-1234-123456789abc\n\
     fourscore 0\n\
     port0 1\n\
     |0|........|\n\
     |0|.......A|\n\
     |0|RLDUTSBA|\n";

#[test]
fn tiny_fixture_parses_with_nes_bit_order() {
    let m = parse_fm2(TINY).expect("tiny fm2 must parse");
    assert_eq!(m.frames.len(), 3);
    assert_eq!(m.frames[0].pad1(), 0x00);
    assert_eq!(m.frames[1].pad1(), 0x01); // A = bit 0
    assert_eq!(m.frames[2].pad1(), 0xFF); // RLDUTSBA = all buttons
    assert!(m.header.rom_name_looks_like_z2_usa());
    assert!(m.warnings().is_empty());
    assert_eq!(m.pad1_track(), vec![0x00, 0x01, 0xFF]);
}

#[test]
fn each_column_maps_to_its_bit() {
    for (col, bit) in FM2_COL_BITS.iter().enumerate() {
        let mut cells = ['.'; 8];
        cells[col] = FM2_COL_MNEMONIC[col];
        let field: String = cells.iter().collect();
        let m = parse_fm2(&format!("|0|{field}|\n")).unwrap();
        assert_eq!(m.frames[0].pad1(), 1 << bit, "col {col}");
    }
}
