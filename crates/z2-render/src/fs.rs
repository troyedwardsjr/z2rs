//! Native-only filesystem helpers (not compiled for wasm32).

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::pack::{HdPack, PackError};

/// Load the pack in directory `dir` (reads `pack.json`, then only the files
/// it references).
///
/// # Errors
/// [`PackError`]; unreadable files surface as `MissingFile` with the OS error.
pub fn load_pack_dir(dir: &Path) -> Result<HdPack, PackError> {
    HdPack::load(&|rel: &str| {
        let p = dir.join(rel);
        std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
    })
}

/// Write `(relative path, bytes)` files under `dir`, creating parents.
/// Returns the written paths in input order.
///
/// # Errors
/// `InvalidInput` for an absolute or `..` path; any I/O error otherwise.
pub fn write_files(dir: &Path, files: &[(String, Vec<u8>)]) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::with_capacity(files.len());
    for (rel, bytes) in files {
        let rp = Path::new(rel);
        if rel.is_empty()
            || !rp
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to write non-relative path \"{rel}\""),
            ));
        }
        let path = dir.join(rp);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
        written.push(path);
    }
    Ok(written)
}

/// Absolute form of `path` with symlinks resolved for every component that
/// exists; the non-existent tail is appended lexically.
///
/// # Errors
/// When the current directory is unavailable or canonicalisation fails.
pub fn resolve_path(path: &Path) -> io::Result<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut cur = PathBuf::new();
    let mut exists = true;
    for comp in abs.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => cur.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if exists {
                    cur = cur.join("..").canonicalize()?;
                } else {
                    cur.pop();
                }
            }
            Component::Normal(name) => {
                let next = cur.join(name);
                if exists && next.exists() {
                    cur = next.canonicalize()?;
                } else {
                    exists = false;
                    cur = next;
                }
            }
        }
    }
    Ok(cur)
}

/// The root of the git work tree containing `path` (existing or not), if any.
///
/// Tools that write ROM-derived output (template sheets) refuse such paths
/// unless explicitly overridden (LEGAL.md §1).
#[must_use]
pub fn enclosing_git_worktree(path: &Path) -> Option<PathBuf> {
    let abs = resolve_path(path).ok()?;
    abs.ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}
