//! Randomizer seam for the native frontend (`--seed`, `--rando-flags`,
//! `--rando-spoiler`, `--sprite-ips`).
//!
//! The verified vanilla body stays the single input everywhere (startup, a
//! ROM dropped on the window, a netplay restart). Every emulator build runs
//! it through [`RandoSpec::run`] when a spec is present, so the patched game
//! is always regenerated from the same seed and flags: nothing patched is
//! ever written to disk except the optional spoiler log.
//!
//! The spec is carried by [`crate::app::Features`], which is `Copy`; the spec
//! itself is created once per process and leaked into a `&'static`
//! ([`RandoSpec::leak`]) so every rebuild path shares it for free.

use std::path::{Path, PathBuf};

use z2_rando::flags::Flags;

/// What to randomize with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandoSpec {
    /// Seed text as typed.
    pub seed: String,
    /// Parsed options.
    pub flags: Flags,
    /// Where to write the spoiler log, if anywhere.
    pub spoiler_path: Option<PathBuf>,
    /// Bring-your-own sprite IPS bytes.
    pub sprite_ips: Option<Vec<u8>>,
}

impl RandoSpec {
    /// Build a spec from the CLI values. `None` when none of the randomizer
    /// flags was given. A missing `--rando-flags` means the vanilla flags; a
    /// missing `--seed` means the empty seed.
    ///
    /// # Errors
    /// An unparseable flag string, or a sprite IPS file that cannot be read.
    pub fn from_cli(
        seed: Option<&str>,
        flags: Option<&str>,
        spoiler: Option<&str>,
        sprite_ips: Option<&str>,
    ) -> Result<Option<RandoSpec>, String> {
        if seed.is_none() && flags.is_none() && sprite_ips.is_none() {
            if spoiler.is_some() {
                return Err("--rando-spoiler needs --seed and/or --rando-flags".to_string());
            }
            return Ok(None);
        }
        let flags = match flags {
            Some(s) => Flags::from_flag_string(s).map_err(|e| format!("--rando-flags: {e}"))?,
            None => Flags::default(),
        };
        let sprite_ips = match sprite_ips {
            Some(p) => Some(std::fs::read(p).map_err(|e| format!("--sprite-ips {p}: {e}"))?),
            None => None,
        };
        Ok(Some(RandoSpec {
            seed: seed.unwrap_or_default().to_string(),
            flags,
            spoiler_path: spoiler.map(PathBuf::from),
            sprite_ips,
        }))
    }

    /// Leak into a `&'static` so the `Copy` [`crate::app::Features`] can
    /// carry it (one spec per process; the leak is a few hundred bytes).
    #[must_use]
    pub fn leak(self) -> &'static RandoSpec {
        Box::leak(Box::new(self))
    }

    /// Randomize `vanilla` (already verified by the caller) and write the
    /// spoiler when asked for one.
    ///
    /// # Errors
    /// The randomizer's own error, as text.
    pub fn run(&self, vanilla: &[u8]) -> Result<z2_rando::Output, String> {
        let extras = z2_rando::Extras {
            sprite_ips: self.sprite_ips.clone(),
        };
        let out = z2_rando::randomize_with(vanilla, &self.seed, &self.flags, &extras)
            .map_err(|e| format!("randomizer: {e}"))?;
        if let Some(path) = &self.spoiler_path {
            write_spoiler(path, &out.spoiler);
        }
        Ok(out)
    }
}

fn write_spoiler(path: &Path, text: &str) {
    match std::fs::write(path, text) {
        Ok(()) => eprintln!("randomizer: spoiler written to {}", path.display()),
        Err(e) => eprintln!(
            "randomizer: could not write spoiler {}: {e}",
            path.display()
        ),
    }
}

/// Battery-RAM file name for a game: `sram.sav` for the vanilla game,
/// `sram-<HASH>.sav` for a randomized seed (so seeds never share saves).
/// `coop` selects the netplay-guest variant.
#[must_use]
pub fn sram_file_name(hash: Option<&str>, coop: bool) -> String {
    let base = if coop { "sram-coop" } else { "sram" };
    match hash {
        Some(h) => format!("{base}-{h}.sav"),
        None => format!("{base}.sav"),
    }
}

/// Save-state file name for `slot`: `savestate<N>.z2snap` for the vanilla
/// game, `savestate-<HASH>-<N>.z2snap` for a randomized seed. Save states do
/// not record which ROM they came from, so the name is what keeps seeds
/// apart.
#[must_use]
pub fn savestate_file_name(hash: Option<&str>, slot: u8) -> String {
    match hash {
        Some(h) => format!("savestate-{h}-{slot}.z2snap"),
        None => format!("savestate{slot}.z2snap"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_flags_no_spec() {
        assert_eq!(RandoSpec::from_cli(None, None, None, None), Ok(None));
        assert!(RandoSpec::from_cli(None, None, Some("s.txt"), None).is_err());
    }

    #[test]
    fn parses_flags_and_defaults() {
        let s = RandoSpec::from_cli(Some("abc"), None, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(s.seed, "abc");
        assert!(s.flags.is_vanilla());
        let std = z2_rando::flags::Preset::Standard.flags();
        let s = RandoSpec::from_cli(None, Some(&std.to_flag_string()), Some("x.txt"), None)
            .unwrap()
            .unwrap();
        assert_eq!(s.flags, std);
        assert_eq!(s.seed, "");
        assert_eq!(s.spoiler_path, Some(PathBuf::from("x.txt")));
        assert!(RandoSpec::from_cli(Some("a"), Some("not-flags"), None, None).is_err());
        assert!(RandoSpec::from_cli(None, None, None, Some("/no/such/file.ips")).is_err());
    }

    #[test]
    fn file_names_split_by_seed() {
        assert_eq!(sram_file_name(None, false), "sram.sav");
        assert_eq!(sram_file_name(None, true), "sram-coop.sav");
        assert_eq!(sram_file_name(Some("AB12CD"), false), "sram-AB12CD.sav");
        assert_eq!(sram_file_name(Some("AB12CD"), true), "sram-coop-AB12CD.sav");
        assert_eq!(savestate_file_name(None, 3), "savestate3.z2snap");
        assert_eq!(savestate_file_name(Some("X"), 3), "savestate-X-3.z2snap");
    }
}
