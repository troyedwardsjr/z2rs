//! Integration tests for the `.bk2` parser.
//!
//! Wiring: `z2-verify` lib target with `mod movie_bk2;`.
//! Run: `cargo test -p z2-verify --test movie_bk2` (no ROM, no network).
//!
//! NOTE: these tests use `Stored` (uncompressed) fixtures. Real BizHawk
//! `.bk2` files deflate every member; that path (`flate2`) and the real
//! console-cell + `LogKey` shape are covered by the lib tests in
//! `src/movie_bk2.rs`.

use z2_verify::movie_bk2::{parse_bk2_zip, parse_input_log};

#[test]
fn tiny_input_log_parses_with_nes_bit_order() {
    let log = parse_input_log(
        "[Input]\n\
         LogKey: #NES P1|#NES P2\n\
         | 0|........|........|\n\
         | 1|.......A|........|\n\
         | 2|RLDUTSBA|........|\n",
    )
    .expect("tiny input log must parse");
    assert_eq!(log.frames.len(), 3);
    assert_eq!(log.pad1_track(), vec![0x00, 0x01, 0xFF]);
}

#[test]
fn garbage_is_not_a_bk2() {
    assert!(parse_bk2_zip(b"not a zip at all..............").is_err());
}
