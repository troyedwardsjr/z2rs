//! Native frontend configuration.
//!
//! JSON config (via `serde_json` only — no TOML crate, to keep the native
//! dependency footprint minimal). The file lives at
//! [`config_path`] (`<data-dir>/z2-native.json`); see [`data_dir`] for the
//! platform resolution (manual XDG / macOS Application Support / `%APPDATA%`,
//! no `dirs` crate).
//!
//! Nothing here touches a window or an audio device, so unit tests run on
//! display-less CI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Config file name inside the data dir.
pub const CONFIG_FILE_NAME: &str = "z2-native.json";
/// Application dir leaf.
pub const APP_DIR_NAME: &str = "z2rs";
/// Extracted assets image name inside the data dir.
pub const ASSETS_FILE_NAME: &str = "assets.bin";

/// Resolve the platform data dir (manual, no `dirs` dependency):
///
/// * `$XDG_DATA_HOME/z2rs` when `XDG_DATA_HOME` is set (Linux/BSD; also
///   honoured on macOS for hermetic testing),
/// * `~/Library/Application Support/z2rs` on macOS (`HOME` based),
/// * `%APPDATA%/z2rs` on Windows (`APPDATA` based),
/// * fallback: `$HOME/.local/share/z2rs`, else `./.z2rs-data`.
///
/// The ROM itself is **never** stored here — only `assets.bin` (extracted),
/// save states, SRAM and this JSON config.
pub fn data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(APP_DIR_NAME);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join(APP_DIR_NAME);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join(APP_DIR_NAME);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(APP_DIR_NAME);
        }
    }
    PathBuf::from(".z2rs-data")
}

/// Full path of the JSON config file.
pub fn config_path() -> PathBuf {
    data_dir().join(CONFIG_FILE_NAME)
}

/// Assets image path inside the data dir.
pub fn assets_path() -> PathBuf {
    data_dir().join(ASSETS_FILE_NAME)
}

/// Keyboard binding table: physical-key name → NES button bit (0..7).
///
/// Key names are the `winit` [`winit::keyboard::KeyCode`] debug names
/// (`"KeyZ"`, `"Enter"`, `"ArrowUp"`, …). Keeping the table string-keyed
/// keeps this module (and its tests) free of any `winit` import — the
/// `winit` adapter lives in [`crate::app`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBindings {
    /// Ordered `(key-name, bit)` pairs.
    pub map: HashMap<String, u8>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        // Layout: Z=A, X=B, Enter=Start, ShiftRight=Select,
        // arrows=dpad. Mirrors the README quick-start.
        let pairs = [
            ("KeyZ", 0u8),       // A
            ("KeyX", 1u8),       // B
            ("ShiftRight", 2u8), // Select
            ("Enter", 3u8),      // Start
            ("ArrowUp", 4u8),
            ("ArrowDown", 5u8),
            ("ArrowLeft", 6u8),
            ("ArrowRight", 7u8),
        ];
        Self {
            map: pairs.into_iter().map(|(k, b)| (k.to_string(), b)).collect(),
        }
    }
}

impl KeyBindings {
    /// Bit for `key_name`, if bound. Rejects out-of-range bits defensively.
    pub fn bit_for(&self, key_name: &str) -> Option<u8> {
        self.map.get(key_name).copied().filter(|&b| b < 8)
    }

    /// Second-player default layout (local co-op, `--coop-local`).
    ///
    /// `G`=A, `F`=B, `R`=Select, `T`=Start, `W`/`S`/`A`/`D`=d-pad. Chosen to
    /// sit on the left half of the keyboard so two people can share one
    /// board with player 1 on the arrows. Every key named here must also
    /// appear in the `key_name` whitelist in [`crate::app`] (pinned by the
    /// `key_name_covers_default_bindings` test).
    pub fn default_p2() -> Self {
        let pairs = [
            ("KeyG", 0u8), // A
            ("KeyF", 1u8), // B
            ("KeyR", 2u8), // Select
            ("KeyT", 3u8), // Start
            ("KeyW", 4u8),
            ("KeyS", 5u8),
            ("KeyA", 6u8),
            ("KeyD", 7u8),
        ];
        Self {
            map: pairs.into_iter().map(|(k, b)| (k.to_string(), b)).collect(),
        }
    }
}

/// Gamepad binding table: `gilrs` button name → NES button bit.
///
/// String-keyed for the same testability reason as [`KeyBindings`]; the
/// `gilrs::Button` adapter lives in [`crate::input`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GamepadBindings {
    /// Ordered `(button-name, bit)` pairs.
    pub map: HashMap<String, u8>,
}

impl Default for GamepadBindings {
    fn default() -> Self {
        let pairs = [
            ("East", 0u8),     // A (right face button)
            ("South", 1u8),    // B (bottom face button)
            ("Select", 2u8),   // Select
            ("Start", 3u8),    // Start
            ("DPadUp", 4u8),   // Up
            ("DPadDown", 5u8), // Down
            ("DPadLeft", 6u8), // Left
            ("DPadRight", 7u8),
        ];
        Self {
            map: pairs.into_iter().map(|(k, b)| (k.to_string(), b)).collect(),
        }
    }
}

/// Netplay (online co-op) settings.
///
/// Every field has a serde default, so a config file written before netplay
/// existed still parses (the whole `netplay` object may be absent).
/// ICE credentials are read from the user's own config only: they are never
/// logged and never put in the window title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetplayConfig {
    /// Online co-op mode: `"rollback"` (default) or `"lockstep"`.
    #[serde(default)]
    pub mode: NetMode,
    /// Signalling server base URL (`ws://host[:port]` / `wss://host[:port]`).
    #[serde(default = "default_signal_url")]
    pub signal_url: String,
    /// Input delay in frames (rollback `0..=3`, lockstep `0..=8`; the host's
    /// value wins in a session).
    #[serde(default = "default_input_delay")]
    pub input_delay: u8,
    /// Closing a session after this long without the remote pad.
    #[serde(default = "default_stall_timeout_ms")]
    pub stall_timeout_ms: u64,
    /// ICE servers in the shared text form (`z2_net::ice`): space-separated
    /// `stun:`/`turn:` URLs, `none` for same machine / LAN only. None = the
    /// default STUN pair. `--ice` overrides it.
    #[serde(default)]
    pub ice_url: Option<String>,
    /// TURN username.
    #[serde(default)]
    pub ice_username: Option<String>,
    /// TURN credential. Never logged.
    #[serde(default)]
    pub ice_credential: Option<String>,
}

/// Online co-op synchronisation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetMode {
    /// Apply the local pad at once, predict the remote pad and re-simulate
    /// on a misprediction (`z2_net::RollbackSession`).
    #[default]
    Rollback,
    /// Step only frames whose pads both peers have (`z2_net::Session`).
    Lockstep,
}

impl NetMode {
    /// Parse a `--net-mode` value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rollback" => Some(NetMode::Rollback),
            "lockstep" => Some(NetMode::Lockstep),
            _ => None,
        }
    }

    /// Lower-case name, as in the config file.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            NetMode::Rollback => "rollback",
            NetMode::Lockstep => "lockstep",
        }
    }

    /// Largest input delay the mode accepts.
    #[must_use]
    pub fn max_delay(self) -> u8 {
        match self {
            NetMode::Rollback => z2_net::MAX_ROLLBACK_DELAY,
            NetMode::Lockstep => z2_net::MAX_DELAY,
        }
    }
}

fn default_signal_url() -> String {
    z2_net::DEFAULT_SIGNAL_URL.to_string()
}
fn default_input_delay() -> u8 {
    z2_net::DEFAULT_DELAY
}
fn default_stall_timeout_ms() -> u64 {
    z2_net::DEFAULT_STALL_TIMEOUT_MS
}

impl Default for NetplayConfig {
    fn default() -> Self {
        Self {
            mode: NetMode::default(),
            signal_url: default_signal_url(),
            input_delay: default_input_delay(),
            stall_timeout_ms: default_stall_timeout_ms(),
            ice_url: None,
            ice_username: None,
            ice_credential: None,
        }
    }
}

/// Full native-frontend configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeConfig {
    /// Explicit ROM path (`--rom` overrides). `None` = ask via CLI error.
    #[serde(default)]
    pub rom_path: Option<String>,
    /// Data-dir override (`None` = [`data_dir`]).
    #[serde(default)]
    pub data_dir_override: Option<String>,
    /// Audio output rate (only 44100 / 48000 are shipped game rates).
    #[serde(default = "default_audio_rate")]
    pub audio_rate: u32,
    /// Integer scaling on/off.
    #[serde(default = "default_true")]
    pub integer_scaling: bool,
    /// Aspect correction on/off (correct 8:7 PAR when set).
    #[serde(default = "default_true")]
    pub aspect_correction: bool,
    /// Pause emulation when the window loses focus.
    #[serde(default = "default_true")]
    pub pause_on_focus_loss: bool,
    /// Fast-forward multiplier while the hotkey is held.
    #[serde(default = "default_ff")]
    pub fast_forward_multiplier: u32,
    /// Frames of audio to buffer (see `audio.rs` sizing math).
    #[serde(default = "default_audio_buffer_frames")]
    pub audio_buffer_frames: u32,
    /// Keyboard bindings.
    #[serde(default)]
    pub keys: KeyBindings,
    /// Gamepad bindings.
    #[serde(default)]
    pub gamepad: GamepadBindings,
    /// Widescreen margin preset: `"off"` | `"16:10"` (8 tiles/side) |
    /// `"16:9"` (11) | `"N"` tiles per side (`0..=16`). Display only — the
    /// 256x240 NES frame is never touched. Default `"off"`.
    #[serde(default = "default_widescreen")]
    pub widescreen: String,
    /// Paint the NES window's clipped left 8 columns from the fetched margin
    /// tiles (widescreen only). The overworld sets `PPUMASK $18`, which blanks
    /// those columns, so with this off there is a black 8-px seam between the
    /// left margin and the play field. Cosmetic; the 256x240 NES frame is
    /// never touched either way. Default on.
    #[serde(default = "default_true")]
    pub widescreen_fill_left_clip: bool,
    /// Paint the NES window's right 8 columns (x 248-255) from the line's own
    /// tiles wherever the game hid them behind an opaque edge-mask sprite
    /// (widescreen only). The overworld parks a column of black 8x16 sprites
    /// there, so with this off there is a black 8-px seam between the play
    /// field and the right margin. Cosmetic; the 256x240 NES frame is never
    /// touched either way. Default on.
    #[serde(default = "default_true")]
    pub widescreen_fill_right_clip: bool,
    /// HD graphics pack directory (the one holding `pack.json`), or `None` for
    /// the original art.
    #[serde(default)]
    pub hd_pack: Option<String>,
    /// Output multiplier 1..=8 for the presented image. With a pack loaded, a
    /// scale that does not divide the pack's own scale is replaced by the
    /// pack's scale.
    #[serde(default = "default_hd_scale")]
    pub hd_scale: u32,
    /// Directory to write a recorded template pack into when the app exits
    /// (the `(page, tile, palette)` combinations this session actually drew).
    /// ROM-derived output: it must be outside any git work tree.
    #[serde(default)]
    pub hd_record: Option<String>,
    /// Start with local two-player co-op enabled (same as `--coop-local`).
    #[serde(default)]
    pub coop_local: bool,
    /// Pin player 2 to this gamepad by connection order (same as `--p2-pad`).
    #[serde(default)]
    pub gamepad_p2_index: Option<usize>,
    /// Second-player keyboard bindings (see [`KeyBindings::default_p2`]).
    #[serde(default = "KeyBindings::default_p2")]
    pub keys_p2: KeyBindings,
    /// Second-player gamepad bindings (same button names as [`Self::gamepad`]).
    #[serde(default)]
    pub gamepad_p2: GamepadBindings,
    /// Netplay settings.
    #[serde(default)]
    pub netplay: NetplayConfig,
}

fn default_widescreen() -> String {
    "off".to_string()
}

fn default_hd_scale() -> u32 {
    1
}

fn default_audio_rate() -> u32 {
    44100
}
fn default_true() -> bool {
    true
}
fn default_ff() -> u32 {
    4
}
fn default_audio_buffer_frames() -> u32 {
    2
}

impl Default for NativeConfig {
    fn default() -> Self {
        Self {
            rom_path: None,
            data_dir_override: None,
            audio_rate: default_audio_rate(),
            integer_scaling: true,
            aspect_correction: true,
            pause_on_focus_loss: true,
            fast_forward_multiplier: default_ff(),
            audio_buffer_frames: default_audio_buffer_frames(),
            keys: KeyBindings::default(),
            gamepad: GamepadBindings::default(),
            widescreen: default_widescreen(),
            widescreen_fill_left_clip: true,
            widescreen_fill_right_clip: true,
            hd_pack: None,
            hd_scale: default_hd_scale(),
            hd_record: None,
            coop_local: false,
            gamepad_p2_index: None,
            keys_p2: KeyBindings::default_p2(),
            gamepad_p2: GamepadBindings::default(),
            netplay: NetplayConfig::default(),
        }
    }
}

impl NativeConfig {
    /// Effective data dir (override or [`data_dir`]).
    pub fn effective_data_dir(&self) -> PathBuf {
        match &self.data_dir_override {
            Some(s) => PathBuf::from(s),
            None => data_dir(),
        }
    }

    /// Widescreen margin tiles per side from [`Self::widescreen`].
    ///
    /// An unparseable preset is `0` (off) rather than an error: a config file
    /// must never stop the app from starting. `--widescreen` on the command
    /// line *is* validated and rejects bad values with exit 2.
    #[must_use]
    pub fn widescreen_tiles(&self) -> u8 {
        z2_ppu::preset_tiles(&self.widescreen).unwrap_or(0)
    }

    /// Effective output multiplier, clamped into `1..=z2_render::MAX_SCALE`
    /// so a nonsense config value cannot stop the app from starting (the
    /// `--hd-scale` flag *is* validated and exits 2 instead).
    #[must_use]
    pub fn effective_hd_scale(&self) -> u32 {
        self.hd_scale.clamp(1, z2_render::MAX_SCALE)
    }

    /// Effective audio rate (falls back to 44100 for anything unsupported).
    pub fn effective_audio_rate(&self) -> u32 {
        match self.audio_rate {
            44100 | 48000 => self.audio_rate,
            _ => 44100,
        }
    }

    /// Load from `path` (JSON). Missing file → defaults (first-run path).
    pub fn load_from(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice::<Self>(&bytes).unwrap_or_default()
    }

    /// Load from the standard [`config_path`].
    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    /// Save as pretty JSON, creating parent dirs.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| format!("encode config: {e}"))?;
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(())
    }

    /// Save to the standard [`config_path`].
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&config_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = NativeConfig::default();
        assert_eq!(c.effective_audio_rate(), 44100);
        assert!(c.integer_scaling);
        assert!(c.aspect_correction);
        assert!(c.pause_on_focus_loss);
        assert_eq!(c.fast_forward_multiplier, 4);
        assert_eq!(c.keys.bit_for("KeyZ"), Some(0));
        assert_eq!(c.keys.bit_for("ArrowUp"), Some(4));
        assert_eq!(c.keys.bit_for("Nope"), None);
        // Every new feature is OFF by default: a user who upgrades sees the
        // same 256x240 single-player app until they ask for more.
        assert_eq!(c.widescreen, "off");
        assert_eq!(c.widescreen_tiles(), 0);
        assert!(c.widescreen_fill_left_clip, "cosmetic, on by default");
        assert!(c.widescreen_fill_right_clip, "cosmetic, on by default");
        assert!(c.hd_pack.is_none(), "original art unless a pack is named");
        assert_eq!(c.hd_scale, 1);
        assert_eq!(c.effective_hd_scale(), 1);
        assert!(c.hd_record.is_none());
        assert!(!c.coop_local);
        assert!(c.gamepad_p2_index.is_none());
        assert_eq!(c.keys_p2.bit_for("KeyG"), Some(0));
        assert_eq!(
            c.gamepad_p2.map.get("East").copied(),
            Some(0),
            "positional: right face button is A"
        );
        assert_eq!(
            c.gamepad_p2.map.get("South").copied(),
            Some(1),
            "positional: bottom face button is B"
        );
        assert_eq!(c.netplay.signal_url, "ws://127.0.0.1:3536");
        assert_eq!(c.netplay.input_delay, 2);
        assert_eq!(c.netplay.mode, NetMode::Rollback);
        assert!(c.netplay.ice_url.is_none());
    }

    #[test]
    fn widescreen_presets_and_bad_values() {
        let tiles = |s: &str| {
            NativeConfig {
                widescreen: s.to_string(),
                ..NativeConfig::default()
            }
            .widescreen_tiles()
        };
        assert_eq!(tiles("off"), 0);
        assert_eq!(tiles("16:10"), 8, "384x240");
        assert_eq!(tiles("16:9"), 11, "432x240");
        assert_eq!(tiles("4"), 4);
        assert_eq!(tiles("16"), 16, "widest supported margin");
        // A bad preset in the CONFIG degrades to off rather than refusing to
        // start (the CLI flag is validated and exits 2 instead).
        assert_eq!(tiles("17"), 0, "out of range -> off");
        assert_eq!(tiles("21:9"), 0, "unknown preset -> off");
        assert_eq!(tiles(""), 0);
        assert_eq!(tiles("garbage"), 0);
    }

    /// A config file written by the previous release has none of the new
    /// keys. It must still parse, and every missing key must take its
    /// default — this is the compatibility contract for `serde(default)`.
    #[test]
    fn old_config_without_new_keys_still_parses() {
        let old = r#"{
            "rom_path": "/tmp/z2.nes",
            "data_dir_override": null,
            "audio_rate": 48000,
            "integer_scaling": true,
            "aspect_correction": true,
            "pause_on_focus_loss": false,
            "fast_forward_multiplier": 8,
            "audio_buffer_frames": 3,
            "keys": { "map": { "KeyZ": 0, "KeyX": 1 } },
            "gamepad": { "map": { "South": 0 } }
        }"#;
        let c: NativeConfig = serde_json::from_str(old).expect("old config parses");
        // Old keys survive …
        assert_eq!(c.rom_path.as_deref(), Some("/tmp/z2.nes"));
        assert_eq!(c.effective_audio_rate(), 48000);
        assert!(!c.pause_on_focus_loss);
        assert_eq!(c.fast_forward_multiplier, 8);
        // … and the new ones default rather than erroring.
        assert_eq!(c.widescreen, "off");
        assert!(!c.coop_local);
        assert_eq!(c.keys_p2, KeyBindings::default_p2());
        assert_eq!(c.gamepad_p2, GamepadBindings::default());
        assert_eq!(c.netplay, NetplayConfig::default());
        assert!(c.widescreen_fill_left_clip);
        assert!(c.widescreen_fill_right_clip);
        assert!(c.hd_pack.is_none());
        assert_eq!(c.hd_scale, 1);
        assert!(c.hd_record.is_none());
        assert!(c.gamepad_p2_index.is_none());
    }

    /// Every new key parses when it IS present, and the scale is clamped
    /// rather than rejected (a stale config must never stop the app).
    #[test]
    fn new_keys_parse_and_bad_values_degrade() {
        let text = r#"{
            "widescreen": "16:9",
            "widescreen_fill_left_clip": false,
            "widescreen_fill_right_clip": false,
            "hd_pack": "/home/me/art/pack",
            "hd_scale": 4,
            "hd_record": "/home/me/art/rec",
            "gamepad_p2_index": 2
        }"#;
        let c: NativeConfig = serde_json::from_str(text).expect("new keys parse");
        assert_eq!(c.widescreen_tiles(), 11);
        assert!(!c.widescreen_fill_left_clip);
        assert!(!c.widescreen_fill_right_clip);
        assert_eq!(c.hd_pack.as_deref(), Some("/home/me/art/pack"));
        assert_eq!(c.effective_hd_scale(), 4);
        assert_eq!(c.hd_record.as_deref(), Some("/home/me/art/rec"));
        assert_eq!(c.gamepad_p2_index, Some(2));

        for (raw, want) in [("0", 1u32), ("99", 8), ("3", 3)] {
            let c: NativeConfig =
                serde_json::from_str(&format!("{{ \"hd_scale\": {raw} }}")).expect("parses");
            assert_eq!(c.effective_hd_scale(), want, "hd_scale {raw}");
        }
    }

    /// `netplay.mode` round-trips as a lower-case string.
    #[test]
    fn netplay_mode_parses_both_names() {
        let c: NativeConfig =
            serde_json::from_str(r#"{ "netplay": { "mode": "lockstep" } }"#).expect("parses");
        assert_eq!(c.netplay.mode, NetMode::Lockstep);
        let text = serde_json::to_string(&c.netplay).expect("serialises");
        assert!(text.contains(r#""mode":"lockstep""#), "{text}");
        assert!(
            serde_json::from_str::<NativeConfig>(r#"{ "netplay": { "mode": "ggpo" } }"#).is_err()
        );
        assert_eq!(NetMode::parse("rollback"), Some(NetMode::Rollback));
        assert_eq!(NetMode::parse("x"), None);
        assert_eq!(NetMode::Rollback.max_delay(), 3);
    }

    /// A partially written `netplay` object takes defaults per missing field.
    #[test]
    fn partial_netplay_object_defaults_field_by_field() {
        let text = r#"{ "netplay": { "input_delay": 5 } }"#;
        let c: NativeConfig = serde_json::from_str(text).expect("partial netplay parses");
        assert_eq!(c.netplay.input_delay, 5, "given field is honoured");
        assert_eq!(
            c.netplay.mode,
            NetMode::Rollback,
            "mode defaults to rollback"
        );
        assert_eq!(
            c.netplay.signal_url,
            NetplayConfig::default().signal_url,
            "missing field defaults"
        );
        assert_eq!(
            c.netplay.stall_timeout_ms,
            NetplayConfig::default().stall_timeout_ms
        );
    }

    /// Unknown keys are ignored (no `deny_unknown_fields`), so a config from
    /// a *newer* build also loads instead of resetting everything.
    #[test]
    fn unknown_keys_are_ignored_not_fatal() {
        let text = r#"{ "widescreen": "16:9", "hd_pack": "/some/pack", "future_thing": 42 }"#;
        let c: NativeConfig = serde_json::from_str(text).expect("unknown keys ignored");
        assert_eq!(c.widescreen_tiles(), 11);
    }

    #[test]
    fn bad_rate_falls_back() {
        let c = NativeConfig {
            audio_rate: 22050,
            ..NativeConfig::default()
        };
        assert_eq!(c.effective_audio_rate(), 44100);
        let c = NativeConfig {
            audio_rate: 48000,
            ..NativeConfig::default()
        };
        assert_eq!(c.effective_audio_rate(), 48000);
    }

    #[test]
    fn roundtrip_through_temp_file() {
        let dir = std::env::temp_dir().join(format!("z2cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("z2-native.json");
        let c = NativeConfig {
            rom_path: Some("/tmp/z2.nes".into()),
            ..NativeConfig::default()
        };
        c.save_to(&p).expect("save");
        let back = NativeConfig::load_from(&p);
        assert_eq!(back, c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let p = std::env::temp_dir().join(format!("z2cfg-missing-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            NativeConfig::load_from(&p),
            NativeConfig::default(),
            "first run must not error"
        );
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("z2cfg-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("z2-native.json");
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(NativeConfig::load_from(&p), NativeConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
