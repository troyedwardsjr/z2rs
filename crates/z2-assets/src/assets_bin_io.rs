//! `std` file-I/O helpers around the extractor core.
//!
//! This is the only extractor module that touches `std` file APIs, so
//! `extract` / `extract_tables` / `assets_bin` stay `no_std`-friendly and
//! `wasm32`-compatible (extraction runs in the browser tab in `z2-web`; the
//! web frontend ships the ROM bytes from IndexedDB and calls
//! [`crate::extract::extract`] directly).
//!
//! ROM loading reuses the ROM hash gate: [`crate::rom::open`] (via
//! `Z2_ROM`) and [`crate::rom::open_at`] (explicit path). Headered files are
//! accepted — [`crate::rom::strip_ines_header`] is applied before slicing.

use std::fmt;
use std::fs;
use std::path::Path;

use crate::assets_bin;
use crate::extract::{self, ExtractError};
use crate::rom::{self, RomError};

/// File-layer failure (read ROM, extract, write `assets.bin`).
#[derive(Debug)]
pub enum IoError {
    /// The ROM file could not be read.
    Read { path: String, msg: String },
    /// The ROM hash gate rejected the input.
    Rom(RomError),
    /// Slicing / validation failed.
    Extract(ExtractError),
    /// `assets.bin` could not be written.
    Write { path: String, msg: String },
}

impl From<RomError> for IoError {
    fn from(e: RomError) -> Self {
        IoError::Rom(e)
    }
}

impl From<ExtractError> for IoError {
    fn from(e: ExtractError) -> Self {
        IoError::Extract(e)
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::Read { path, msg } => {
                write!(f, "could not read ROM file {path}: {msg}")
            }
            IoError::Rom(e) => write!(f, "{e}"),
            IoError::Extract(e) => write!(f, "{e}"),
            IoError::Write { path, msg } => {
                write!(f, "could not write assets file {path}: {msg}")
            }
        }
    }
}

impl std::error::Error for IoError {}

/// Extraction summary returned to CLI callers (e.g. `xtask extract`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Sections written, in table order.
    pub sections: usize,
    /// Total `assets.bin` bytes.
    pub bytes: usize,
    /// CRC32 of the source ROM body.
    pub body_crc32: u32,
    /// SHA1 (hex) of the source ROM body.
    pub body_sha1_hex: String,
    /// SHA1 (hex) of the emitted `assets.bin` (determinism fingerprint).
    pub assets_sha1_hex: String,
}

/// Read a ROM file (headered or bare), verify it, and encode `assets.bin`
/// bytes. Pure byte pipeline behind [`crate::rom::open_at`].
pub fn extract_rom_file_to_vec(path: &Path) -> Result<(Vec<u8>, Summary), IoError> {
    let file = fs::read(path).map_err(|e| IoError::Read {
        path: path.display().to_string(),
        msg: e.to_string(),
    })?;
    // Reuse the gate's own header handling; verify + slice the body.
    let body = rom::strip_ines_header(&file);
    let extracted = extract::extract(body)?;
    let image = assets_bin::encode(
        &extracted.sections,
        extracted.body_crc32,
        extracted.body_sha1,
    );
    let summary = Summary {
        sections: extracted.sections.len(),
        bytes: image.len(),
        body_crc32: extracted.body_crc32,
        body_sha1_hex: rom::sha1_hex(body),
        assets_sha1_hex: rom::sha1_hex(&image),
    };
    Ok((image, summary))
}

/// Extract `--rom PATH` to `--out PATH` and print the summary.
///
/// Returns the [`Summary`] on success so `xtask extract` can assert on it in
/// tests without scraping stdout.
pub fn run(rom_path: &Path, out_path: &Path) -> Result<Summary, IoError> {
    let (image, summary) = extract_rom_file_to_vec(rom_path)?;
    fs::write(out_path, &image).map_err(|e| IoError::Write {
        path: out_path.display().to_string(),
        msg: e.to_string(),
    })?;
    println!(
        "extract: {} sections, {} bytes -> {} (rom crc32 {:08X}, rom sha1 {}, assets sha1 {})",
        summary.sections,
        summary.bytes,
        out_path.display(),
        summary.body_crc32,
        summary.body_sha1_hex,
        summary.assets_sha1_hex,
    );
    Ok(summary)
}

/// Entry point behind the `xtask extract` subcommand
/// (`tools/xtask/src/main.rs` calls `extract_main(&rest) as u8`).
///
/// Usage: `cargo xtask extract --rom $Z2_ROM --out assets.bin`.
/// `--rom` defaults to the `Z2_ROM` env var; `--out` defaults to
/// `assets.bin` in the current directory.
pub fn extract_main(args: &[String]) -> i32 {
    let mut rom: Option<String> = None;
    let mut out: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rom" => {
                i += 1;
                rom = args.get(i).cloned();
            }
            "--out" => {
                i += 1;
                out = args.get(i).cloned();
            }
            "-h" | "--help" | "help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("xtask extract: unexpected argument '{other}'\n\n{USAGE}");
                return 2;
            }
        }
        i += 1;
    }

    let rom_path = match rom {
        Some(p) => p,
        None => match std::env::var(rom::ROM_ENV_VAR) {
            Ok(p) => p,
            Err(_) => {
                eprintln!(
                    "xtask extract: no --rom given and {} is not set",
                    rom::ROM_ENV_VAR
                );
                return 2;
            }
        },
    };
    let out_path = out.unwrap_or_else(|| "assets.bin".to_owned());

    // `--rom` may name a headered or bare file; `open_at` enforces the gate
    // uniformly, then `run` performs the identical pipeline on the path.
    if let Err(e) = rom::open_at(Path::new(&rom_path)) {
        eprintln!("xtask extract: {e}");
        return 1;
    }
    match run(Path::new(&rom_path), Path::new(&out_path)) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("xtask extract: {e}");
            1
        }
    }
}

const USAGE: &str = "z2rs xtask extract\n\
     \n\
     Extract assets.bin from a user-supplied Zelda II ROM.\n\
     \n\
     USAGE:\n\
     \x20   cargo xtask extract --rom $Z2_ROM --out assets.bin\n\
     \n\
     OPTIONS:\n\
     \x20   --rom PATH   ROM file (default: $Z2_ROM)\n\
     \x20   --out PATH   output file (default: ./assets.bin)\n";
