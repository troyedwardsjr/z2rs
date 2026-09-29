//! Launcher settings: what the player picked, how it is saved, how it turns
//! into a game command line, and what is wrong with it.
//!
//! Everything here is plain data plus a few file-existence checks, so it is
//! unit tested without a window.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Application dir leaf (same as the game's).
pub const APP_DIR_NAME: &str = "z2rs";
/// Launcher settings file name inside the data dir.
pub const SETTINGS_FILE_NAME: &str = "launcher.json";
/// The game's own config file name inside the data dir.
pub const GAME_CONFIG_FILE_NAME: &str = "z2-native.json";
/// Public signalling server used for online co-op.
pub const DEFAULT_SIGNAL_URL: &str = "wss://signal.z2rs.com";
/// Largest window scale the launcher offers.
pub const MAX_SCALE: u32 = 6;
/// Largest HD output multiplier the game accepts.
pub const MAX_HD_SCALE: u32 = 8;
/// Largest input delay in rollback mode.
pub const MAX_ROLLBACK_DELAY: u8 = 3;
/// Largest input delay in lockstep mode.
pub const MAX_LOCKSTEP_DELAY: u8 = 8;
/// Longest room name the game accepts.
pub const MAX_ROOM_LEN: usize = 32;

/// Resolve the platform data dir. Kept in step with `data_dir()` in
/// `crates/z2-native/src/config.rs` (duplicated so the launcher does not
/// link the game and its graphics/audio stack):
///
/// * `$XDG_DATA_HOME/z2rs` when `XDG_DATA_HOME` is set,
/// * `~/Library/Application Support/z2rs` on macOS,
/// * `%APPDATA%/z2rs` on Windows,
/// * fallback: `$HOME/.local/share/z2rs`, else `./.z2rs-data`.
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

/// Widescreen margins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Widescreen {
    /// The original 4:3 picture.
    #[default]
    Off,
    /// 16:10 (8 extra tiles each side).
    W16x10,
    /// 16:9 (11 extra tiles each side).
    W16x9,
    /// 21:9 ultrawide (19 extra tiles each side).
    W21x9,
}

impl Widescreen {
    /// Value for `--widescreen`.
    pub fn flag_value(self) -> &'static str {
        match self {
            Widescreen::Off => "off",
            Widescreen::W16x10 => "16:10",
            Widescreen::W16x9 => "16:9",
            Widescreen::W21x9 => "21:9",
        }
    }

    /// Label shown in the launcher.
    pub fn label(self) -> &'static str {
        match self {
            Widescreen::Off => "Off (original 4:3)",
            Widescreen::W16x10 => "16:10",
            Widescreen::W16x9 => "16:9",
            Widescreen::W21x9 => "21:9 ultrawide",
        }
    }
}

/// Who is playing, and where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Multiplayer {
    /// One player.
    #[default]
    Single,
    /// Two players on this computer.
    Local,
    /// Host an online session (player 1).
    Host,
    /// Join an online session (player 2).
    Join,
}

impl Multiplayer {
    /// Host or join.
    pub fn is_online(self) -> bool {
        matches!(self, Multiplayer::Host | Multiplayer::Join)
    }
}

/// Online sync method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NetMode {
    /// Local input is instant; the other player is predicted and corrected.
    #[default]
    Rollback,
    /// Both games wait for both players' input every frame.
    Lockstep,
}

impl NetMode {
    /// Value for `--net-mode`.
    pub fn flag_value(self) -> &'static str {
        match self {
            NetMode::Rollback => "rollback",
            NetMode::Lockstep => "lockstep",
        }
    }

    /// Largest `--net-delay` for this mode.
    pub fn max_delay(self) -> u8 {
        match self {
            NetMode::Rollback => MAX_ROLLBACK_DELAY,
            NetMode::Lockstep => MAX_LOCKSTEP_DELAY,
        }
    }
}

/// Everything the launcher remembers between runs (`<data-dir>/launcher.json`).
///
/// Unknown or missing fields fall back to defaults, so older and newer
/// launchers can share one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The player's own Zelda II (USA) ROM file. Only its path is stored.
    pub rom_path: String,
    /// Initial window size multiplier (1-6).
    pub scale: u32,
    /// Start in borderless fullscreen.
    pub fullscreen: bool,
    /// Scale by whole numbers only (sharper, may leave black borders).
    /// Off = fill the window / screen height (`--scale-mode fit`).
    pub integer_scale: bool,
    /// Widescreen margins.
    pub widescreen: Widescreen,
    /// With widescreen, enemies spawn and move in the margins.
    pub wide_gameplay: bool,
    /// With widescreen, draw sprites that are outside the original picture.
    pub margin_sprites: bool,
    /// HD graphics pack folder (the one with `pack.json`); empty = none.
    pub hd_pack: String,
    /// HD output multiplier (1-8).
    pub hd_scale: u32,
    /// Single player, local co-op or online.
    pub multiplayer: Multiplayer,
    /// Pin player 2 to this gamepad (local co-op only).
    pub p2_pad: Option<u32>,
    /// Online room name.
    pub room: String,
    /// Signalling server URL.
    pub signal_url: String,
    /// Online sync method.
    pub net_mode: NetMode,
    /// Online input delay in frames.
    pub net_delay: u8,
    /// ICE servers (`--ice`); empty = the game's default.
    pub ice: String,
    /// Manual game binary path; empty = find it automatically.
    pub game_bin_override: String,
    /// Close the launcher once the game has started.
    pub close_on_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            rom_path: String::new(),
            scale: 3,
            fullscreen: false,
            integer_scale: false,
            widescreen: Widescreen::Off,
            wide_gameplay: true,
            margin_sprites: true,
            hd_pack: String::new(),
            hd_scale: 4,
            multiplayer: Multiplayer::Single,
            p2_pad: None,
            room: String::new(),
            signal_url: DEFAULT_SIGNAL_URL.to_string(),
            net_mode: NetMode::Rollback,
            net_delay: 2,
            ice: String::new(),
            game_bin_override: String::new(),
            close_on_start: false,
        }
    }
}

impl Settings {
    /// Settings file path inside `data_dir`.
    pub fn path_in(data_dir: &Path) -> PathBuf {
        data_dir.join(SETTINGS_FILE_NAME)
    }

    /// Load from `path`; a missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save to `path`, creating the parent folder.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

/// Game command-line arguments (without the program name) for `s`.
///
/// Every choice the launcher shows is passed explicitly, so what the preview
/// says is what the game does regardless of its own config file.
pub fn build_args(s: &Settings) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |k: &str, v: Option<String>| {
        a.push(k.to_string());
        if let Some(v) = v {
            a.push(v);
        }
    };
    let rom = s.rom_path.trim();
    if !rom.is_empty() {
        push("--rom", Some(rom.to_string()));
    }
    push("--scale", Some(s.scale.to_string()));
    if s.fullscreen {
        push("--fullscreen", None);
    }
    push(
        "--scale-mode",
        Some(if s.integer_scale { "integer" } else { "fit" }.to_string()),
    );
    push("--widescreen", Some(s.widescreen.flag_value().to_string()));
    if s.widescreen != Widescreen::Off {
        push("--wide-gameplay", Some(on_off(s.wide_gameplay)));
        push("--margin-sprites", Some(on_off(s.margin_sprites)));
    }
    // "None" is sent as `--hd-pack ''` so a pack set in z2-native.json
    // cannot come back when the launcher says there is none.
    let pack = s.hd_pack.trim();
    push("--hd-pack", Some(pack.to_string()));
    if !pack.is_empty() {
        push("--hd-scale", Some(s.hd_scale.to_string()));
    }
    match s.multiplayer {
        Multiplayer::Single => {}
        Multiplayer::Local => {
            push("--coop-local", None);
            if let Some(pad) = s.p2_pad {
                push("--p2-pad", Some(pad.to_string()));
            }
        }
        Multiplayer::Host | Multiplayer::Join => {
            let flag = if s.multiplayer == Multiplayer::Host {
                "--coop-host"
            } else {
                "--coop-join"
            };
            push(flag, Some(s.room.trim().to_string()));
            push("--signal", Some(s.signal_url.trim().to_string()));
            push("--net-mode", Some(s.net_mode.flag_value().to_string()));
            push("--net-delay", Some(s.net_delay.to_string()));
            let ice = s.ice.trim();
            if !ice.is_empty() {
                push("--ice", Some(ice.to_string()));
            }
        }
    }
    a
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.to_string()
}

/// `true` when `bytes` start with the iNES header magic `NES\x1a`.
pub fn is_ines(bytes: &[u8]) -> bool {
    bytes.starts_with(b"NES\x1a")
}

fn file_starts_with_ines(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 4];
    let mut f = std::fs::File::open(path)?;
    let mut got = 0;
    while got < head.len() {
        let n = f.read(&mut head[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(is_ines(&head[..got]))
}

/// Room names the game accepts: 1-32 of `A-Z a-z 0-9 _ -`.
pub fn room_name_valid(name: &str) -> bool {
    (1..=MAX_ROOM_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Signalling URLs the game accepts: `ws://host[:port]` or `wss://host[:port]`.
pub fn signal_url_valid(url: &str) -> bool {
    let base = url.trim().trim_end_matches('/');
    let Some(rest) = base
        .strip_prefix("ws://")
        .or_else(|| base.strip_prefix("wss://"))
    else {
        return false;
    };
    !rest.is_empty() && !rest.contains(['/', '?', '#', ' '])
}

/// Human-readable problems that would stop the game from starting.
/// Empty means the settings are good to go.
pub fn validate(s: &Settings) -> Vec<String> {
    let mut out = Vec::new();

    let rom = s.rom_path.trim();
    if rom.is_empty() {
        out.push("Choose your Zelda II ROM file (a .nes file) to play.".to_string());
    } else {
        let p = Path::new(rom);
        if !p.is_file() {
            out.push(format!("The ROM file was not found: {rom}"));
        } else {
            match file_starts_with_ines(p) {
                Ok(true) => {}
                Ok(false) => out.push(format!(
                    "That file does not look like a NES ROM (.nes). Pick your Zelda II (USA) \
                     .nes file: {rom}"
                )),
                Err(e) => out.push(format!("The ROM file could not be read ({e}): {rom}")),
            }
        }
    }

    if !(1..=MAX_SCALE).contains(&s.scale) {
        out.push(format!("Window size must be 1x to {MAX_SCALE}x."));
    }

    let pack = s.hd_pack.trim();
    if !pack.is_empty() {
        let dir = Path::new(pack);
        if !dir.is_dir() {
            out.push(format!("The HD pack folder was not found: {pack}"));
        } else if !dir.join("pack.json").is_file() {
            out.push(format!(
                "That folder is not an HD pack (it has no pack.json inside): {pack}"
            ));
        }
        if !(1..=MAX_HD_SCALE).contains(&s.hd_scale) {
            out.push(format!("HD scale must be 1 to {MAX_HD_SCALE}."));
        }
    }

    if s.multiplayer.is_online() {
        let room = s.room.trim();
        if room.is_empty() {
            out.push("Type a room name for online play.".to_string());
        } else if room.chars().any(char::is_whitespace) {
            out.push("The room name cannot contain spaces.".to_string());
        } else if !room_name_valid(room) {
            out.push(format!(
                "The room name can only use letters, numbers, - and _ (up to {MAX_ROOM_LEN} \
                 characters)."
            ));
        }
        let url = s.signal_url.trim();
        if !(url.starts_with("ws://") || url.starts_with("wss://")) {
            out.push("The server address must start with ws:// or wss://.".to_string());
        } else if !signal_url_valid(url) {
            out.push(
                "The server address should look like wss://host or wss://host:port \
                 (no path)."
                    .to_string(),
            );
        }
        let max = s.net_mode.max_delay();
        if s.net_delay > max {
            out.push(format!(
                "Input delay must be 0 to {max} for {} mode.",
                s.net_mode.flag_value()
            ));
        }
    }
    out
}

/// An HD pack found under `<data-dir>/packs/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEntry {
    /// Folder name, shown in the dropdown.
    pub name: String,
    /// Folder path (the one holding `pack.json`).
    pub path: PathBuf,
}

/// Packs in `<packs_dir>/*/pack.json`, sorted by name. A missing folder
/// gives an empty list.
pub fn scan_packs(packs_dir: &Path) -> Vec<PackEntry> {
    let Ok(rd) = std::fs::read_dir(packs_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PackEntry> = rd
        .flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let root = find_pack_root(&p)?;
            Some(PackEntry {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: root,
            })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The folder holding `pack.json`: `dir` itself, or its only subfolder with
/// one. Unzipping a pack often adds that extra folder level, and players
/// then pick the outer folder.
pub fn find_pack_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("pack.json").is_file() {
        return Some(dir.to_path_buf());
    }
    let mut found = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("pack.json").is_file());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Quote one argument for display in a shell command line.
pub fn quote_arg(a: &str) -> String {
    let plain = !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=,+@%".contains(c));
    if plain {
        a.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", a.replace('"', "\\\""))
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// `program` and `args` as one copy-pasteable command line.
pub fn format_command(program: &str, args: &[String]) -> String {
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "z2-launcher-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A 16-byte fake header only (never real ROM data).
    fn fake_rom(dir: &Path) -> PathBuf {
        let p = dir.join("fake.nes");
        std::fs::write(&p, b"NES\x1a\x08\x10\x12\0\0\0\0\0\0\0\0\0").unwrap();
        p
    }

    #[test]
    fn default_args_are_minimal() {
        let s = Settings::default();
        assert_eq!(
            build_args(&s),
            vec![
                "--scale",
                "3",
                "--scale-mode",
                "fit",
                "--widescreen",
                "off",
                "--hd-pack",
                ""
            ]
        );
    }

    #[test]
    fn display_and_widescreen_args() {
        let s = Settings {
            rom_path: " /roms/z2.nes ".into(),
            scale: 4,
            fullscreen: true,
            widescreen: Widescreen::W16x9,
            wide_gameplay: false,
            margin_sprites: true,
            ..Settings::default()
        };
        assert_eq!(
            build_args(&s),
            vec![
                "--rom",
                "/roms/z2.nes",
                "--scale",
                "4",
                "--fullscreen",
                "--scale-mode",
                "fit",
                "--widescreen",
                "16:9",
                "--wide-gameplay",
                "off",
                "--margin-sprites",
                "on",
                "--hd-pack",
                ""
            ]
        );
    }

    #[test]
    fn hd_pack_args() {
        let s = Settings {
            hd_pack: "/packs/mine".into(),
            hd_scale: 2,
            widescreen: Widescreen::W16x10,
            ..Settings::default()
        };
        let a = build_args(&s);
        assert!(a.windows(2).any(|w| w == ["--widescreen", "16:10"]));
        assert!(a.windows(2).any(|w| w == ["--hd-pack", "/packs/mine"]));
        assert!(a.windows(2).any(|w| w == ["--hd-scale", "2"]));
    }

    #[test]
    fn local_coop_args() {
        let mut s = Settings {
            multiplayer: Multiplayer::Local,
            ..Settings::default()
        };
        assert!(build_args(&s).contains(&"--coop-local".to_string()));
        assert!(!build_args(&s).contains(&"--p2-pad".to_string()));
        s.p2_pad = Some(1);
        assert!(build_args(&s).windows(2).any(|w| w == ["--p2-pad", "1"]));
        // No online flags leak into local play.
        assert!(!build_args(&s).contains(&"--signal".to_string()));
    }

    #[test]
    fn online_args() {
        let mut s = Settings {
            multiplayer: Multiplayer::Host,
            room: "zelda-night".into(),
            net_mode: NetMode::Lockstep,
            net_delay: 5,
            ..Settings::default()
        };
        assert_eq!(
            build_args(&s)[8..],
            [
                "--coop-host",
                "zelda-night",
                "--signal",
                DEFAULT_SIGNAL_URL,
                "--net-mode",
                "lockstep",
                "--net-delay",
                "5"
            ]
        );
        s.multiplayer = Multiplayer::Join;
        s.ice = "none".into();
        let a = build_args(&s);
        assert!(a.windows(2).any(|w| w == ["--coop-join", "zelda-night"]));
        assert!(a.windows(2).any(|w| w == ["--ice", "none"]));
        assert!(!a.contains(&"--coop-host".to_string()));
    }

    #[test]
    fn validate_rom() {
        let d = temp_dir("rom");
        let mut s = Settings::default();
        assert_eq!(validate(&s).len(), 1, "missing ROM is reported");

        s.rom_path = d.join("nope.nes").to_string_lossy().into_owned();
        assert!(validate(&s)[0].contains("not found"));

        let bad = d.join("bad.nes");
        std::fs::write(&bad, b"not a rom").unwrap();
        s.rom_path = bad.to_string_lossy().into_owned();
        assert!(validate(&s)[0].contains("does not look like"));

        s.rom_path = fake_rom(&d).to_string_lossy().into_owned();
        assert!(validate(&s).is_empty(), "{:?}", validate(&s));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pack_root_is_found_one_level_down() {
        let d = temp_dir("pack-root");
        let outer = d.join("packs").join("mypack");
        let inner = outer.join("mypack-v1");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("pack.json"), "{}").unwrap();
        assert_eq!(find_pack_root(&outer), Some(inner.clone()));
        assert_eq!(find_pack_root(&inner), Some(inner.clone()));
        let packs = scan_packs(&d.join("packs"));
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "mypack");
        assert_eq!(packs[0].path, inner);
        // Two candidate subfolders are ambiguous: pick neither.
        let other = outer.join("mypack-v2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("pack.json"), "{}").unwrap();
        assert_eq!(find_pack_root(&outer), None);
    }

    #[test]
    fn validate_hd_pack() {
        let d = temp_dir("pack");
        let mut s = Settings {
            rom_path: fake_rom(&d).to_string_lossy().into_owned(),
            hd_pack: d.join("missing").to_string_lossy().into_owned(),
            ..Settings::default()
        };
        assert_eq!(validate(&s).len(), 1);
        let pack = d.join("mypack");
        std::fs::create_dir_all(&pack).unwrap();
        s.hd_pack = pack.to_string_lossy().into_owned();
        assert!(validate(&s)[0].contains("pack.json"));
        std::fs::write(pack.join("pack.json"), "{}").unwrap();
        assert!(validate(&s).is_empty());
        s.hd_scale = 9;
        assert_eq!(validate(&s).len(), 1);

        let found = scan_packs(&d);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "mypack");
        assert!(scan_packs(&d.join("absent")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn validate_online() {
        let d = temp_dir("online");
        let mut s = Settings {
            rom_path: fake_rom(&d).to_string_lossy().into_owned(),
            multiplayer: Multiplayer::Join,
            ..Settings::default()
        };
        assert_eq!(validate(&s).len(), 1, "empty room");
        s.room = "two words".into();
        assert!(validate(&s)[0].contains("spaces"));
        s.room = "caf\u{e9}".into();
        assert!(validate(&s)[0].contains("letters"));
        s.room = "room_1".into();
        assert!(validate(&s).is_empty());

        s.signal_url = "https://signal.z2rs.com".into();
        assert!(validate(&s)[0].contains("ws://"));
        s.signal_url = "wss://host/path".into();
        assert!(validate(&s)[0].contains("no path"));
        s.signal_url = "ws://127.0.0.1:3536".into();
        assert!(validate(&s).is_empty());

        s.net_delay = 4;
        assert_eq!(validate(&s).len(), 1, "rollback max is 3");
        s.net_mode = NetMode::Lockstep;
        assert!(validate(&s).is_empty());
        s.net_delay = 9;
        assert_eq!(validate(&s).len(), 1);

        // Offline play ignores the online fields entirely.
        s.multiplayer = Multiplayer::Local;
        s.room.clear();
        assert!(validate(&s).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn settings_round_trip_and_tolerate_old_files() {
        let d = temp_dir("save");
        let p = Settings::path_in(&d);
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            rom_path: "/x.nes".into(),
            widescreen: Widescreen::W16x10,
            multiplayer: Multiplayer::Host,
            p2_pad: Some(2),
            ..Settings::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        std::fs::write(&p, r#"{"rom_path":"/y.nes","future_field":1}"#).unwrap();
        let loaded = Settings::load(&p);
        assert_eq!(loaded.rom_path, "/y.nes");
        assert_eq!(loaded.signal_url, DEFAULT_SIGNAL_URL);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn command_line_quoting() {
        let args = vec!["--rom".to_string(), "/my roms/z2.nes".to_string()];
        let cmd = format_command("/bin/z2-native", &args);
        if cfg!(windows) {
            assert_eq!(cmd, "/bin/z2-native --rom \"/my roms/z2.nes\"");
        } else {
            assert_eq!(cmd, "/bin/z2-native --rom '/my roms/z2.nes'");
            assert_eq!(quote_arg("it's"), "'it'\\''s'");
        }
        assert_eq!(quote_arg("16:9"), "16:9");
        assert_eq!(quote_arg(""), if cfg!(windows) { "\"\"" } else { "''" });
    }
}
