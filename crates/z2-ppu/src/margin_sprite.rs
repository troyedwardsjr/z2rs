//! Sprites drawn outside the NES window in widescreen.
//!
//! OAM X is 8 bits, so an object the game keeps alive beyond the 256-pixel
//! window has no place in OAM. Providers in `z2-core` describe such objects
//! as [`MarginSprite`]s in window coordinates; the wide compositors draw the
//! parts that fall in the margins.

/// One hardware sprite in window coordinates.
///
/// Same meaning as an OAM entry except that `x` is signed and may lie outside
/// `0..256` (window x = 0 is NES column 0). `y` is the OAM Y byte (the sprite's
/// top row is `y + 1`); `tile` and `attr` are the OAM tile and attribute bytes,
/// read against the line's recorded `PPUCTRL` sprite size and pattern table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MarginSprite {
    /// Window X of the sprite's left column (may be negative or >= 256).
    pub x: i16,
    /// OAM Y byte.
    pub y: u8,
    /// OAM tile byte.
    pub tile: u8,
    /// OAM attribute byte (palette, priority, flips).
    pub attr: u8,
}
