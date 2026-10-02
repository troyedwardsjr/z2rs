//! Overworld terrain codes (the low nibble of every map run byte).

/// One overworld tile type. The discriminant is the 4-bit code the game
/// stores in the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Terrain {
    /// Town entrance.
    Town = 0,
    /// Cave mouth.
    Cave = 1,
    /// Palace entrance.
    Palace = 2,
    /// Bridge.
    Bridge = 3,
    /// Desert.
    Desert = 4,
    /// Grass.
    Grass = 5,
    /// Forest.
    Forest = 6,
    /// Swamp (slow).
    Swamp = 7,
    /// Graveyard.
    Grave = 8,
    /// Road.
    Road = 9,
    /// Lava.
    Lava = 10,
    /// Mountain (impassable).
    Mountain = 11,
    /// Water (impassable).
    Water = 12,
    /// Water the boots can walk on.
    WalkableWater = 13,
    /// Boulder (the hammer breaks it).
    Rock = 14,
    /// River devil (the flute drives it away).
    RiverDevil = 15,
}

impl Terrain {
    /// All sixteen codes in order.
    pub const ALL: [Terrain; 16] = [
        Terrain::Town,
        Terrain::Cave,
        Terrain::Palace,
        Terrain::Bridge,
        Terrain::Desert,
        Terrain::Grass,
        Terrain::Forest,
        Terrain::Swamp,
        Terrain::Grave,
        Terrain::Road,
        Terrain::Lava,
        Terrain::Mountain,
        Terrain::Water,
        Terrain::WalkableWater,
        Terrain::Rock,
        Terrain::RiverDevil,
    ];

    /// Terrain for a 4-bit code (the high bits are ignored).
    #[must_use]
    pub fn from_code(code: u8) -> Terrain {
        Terrain::ALL[usize::from(code & 0x0F)]
    }

    /// The 4-bit code.
    #[must_use]
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Plain ground Link walks on without any item (towns, caves and
    /// palaces included).
    #[must_use]
    pub fn is_open(self) -> bool {
        !matches!(
            self,
            Terrain::Mountain
                | Terrain::Water
                | Terrain::WalkableWater
                | Terrain::Rock
                | Terrain::RiverDevil
        )
    }

    /// Passable at all, ignoring items (rocks, river devils and walkable
    /// water count as passable here; item gates are checked separately).
    #[must_use]
    pub fn is_passable_ignoring_items(self) -> bool {
        !matches!(self, Terrain::Mountain | Terrain::Water)
    }

    /// Ground terrain a generator may grow as filler around locations.
    #[must_use]
    pub fn is_ground(self) -> bool {
        matches!(
            self,
            Terrain::Desert
                | Terrain::Grass
                | Terrain::Forest
                | Terrain::Swamp
                | Terrain::Grave
                | Terrain::Road
                | Terrain::Lava
        )
    }

    /// Short single-character code for debug dumps and tests.
    #[must_use]
    pub fn glyph(self) -> char {
        b"TCP=d.fsgrLMWwRD"[self as usize] as char
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for c in 0..16u8 {
            assert_eq!(Terrain::from_code(c).code(), c);
            assert_eq!(Terrain::from_code(c | 0xF0).code(), c);
        }
    }

    #[test]
    fn walkability_classes() {
        assert!(Terrain::Road.is_open());
        assert!(Terrain::Cave.is_open());
        assert!(!Terrain::Mountain.is_passable_ignoring_items());
        assert!(!Terrain::Water.is_passable_ignoring_items());
        assert!(Terrain::Rock.is_passable_ignoring_items());
        assert!(!Terrain::Rock.is_open());
        assert!(!Terrain::WalkableWater.is_open());
    }
}
