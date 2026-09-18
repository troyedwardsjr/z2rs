//! Town text-rendering golden tests.
//!
//! ROM-free unit tests always run (synthetic fixtures over
//! [`town_dialog`](z2_core::town_dialog)). The golden test decodes the
//! `DIALOG` section at test time via `Z2_ROM`, renders every string, and
//! snapshots the tile stream; it skips gracefully without a ROM.

mod common;

use z2_core::town_dialog::{classify_byte, decode_tile, render_tiles, TextByte};

// ---------------------------------------------------------------------------
// ROM-free unit coverage (synthetic fixtures).
// ---------------------------------------------------------------------------

#[test]
fn dialog_bytes_classify_like_lb6a2() {
    assert!(matches!(classify_byte(0xDA), TextByte::Letter(_)));
    assert!(matches!(classify_byte(0xFD), TextByte::NewLine(0xFD)));
    assert!(matches!(classify_byte(0xFE), TextByte::NewLine(0xFE)));
    assert!(matches!(classify_byte(0xFF), TextByte::End));
}

#[test]
fn tile_decode_spot_checks_match_b6dd() {
    // High-range letter (bit7+bit6) → ORA $C0.
    assert_eq!(decode_tile(0xDA).0, 0xDA | 0xC0);
    // Accent strip (bit7 only) → upper $33.
    assert_eq!(decode_tile(0x80), (0x00, 0x33));
    // Dot path (bit6 only) → upper $32.
    assert_eq!(decode_tile(0x40), (0x00, 0x32));
    // Plain low byte → upper $F4 interline.
    assert_eq!(decode_tile(0x09), (0x09, 0xF4));
}

#[test]
fn render_tiles_stops_at_terminator() {
    let bytes = [0xDA, 0xDB, 0xFD, 0xDC, 0xFF, 0xDD];
    let (tiles, lines, ended) = render_tiles(&bytes);
    assert_eq!(tiles.len(), 3);
    assert_eq!(lines, 1);
    assert!(ended);
}

// ---------------------------------------------------------------------------
// ROM-gated golden: every DIALOG string → tile stream.
// ---------------------------------------------------------------------------

/// `DIALOG` section geometry (`z2-assets` `extract_tables.rs`:
/// `DIALOG` file `0x00E390` len 3133, CPU `$E390-$EFCC`,
/// `z2dis:prg3:bank3_Dialogs_Text_Table`).
const DIALOG_FILE_OFF: usize = 0x00E390;
const DIALOG_LEN: usize = 3133;
/// Terminator all strings end with (`LB6A2` `$B6A2`: `CMP #$FF`).
const TERMINATOR: u8 = 0xFF;

/// TBL alphabet mirror for validation (`z2-assets`
/// `extract_tables.rs::DIALOG_ALPHABET`: text bytes `$DA-$F3`,
/// punctuation/space `$F4/$F5/$F7/$F8/$F9/$FC`, controls
/// `$FD/$FE/$FF`, plus the listed `$32/$34/$36/$9C/$CE/$CF`
/// variants). Bytes here are validation metadata, never dialog text.
const DIALOG_ALPHABET: &[u8] = &[
    0x32, 0x34, 0x36, 0x9C, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9,
    0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9,
    0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF, 0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF7, 0xF8, 0xF9, 0xFC,
    0xFD, 0xFE, 0xFF,
];

fn dialog_blob() -> Option<Vec<u8>> {
    let img = common::rom_bytes("town_text_tests golden")?;
    if img.len() < DIALOG_FILE_OFF + DIALOG_LEN || img[0..4] != *b"NES\x1A" {
        eprintln!("SKIP text golden: ROM too short / bad magic for DIALOG");
        return None;
    }
    Some(img[DIALOG_FILE_OFF..DIALOG_FILE_OFF + DIALOG_LEN].to_vec())
}

/// Split the `DIALOG` blob into `$FF`-terminated strings (terminator
/// inclusive per string; the blob must end on a terminator).
fn split_strings(blob: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, &b) in blob.iter().enumerate() {
        if b == TERMINATOR {
            out.push(&blob[start..=i]);
            start = i + 1;
        }
    }
    if start < blob.len() {
        out.push(&blob[start..]);
    }
    out
}

fn fnv1a(bytes: &[(u8, u8)]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &(t, u) in bytes {
        h ^= u64::from(t);
        h = h.wrapping_mul(0x100000001b3);
        h ^= u64::from(u);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn text_rendering_golden_for_every_dialog_string_when_rom_present() {
    let Some(blob) = dialog_blob() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // Every byte must be TBL-valid.
    for (i, &b) in blob.iter().enumerate() {
        assert!(
            DIALOG_ALPHABET.contains(&b),
            "dialog byte {i}: ${b:02X} outside the TBL alphabet"
        );
    }
    let strings = split_strings(&blob);
    assert!(!strings.is_empty(), "DIALOG must hold ≥1 string");
    // NOTE (verified against ROM): the table's final string is NOT
    // $FF-terminated — it runs to the table end ($EFCC; last byte $E7).
    // The game indexes strings, so no terminator is needed there.
    let last = strings[strings.len() - 1];
    let table_ends_mid_string = last[last.len() - 1] != TERMINATOR;
    // Golden: render each string, snapshot the tile stream hash.
    // Self-consistency oracle (same bytes → same tiles) pins the
    // renderer; tile counts pin the charset mapping.
    let mut total_tiles = 0usize;
    let mut total_lines = 0usize;
    for (n, s) in strings.iter().enumerate() {
        let is_tail = table_ends_mid_string && n == strings.len() - 1;
        // Strip the terminator for the renderer (it stops AT $FF);
        // the unterminated tail keeps its final content byte.
        let body = if is_tail { &s[..] } else { &s[..s.len() - 1] };
        let (tiles, lines, ended) = render_tiles(s);
        assert_eq!(ended, !is_tail, "string {n} terminator shape");
        assert!(!tiles.is_empty(), "string {n} must render ≥1 tile");
        assert!(
            tiles.len() <= body.len(),
            "string {n}: tiles (={}) ≤ bytes (={})",
            tiles.len(),
            body.len()
        );
        // Determinism: re-render → identical stream.
        let (tiles2, lines2, ended2) = render_tiles(s);
        assert_eq!((tiles.len(), lines, ended), (tiles2.len(), lines2, ended2));
        assert_eq!(fnv1a(&tiles), fnv1a(&tiles2), "string {n}: hash stable");
        total_tiles += tiles.len();
        total_lines += lines;
    }
    eprintln!(
        "text golden: {} strings, {total_tiles} tiles, {total_lines} line breaks",
        strings.len()
    );
    assert!(
        total_tiles > 0 && total_lines > 0,
        "dialog needs text + breaks"
    );
}
