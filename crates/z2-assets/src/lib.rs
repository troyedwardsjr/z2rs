//! `z2-assets`: asset extractor (user-supplied ROM -> `assets.bin`).
//!
//! The ROM hash gate lives in [`rom`]. The extractor builds on top of it:
//! add new modules here and re-export them, but leave `rom.rs` alone.
//!
//! See `LEGAL.md`: no ROM-derived bytes may be committed to this repo.

pub mod assets_bin;
pub mod assets_bin_io;
pub mod extract;
pub mod extract_tables;
pub mod rom;
