//! Display-only enhancement settings ([`DisplayEnh`]): data only.
//!
//! These never change the game, never join the netplay identity and are
//! allowed under `--movie`. The type lives in `z2-core` only so every
//! frontend (native, launcher, web) shares one serde shape; the effects are
//! implemented by the frontends (`z2-native/src/display_enh.rs`).
//!
//! ZALiA reference behaviour:
//!
//! * `screen_shake`: shake the picture on boss explosions and similar events
//!   (ZALiA `ScreenShake_user_pref`, default on there).
//! * `flash_color` ([`FlashColor`]): the colour the background flashes on
//!   spell casts and boss explosions; `None` turns flashing off
//!   (photosensitivity). ZALiA default red.
//! * `effects_enabled` + `brightness`, `saturation`, `scanlines`, `bloom`,
//!   `blur`: ZALiA's Graphics Effects Editor post-process chain (off by
//!   default there; its values when on are 0.05, -0.075, 0.5, 0.16, 0.85).
//! * `quest_timer`: speedrun-style `00:00:00.00` timer since the quest
//!   started (ZALiA `QuestTimer_show`).
//! * `music_volume`, `sfx_volume` (`0..=10`): ZALiA's volume options
//!   (default 5 there; 10 = unchanged here).
//! * `low_hp_beep_reduced`: the low-HP beep plays less often after a few
//!   repeats (ZALiA `mod_LOW_HP_SOUND=1`).
//! * `dev_hitboxes`, `dev_xy`, `dev_hp`, `dev_framecount`: ZALiA DEV TOOLS
//!   overlays (hitboxes, XY points, HP numbers, frame count).

use serde::{Deserialize, Serialize};

/// Largest [`DisplayEnh::music_volume`] / [`DisplayEnh::sfx_volume`].
pub const MAX_VOLUME: u8 = 10;

/// Background flash colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashColor {
    /// Original: whatever the game flashes.
    #[default]
    Og,
    /// No flashing (black).
    None,
    /// Gray.
    Gray,
    /// Red (ZALiA default).
    Red,
    /// Violet.
    Violet,
    /// Green.
    Green,
}

impl FlashColor {
    /// Every value (for menus).
    pub const ALL: [FlashColor; 6] = [
        FlashColor::Og,
        FlashColor::None,
        FlashColor::Gray,
        FlashColor::Red,
        FlashColor::Violet,
        FlashColor::Green,
    ];

    /// Menu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            FlashColor::Og => "Original",
            FlashColor::None => "None (no flashing)",
            FlashColor::Gray => "Gray",
            FlashColor::Red => "Red",
            FlashColor::Violet => "Violet",
            FlashColor::Green => "Green",
        }
    }
}

/// Display-only enhancements. `Default` changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayEnh {
    /// Shake the picture on big hits and explosions.
    pub screen_shake: bool,
    /// Background flash colour.
    pub flash_color: FlashColor,
    /// Brightness, `-1.0..=1.0` (`0` = unchanged).
    pub brightness: f32,
    /// Saturation, `-1.0..=1.0` (`0` = unchanged).
    pub saturation: f32,
    /// Scanline strength, `0.0..=1.0`.
    pub scanlines: f32,
    /// Bloom strength, `0.0..=1.0`.
    pub bloom: f32,
    /// Blur strength, `0.0..=1.0`.
    pub blur: f32,
    /// Master switch for the five post effects above.
    pub effects_enabled: bool,
    /// Show the quest timer.
    pub quest_timer: bool,
    /// Music volume, `0..=10` (`10` = unchanged).
    pub music_volume: u8,
    /// Sound-effect volume, `0..=10` (`10` = unchanged).
    pub sfx_volume: u8,
    /// Quieter low-HP beep.
    pub low_hp_beep_reduced: bool,
    /// Draw hitboxes.
    pub dev_hitboxes: bool,
    /// Draw object XY points.
    pub dev_xy: bool,
    /// Draw enemy HP.
    pub dev_hp: bool,
    /// Draw the frame counter.
    pub dev_framecount: bool,
}

impl Default for DisplayEnh {
    fn default() -> Self {
        Self {
            screen_shake: false,
            flash_color: FlashColor::Og,
            brightness: 0.0,
            saturation: 0.0,
            scanlines: 0.0,
            bloom: 0.0,
            blur: 0.0,
            effects_enabled: false,
            quest_timer: false,
            music_volume: MAX_VOLUME,
            sfx_volume: MAX_VOLUME,
            low_hp_beep_reduced: false,
            dev_hitboxes: false,
            dev_xy: false,
            dev_hp: false,
            dev_framecount: false,
        }
    }
}

impl DisplayEnh {
    /// Whether this is the default (nothing changes).
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// ZALiA's display defaults: screen shake on, red flash, the reduced
    /// low-HP beep, and the Graphics Effects Editor values (left switched
    /// off, as in ZALiA).
    #[must_use]
    pub fn zalia_preset() -> Self {
        Self {
            screen_shake: true,
            flash_color: FlashColor::Red,
            brightness: 0.05,
            saturation: -0.075,
            scanlines: 0.5,
            bloom: 0.16,
            blur: 0.85,
            effects_enabled: false,
            low_hp_beep_reduced: true,
            ..Self::default()
        }
    }

    /// Parse JSON (missing fields default, unknown ignored).
    ///
    /// # Errors
    /// Malformed JSON or a value of the wrong type.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("display enhancements JSON: {e}"))
    }

    /// Compact JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The same settings with every value clamped into its documented range
    /// (NaN becomes `0`).
    #[must_use]
    pub fn clamped(mut self) -> Self {
        let c = |v: f32, lo: f32, hi: f32| if v.is_nan() { 0.0 } else { v.clamp(lo, hi) };
        self.brightness = c(self.brightness, -1.0, 1.0);
        self.saturation = c(self.saturation, -1.0, 1.0);
        self.scanlines = c(self.scanlines, 0.0, 1.0);
        self.bloom = c(self.bloom, 0.0, 1.0);
        self.blur = c(self.blur, 0.0, 1.0);
        self.music_volume = self.music_volume.min(MAX_VOLUME);
        self.sfx_volume = self.sfx_volume.min(MAX_VOLUME);
        self
    }
}
