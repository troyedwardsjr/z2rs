//! Shared gating helpers for ROM/corpus-dependent tests (test-only).
//!
//! A gate variable (`Z2_ROM`, `Z2_CORPUS`, `Z2_MOVIES`, ...) counts as
//! **absent** when it is unset, empty, **or** does not name an existing
//! file/directory. The empty case is the one that used to bite: public CI
//! exported `Z2_ROM=""`, which an `is_err()`-style guard reads as "ROM
//! present", so the test ran and panicked instead of skipping.
//!
//! Every helper prints one `skipping <test>: ...` line and returns `None`
//! when the gate is not satisfied. When it *is* satisfied the caller runs
//! and asserts in full -- these helpers only decide whether to run, never
//! what to assert.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Value of `key`, treating unset and empty (or all-whitespace) alike as
/// absent. Use this instead of `env::var(..).ok()` in any gate.
pub fn var_present(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Path named by `key`, which must be an existing file.
pub fn env_file(key: &str, test: &str) -> Option<PathBuf> {
    let Some(v) = var_present(key) else {
        eprintln!("skipping {test}: {key} not set to an existing file");
        return None;
    };
    let p = PathBuf::from(v);
    if p.is_file() {
        return Some(p);
    }
    eprintln!(
        "skipping {test}: {key} ({}) is not an existing file",
        p.display()
    );
    None
}

/// `path` when it is an existing file, else skip.
pub fn file_present(path: &Path, test: &str) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    eprintln!(
        "skipping {test}: {} is not an existing file",
        path.display()
    );
    None
}

/// The reference ROM path (`$Z2_ROM`), read-only and never copied.
pub fn rom_path(test: &str) -> Option<PathBuf> {
    env_file("Z2_ROM", test)
}

/// Contents of `$Z2_ROM`.
pub fn rom_bytes(test: &str) -> Option<Vec<u8>> {
    let p = rom_path(test)?;
    match std::fs::read(&p) {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("skipping {test}: cannot read {} ({e})", p.display());
            None
        }
    }
}

/// Directory named by `key`, falling back to `default` when `key` is absent;
/// `None` unless the directory exists.
pub fn env_dir(key: &str, default: &str, test: &str) -> Option<PathBuf> {
    let p = PathBuf::from(var_present(key).unwrap_or_else(|| default.to_string()));
    if p.is_dir() {
        return Some(p);
    }
    eprintln!(
        "skipping {test}: {key} ({}) is not an existing directory",
        p.display()
    );
    None
}

/// [`env_dir`], opened for reading.
pub fn env_read_dir(key: &str, default: &str, test: &str) -> Option<std::fs::ReadDir> {
    let dir = env_dir(key, default, test)?;
    match std::fs::read_dir(&dir) {
        Ok(rd) => Some(rd),
        Err(e) => {
            eprintln!("skipping {test}: cannot read {} ({e})", dir.display());
            None
        }
    }
}

/// The out-of-tree snapshot corpus (`$Z2_CORPUS`, else `corpus/snapshots`).
pub fn corpus_snapshots(test: &str) -> Option<std::fs::ReadDir> {
    env_read_dir("Z2_CORPUS", "corpus/snapshots", test)
}

/// File path named by `key`, falling back to `default` when `key` is absent;
/// `None` unless the file exists (a missing movie skips, never panics).
pub fn env_path_file(key: &str, default: &str, test: &str) -> Option<PathBuf> {
    let p = PathBuf::from(var_present(key).unwrap_or_else(|| default.to_string()));
    file_present(&p, test)
}
