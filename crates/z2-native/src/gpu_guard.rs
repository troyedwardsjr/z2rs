//! Crash-loop guard for graphics backend start-up: `<data-dir>/gpu-backend-guard.txt`.
//!
//! An access violation inside a graphics driver (the AMD OpenGL ICD
//! `atio6axx.dll` in one v0.4.0 report) is not a Rust panic: `catch_unwind`
//! never sees it, the process dies, and the next launch would try the same
//! backend and die the same way. So before each backend attempt
//! [`crate::gpu_present::create_pixels`] writes `attempting <label>` to this
//! file, and the line is cleared once the first frame has been presented
//! ([`confirm`]). A launch that finds an `attempting` line knows the previous
//! run died inside that backend, records it as `crashed <label>` and, when
//! the backend list was chosen automatically, skips it from then on.
//!
//! A crashed backend stays skipped until it works again: naming it in
//! `gpu_backend` (or `WGPU_BACKEND`) tries it anyway, and a successful start
//! removes it from the list. Deleting the file resets everything.
//!
//! The launcher reads the same file (format: one `crashed <label>` line per
//! backend, then at most one `attempting <label>` line) to explain what the
//! game is doing.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Guard file name inside the data dir (next to `z2-native.log`).
pub const GUARD_FILE_NAME: &str = "gpu-backend-guard.txt";

/// What the guard file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardState {
    /// Backends that killed an earlier run while starting.
    pub crashed: Vec<String>,
    /// The backend being started right now (or, read at launch, the one the
    /// previous run died in).
    pub attempting: Option<String>,
}

impl GuardState {
    /// Parse the file. Unknown lines are ignored.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(l) = line.strip_prefix("crashed ") {
                let l = l.trim();
                if !l.is_empty() && !s.crashed.iter().any(|c| c == l) {
                    s.crashed.push(l.to_string());
                }
            } else if let Some(l) = line.strip_prefix("attempting ") {
                let l = l.trim();
                if !l.is_empty() {
                    s.attempting = Some(l.to_string());
                }
            }
        }
        s
    }

    /// The file text for this state.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for c in &self.crashed {
            out.push_str("crashed ");
            out.push_str(c);
            out.push('\n');
        }
        if let Some(a) = &self.attempting {
            out.push_str("attempting ");
            out.push_str(a);
            out.push('\n');
        }
        out
    }

    /// At launch: an unfinished attempt means the previous run died in it.
    /// Moves it to [`Self::crashed`] and returns it.
    pub fn take_crash(&mut self) -> Option<String> {
        let a = self.attempting.take()?;
        if !self.crashed.contains(&a) {
            self.crashed.push(a.clone());
        }
        Some(a)
    }

    /// `label` started fine: forget any earlier crash of it.
    pub fn succeeded(&mut self, label: &str) {
        self.attempting = None;
        self.crashed.retain(|c| c != label);
    }
}

/// `attempts` without the backends in `crashed`, plus the labels skipped.
///
/// Only an automatic list (`automatic`, more than one entry) is filtered: a
/// backend the player named is always tried. When every entry has crashed
/// before, nothing is skipped (better to try again than to give up without
/// trying).
#[must_use]
pub fn filter_attempts<T: Copy>(
    attempts: &[(&'static str, T)],
    crashed: &[String],
    automatic: bool,
) -> (Vec<(&'static str, T)>, Vec<&'static str>) {
    if !automatic || attempts.len() < 2 {
        return (attempts.to_vec(), Vec::new());
    }
    let (skipped, kept): (Vec<_>, Vec<_>) = attempts
        .iter()
        .copied()
        .partition(|(l, _)| crashed.iter().any(|c| c == l));
    if kept.is_empty() {
        return (attempts.to_vec(), Vec::new());
    }
    (kept, skipped.into_iter().map(|(l, _)| l).collect())
}

/// The live guard: the file path and the state last written to it.
#[derive(Debug)]
pub struct Guard {
    path: Option<PathBuf>,
    state: GuardState,
}

impl Guard {
    /// Read the guard in `dir` (`None`: keep it in memory only). Returns
    /// the guard and the backend the previous run died in, if any (already
    /// recorded as crashed and written back).
    pub fn open(dir: Option<&Path>) -> (Self, Option<String>) {
        let path = dir.map(|d| d.join(GUARD_FILE_NAME));
        let mut state = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| GuardState::parse(&t))
            .unwrap_or_default();
        let crashed = state.take_crash();
        let g = Self { path, state };
        if crashed.is_some() {
            g.write();
        }
        (g, crashed)
    }

    /// Backends recorded as crashed.
    #[must_use]
    pub fn crashed(&self) -> &[String] {
        &self.state.crashed
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> &GuardState {
        &self.state
    }

    /// About to start `label`: written (and flushed) before any driver call.
    pub fn begin(&mut self, label: &str) {
        self.state.attempting = Some(label.to_string());
        self.write();
    }

    /// `label` failed cleanly (an error or a caught panic, not a crash).
    pub fn failed(&mut self) {
        self.state.attempting = None;
        self.write();
    }

    /// The backend being attempted works (first frame presented). Returns
    /// `true` when there was an attempt to confirm.
    pub fn confirm(&mut self) -> bool {
        let Some(a) = self.state.attempting.clone() else {
            return false;
        };
        self.state.succeeded(&a);
        self.write();
        true
    }

    fn write(&self) {
        let Some(p) = &self.path else { return };
        let text = self.state.render();
        let res = if text.is_empty() {
            match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            write_synced(p, &text)
        };
        if let Err(e) = res {
            crate::diag::breadcrumb(format_args!(
                "gpu guard: could not write {}: {e}",
                p.display()
            ));
        }
    }
}

/// Write `text` to `path` and flush it to disk, so a crash right after
/// still leaves the line behind.
fn write_synced(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

static GUARD: Mutex<Option<Guard>> = Mutex::new(None);

/// Run `f` on the process-wide guard, opening it next to the breadcrumb log
/// ([`crate::diag::log_path`]) on first use. The second value is the backend
/// the previous run died in, reported only on the call that opened it.
pub fn with_guard<R>(f: impl FnOnce(&mut Guard, Option<String>) -> R) -> R {
    let mut slot = GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut crashed_last = None;
    if slot.is_none() {
        let dir = crate::diag::log_path().and_then(|p| p.parent().map(Path::to_path_buf));
        let (g, c) = Guard::open(dir.as_deref());
        crashed_last = c;
        *slot = Some(g);
    }
    let g = slot.as_mut().expect("guard just opened");
    f(g, crashed_last)
}

/// The first frame was presented: the backend being started works.
pub fn confirm() {
    let confirmed = with_guard(|g, _| g.confirm());
    if confirmed {
        crate::diag::breadcrumb("gpu: backend confirmed working (crash guard cleared)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_render_round_trip() {
        let s = GuardState {
            crashed: vec!["GL".into(), "Vulkan".into()],
            attempting: Some("DX12".into()),
        };
        let text = s.render();
        assert_eq!(text, "crashed GL\ncrashed Vulkan\nattempting DX12\n");
        assert_eq!(GuardState::parse(&text), s);
        assert_eq!(GuardState::parse(""), GuardState::default());
        // Junk and duplicates are tolerated.
        let p = GuardState::parse("hello\ncrashed GL\r\ncrashed GL\nattempting \n");
        assert_eq!(p.crashed, vec!["GL".to_string()]);
        assert_eq!(p.attempting, None);
    }

    #[test]
    fn unfinished_attempt_becomes_a_crash() {
        let mut s = GuardState::parse("attempting GL\n");
        assert_eq!(s.take_crash().as_deref(), Some("GL"));
        assert_eq!(s.crashed, vec!["GL".to_string()]);
        assert_eq!(s.attempting, None);
        // Nothing pending: nothing crashed.
        assert_eq!(s.take_crash(), None);
        // Crashing again does not duplicate the entry.
        s.attempting = Some("GL".into());
        assert_eq!(s.take_crash().as_deref(), Some("GL"));
        assert_eq!(s.crashed.len(), 1);
        // A later success clears it.
        s.succeeded("GL");
        assert!(s.crashed.is_empty());
    }

    #[test]
    fn filter_skips_crashed_only_in_automatic_lists() {
        let list = [("DX12", 1u8), ("Vulkan", 2u8)];
        let crashed = vec!["DX12".to_string()];
        let (kept, skipped) = filter_attempts(&list, &crashed, true);
        assert_eq!(kept, vec![("Vulkan", 2)]);
        assert_eq!(skipped, vec!["DX12"]);
        // Named by the player: tried anyway.
        let (kept, skipped) = filter_attempts(&list[..1], &crashed, false);
        assert_eq!(kept, vec![("DX12", 1)]);
        assert!(skipped.is_empty());
        // A one-entry automatic list (wgpu picks) is never emptied.
        assert_eq!(filter_attempts(&list[..1], &crashed, true).0.len(), 1);
        // Everything crashed before: try them all again.
        let all = vec!["DX12".to_string(), "Vulkan".to_string()];
        let (kept, skipped) = filter_attempts(&list, &all, true);
        assert_eq!(kept.len(), 2);
        assert!(skipped.is_empty());
    }

    #[test]
    fn guard_file_lifecycle() {
        let dir = std::env::temp_dir().join(format!("z2-gpu-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(GUARD_FILE_NAME);

        // First run: nothing recorded; the attempt is on disk before the
        // driver is touched.
        let (mut g, crashed) = Guard::open(Some(&dir));
        assert_eq!(crashed, None);
        g.begin("DX12");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "attempting DX12\n");
        // ... and the process dies here.
        drop(g);

        // Next run sees the crash, records it and skips DX12.
        let (mut g, crashed) = Guard::open(Some(&dir));
        assert_eq!(crashed.as_deref(), Some("DX12"));
        assert_eq!(g.crashed(), ["DX12".to_string()]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "crashed DX12\n");
        g.begin("Vulkan");
        g.failed();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "crashed DX12\n");
        g.begin("Vulkan");
        assert!(g.confirm());
        assert!(!g.confirm(), "nothing left to confirm");
        // Vulkan worked; DX12 stays skipped.
        let (g, crashed) = Guard::open(Some(&dir));
        assert_eq!(crashed, None);
        assert_eq!(g.crashed(), ["DX12".to_string()]);
        drop(g);

        // DX12 named explicitly and working again clears the file.
        let (mut g, _) = Guard::open(Some(&dir));
        g.begin("DX12");
        assert!(g.confirm());
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_only_guard_still_tracks_state() {
        let (mut g, crashed) = Guard::open(None);
        assert_eq!(crashed, None);
        g.begin("GL");
        assert_eq!(g.state().attempting.as_deref(), Some("GL"));
        assert!(g.confirm());
    }
}
