//! ROM → raw-section extraction core.
//!
//! [`extract`] takes the **header-stripped body** returned by
//! [`crate::rom::open`] (or [`crate::rom::open_at`]), verifies it with the
//! ROM hash gate, slices every [`SECTION_TABLE`](crate::extract_tables)
//! record, and runs structural validation (pointer ranges, palette ranges,
//! dialog alphabet, enemy-block coverage).
//!
//! Offsets in the section table are headered-file offsets (Data
//! Crystal / disassembly convention); they are converted to body indexes by
//! subtracting [`crate::rom::INES_HEADER_LEN`].
//!
//! Core-only (`core` + `alloc`): no `std` paths in this file so the extractor
//! core stays `no_std`-friendly and `wasm32`-compatible. Only pure,
//! dependency-free helpers from [`crate::rom`] are used
//! (`verify_body`, `crc32_ieee`, `sha1_digest`); ROM *file I/O* lives in
//! [`crate::assets_bin_io`].

extern crate alloc;

use alloc::vec::Vec;

use crate::extract_tables::{
    find_by_id, section_id, SectionDef, DIALOG_ALPHABET, ENEMY_PTR_QUIRKS, SECTION_TABLE,
};
use crate::rom::{self, RomError};

/// A sliced record borrowing the verified ROM body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSection<'a> {
    /// Table entry describing this record.
    pub def: &'a SectionDef,
    /// Raw bytes (`def.len` long) borrowed from the ROM body.
    pub bytes: &'a [u8],
}

/// Successful extraction: provenance hashes plus every section in table order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted<'a> {
    /// CRC32 (IEEE) of the verified ROM body.
    pub body_crc32: u32,
    /// SHA1 of the verified ROM body.
    pub body_sha1: [u8; 20],
    /// One entry per [`SECTION_TABLE`](crate::extract_tables) row, in order.
    pub sections: Vec<RawSection<'a>>,
}

impl<'a> Extracted<'a> {
    /// Fetch a section by id.
    #[must_use]
    pub fn get(&self, id: u16) -> Option<&'a [u8]> {
        self.sections
            .iter()
            .find(|s| s.def.id == id)
            .map(|s| s.bytes)
    }
}

/// Extraction / validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// The ROM hash gate rejected the input.
    Rom(RomError),
    /// A table record does not fit the supplied image.
    BadSection {
        name: &'static str,
        file_off: u32,
        len: u32,
    },
    /// A structural check failed (wrong ROM or table drift).
    Validation {
        section: &'static str,
        msg: &'static str,
    },
}

impl From<RomError> for ExtractError {
    fn from(e: RomError) -> Self {
        ExtractError::Rom(e)
    }
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExtractError::Rom(e) => write!(f, "ROM gate: {e}"),
            ExtractError::BadSection {
                name,
                file_off,
                len,
            } => write!(
                f,
                "section {name} (file offset ${file_off:05X}, len {len}) \
                 does not fit the ROM image"
            ),
            ExtractError::Validation { section, msg } => {
                write!(f, "validation failed for {section}: {msg}")
            }
        }
    }
}

/// Extract every section from a header-stripped ROM body.
///
/// The body is verified with [`crate::rom::verify_body`] first, so any error
/// after that point means the section table drifted from the ROM layout.
pub fn extract(body: &[u8]) -> Result<Extracted<'_>, ExtractError> {
    rom::verify_body(body)?;
    let body_crc32 = rom::crc32_ieee(body);
    let body_sha1 = rom::sha1_digest(body);

    let mut sections = Vec::with_capacity(SECTION_TABLE.len());
    for def in SECTION_TABLE {
        let start = def
            .file_off
            .checked_sub(rom::INES_HEADER_LEN as u32)
            .and_then(|o| usize::try_from(o).ok())
            .ok_or(ExtractError::BadSection {
                name: def.name,
                file_off: def.file_off,
                len: def.len,
            })?;
        let len = usize::try_from(def.len).map_err(|_| ExtractError::BadSection {
            name: def.name,
            file_off: def.file_off,
            len: def.len,
        })?;
        let bytes = body
            .get(start..start.saturating_add(len))
            .ok_or(ExtractError::BadSection {
                name: def.name,
                file_off: def.file_off,
                len: def.len,
            })?;
        if bytes.len() != len {
            return Err(ExtractError::BadSection {
                name: def.name,
                file_off: def.file_off,
                len: def.len,
            });
        }
        sections.push(RawSection { def, bytes });
    }

    let extracted = Extracted {
        body_crc32,
        body_sha1,
        sections,
    };
    validate(&extracted)?;
    Ok(extracted)
}

/// Read one little-endian word from a 126-byte (63-word) pointer table.
fn table_word(bytes: &[u8], index: usize) -> u16 {
    let o = index * 2;
    u16::from_le_bytes([bytes[o], bytes[o + 1]])
}

/// NES CPU address → headered-file offset for a bank mapped at `$8000`.
fn cpu_to_file(cpu: u16, bank_file_base: u32) -> Option<u32> {
    if !(0x8000..0xC000).contains(&cpu) {
        return None;
    }
    Some(bank_file_base + u32::from(cpu - 0x8000))
}

/// Structural validation over the sliced sections.
///
/// These checks use only ROM-structure facts (address ranges, alphabets,
/// cross-table consistency) — no ROM content is asserted beyond what the
/// hash gate already pins.
fn validate(ex: &Extracted<'_>) -> Result<(), ExtractError> {
    let get = |id: u16| -> Result<&[u8], ExtractError> {
        find_by_id(id)
            .and_then(|d| ex.sections.iter().find(|s| s.def.id == d.id))
            .map(|s| s.bytes)
            .ok_or(ExtractError::Validation {
                section: "table",
                msg: "section missing after slice",
            })
    };

    // Map-pointer tables: 63 words each in CPU $8000-$BFFF (bank-mapped ROM).
    for id in [
        section_id::SB1_MAPPTR_A,
        section_id::SB1_MAPPTR_B,
        section_id::SB2_MAPPTR_A,
        section_id::SB2_MAPPTR_B,
        section_id::SB3_MAPPTR_A,
        section_id::SB4_MAPPTR_A,
        section_id::SB4_MAPPTR_B,
        section_id::SB5_MAPPTR_A,
    ] {
        let bytes = get(id)?;
        if bytes.len() != 126 {
            return Err(ExtractError::Validation {
                section: "mapptr",
                msg: "map-pointer table is not 63 words",
            });
        }
        for i in 0..63 {
            let p = table_word(bytes, i);
            if !(0x8000..0xC000).contains(&p) {
                return Err(ExtractError::Validation {
                    section: "mapptr",
                    msg: "map pointer outside bank-mapped ROM ($8000-$BFFF)",
                });
            }
        }
    }

    // Enemy-pointer tables: 63 words each in SRAM $7000-$73FF (the copied
    // enemy block), except documented quirks (e.g. bank2/setA/#21 = $8CF4).
    for id in [
        section_id::SB1_ENEMYPTR_A,
        section_id::SB1_ENEMYPTR_B,
        section_id::SB2_ENEMYPTR_A,
        section_id::SB2_ENEMYPTR_B,
        section_id::SB3_ENEMYPTR_A,
        section_id::SB4_ENEMYPTR_A,
        section_id::SB4_ENEMYPTR_B,
        section_id::SB5_ENEMYPTR_A,
    ] {
        let bytes = get(id)?;
        if bytes.len() != 126 {
            return Err(ExtractError::Validation {
                section: "enemyptr",
                msg: "enemy-pointer table is not 63 words",
            });
        }
        for i in 0..63 {
            let p = table_word(bytes, i);
            let quirk = ENEMY_PTR_QUIRKS
                .iter()
                .any(|&(qid, qidx, qval)| qid == id && qidx == i && qval == p);
            if !(0x7000..0x7400).contains(&p) && !quirk {
                return Err(ExtractError::Validation {
                    section: "enemyptr",
                    msg: "enemy pointer outside SRAM $7000-$73FF (and not a quirk)",
                });
            }
        }
    }

    // Enemy blocks: every SRAM-referenced list's header-declared bytes must
    // fit the 1024-byte block the game copies ($88A0 -> $7000, 4x256B loops).
    let enemy_sets: &[(u16, u16, u16)] = &[
        (
            section_id::SB1_ENEMIES,
            section_id::SB1_ENEMYPTR_A,
            section_id::SB1_ENEMYPTR_B,
        ),
        (
            section_id::SB2_ENEMIES,
            section_id::SB2_ENEMYPTR_A,
            section_id::SB2_ENEMYPTR_B,
        ),
        (
            section_id::SB3_ENEMIES,
            section_id::SB3_ENEMYPTR_A,
            section_id::SB3_ENEMYPTR_A,
        ),
        (
            section_id::SB4_ENEMIES,
            section_id::SB4_ENEMYPTR_A,
            section_id::SB4_ENEMYPTR_B,
        ),
        (
            section_id::SB5_ENEMIES,
            section_id::SB5_ENEMYPTR_A,
            section_id::SB5_ENEMYPTR_A,
        ),
    ];
    for &(block_id, ptr_a, ptr_b) in enemy_sets {
        let block = get(block_id)?;
        if block.len() != 1024 {
            return Err(ExtractError::Validation {
                section: "enemies",
                msg: "enemy block is not the 1024 bytes the game copies",
            });
        }
        for ptr_id in [ptr_a, ptr_b] {
            let table = get(ptr_id)?;
            for i in 0..63 {
                let p = table_word(table, i);
                if !(0x7000..0x7400).contains(&p) {
                    continue; // documented quirk; covered by the range check
                }
                let span = usize::from(p - 0x7000);
                let Some(&hdr) = block.get(span) else {
                    return Err(ExtractError::Validation {
                        section: "enemies",
                        msg: "enemy pointer past end of enemy block",
                    });
                };
                if span + 1 + usize::from(hdr) > block.len() {
                    return Err(ExtractError::Validation {
                        section: "enemies",
                        msg: "enemy list overruns the copied enemy block",
                    });
                }
            }
        }
    }

    // Palettes: every entry is a PPU palette index ($00-$3F).
    for id in [
        section_id::PAL_EXTERIOR,
        section_id::PAL_INTERIOR,
        section_id::PAL_OVERWORLD,
        section_id::PAL_OVERWORLD_SPR,
    ] {
        let pal = get(id)?;
        if pal.iter().any(|&b| b > 0x3F) {
            return Err(ExtractError::Validation {
                section: "palette",
                msg: "palette entry outside PPU range $00-$3F",
            });
        }
    }

    // Dialog: every byte is in the Data Crystal text-table alphabet.
    {
        let dlg = get(section_id::DIALOG)?;
        let mut ok = [false; 256];
        for &b in DIALOG_ALPHABET {
            ok[usize::from(b)] = true;
        }
        if dlg.iter().any(|&b| !ok[usize::from(b)]) {
            return Err(ExtractError::Validation {
                section: "dialog",
                msg: "dialog byte outside the text-table alphabet",
            });
        }
    }

    // Overworld map pointer tables resolve to the extracted map records.
    let ow_maps: &[(u16, u16, u32)] = &[
        (section_id::OWPTR_B1, section_id::MAP_WEST, 0x004010),
        (section_id::OWPTR_B1, section_id::MAP_DM, 0x004010),
        (section_id::OWPTR_B2, section_id::MAP_EAST, 0x008010),
        (section_id::OWPTR_B2, section_id::MAP_MAZE, 0x008010),
    ];
    for &(tab_id, map_id, bank_base) in ow_maps {
        let tab = get(tab_id)?;
        let map_def = find_by_id(map_id).ok_or(ExtractError::Validation {
            section: "owptr",
            msg: "map section missing from table",
        })?;
        let mut hit = false;
        for i in 0..(tab.len() / 2) {
            let p = u16::from_le_bytes([tab[2 * i], tab[2 * i + 1]]);
            if cpu_to_file(p, bank_base) == Some(map_def.file_off) {
                hit = true;
                break;
            }
        }
        if !hit {
            return Err(ExtractError::Validation {
                section: "owptr",
                msg: "map pointer table does not reference its map record",
            });
        }
    }

    // Death Mountain and Maze Island ship identical RLE blobs in the ROM
    // (verified: same bytes, same 743-byte length). This guards asymmetric
    // slicing mistakes.
    if get(section_id::MAP_DM)? != get(section_id::MAP_MAZE)? {
        return Err(ExtractError::Validation {
            section: "map_dm/map_maze",
            msg: "Death Mountain and Maze Island blobs diverged",
        });
    }

    Ok(())
}
