//! The NES master palette (64 display RGB entries) and its file forms.

/// 64 RGB entries indexed by NES colour `$00-$3F`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterPalette(pub [[u8; 3]; 64]);

impl Default for MasterPalette {
    fn default() -> Self {
        Self::NES
    }
}

impl MasterPalette {
    /// The palette the frontends use today (`z2_ppu::NES_PALETTE_RGB`).
    pub const NES: MasterPalette = MasterPalette(z2_ppu::NES_PALETTE_RGB);

    /// RGB for an NES colour (masked to 6 bits).
    #[inline]
    #[must_use]
    pub fn rgb(&self, index: u8) -> [u8; 3] {
        self.0[(index & 0x3F) as usize]
    }

    /// Opaque RGBA for an NES colour (masked to 6 bits).
    #[inline]
    #[must_use]
    pub fn rgba(&self, index: u8) -> [u8; 4] {
        let [r, g, b] = self.rgb(index);
        [r, g, b, 0xFF]
    }

    /// Parse a `.pal` file: 192 bytes (64 RGB triples) or 1536 bytes (512
    /// entries with emphasis variants; only the first 64 are used).
    ///
    /// # Errors
    /// A message naming the byte length when it is neither size.
    pub fn from_pal_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 192 && bytes.len() != 1536 {
            return Err(format!(
                ".pal file is {} bytes; expected 192 (64 RGB triples) or 1536 (512 triples)",
                bytes.len()
            ));
        }
        let mut pal = [[0u8; 3]; 64];
        for (i, e) in pal.iter_mut().enumerate() {
            e.copy_from_slice(&bytes[i * 3..i * 3 + 3]);
        }
        Ok(Self(pal))
    }

    /// Serialise as a 192-byte `.pal` file.
    #[must_use]
    pub fn to_pal_bytes(&self) -> Vec<u8> {
        self.0.iter().flatten().copied().collect()
    }

    /// Parse one `"#RRGGBB"` (or `"RRGGBB"`) string.
    ///
    /// # Errors
    /// A message quoting the offending string.
    pub fn parse_hex_color(s: &str) -> Result<[u8; 3], String> {
        let h = s.strip_prefix('#').unwrap_or(s);
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("\"{s}\" is not a #RRGGBB colour"));
        }
        let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
        Ok([byte(0), byte(2), byte(4)])
    }
}
