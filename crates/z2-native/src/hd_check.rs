//! Up-front HD pack check for frontends that install packs themselves.
//!
//! The Android launcher imports a pack (a `.zip` or a folder) into the app's
//! private storage and calls [`check_reply`] over JNI before it keeps it, so a
//! broken pack is rejected in the launcher with the loader's own message
//! instead of stopping the game at start. The check is the game's loader
//! ([`z2_render::fs::load_pack_dir`]): every sheet and layer `pack.json`
//! names is read and decoded, exactly as [`crate::app::Display::new`] does.

use std::path::Path;

/// What a pack that loaded is, for the launcher's status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSummary {
    /// `pack.json` `name`.
    pub name: String,
    /// HD pixels per NES pixel.
    pub scale: u32,
    /// Distinct CHR tiles the pack replaces.
    pub tiles: usize,
    /// Scene layers.
    pub layers: usize,
}

/// Load the pack in `dir` (the directory holding `pack.json`) the way the
/// game will, and summarise it.
///
/// # Errors
/// The loader's message (it names the file and the `pack.json` entry).
pub fn check_pack_dir(dir: &Path) -> Result<PackSummary, String> {
    if !dir.join(z2_render::PACK_MANIFEST).is_file() {
        return Err(format!(
            "no {} in {}",
            z2_render::PACK_MANIFEST,
            dir.display()
        ));
    }
    let pack = z2_render::fs::load_pack_dir(dir).map_err(|e| e.to_string())?;
    Ok(PackSummary {
        name: pack.name().to_string(),
        scale: pack.scale(),
        tiles: pack.tile_count(),
        layers: pack.layers().len(),
    })
}

/// [`check_pack_dir`] as one line of tab-separated fields, for callers across
/// a string-only boundary (the Android launcher's `NativeBridge.checkHdPack`):
///
/// * `ok\t<name>\t<scale>\t<tiles>\t<layers>`
/// * `error\t<message>`
///
/// Tabs and line breaks inside the name or message become spaces, so the
/// field count is fixed.
#[must_use]
pub fn check_reply(dir: &Path) -> String {
    match check_pack_dir(dir) {
        Ok(s) => format!(
            "ok\t{}\t{}\t{}\t{}",
            one_field(&s.name),
            s.scale,
            s.tiles,
            s.layers
        ),
        Err(e) => format!("error\t{}", one_field(&e)),
    }
}

fn one_field(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "z2-hdcheck-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Scale-1 page sheet (16x16 cells of 8 px) with tile 0x42 painted red.
    fn one_tile_page_sheet() -> Vec<u8> {
        let (w, h) = (128u32, 128u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let (cx, cy) = (2 * 8, 4 * 8); // tile 0x42: column 2, row 4
        for y in cy..cy + 8 {
            for x in cx..cx + 8 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        z2_render::encode_png_rgba(w, h, &rgba, &[]).unwrap()
    }

    #[test]
    fn a_valid_pack_is_summarised() {
        let d = temp_dir("ok");
        std::fs::create_dir_all(d.join("sheets")).unwrap();
        std::fs::write(
            d.join("pack.json"),
            r#"{"format":"z2rs-hdpack","version":1,"name":"My\tPack","scale":1,
                "sheets":[{"file":"sheets/p5.png","page":5}]}"#,
        )
        .unwrap();
        std::fs::write(d.join("sheets/p5.png"), one_tile_page_sheet()).unwrap();
        let s = check_pack_dir(&d).expect("valid pack");
        assert_eq!((s.scale, s.tiles, s.layers), (1, 1, 0));
        assert_eq!(check_reply(&d), "ok\tMy Pack\t1\t1\t0");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_manifest_is_an_error() {
        let d = temp_dir("nomanifest");
        let reply = check_reply(&d);
        assert!(reply.starts_with("error\tno pack.json"), "{reply}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn loader_errors_come_through_on_one_line() {
        let d = temp_dir("bad");
        std::fs::write(
            d.join("pack.json"),
            r#"{"version":2,"name":"t","scale":1,"sheets":[]}"#,
        )
        .unwrap();
        let reply = check_reply(&d);
        assert!(reply.starts_with("error\t"), "{reply}");
        assert!(reply.contains("version"), "{reply}");

        // A sheet pack.json names but the folder lacks.
        std::fs::write(
            d.join("pack.json"),
            r#"{"version":1,"name":"t","scale":1,"sheets":[{"file":"sheets/p5.png","page":5}]}"#,
        )
        .unwrap();
        let reply = check_reply(&d);
        assert!(
            reply.starts_with("error\t") && reply.contains("p5.png"),
            "{reply}"
        );
        assert!(!reply.contains('\n'), "{reply}");

        std::fs::write(d.join("pack.json"), "{ not json").unwrap();
        assert!(check_reply(&d).starts_with("error\t"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
