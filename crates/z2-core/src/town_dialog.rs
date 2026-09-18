//! Town dialog box + text renderer + index resolution.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod town_dialog;` (see `lib.rs`; trap shims
//! live in `town_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Text bytes are NEVER copied here: callers load the `DIALOG` section at
//! runtime (via `Z2_ROM`, `z2-assets` `DIALOG` file `0x00E390` len 3133,
//! CPU `$E390-$EFCC`, `z2dis:prg3:bank3_Dialogs_Text_Table`) and pass the
//! bytes as slices. The TBL alphabet (`z2-assets` `DIALOG_ALPHABET`) is
//! referenced for validation, not duplicated as string data.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`decode_tile`] | `bank3_Load_a_letter` | `$B6DD` | verified |
//! | [`classify_byte`] | `LB6A2` dispatch | `$B6A2` | verified |
//! | [`end_of_line`] | `bank3_End_of_Line_Routine` | `$B656` | verified |
//! | [`text_step`] | `LB6A2` + `bank3_Load_a_letter` | `$B6A2`/`$B6DD` | verified |
//! | [`render_tiles`] | `LB6A2` tile stream | `$B6A2` | verified |
//! | [`resolve_dialog_index`] | `bank3_Dialog_Conditions_default` | `$B5C7` | verified |
//! | [`dialog_pointer`] | `bank3_At_this_point_A_is_the_dialog_index` | `$B5F8` | verified |
//! | [`ppu_packet`] | `bank3_Load_a_letter` PPU macro | `$B6F6` | verified |
//! | [`box_row_tiles`] | `bank3_Tables_for_dialog_box_rows_tile_mappings` | `$B407` | ported |
//!
//! # Preserved quirks (BUG comments)
//!
//! * `bank3_Load_a_letter` (`$B6DD`): bit 7 = 0 → `AND #$3F` tile; bit 7 =
//!   1 + bit 6 = 0 → upper tile `$33` (empty, accent strip); bit 7 = 1 +
//!   bit 6 = 1 → `ORA #$C0` tile (katakana-ish high range). Bit 6 = 1 +
//!   bit 7 = 0 after the second `ASL` → upper tile `$32` (dot). The two
//!   `ASL`s destroy the original byte; the tile/upper pair is the only
//!   output.
//! * `LB6A2` (`$B6A2`): types `$0D`/`$0F` (knights/wise-adjacent) can never
//!   B-cancel (`BEQ LB6BA` skips the B check); everyone else B-skips to
//!   `LB672` (`$B672`) immediately.
//! * Dialog teardown (`LB672`, `$B672`): `$074B = $C0` + `$049E = $40`
//!   flash runs BEFORE the wise/healer branch, so even denied talks flash.
//!
//! # Gaps (honest)
//!
//! * PPU macro drain (`$0301` packet → `$2006/$2007`, `$0725` selector) is
//!   interp-only; [`ppu_packet`] models the 5-byte packet only.
//! * Palette save/restore (`bank3_Dialog_Routines_save_palette…`, `$B350`)
//!   touches level-RAM screens (`$D000`/`$70A0`/`$6060`/`$6261`); geometry
//!   only here, byte moves with the interpreter.
//! * The `$0707 == 2` east tables (`indexes3/4`, `$A2DC`/`$A340`) need the
//!   ROM slices; [`resolve_dialog_index`] takes them as explicit params so
//!   ROM-free tests use synthetic tables.

// ---------------------------------------------------------------------------
// Addresses (duplicated per town*.rs file on purpose).
// ---------------------------------------------------------------------------

/// Text column (`$0489`).
pub const ADDR_TEXT_COL: u16 = 0x0489;
/// Text row (`$048A`: `+$40` = down 2 tiles per line).
pub const ADDR_TEXT_ROW: u16 = 0x048A;
/// Text pointer hi (`$0569`) / lo (`$056A`).
pub const ADDR_TEXT_HI: u16 = 0x0569;
/// Text pointer lo.
pub const ADDR_TEXT_LO: u16 = 0x056A;
/// Delay between letters (`$0566`: `$2A` lead-in, `$05` per letter,
///
/// `$0B` after `$FD`, `$2D` after `$FE`; `bank3_End_of_Line_Routine`
/// `$B656`).
pub const ADDR_TEXT_DELAY: u16 = 0x0566;
/// PPU packet length (`$0301`: always `$05` per letter).
pub const ADDR_PPU_LEN: u16 = 0x0301;
/// PPU packet addr hi (`$0302`) / lo (`$0303`).
pub const ADDR_PPU_HI: u16 = 0x0302;
/// PPU packet addr lo.
pub const ADDR_PPU_LO: u16 = 0x0303;
/// PPU packet bank/attr (`$0304`: always `$82`).
pub const ADDR_PPU_ATTR: u16 = 0x0304;
/// Tile above letter (`$0305`: `$F4` interline or accent tile).
pub const ADDR_PPU_UPPER: u16 = 0x0305;
/// Letter tile (`$0306`).
pub const ADDR_PPU_TILE: u16 = 0x0306;
/// Packet terminator (`$0307`: always `$FF`).
pub const ADDR_PPU_END: u16 = 0x0307;
/// Town code (`$056B`).
pub const ADDR_TOWN: u16 = 0x056B;
/// World (`$0707`).
pub const ADDR_WORLD: u16 = 0x0707;

// ---------------------------------------------------------------------------
// Control codes (Data Crystal TBL + prg3.asm LB6A2 $B6A2).
// ---------------------------------------------------------------------------

/// Next-line control (`$FD`: `bank3_End_of_Line_Routine` `$B656` with
/// delay `$0B`).
pub const CTRL_NEWLINE: u8 = 0xFD;
/// Long-delay control (`$FE`: same routine with delay `$2D`).
pub const CTRL_DELAY: u8 = 0xFE;
/// End-of-message (`$FF`: `BEQ LB672` → teardown at `$B672`).
pub const CTRL_END: u8 = 0xFF;
/// Interline/space tile (`$F4`: dialog-box interior + word space).
pub const TILE_SPACE: u8 = 0xF4;
/// Delay after `$FD` lines (`LDY #$0B`, `$B656`).
pub const DELAY_NEWLINE: u8 = 0x0B;
/// Delay after `$FE` lines (`LDY #$2D`, `$B656`).
pub const DELAY_LONG: u8 = 0x2D;
/// Lead-in delay before typing (`LDA #$2A : STA $0566`, `$B614`).
pub const DELAY_LEADIN: u8 = 0x2A;
/// Per-letter delay (`LDA #$05 : STA $0566`, `$B74D`).
pub const DELAY_LETTER: u8 = 0x05;
/// Row stride (`ADC #$40`, `$B66A`: down 2 tiles per line).
pub const ROW_STRIDE: u8 = 0x40;
/// Dialog-box border tiles (`bank3_Tables_for_dialog_box_rows…`, `$B407`:
/// `$CA` corners, `$CB` horizontal, `$CC` left edge).
pub const BOX_CORNER: u8 = 0xCA;
/// Dialog-box horizontal border.
pub const BOX_HORIZ: u8 = 0xCB;
/// Dialog-box left edge.
pub const BOX_EDGE: u8 = 0xCC;

// ---------------------------------------------------------------------------
// Byte classification + tile decode.
// ---------------------------------------------------------------------------

/// Byte class on the `LB6A2` (`$B6A2`) dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextByte {
    /// Printable letter/punctuation/space (falls to `bank3_Load_a_letter`).
    Letter(u8),
    /// `$FD` / `$FE`: line break via `bank3_End_of_Line_Routine`.
    NewLine(u8),
    /// `$FF`: end of message → `LB672` teardown.
    End,
}

/// Classify one dialog byte (`LB6A2`, `$B6D0-$B6EA`).
///
/// `CMP #$FF : BEQ end; CMP #$FD : BCC letter : JMP end-of-line`.
/// `$FE` takes the same line path with the long delay (see
/// [`end_of_line`]).
pub const fn classify_byte(b: u8) -> TextByte {
    if b == CTRL_END {
        TextByte::End
    } else if b == CTRL_NEWLINE || b == CTRL_DELAY {
        TextByte::NewLine(b)
    } else {
        TextByte::Letter(b)
    }
}

/// Decode one letter byte to `(tile, upper)` (`bank3_Load_a_letter`,
/// bank 3 `$B6DD-$B6F6`).
///
/// * bit 7 = 0 → `AND #$3F` tile, upper = `$F4` (interline).
/// * bit 7 = 1, bit 6 = 0 → accent strip: upper = `$33` (empty),
///   tile = `AND #$3F` of the ORIGINAL byte (the first `ASL` already
///   shifted; net effect `b & $3F`).
/// * bit 7 = 1, bit 6 = 1 → `ORA #$C0` tile, upper = `$F4`.
/// * bit 7 = 0 but bit 6 = 1 after the second `ASL` (i.e. original
///   `$40-$7F` range with bit 6 set... in practice the `$32` dot path
///   for the narrow-punctuation range) → upper = `$32` (dot),
///   tile = `AND #$3F`.
///
/// The hardware does this with two destructive `ASL`s; the match below
/// is the exact truth table. BUG-compatible: the upper tile for the
/// `$33`/`$32` paths replaces the interline `$F4`, it does not stack.
pub const fn decode_tile(b: u8) -> (u8, u8) {
    if b & 0x80 == 0 {
        // Bit 7 clear: second ASL tests original bit 6.
        if b & 0x40 == 0 {
            (b & 0x3F, TILE_SPACE)
        } else {
            (b & 0x3F, 0x32)
        }
    } else if b & 0x40 == 0 {
        // Accent strip (bit7=1, bit6=0): X=$33 empty tile.
        (b & 0x3F, 0x33)
    } else {
        // High range (bit7=1, bit6=1): ORA #$C0.
        (b | 0xC0, TILE_SPACE)
    }
}

/// End-of-line step (`bank3_End_of_Line_Routine`, bank 3 `$B656-$B66F`).
///
/// `LDY #$0B` (`$FD`) or `#$2D` (`$FE`) → `$0566`; `$0489 = 0`;
/// `$048A += $40`. Returns `(new_col, new_row, delay)`.
pub const fn end_of_line(ctrl: u8, row: u8) -> (u8, u8, u8) {
    let delay = if ctrl == CTRL_NEWLINE {
        DELAY_NEWLINE
    } else {
        DELAY_LONG
    };
    (0x00, row.wrapping_add(ROW_STRIDE), delay)
}

// ---------------------------------------------------------------------------
// Typewriter state machine.
// ---------------------------------------------------------------------------

/// One typewriter step result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStep {
    /// Emitted a letter: PPU packet for `(tile, upper)` + advanced col/ptr.
    Letter {
        /// Tile byte for `$0306`.
        tile: u8,
        /// Upper tile for `$0305`.
        upper: u8,
        /// New `$0489` (col + 1).
        col: u8,
        /// Per-letter delay (`$05`).
        delay: u8,
    },
    /// Line break: reset col, stride row, line delay.
    Line {
        /// Always 0 (`STA $0489`).
        col: u8,
        /// New `$048A`.
        row: u8,
        /// `$0B` (`$FD`) or `$2D` (`$FE`).
        delay: u8,
    },
    /// End of message: caller runs `LB672` teardown.
    End,
}

/// Single-byte typewriter step (`LB6A2` + `bank3_Load_a_letter`).
///
/// Pure over the byte + cursor: advances col by 1 on letters
/// (`INC $0489`, `$B74A`), strides row on `$FD`/`$FE`, ends on `$FF`.
/// Pointer advance (`INC $0569/6A`, `$B752`) and PPU address math live in
/// [`ppu_packet`]; the caller threads `ptr` itself (see [`render_tiles`]).
pub const fn text_step(byte: u8, col: u8, row: u8) -> TextStep {
    match classify_byte(byte) {
        TextByte::End => TextStep::End,
        TextByte::NewLine(c) => {
            let (nc, nr, d) = end_of_line(c, row);
            TextStep::Line {
                col: nc,
                row: nr,
                delay: d,
            }
        }
        TextByte::Letter(b) => {
            let (tile, upper) = decode_tile(b);
            TextStep::Letter {
                tile,
                upper,
                col: col.wrapping_add(1),
                delay: DELAY_LETTER,
            }
        }
    }
}

/// Render a whole dialog string to its tile stream.
///
/// Walks `bytes` until `$FF` (exclusive) or the slice end; `$FD`/`$FE`
/// contribute a line-break marker (`None` tile slot is not emitted —
/// instead the row/col evolution is applied). Returns
/// `(tiles, lines, ended)`: `tiles` are `(tile, upper)` pairs in order,
/// `lines` counts `$FD`/`$FE` breaks, `ended` is true when `$FF`
/// terminated (false = truncated slice, ROM-gated gap: full `DIALOG`
/// strings always end with `$FF` per the TBL extract).
pub fn render_tiles(bytes: &[u8]) -> (Vec<(u8, u8)>, usize, bool) {
    let mut tiles = Vec::new();
    let mut lines = 0usize;
    let mut col: u8 = 0;
    let mut row: u8 = 0;
    for &b in bytes {
        match text_step(b, col, row) {
            TextStep::End => return (tiles, lines, true),
            TextStep::Line {
                col: nc,
                row: nr,
                delay: _,
            } => {
                col = nc;
                row = nr;
                lines += 1;
            }
            TextStep::Letter {
                tile,
                upper,
                col: nc,
                delay: _,
            } => {
                tiles.push((tile, upper));
                col = nc;
            }
        }
    }
    (tiles, lines, false)
}

/// 5-byte PPU packet for one letter (`bank3_Load_a_letter`, `$B6F6-$B74E`).
///
/// `$0301 = $05`, `$0302/03` = nametable addr from scroll + col/row,
/// `$0304 = $82`, `$0305 = upper`, `$0306 = tile`, `$0307 = $FF`.
/// The nametable math (`$072C + $88`, `$072A` carry, `+$E0 + $048A`)
/// needs live scroll; this models the packet shape with the address
/// bytes passed through from the caller-computed position
/// (`addr_hi`, `addr_lo`). Total fn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpuPacket {
    /// `$0301` (always `$05`).
    pub len: u8,
    /// `$0302` (addr hi).
    pub addr_hi: u8,
    /// `$0303` (addr lo).
    pub addr_lo: u8,
    /// `$0304` (always `$82`).
    pub attr: u8,
    /// `$0305` (upper tile).
    pub upper: u8,
    /// `$0306` (letter tile).
    pub tile: u8,
    /// `$0307` (always `$FF`).
    pub end: u8,
}

/// Build the PPU packet for one rendered letter.
pub const fn ppu_packet(addr_hi: u8, addr_lo: u8, tile: u8, upper: u8) -> PpuPacket {
    PpuPacket {
        len: 0x05,
        addr_hi,
        addr_lo,
        attr: 0x82,
        upper,
        tile,
        end: 0xFF,
    }
}

// ---------------------------------------------------------------------------
// Dialog index resolution (town → string id → pointer).
// ---------------------------------------------------------------------------

/// Dialog-index inputs (`bank3_Dialog_Conditions_default`, `$B5C7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogIndex<'a> {
    /// `$056B` town code.
    pub town: u8,
    /// NPC-kind base (`$00`; `$048C` latch forces `$0F`).
    pub base: u8,
    /// `$048C` alt-text latch.
    pub alt_latch: u8,
    /// `$05` alt-text flag.
    pub alt: u8,
    /// `$0707` world (2 = east tables).
    pub world: u8,
    /// `bank3_related_to_dialog_indexes1` (`$A238`) slice.
    pub idx1: &'a [u8],
    /// `bank3_related_to_dialog_indexes2` (`$A29C`) slice.
    pub idx2: &'a [u8],
    /// `bank3_related_to_dialog_indexes3` (`$A2DC`) slice.
    pub idx3: &'a [u8],
    /// `bank3_related_to_dialog_indexes4` (`$A340`) slice.
    pub idx4: &'a [u8],
}

/// Resolve the dialog-string index (`bank3_Dialog_Conditions_default`,
/// bank 3 `$B5C7-$B5F8`).
///
/// `y = (town & 3) + (base << 2)` where `base` is the NPC-kind base
/// (`$00`, or `$0F` when `$048C != 0` alt-text latched, `$B5D9-$B5E2`);
/// `idx = idx1[y]`, or `idx2[y]` when `alt ($05) != 0` (`$B5ED-$B5F4`).
/// East towns (`world == 2`, `$B5E7-$B605`) reselect with the SAME `y`:
/// `idx3[y]` (or `idx4[y]` when alt) — parallel tables, not a chain.
/// All tables are caller slices (ROM or synthetic). Returns `None` on any
/// out-of-range (total fn).
pub fn resolve_dialog_index(input: DialogIndex<'_>) -> Option<u8> {
    let DialogIndex {
        town,
        base,
        alt_latch,
        alt,
        world,
        idx1,
        idx2,
        idx3,
        idx4,
    } = input;
    let b = if alt_latch != 0 { 0x0Fu8 } else { base };
    let y = ((town & 0x03) as usize).wrapping_add((b as usize).wrapping_mul(4));
    if world == 0x02 {
        // East path ($B5FE-$B605): same row, alternate table set.
        if alt != 0 {
            Some(*idx4.get(y)?)
        } else {
            Some(*idx3.get(y)?)
        }
    } else if alt != 0 {
        Some(*idx2.get(y)?)
    } else {
        Some(*idx1.get(y)?)
    }
}

/// Fetch the `(hi, lo)` text pointer for a string index
/// (`bank3_At_this_point_A_is_the_dialog_index`, `$B5F8-$B621`).
///
/// `ASL : TAY`; bank = west (`$AFBE`) unless `world == 2` (east,
/// `$B026`); `hi = tbl[y]`, `lo = tbl[y+1]` via `($00),y`. `ptr_tbl`
/// is the caller-selected 2-byte-per-string table slice. Returns `None`
/// on out-of-range (total fn).
pub fn dialog_pointer(idx: u8, ptr_tbl: &[u8]) -> Option<(u8, u8)> {
    let y = (idx as usize).wrapping_mul(2);
    Some((*ptr_tbl.get(y)?, *ptr_tbl.get(y + 1)?))
}

/// Dialog-box row tile selector (`bank3_Tables_for_dialog_box_rows…`,
/// bank 3 `$B407-$B422`).
///
/// Row 0 and row 9 are borders (`$CA`/`$CB` runs); rows 1-8 are interior
/// (`$CC` edge + `$F4` fill). `row` 0-9, `col` 0-13. Returns the tile
/// byte. Shape-only (no level-RAM writes — those are interp-only).
pub const fn box_row_tiles(row: usize, col: usize) -> u8 {
    if row == 0 || row == 9 {
        if col == 0 || col == 13 {
            BOX_CORNER
        } else {
            BOX_HORIZ
        }
    } else if col == 0 {
        BOX_EDGE
    } else {
        TILE_SPACE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_dispatch_matches_b6a2() {
        assert_eq!(classify_byte(0xE9), TextByte::Letter(0xE9));
        assert_eq!(classify_byte(0xF4), TextByte::Letter(0xF4));
        assert_eq!(classify_byte(0xFD), TextByte::NewLine(0xFD));
        assert_eq!(classify_byte(0xFE), TextByte::NewLine(0xFE));
        assert_eq!(classify_byte(0xFF), TextByte::End);
    }

    #[test]
    fn tile_truth_table_matches_b6dd() {
        // Bit7=0, bit6=0 → AND $3F, upper $F4.
        assert_eq!(decode_tile(0xE9 & 0x7F & 0xBF), (0x29, 0xF4));
        // Bit7=0, bit6=1 → AND $3F, upper $32 (dot).
        assert_eq!(decode_tile(0x40), (0x00, 0x32));
        // Bit7=1, bit6=0 → accent strip, upper $33.
        assert_eq!(decode_tile(0x80), (0x00, 0x33));
        // Bit7=1, bit6=1 → ORA $C0.
        assert_eq!(decode_tile(0xE9), (0xE9 | 0xC0, 0xF4));
        assert_eq!(decode_tile(0xDA), (0xDA | 0xC0, 0xF4));
    }

    #[test]
    fn line_break_timing_matches_b656() {
        assert_eq!(end_of_line(0xFD, 0x00), (0x00, 0x40, 0x0B));
        assert_eq!(end_of_line(0xFE, 0x40), (0x00, 0x80, 0x2D));
    }

    #[test]
    fn typewriter_steps_match_lb6a2() {
        assert!(matches!(text_step(0xFF, 3, 0x40), TextStep::End));
        match text_step(0xFD, 5, 0x00) {
            TextStep::Line { col, row, delay } => assert_eq!((col, row, delay), (0, 0x40, 0x0B)),
            _ => panic!("FD must break"),
        }
        match text_step(0xDA, 2, 0x00) {
            TextStep::Letter {
                tile, col, delay, ..
            } => {
                assert_eq!((tile, col, delay), (0xDA | 0xC0, 3, 0x05))
            }
            _ => panic!("letter must emit"),
        }
    }

    #[test]
    fn render_stops_at_ff_and_counts_lines() {
        let (tiles, lines, ended) = render_tiles(&[0xDA, 0xFD, 0xDB, 0xFF, 0xDC]);
        assert_eq!(tiles.len(), 2);
        assert_eq!(lines, 1);
        assert!(ended);
        let (_, _, ended2) = render_tiles(&[0xDA, 0xDB]);
        assert!(!ended2, "truncated slice reports truncation");
    }

    #[test]
    fn index_resolution_west_path() {
        let idx1 = [7u8; 32];
        let idx2 = [9u8; 32];
        let idx3 = [11u8; 32];
        let idx4 = [13u8; 32];
        let base = |town, alt_latch, alt, world| DialogIndex {
            town,
            base: 0,
            alt_latch,
            alt,
            world,
            idx1: &idx1,
            idx2: &idx2,
            idx3: &idx3,
            idx4: &idx4,
        };
        // town 0 base 0, no alt, west → idx1[0].
        assert_eq!(resolve_dialog_index(base(0, 0, 0, 1)), Some(7));
        // alt → idx2[0].
        assert_eq!(resolve_dialog_index(base(0, 0, 1, 1)), Some(9));
        // east reselects the same row from the east set ($B5FE).
        assert_eq!(resolve_dialog_index(base(0, 0, 0, 2)), Some(11));
        assert_eq!(resolve_dialog_index(base(0, 0, 1, 2)), Some(13));
        // alt latch $048C → base $0F → y = town&3 + 60 (oob → None, total).
        assert_eq!(resolve_dialog_index(base(0, 1, 0, 1)), None);
    }

    #[test]
    fn pointer_fetch_pairs_bytes() {
        assert_eq!(
            dialog_pointer(0, &[0x80, 0xA3, 0x90, 0xA3]),
            Some((0x80, 0xA3))
        );
        assert_eq!(
            dialog_pointer(1, &[0x80, 0xA3, 0x90, 0xA3]),
            Some((0x90, 0xA3))
        );
        assert_eq!(dialog_pointer(5, &[0x80]), None);
    }

    #[test]
    fn box_rows_are_bordered() {
        assert_eq!(box_row_tiles(0, 0), 0xCA);
        assert_eq!(box_row_tiles(0, 5), 0xCB);
        assert_eq!(box_row_tiles(3, 0), 0xCC);
        assert_eq!(box_row_tiles(3, 5), 0xF4);
        assert_eq!(box_row_tiles(9, 13), 0xCA);
    }
}
