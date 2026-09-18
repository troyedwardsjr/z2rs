//! Seen-tile recorder: which `(page, tile, palette)` combinations a session
//! drew, and palette-specific template packs for exactly those.
//!
//! Frame ingestion (`observe(&FrameRecord)`) arrives with the PPU render
//! record; until then callers feed keys with [`Recorder::insert`] and mark
//! frame boundaries with [`Recorder::end_frame`]. Storage is a `BTreeMap`, so
//! iteration and every written file are deterministic regardless of the
//! order keys were inserted or merged.

use std::collections::BTreeMap;

use serde::Serialize;

use z2_ppu::record::{FrameRecord, NO_PAGE, RECORD_TILES_PER_LINE};
use z2_ppu::{HEIGHT, PPUMASK_SHOW_BG, PPUMASK_SHOW_SPRITES};

use crate::chr::{chr_page, paint_page_sheet, CHR_PAGES, TILES_PER_PAGE};
use crate::compositor::{bg_colors, sprite_colors};
use crate::pack::PACK_MANIFEST;
use crate::template::{
    check_scale, empty_manifest, encode_template_sheet, manifest_json, nes_rgb3, page_sheet_entry,
    sheet_file_name, TemplateError, MARKER_FILE, MARKER_TEXT,
};

/// File listing every recorded key (written beside the template pack).
pub const SEEN_FILE: &str = "hdpack-seen.json";

/// One template cell: tile identity plus the three opaque palette indices
/// (sub-palette entries 1..=3) it was drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeenKey {
    pub page: u8,
    pub tile: u8,
    pub colors: [u8; 3],
}

/// Which layer a key was drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SeenAs {
    Background,
    Sprite,
    Both,
}

impl SeenAs {
    /// Combine two observations.
    #[must_use]
    pub fn union(self, other: SeenAs) -> SeenAs {
        if self == other {
            self
        } else {
            SeenAs::Both
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SeenAs::Background => "background",
            SeenAs::Sprite => "sprite",
            SeenAs::Both => "both",
        }
    }
}

/// Aggregate for one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeenStats {
    /// Times observed (tile cells / sprite draws).
    pub count: u64,
    pub seen_as: SeenAs,
}

/// Accumulates seen keys across frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recorder {
    keys: BTreeMap<SeenKey, SeenStats>,
    frames: u64,
}

#[derive(Serialize)]
struct SeenRow {
    page: u8,
    tile: u8,
    colors: [u8; 3],
    count: u64,
    #[serde(rename = "as")]
    seen_as: &'static str,
}

impl Recorder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observation (colours are masked to 6 bits).
    pub fn insert(&mut self, key: SeenKey, seen_as: SeenAs) {
        self.insert_count(key, seen_as, 1);
    }

    /// Record `count` observations at once.
    pub fn insert_count(&mut self, key: SeenKey, seen_as: SeenAs, count: u64) {
        let key = SeenKey {
            colors: key.colors.map(|c| c & 0x3F),
            ..key
        };
        self.keys
            .entry(key)
            .and_modify(|s| {
                s.count = s.count.saturating_add(count);
                s.seen_as = s.seen_as.union(seen_as);
            })
            .or_insert(SeenStats { count, seen_as });
    }

    /// Fold one finished frame in: every background tile the lines fetched
    /// and every sprite they drew, keyed by the palette colours in force on
    /// that line. Counts one frame.
    ///
    /// Skips unrecorded lines, background tiles on lines with the background
    /// disabled, unfetched tiles and tiles with no CHR page. Fetch slot 32 is
    /// only visible when `fine_x > 0`, and slot 33 never is, so they are
    /// filtered the same way the renderer would.
    pub fn observe(&mut self, record: &FrameRecord) {
        for y in 0..HEIGHT {
            let rec = record.line(y);
            if !rec.valid {
                continue;
            }
            if rec.mask & PPUMASK_SHOW_BG != 0 {
                // Slots 0..32 always show; slot 32 only with a fine-X shift.
                let visible = if rec.fine_x & 7 != 0 { 33 } else { 32 };
                for id in rec.tiles[..visible.min(RECORD_TILES_PER_LINE)].iter() {
                    if !id.fetched || id.page == NO_PAGE {
                        continue;
                    }
                    self.insert(
                        SeenKey {
                            page: id.page,
                            tile: id.tile,
                            colors: bg_colors(rec, id.pal),
                        },
                        SeenAs::Background,
                    );
                }
            }
            if rec.mask & PPUMASK_SHOW_SPRITES != 0 {
                for s in record.sprites_on(y) {
                    if s.page == NO_PAGE {
                        continue;
                    }
                    self.insert(
                        SeenKey {
                            page: s.page,
                            tile: s.tile,
                            colors: sprite_colors(rec, s.pal),
                        },
                        SeenAs::Sprite,
                    );
                }
            }
        }
        self.end_frame();
    }

    /// Mark the end of one observed frame.
    pub fn end_frame(&mut self) {
        self.frames += 1;
    }

    /// Fold another recorder in (counts add, layers union, frames add).
    pub fn merge(&mut self, other: &Recorder) {
        for (k, s) in &other.keys {
            self.insert_count(*k, s.seen_as, s.count);
        }
        self.frames += other.frames;
    }

    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Distinct `(page, tile, colors)` keys.
    #[must_use]
    pub fn distinct_keys(&self) -> usize {
        self.keys.len()
    }

    /// Distinct `(page, tile)` pairs.
    #[must_use]
    pub fn distinct_tiles(&self) -> usize {
        let mut last = None;
        self.keys
            .keys()
            .filter(|k| {
                let id = Some((k.page, k.tile));
                let new = id != last;
                last = id;
                new
            })
            .count()
    }

    /// All keys in ascending `(page, tile, colors)` order.
    pub fn iter(&self) -> impl Iterator<Item = (&SeenKey, &SeenStats)> {
        self.keys.iter()
    }

    /// The most frequent triple for a tile (ties: lowest triple).
    #[must_use]
    pub fn default_colors(&self, page: u8, tile: u8) -> Option<[u8; 3]> {
        let lo = SeenKey {
            page,
            tile,
            colors: [0; 3],
        };
        let hi = SeenKey {
            page,
            tile,
            colors: [0xFF; 3],
        };
        let mut best: Option<([u8; 3], u64)> = None;
        for (k, s) in self.keys.range(lo..=hi) {
            if best.is_none_or(|(_, c)| s.count > c) {
                best = Some((k.colors, s.count));
            }
        }
        best.map(|(c, _)| c)
    }

    /// `hdpack-seen.json` contents: every key, sorted.
    #[must_use]
    pub fn seen_json(&self) -> String {
        let rows: Vec<SeenRow> = self
            .keys
            .iter()
            .map(|(k, s)| SeenRow {
                page: k.page,
                tile: k.tile,
                colors: k.colors,
                count: s.count,
                seen_as: s.seen_as.as_str(),
            })
            .collect();
        serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".to_string()) + "\n"
    }

    /// Render a template pack for the recorded keys.
    ///
    /// Per CHR page seen: `sheets/pageNN.png` paints each seen tile with its
    /// most frequent triple and registers as the palette-independent default;
    /// every other triple seen on that page gets `sheets/pageNN.cAA-BB-CC.png`
    /// holding just the tiles drawn with it, registered as a palette variant.
    /// Unseen cells stay transparent. Also writes `pack.json`,
    /// [`SEEN_FILE`] and the ROM-derived marker file.
    ///
    /// # Errors
    /// [`TemplateError`] for a bad scale, nothing recorded, a CHR image too
    /// short for a recorded page, or an encoder failure.
    pub fn write_pack(
        &self,
        chr_rom: &[u8],
        scale: u32,
        name: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, TemplateError> {
        check_scale(scale)?;
        if self.keys.is_empty() {
            return Err(TemplateError::NoPages);
        }
        let mut manifest = empty_manifest(name, scale);
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();

        let mut page_keys: BTreeMap<u8, Vec<SeenKey>> = BTreeMap::new();
        for k in self.keys.keys() {
            page_keys.entry(k.page).or_default().push(*k);
        }
        for (&page, keys) in &page_keys {
            if usize::from(page) >= CHR_PAGES {
                return Err(TemplateError::Page(page));
            }
            let bytes = chr_page(chr_rom, page).ok_or(TemplateError::ChrTooShort {
                page,
                len: chr_rom.len(),
            })?;
            let mut defaults: [Option<[u8; 3]>; TILES_PER_PAGE] = [None; TILES_PER_PAGE];
            for k in keys {
                if defaults[usize::from(k.tile)].is_none() {
                    defaults[usize::from(k.tile)] = self.default_colors(page, k.tile);
                }
            }
            let img = paint_page_sheet(bytes, scale, |t| defaults[usize::from(t)].map(nes_rgb3));
            files.push((sheet_file_name(page, None), encode_template_sheet(&img)?));
            manifest.sheets.push(page_sheet_entry(page, None));

            let mut variants: BTreeMap<[u8; 3], [bool; TILES_PER_PAGE]> = BTreeMap::new();
            for k in keys {
                if defaults[usize::from(k.tile)] != Some(k.colors) {
                    variants.entry(k.colors).or_insert([false; TILES_PER_PAGE])
                        [usize::from(k.tile)] = true;
                }
            }
            for (colors, mask) in &variants {
                let rgb = nes_rgb3(*colors);
                let img = paint_page_sheet(bytes, scale, |t| mask[usize::from(t)].then_some(rgb));
                files.push((
                    sheet_file_name(page, Some(*colors)),
                    encode_template_sheet(&img)?,
                ));
                manifest.sheets.push(page_sheet_entry(page, Some(*colors)));
            }
        }

        let mut out = vec![(
            PACK_MANIFEST.to_string(),
            manifest_json(&manifest)?.into_bytes(),
        )];
        out.extend(files);
        out.push((SEEN_FILE.to_string(), self.seen_json().into_bytes()));
        out.push((MARKER_FILE.to_string(), MARKER_TEXT.as_bytes().to_vec()));
        Ok(out)
    }
}
