//! `cosmetic` module: looks and sound. Never changes the generated world,
//! the seed or the hash code.
//!
//! Options: [`crate::flags::CosmeticFlags`] (`ctx.flags.cosmetic`) and the
//! player's own sprite patch [`crate::Extras::sprite_ips`]. Randomness comes
//! only from `ctx.rng` (this module's stream, derived from the seed), with a
//! separate sub-stream per option, so a "Random" colour is the same for
//! everyone with the same seed and does not move when another cosmetic
//! option changes.
//!
//! Order: the sprite patch first, then colours (so an explicit colour wins
//! over the patch and `Default` keeps whatever the patch set), then the beam
//! graphic, sprite palettes and music.
//!
//! * **Sprite patch** ([`apply_sprite_ips`]): an IPS file made against the
//!   vanilla iNES image. Only CHR bytes and Link's palette bytes are taken;
//!   every other byte (code, maps, text, the header) is dropped and counted
//!   in the log and the spoiler. Item graphics are protected unless
//!   `change_item_sprites` is on. z2rs ships no sprite art.
//! * **Colours**: Link's outline, skin and tunic at every palette the game
//!   loads for him (sideview sets, overworld, title, game over), the tunic
//!   restore after Shield, and the Shield tunic. `Random` draws a colour
//!   that differs from the other three and avoids `$0D` and the blacks.
//! * **Beam**: the sword beam is drawn with the Fire spell's tile (`$84`)
//!   instead of its own (`$32`, which is also part of Link's sword), and an
//!   8x16 graphic from the player's ROM is copied over `$84` in every sprite
//!   page that has the fireball (so the Fire spell shows it too); the beam's
//!   palette cycling and flip are set per choice (bank 0 `$98EA-$98F7`).
//! * **Shuffle sprite palettes**: each sideview palette set's two enemy
//!   palettes get a random hue turn; brightness and the greys are kept, so
//!   nothing becomes invisible. Link's palette and the shared item/beam
//!   palette stay.
//! * **Disable music**: the music engine keeps running (song timing, and the
//!   game code that waits on `$07FB`, is unchanged) but plays at volume 0:
//!   its pulse volume table, triangle linear counter and drum volume are
//!   zeroed. Sound effects use their own values and still play.
//! * **Custom music** (`randomize_music` and its sub-options): not
//!   implemented; there is no music engine for imported tracks yet. The
//!   options are accepted and logged.

use crate::flags::{BeamSprite, CosmeticFlags, FlagEnum, NesColor};
use crate::rng::Rng;
use crate::{Ctx, RandoError};

/// Where Link's sprite palette (outline, skin, tunic) is stored: the CPU
/// address of the outline byte, the next two bytes being skin and tunic.
/// Found by scanning the vanilla PRG for his palette; checked by a
/// ROM-gated test.
pub const LINK_PALETTE_SITES: &[(u8, u16)] = &[
    // Game-over / continue screen palette.
    (0, 0xA84A),
    // Sideview palette sets (the sprite half of each set).
    (1, 0x809F),
    (1, 0x80AF),
    (1, 0x80BF),
    (1, 0x80CF),
    (2, 0x809F),
    (2, 0x80AF),
    (2, 0x80BF),
    (2, 0x80CF),
    (3, 0x809F),
    (3, 0x80AF),
    (3, 0x80BF),
    (3, 0x80CF),
    (3, 0x80DF),
    (4, 0x809F),
    (4, 0x80AF),
    (4, 0x80BF),
    (4, 0x80CF),
    (5, 0x809F),
    (5, 0x80AF),
    (5, 0x80BF),
    (5, 0x80CF),
    // Title / story screen.
    (5, 0xBC09),
    // Fixed bank: overworld sprite palette and the lives screen.
    (7, 0xC454),
    (7, 0xC46C),
];

/// `LDA #tunic` that restores the tunic when Shield ends (operand).
const TUNIC_RESTORE: (u8, u16) = (0, 0x90DA);
/// `LDA #$16` the Shield spell writes as tunic colour (operand).
const SHIELD_TUNIC: (u8, u16) = (0, 0x8E8E);

/// CHR offset of the fireball tile pair (`$84`) inside a sprite page.
const FIREBALL_TILE_OFF: usize = 0x84 * 16;
/// CHR pages (8 KiB banks) whose sprite half is used in gameplay.
const SPRITE_PAGES: std::ops::Range<usize> = 0..13;
/// Bank 0: the sword-beam branch of the projectile drawer.
const BEAM_CODE: u16 = 0x98EA;
/// `LDA #$32 : STA $0201,Y : LDA $12 : AND #$03 : ORA $0202,Y : AND #$7F`.
const BEAM_CODE_VANILLA: [u8; 14] = [
    0xA9, 0x32, 0x99, 0x01, 0x02, 0xA5, 0x12, 0x29, 0x03, 0x19, 0x02, 0x02, 0x29, 0x7F,
];

/// Sprite tiles (8x16 pairs, even index) that show items; protected from a
/// sprite patch unless `change_item_sprites` is on: key, heart and magic
/// containers, P-bag, magic jar, candle, glove, raft, boots, flute, cross,
/// hammer, magic key and the large container icon.
pub const ITEM_TILES: &[u8] = &[
    0x66, 0x6E, 0x70, 0x72, 0x74, 0x8A, 0x8C, 0x8E, 0x90, 0x92, 0x94, 0x96, 0x98, 0x9A, 0xAC,
];

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let c = ctx.flags.cosmetic.clone();
    if let Some(ips) = ctx.extras.sprite_ips.clone() {
        let report = apply_sprite_ips(ctx, &ips, c.change_item_sprites)?;
        let line = report.summary();
        ctx.log(format!("cosmetic: sprite patch: {line}"));
        ctx.spoiler
            .line("Cosmetics", format!("Sprite patch: {line}"));
    }
    apply_colors(ctx, &c)?;
    if c.beam_sprite != BeamSprite::Default {
        apply_beam(ctx, c.beam_sprite)?;
    }
    if c.shuffle_sprite_palettes {
        shuffle_sprite_palettes(ctx)?;
    }
    if c.disable_music {
        disable_music(ctx)?;
    } else if c.randomize_music {
        ctx.log("cosmetic: custom music is not implemented yet; original music kept");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Colours.
// ---------------------------------------------------------------------------

/// Colours `Random` may pick: every NES colour except `$0D` (the
/// "blacker than black" entry some displays mishandle) and the blacks
/// (`$0E`, `$0F`, `$1D`-`$1F`, `$2E`, `$2F`, `$3E`, `$3F`), which would hide
/// Link against dark rooms.
#[must_use]
pub fn random_color_pool() -> Vec<u8> {
    (0..0x40u8)
        .filter(|c| !matches!(c, 0x0D..=0x0F | 0x1D..=0x1F | 0x2E | 0x2F | 0x3E | 0x3F))
        .collect()
}

fn site_byte(ctx: &Ctx, (bank, addr): (u8, u16)) -> Result<u8, RandoError> {
    ctx.rom.read_cpu(bank, addr)
}

/// Resolve the four colour options against the colours the image has now.
/// Returns `[outline, skin, tunic, shield]`, `None` meaning "leave as is".
fn resolve_colors(ctx: &Ctx, c: &CosmeticFlags) -> Result<[Option<u8>; 4], RandoError> {
    let first = LINK_PALETTE_SITES[1];
    let current = [
        site_byte(ctx, first)?,
        site_byte(ctx, (first.0, first.1 + 1))?,
        site_byte(ctx, (first.0, first.1 + 2))?,
        site_byte(ctx, SHIELD_TUNIC)?,
    ];
    let opts = [c.tunic_outline, c.skin_tone, c.tunic, c.shield_tunic];
    let names = ["outline", "skin", "tunic", "shield"];
    let mut out: [Option<u8>; 4] = [None; 4];
    let mut taken: [u8; 4] = current;
    for (i, o) in opts.iter().enumerate() {
        if let NesColor::Color(v) = o {
            out[i] = Some(v & 0x3F);
            taken[i] = v & 0x3F;
        }
    }
    let pool = random_color_pool();
    for (i, o) in opts.iter().enumerate() {
        if *o == NesColor::Random {
            let mut rng: Rng = ctx.rng.derive(names[i]);
            let others: Vec<u8> = (0..4).filter(|&j| j != i).map(|j| taken[j]).collect();
            let choices: Vec<u8> = pool
                .iter()
                .copied()
                .filter(|v| !others.contains(v))
                .collect();
            let v = *rng.pick(&choices).unwrap_or(&taken[i]);
            out[i] = Some(v);
            taken[i] = v;
        }
    }
    Ok(out)
}

fn apply_colors(ctx: &mut Ctx, c: &CosmeticFlags) -> Result<(), RandoError> {
    let [outline, skin, tunic, shield] = resolve_colors(ctx, c)?;
    for &(bank, addr) in LINK_PALETTE_SITES {
        for (k, v) in [outline, skin, tunic].into_iter().enumerate() {
            if let Some(v) = v {
                ctx.rom.write_cpu(bank, addr + k as u16, &[v])?;
            }
        }
    }
    if let Some(v) = tunic {
        ctx.rom.write_cpu(TUNIC_RESTORE.0, TUNIC_RESTORE.1, &[v])?;
    }
    if let Some(v) = shield {
        ctx.rom.write_cpu(SHIELD_TUNIC.0, SHIELD_TUNIC.1, &[v])?;
    }
    for (name, v) in [
        ("Outline", outline),
        ("Skin", skin),
        ("Tunic", tunic),
        ("Shield tunic", shield),
    ] {
        if let Some(v) = v {
            ctx.spoiler
                .line("Cosmetics", format!("{name} colour: ${v:02X}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Beam.
// ---------------------------------------------------------------------------

/// Where a beam graphic comes from in the player's CHR, and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeamSource {
    /// `(CHR page, even 8x16 tile index)` copied over the fireball tile
    /// `$84`, or `None` to show the fireball itself.
    pub tile: Option<(u8, u8)>,
    /// Cycle the four sprite palettes like the vanilla beam (`true`), or a
    /// steady palette 1 (red, orange, white).
    pub flashing: bool,
    /// Keep the vertical flip every 4 frames (a tumbling look).
    pub spin: bool,
}

/// The graphic each beam choice uses (picked by eye in the vanilla CHR;
/// copied from the player's ROM at run time).
#[must_use]
pub fn beam_source(b: BeamSprite) -> Option<BeamSource> {
    let s = |tile, flashing, spin| {
        Some(BeamSource {
            tile,
            flashing,
            spin,
        })
    };
    match b {
        BeamSprite::Default | BeamSprite::Random => None,
        BeamSprite::Fire => s(None, false, false),
        BeamSprite::SpicyChicken => s(None, true, true),
        BeamSprite::Bubble => s(Some((2, 0xAA)), true, false),
        BeamSprite::Rock => s(Some((2, 0xAE)), false, true),
        BeamSprite::EnergyBall => s(Some((2, 0xCE)), true, false),
        BeamSprite::WizardBeam => s(Some((2, 0x86)), true, true),
        BeamSprite::Axe => s(Some((1, 0xFA)), false, true),
        BeamSprite::Hammer => s(Some((5, 0xEE)), false, true),
        BeamSprite::GeruMace => s(Some((9, 0xEE)), false, true),
        BeamSprite::GumaMace => s(Some((4, 0xEC)), false, true),
        BeamSprite::Boomerang => s(Some((1, 0xF6)), false, true),
    }
}

/// The projectile drawer (bank 0 `$98C0`) draws the Fire spell with tile
/// `$84` and the sword beam (`$05CA,X` set) with tile `$32`, a palette
/// cycling with the frame counter and no vertical flip. `$32` is also part
/// of Link's own sprite, so the beam choices draw the beam with the
/// fireball tile `$84` instead (and copy the chosen graphic there, which
/// the Fire spell then shares).
fn apply_beam(ctx: &mut Ctx, choice: BeamSprite) -> Result<(), RandoError> {
    let choice = if choice == BeamSprite::Random {
        let pool: Vec<BeamSprite> = BeamSprite::all()
            .iter()
            .copied()
            .filter(|b| beam_source(*b).is_some())
            .collect();
        let mut rng = ctx.rng.derive("beam");
        *rng.pick(&pool).unwrap_or(&BeamSprite::Default)
    } else {
        choice
    };
    let Some(src) = beam_source(choice) else {
        return Ok(());
    };
    let off = ctx.rom.cpu_offset(0, BEAM_CODE)?;
    if ctx.rom.read_slice(off, BEAM_CODE_VANILLA.len())? != BEAM_CODE_VANILLA {
        ctx.log("cosmetic: beam sprite skipped: beam drawing code moved");
        return Ok(());
    }
    // `LDA #$32` -> `LDA #$84`.
    ctx.rom.write_cpu(0, BEAM_CODE + 1, &[0x84])?;
    if !src.flashing {
        // `LDA $12` (frame counter) -> `LDA #$01`, then `AND #$03`.
        ctx.rom.write_cpu(0, BEAM_CODE + 5, &[0xA9, 0x01])?;
    }
    if src.spin {
        // `AND #$7F` (clear the vertical flip) -> `AND #$FF`.
        ctx.rom.write_cpu(0, BEAM_CODE + 13, &[0xFF])?;
    }
    let mut pages = 0;
    if let Some((page, tile)) = src.tile {
        let from = usize::from(page) * 0x2000 + usize::from(tile) * 16;
        let gfx: Vec<u8> = ctx.vanilla.chr()[from..from + 32].to_vec();
        // Only pages whose `$84` is the vanilla fireball (page 0's).
        let reference: Vec<u8> = ctx.vanilla.chr()[FIREBALL_TILE_OFF..][..32].to_vec();
        for p in SPRITE_PAGES {
            let at = p * 0x2000 + FIREBALL_TILE_OFF;
            if ctx.vanilla.chr()[at..at + 32] == reference[..] {
                ctx.rom.write_chr(at, &gfx)?;
                pages += 1;
            }
        }
    }
    ctx.log(format!(
        "cosmetic: beam = {} ({pages} sprite pages redrawn)",
        choice.label_of()
    ));
    ctx.spoiler
        .line("Cosmetics", format!("Beam sprite: {}", choice.label_of()));
    Ok(())
}

// ---------------------------------------------------------------------------
// Sprite palettes.
// ---------------------------------------------------------------------------

/// Turn the hue of an NES colour by `turn` steps (1-12 are the hues; the
/// greys `$x0` and the blacks `$xD-$xF` are kept).
#[must_use]
pub fn turn_hue(c: u8, turn: u8) -> u8 {
    let hue = c & 0x0F;
    if hue == 0 || hue >= 0x0D {
        return c;
    }
    let h = (hue - 1 + turn % 12) % 12 + 1;
    (c & 0x30) | h
}

fn shuffle_sprite_palettes(ctx: &mut Ctx) -> Result<(), RandoError> {
    let mut rng = ctx.rng.derive("sprite palettes");
    let mut n = 0;
    for &(bank, addr) in LINK_PALETTE_SITES {
        // Only the sideview sets: Link's group, then three 4-byte groups
        // (a placeholder byte and three colours). Groups 2 and 3 are the
        // enemy palettes.
        if !(1..=5).contains(&bank) || addr >= 0x8100 {
            continue;
        }
        for g in 2..=3u16 {
            let turn = rng.range_u8(1, 11);
            for k in 0..3u16 {
                let a = addr + 4 * g + k;
                let v = ctx.rom.read_cpu(bank, a)?;
                ctx.rom.write_cpu(bank, a, &[turn_hue(v, turn)])?;
            }
            n += 1;
        }
    }
    ctx.log(format!("cosmetic: {n} enemy palettes recoloured"));
    Ok(())
}

// ---------------------------------------------------------------------------
// Music.
// ---------------------------------------------------------------------------

/// Bank 6 holds two sound engines: the title/ending engine at `$8000` and
/// the gameplay engine at `$9000`. Each reads music volumes from its own
/// tables and immediates; sound effects use separate values.
///
/// Title engine: pulse volume tables (three envelopes back to back, duty in
/// the high nibble, constant volume in the low one).
const TITLE_PULSE_VOLUMES: u16 = 0x8017;
const TITLE_PULSE_VOLUME_LEN: usize = 70;
/// Title engine: drum (noise) volumes, every fourth byte of the drum table.
const TITLE_DRUM_VOLUMES: [u16; 5] = [0x805F, 0x8063, 0x8067, 0x806B, 0x806F];
/// Title engine: `LDA #$81` written to the triangle's linear counter.
const TITLE_TRIANGLE_ON: u16 = 0x8486;
/// Gameplay engine: pulse volume table (24 entries).
const MUSIC_PULSE_VOLUMES: u16 = 0x9135;
const MUSIC_PULSE_VOLUME_LEN: usize = 24;

fn disable_music(ctx: &mut Ctx) -> Result<(), RandoError> {
    let title_off = ctx.rom.cpu_offset(6, TITLE_PULSE_VOLUMES)?;
    let title = ctx
        .rom
        .read_slice(title_off, TITLE_PULSE_VOLUME_LEN)?
        .to_vec();
    let off = ctx.rom.cpu_offset(6, MUSIC_PULSE_VOLUMES)?;
    let table = ctx.rom.read_slice(off, MUSIC_PULSE_VOLUME_LEN)?.to_vec();
    let mut sites_ok = table.iter().all(|b| b & 0xF0 == 0x90)
        && title.iter().all(|b| matches!(b & 0xF0, 0x50 | 0x90))
        && ctx.rom.read_cpu(6, TITLE_TRIANGLE_ON)? == 0xA9
        && ctx.rom.read_cpu(6, 0x9D5D)? == 0xA0
        && ctx.rom.read_cpu(6, 0x9D61)? == 0xA0
        && ctx.rom.read_cpu(6, 0x9DA2)? == 0xA9;
    for a in TITLE_DRUM_VOLUMES {
        sites_ok &= ctx.rom.read_cpu(6, a)? & 0xF0 == 0x10;
    }
    if !sites_ok {
        ctx.log("cosmetic: disable music skipped: music engine moved");
        return Ok(());
    }
    // Title engine: pulses at volume 0 (duty kept), drums at volume 0,
    // triangle linear counter 0.
    let silent: Vec<u8> = title.iter().map(|b| b & 0xF0).collect();
    ctx.rom.write(title_off, &silent)?;
    for a in TITLE_DRUM_VOLUMES {
        let v = ctx.rom.read_cpu(6, a)?;
        ctx.rom.write_cpu(6, a, &[v & 0xF0])?;
    }
    ctx.rom.write_cpu(6, TITLE_TRIANGLE_ON + 1, &[0x80])?;
    // Gameplay engine: pulses at volume 0 (duty kept).
    let silent: Vec<u8> = table.iter().map(|b| b & 0xF0).collect();
    ctx.rom.write(off, &silent)?;
    // Triangle: linear counter 0 (`LDY #$1F` / `LDY #$60` before `STY $4008`).
    ctx.rom.write_cpu(6, 0x9D5E, &[0x00])?;
    ctx.rom.write_cpu(6, 0x9D62, &[0x00])?;
    // Drums: constant volume 0 (`LDA #$17` before the noise write).
    ctx.rom.write_cpu(6, 0x9DA3, &[0x10])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Bring-your-own sprite patch.
// ---------------------------------------------------------------------------

/// What happened to a sprite patch's bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpriteReport {
    /// Records in the patch.
    pub records: usize,
    /// CHR bytes written.
    pub chr: usize,
    /// Link palette bytes written.
    pub palette: usize,
    /// Item-graphics bytes left alone (`change_item_sprites` off).
    pub item_bytes_kept: usize,
    /// Bytes outside CHR and Link's palette that were dropped.
    pub dropped: usize,
}

impl SpriteReport {
    /// One-line description for the log and the spoiler.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} records, {} graphics bytes, {} palette bytes",
            self.records, self.chr, self.palette
        );
        if self.item_bytes_kept > 0 {
            s.push_str(&format!(
                ", {} item-graphics bytes kept original",
                self.item_bytes_kept
            ));
        }
        if self.dropped > 0 {
            s.push_str(&format!(
                ", WARNING: {} bytes outside graphics and Link's palette ignored",
                self.dropped
            ));
        }
        s
    }
}

/// iNES file offsets of the vanilla image.
const INES_PRG_START: usize = 0x10;
const INES_CHR_START: usize = 0x10 + 0x20000;
const INES_END: usize = INES_CHR_START + 0x20000;

/// Whether the vanilla iNES file offset `off` is one of Link's palette
/// bytes (or the tunic-restore / Shield operands).
fn is_link_palette_byte(off: usize) -> bool {
    let cpu = |bank: u8, addr: u16| -> usize {
        let base = if addr >= 0xC000 {
            7 * 0x4000 + usize::from(addr - 0xC000)
        } else {
            usize::from(bank) * 0x4000 + usize::from(addr - 0x8000)
        };
        INES_PRG_START + base
    };
    LINK_PALETTE_SITES
        .iter()
        .any(|&(b, a)| (cpu(b, a)..cpu(b, a) + 3).contains(&off))
        || off == cpu(TUNIC_RESTORE.0, TUNIC_RESTORE.1)
        || off == cpu(SHIELD_TUNIC.0, SHIELD_TUNIC.1)
}

/// Whether CHR offset `chr_off` is inside an item tile of a sprite page.
fn is_item_chr_byte(chr_off: usize) -> bool {
    let page = chr_off / 0x2000;
    let in_page = chr_off % 0x2000;
    if !SPRITE_PAGES.contains(&page) || in_page >= 0x1000 {
        return false;
    }
    let tile = (in_page / 16) as u8 & 0xFE;
    ITEM_TILES.contains(&tile)
}

/// Apply a bring-your-own sprite IPS patch: CHR bytes (minus item tiles
/// unless `items`) and Link's palette bytes; everything else is dropped and
/// counted.
pub fn apply_sprite_ips(
    ctx: &mut Ctx,
    ips: &[u8],
    items: bool,
) -> Result<SpriteReport, RandoError> {
    let records = crate::ips::parse(ips)?;
    let mut rep = SpriteReport {
        records: records.len(),
        ..SpriteReport::default()
    };
    for r in &records {
        for (i, &b) in r.data.iter().enumerate() {
            let off = r.offset + i;
            if off < INES_PRG_START {
                // The header: ours is synthetic; ignore quietly.
                continue;
            }
            if (INES_CHR_START..INES_END).contains(&off) {
                let c = off - INES_CHR_START;
                if !items && is_item_chr_byte(c) {
                    rep.item_bytes_kept += 1;
                    continue;
                }
                ctx.rom.write_chr(c, &[b])?;
                rep.chr += 1;
            } else if off < INES_CHR_START && is_link_palette_byte(off) {
                let p = ctx.rom.prg_from_ines(off)?;
                ctx.rom.write(p, &[b])?;
                rep.palette += 1;
            } else {
                rep.dropped += 1;
            }
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Flags;
    use crate::rom::{Rom, VANILLA_BODY_LEN};
    use crate::spoiler::Spoiler;
    use crate::{Extras, State};

    fn put(body: &mut [u8], bank: u8, addr: u16, b: &[u8]) {
        let off = if addr >= 0xC000 {
            7 * 0x4000 + usize::from(addr - 0xC000)
        } else {
            usize::from(bank) * 0x4000 + usize::from(addr - 0x8000)
        };
        body[off..off + b.len()].copy_from_slice(b);
    }

    /// Synthetic image: Link's palette at every site, the beam and music
    /// code shapes, recognisable CHR.
    fn image() -> Vec<u8> {
        let mut body = vec![0u8; VANILLA_BODY_LEN];
        for &(bank, addr) in LINK_PALETTE_SITES {
            put(&mut body, bank, addr, &[0x18, 0x36, 0x2A]);
            if (1..=5).contains(&bank) && addr < 0x8100 {
                for g in 1..=3u16 {
                    put(&mut body, bank, addr + 4 * g - 1, &[0xFF, 0x0F, 0x16, 0x27]);
                }
            }
        }
        put(&mut body, 0, 0x90D9, &[0xA9, 0x2A]);
        put(&mut body, 0, 0x8E8D, &[0xA9, 0x16]);
        put(&mut body, 0, BEAM_CODE, &BEAM_CODE_VANILLA);
        put(
            &mut body,
            6,
            MUSIC_PULSE_VOLUMES,
            &[0x94; MUSIC_PULSE_VOLUME_LEN],
        );
        put(&mut body, 6, 0x9D5D, &[0xA0, 0x1F]);
        put(&mut body, 6, 0x9D61, &[0xA0, 0x60]);
        put(&mut body, 6, 0x9DA2, &[0xA9, 0x17]);
        put(
            &mut body,
            6,
            TITLE_PULSE_VOLUMES,
            &[0x55; TITLE_PULSE_VOLUME_LEN],
        );
        for a in TITLE_DRUM_VOLUMES {
            put(&mut body, 6, a, &[0x17]);
        }
        put(&mut body, 6, TITLE_TRIANGLE_ON, &[0xA9, 0x81]);
        let chr = &mut body[0x20000..];
        for (i, b) in chr.iter_mut().enumerate() {
            *b = (i / 32) as u8;
        }
        // Same beam tile in every sprite page.
        for p in 0..16 {
            let at = p * 0x2000 + FIREBALL_TILE_OFF;
            chr[at..at + 32].fill(0xBE);
        }
        body
    }

    fn ctx_with(flags: Flags, image: Vec<u8>) -> Ctx {
        let rom = Rom::from_body(&image).unwrap();
        Ctx {
            vanilla: rom.clone(),
            rom,
            rng: Rng::new(42),
            flags,
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        }
    }

    fn rd(c: &Ctx, bank: u8, addr: u16) -> u8 {
        c.rom.read_cpu(bank, addr).unwrap()
    }

    #[test]
    fn default_flags_change_nothing() {
        let mut c = ctx_with(Flags::default(), image());
        apply(&mut c).unwrap();
        assert_eq!(c.rom.body(), image());
    }

    #[test]
    fn explicit_colours_hit_every_site() {
        let mut f = Flags::default();
        f.cosmetic.tunic = NesColor::Color(0x11);
        f.cosmetic.skin_tone = NesColor::Color(0x27);
        f.cosmetic.tunic_outline = NesColor::Color(0x0F);
        f.cosmetic.shield_tunic = NesColor::Color(0x30);
        let mut c = ctx_with(f, image());
        apply(&mut c).unwrap();
        for &(bank, addr) in LINK_PALETTE_SITES {
            assert_eq!(rd(&c, bank, addr), 0x0F);
            assert_eq!(rd(&c, bank, addr + 1), 0x27);
            assert_eq!(rd(&c, bank, addr + 2), 0x11);
        }
        assert_eq!(rd(&c, 0, 0x90DA), 0x11);
        assert_eq!(rd(&c, 0, 0x8E8E), 0x30);
    }

    #[test]
    fn random_colours_are_seeded_distinct_and_safe() {
        let mut f = Flags::default();
        f.cosmetic.tunic = NesColor::Random;
        f.cosmetic.skin_tone = NesColor::Random;
        f.cosmetic.tunic_outline = NesColor::Random;
        f.cosmetic.shield_tunic = NesColor::Random;
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..40u64 {
            let run = |seed| {
                let mut c = ctx_with(f.clone(), image());
                c.rng = Rng::new(seed);
                apply(&mut c).unwrap();
                let (b, a) = LINK_PALETTE_SITES[0];
                [
                    rd(&c, b, a),
                    rd(&c, b, a + 1),
                    rd(&c, b, a + 2),
                    rd(&c, 0, 0x8E8E),
                ]
            };
            let v = run(seed);
            assert_eq!(v, run(seed), "same seed, same colours");
            let mut d = v.to_vec();
            d.sort_unstable();
            d.dedup();
            assert_eq!(d.len(), 4, "{v:02X?}");
            assert!(v.iter().all(|c| random_color_pool().contains(c)));
            seen.insert(v);
        }
        assert!(seen.len() > 30, "colours vary with the seed");
    }

    #[test]
    fn random_tunic_avoids_the_kept_colours() {
        let mut f = Flags::default();
        f.cosmetic.tunic = NesColor::Random;
        for seed in 0..200u64 {
            let mut c = ctx_with(f.clone(), image());
            c.rng = Rng::new(seed);
            apply(&mut c).unwrap();
            let t = rd(&c, 1, 0x80A1);
            assert!(![0x18, 0x36, 0x16].contains(&t), "{t:02X}");
        }
    }

    #[test]
    fn beam_copies_the_tile_and_sets_operands() {
        let mut f = Flags::default();
        f.cosmetic.beam_sprite = BeamSprite::Axe;
        let base = image();
        let mut c = ctx_with(f, base.clone());
        apply(&mut c).unwrap();
        let src = beam_source(BeamSprite::Axe).unwrap();
        let (page, tile) = src.tile.unwrap();
        let from = usize::from(page) * 0x2000 + usize::from(tile) * 16;
        let want = base[0x20000 + from..0x20000 + from + 32].to_vec();
        for p in 0..16 {
            let at = p * 0x2000 + FIREBALL_TILE_OFF;
            let got = &c.rom.chr()[at..at + 32];
            if SPRITE_PAGES.contains(&p) {
                assert_eq!(got, &want[..], "page {p}");
            } else {
                assert_eq!(got, &[0xBE; 32], "page {p} untouched");
            }
        }
        let off = c.rom.cpu_offset(0, BEAM_CODE).unwrap();
        let code = c.rom.read_slice(off, 14).unwrap();
        // Tile $84, steady palette 1, vertical flip kept.
        assert_eq!(code[1], 0x84);
        assert_eq!(&code[5..7], &[0xA9, 0x01]);
        assert_eq!(code[13], 0xFF);
    }

    #[test]
    fn fire_beam_uses_the_fireball_as_is() {
        let mut f = Flags::default();
        f.cosmetic.beam_sprite = BeamSprite::Fire;
        let mut c = ctx_with(f, image());
        apply(&mut c).unwrap();
        assert_eq!(c.rom.chr(), &image()[0x20000..], "no CHR change");
        let off = c.rom.cpu_offset(0, BEAM_CODE).unwrap();
        let code = c.rom.read_slice(off, 14).unwrap();
        assert_eq!(code[1], 0x84);
        assert_eq!(code[13], 0x7F);
    }

    #[test]
    fn every_beam_choice_has_a_source() {
        for b in BeamSprite::all() {
            let s = beam_source(*b);
            assert_eq!(
                s.is_none(),
                matches!(b, BeamSprite::Default | BeamSprite::Random)
            );
            if let Some((page, tile)) = s.and_then(|s| s.tile) {
                assert_eq!(tile & 1, 0);
                assert!(SPRITE_PAGES.contains(&usize::from(page)));
            }
        }
    }

    #[test]
    fn hue_turn_keeps_brightness_and_greys() {
        assert_eq!(turn_hue(0x16, 1), 0x17);
        assert_eq!(turn_hue(0x1C, 1), 0x11);
        assert_eq!(turn_hue(0x30, 5), 0x30);
        assert_eq!(turn_hue(0x0F, 5), 0x0F);
        assert_eq!(turn_hue(0x2D, 5), 0x2D);
        for c in 0..0x40u8 {
            for t in 1..12 {
                assert_eq!(turn_hue(c, t) & 0x30, c & 0x30);
            }
        }
    }

    #[test]
    fn shuffled_palettes_touch_enemy_groups_only() {
        let mut f = Flags::default();
        f.cosmetic.shuffle_sprite_palettes = true;
        let mut c = ctx_with(f, image());
        apply(&mut c).unwrap();
        let mut changed = 0;
        for &(bank, addr) in LINK_PALETTE_SITES {
            assert_eq!(rd(&c, bank, addr + 2), 0x2A, "Link kept");
            if !(1..=5).contains(&bank) || addr >= 0x8100 {
                continue;
            }
            // Group 1 (items, beam) kept.
            assert_eq!(rd(&c, bank, addr + 4), 0x0F);
            assert_eq!(rd(&c, bank, addr + 5), 0x16);
            for g in 2..=3u16 {
                assert_eq!(rd(&c, bank, addr + 4 * g), 0x0F, "black kept");
                if rd(&c, bank, addr + 4 * g + 1) != 0x16 {
                    changed += 1;
                }
            }
        }
        assert_eq!(changed, 2 * 21);
    }

    #[test]
    fn music_off_zeroes_the_music_volumes() {
        let mut f = Flags::default();
        f.cosmetic.disable_music = true;
        let mut c = ctx_with(f, image());
        apply(&mut c).unwrap();
        for i in 0..MUSIC_PULSE_VOLUME_LEN as u16 {
            assert_eq!(rd(&c, 6, MUSIC_PULSE_VOLUMES + i), 0x90);
        }
        assert_eq!(rd(&c, 6, 0x9D5E), 0);
        assert_eq!(rd(&c, 6, 0x9D62), 0);
        assert_eq!(rd(&c, 6, 0x9DA3), 0x10);
        for i in 0..TITLE_PULSE_VOLUME_LEN as u16 {
            assert_eq!(rd(&c, 6, TITLE_PULSE_VOLUMES + i), 0x50);
        }
        for a in TITLE_DRUM_VOLUMES {
            assert_eq!(rd(&c, 6, a), 0x10);
        }
        assert_eq!(rd(&c, 6, TITLE_TRIANGLE_ON + 1), 0x80);
    }

    fn ips(records: &[(usize, &[u8])]) -> Vec<u8> {
        let mut p = b"PATCH".to_vec();
        for (off, data) in records {
            p.extend_from_slice(&[(off >> 16) as u8, (off >> 8) as u8, *off as u8]);
            p.extend_from_slice(&(data.len() as u16).to_be_bytes());
            p.extend_from_slice(data);
        }
        p.extend_from_slice(b"EOF");
        p
    }

    #[test]
    fn sprite_patch_takes_graphics_and_palette_only() {
        let link_chr = INES_CHR_START + 0x0100; // Link's tiles, page 0
        let key_chr = INES_CHR_START + 0x66 * 16; // the key, page 0
        let palette = INES_PRG_START + 0x4000 + 0x009F + 2; // bank 1 tunic
        let code = INES_PRG_START + 0x1234; // bank 0 code
        let p = ips(&[
            (0, &[0x4E, 0x45]),
            (link_chr, &[1, 2, 3, 4]),
            (key_chr, &[9, 9]),
            (palette, &[0x21]),
            (code, &[0xEA, 0xEA, 0xEA]),
        ]);
        let mut c = ctx_with(Flags::default(), image());
        c.extras.sprite_ips = Some(p.clone());
        apply(&mut c).unwrap();
        assert_eq!(&c.rom.chr()[0x100..0x104], &[1, 2, 3, 4]);
        assert_ne!(&c.rom.chr()[0x660..0x662], &[9, 9], "item kept");
        assert_eq!(rd(&c, 1, 0x80A1), 0x21);
        assert_eq!(c.rom.prg()[0x1234], 0, "code untouched");
        assert!(
            c.log.iter().any(|l| l.contains("3 bytes outside")),
            "{:?}",
            c.log
        );
        assert!(c.spoiler.render().contains("Sprite patch"));

        // With item sprites allowed the key changes too; an explicit
        // tunic colour still wins over the patch.
        let mut f = Flags::default();
        f.cosmetic.change_item_sprites = true;
        f.cosmetic.tunic = NesColor::Color(0x12);
        let mut c = ctx_with(f, image());
        c.extras.sprite_ips = Some(p);
        apply(&mut c).unwrap();
        assert_eq!(&c.rom.chr()[0x660..0x662], &[9, 9]);
        assert_eq!(rd(&c, 1, 0x80A1), 0x12);
    }

    #[test]
    fn malformed_sprite_patch_is_an_error() {
        let mut c = ctx_with(Flags::default(), image());
        c.extras.sprite_ips = Some(b"NOPE".to_vec());
        assert!(matches!(apply(&mut c), Err(RandoError::Ips(_))));
    }

    /// ROM-gated: the palette sites hold Link's vanilla palette, the music
    /// and beam code have the shapes the module expects, and every option
    /// applies on several seeds without touching the fixed bank's code.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_cosmetic_sites_and_options() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let rom = Rom::from_body(&body).unwrap();
        for &(bank, addr) in LINK_PALETTE_SITES {
            let off = rom.cpu_offset(bank, addr).unwrap();
            assert_eq!(
                rom.read_slice(off, 3).unwrap(),
                [0x18, 0x36, 0x2A],
                "bank {bank} ${addr:04X}"
            );
        }
        assert_eq!(rom.read_cpu(0, 0x90D9).unwrap(), 0xA9);
        assert_eq!(rom.read_cpu(0, 0x90DA).unwrap(), 0x2A);
        assert_eq!(rom.read_cpu(0, 0x8E8E).unwrap(), 0x16);
        // The fireball tile is the same in 12 sprite pages, and the beam
        // drawer has the expected shape.
        let chr = rom.chr();
        let pages = (0..13)
            .filter(|p| {
                chr[p * 0x2000 + FIREBALL_TILE_OFF..][..32] == chr[FIREBALL_TILE_OFF..][..32]
            })
            .count();
        assert_eq!(pages, 12);
        let off = rom.cpu_offset(0, BEAM_CODE).unwrap();
        assert_eq!(rom.read_slice(off, 14).unwrap(), BEAM_CODE_VANILLA);
        for b in BeamSprite::all() {
            let mut f = Flags::default();
            f.cosmetic.tunic = NesColor::Random;
            f.cosmetic.skin_tone = NesColor::Random;
            f.cosmetic.tunic_outline = NesColor::Random;
            f.cosmetic.shield_tunic = NesColor::Random;
            f.cosmetic.beam_sprite = *b;
            f.cosmetic.shuffle_sprite_palettes = true;
            f.cosmetic.disable_music = true;
            for seed in ["1", "cosmetic"] {
                let out = crate::randomize(&body, seed, &f).unwrap();
                assert!(
                    !out.log.iter().any(|l| l.contains("skipped")),
                    "{:?}",
                    out.log
                );
                // Only palette data in the fixed bank changes.
                let palette = [0xC454, 0xC455, 0xC456, 0xC46C, 0xC46D, 0xC46E];
                assert!(
                    out.fixed_bank_changes.iter().all(|a| palette.contains(a)),
                    "{b:?}: {:04X?}",
                    out.fixed_bank_changes
                );
                assert_eq!(out.hash_code, crate::hash_code(seed, &Flags::default()));
            }
        }
    }
}
