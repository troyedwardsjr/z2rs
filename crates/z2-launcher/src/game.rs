//! Finding the game program and running it.
//!
//! The game's standard output and error go to `<data-dir>/last-game.log`
//! rather than a pipe: the game keeps running (and printing) after the
//! launcher closes, and a write into a pipe nobody reads any more would stop
//! it. The file also survives for bug reports.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Environment variable that names the game program explicitly.
pub const GAME_BIN_ENV: &str = "Z2RS_GAME_BIN";
/// Game output log file name inside the data dir.
pub const GAME_LOG_FILE_NAME: &str = "last-game.log";
/// How many trailing log lines an error shows.
pub const ERROR_TAIL_LINES: usize = 20;
/// The game's own startup / crash breadcrumb log inside the data dir (kept
/// in step with `z2_native::diag::LOG_FILE_NAME`).
pub const GAME_CRASH_LOG_FILE_NAME: &str = "z2-native.log";
/// The game's graphics-backend crash guard inside the data dir (kept in
/// step with `z2_native::gpu_guard::GUARD_FILE_NAME`): `crashed <label>`
/// lines for backends that killed a run while starting, then at most one
/// `attempting <label>` line, left behind when the run died in that backend.
pub const GPU_GUARD_FILE_NAME: &str = "gpu-backend-guard.txt";
/// The backends the game tries on its own on Windows, in order (kept in
/// step with `z2_native::gpu_present::backend_attempts`).
pub const AUTO_GPU_BACKENDS: [&str; 2] = ["DX12", "Vulkan"];

/// File names of the vendor OpenGL drivers (ICDs) `opengl32.dll` loads.
const GL_ICD_MODULES: [&str; 5] = [
    "atio6axx.dll",
    "atioglxx.dll",
    "nvoglv64.dll",
    "ig9icd64.dll",
    "ig75icd64.dll",
];

/// What the launcher should say about the game's graphics-backend guard
/// file (`text`), or `None` when there is nothing to say.
///
/// A leftover `attempting` line means the last run died while starting that
/// backend; the game will skip it (when it chooses the backend itself) and
/// try the next one.
#[must_use]
pub fn gpu_guard_notice(text: &str) -> Option<String> {
    let mut crashed: Vec<&str> = Vec::new();
    let mut attempting = None;
    for line in text.lines().map(str::trim) {
        if let Some(l) = line.strip_prefix("crashed ").map(str::trim) {
            if !l.is_empty() && !crashed.contains(&l) {
                crashed.push(l);
            }
        } else if let Some(l) = line.strip_prefix("attempting ").map(str::trim) {
            if !l.is_empty() {
                attempting = Some(l);
            }
        }
    }
    if let Some(a) = attempting {
        if !crashed.contains(&a) {
            crashed.push(a);
        }
    }
    let next: Vec<&str> = AUTO_GPU_BACKENDS
        .iter()
        .copied()
        .filter(|b| !crashed.contains(b))
        .collect();
    let auto_plan = if next.is_empty() {
        "every automatic backend has crashed before, so the game will try them all again"
            .to_string()
    } else {
        format!(
            "with \"gpu_backend\" on \"auto\" the game tries {} instead",
            next.join(", then ")
        )
    };
    match attempting {
        Some(a) if a == "GL" || !AUTO_GPU_BACKENDS.contains(&a) => Some(format!(
            "Graphics backend {a} crashed the game last time while starting. It was \
             chosen in \"gpu_backend\" (z2-native.json) or WGPU_BACKEND, so the game \
             will try it again; set \"gpu_backend\" back to \"auto\" ({auto_plan})."
        )),
        Some(a) => Some(format!(
            "Graphics backend {a} crashed the game last time while starting; {auto_plan}."
        )),
        None if crashed.is_empty() => None,
        None => Some(format!(
            "Skipping graphics backend {}, which crashed the game before; {auto_plan}. \
             Delete {GPU_GUARD_FILE_NAME} in the data folder to try it again.",
            crashed.join(", ")
        )),
    }
}

/// A graphics-specific explanation of a crash, from the game's output
/// (`log_text`: its stderr and/or `z2-native.log`), or `None`.
///
/// Recognizes a fault inside a vendor OpenGL driver (named by the game's
/// crash logger, or the last backend breadcrumb being GL) and a crash while
/// any backend was still starting (no `first frame presented` after the
/// last `trying backend` line).
#[must_use]
pub fn gpu_crash_hint(log_text: &str) -> Option<String> {
    let lower = log_text.to_ascii_lowercase();
    let icd = GL_ICD_MODULES.iter().find(|m| {
        lower
            .lines()
            .any(|l| l.contains("fatal:") && l.contains(*m))
    });
    let mut trying: Option<String> = None;
    let mut started = false;
    let mut using_gl = false;
    for line in log_text.lines() {
        if let Some(rest) = line.split("gpu: trying backend ").nth(1) {
            trying = rest.split_whitespace().next().map(str::to_string);
            started = false;
            using_gl = false;
        } else if line.contains("gpu: using '") {
            using_gl = line.contains(" via Gl ");
        } else if line.contains("gpu: first frame presented") {
            started = true;
        }
    }
    let gl = icd.is_some() || using_gl || trying.as_deref() == Some("GL");
    if gl {
        let module = icd.map_or_else(String::new, |m| format!(" ({m})"));
        return Some(format!(
            "The crash looks like it came from the OpenGL graphics driver{module}: the \
             vendor OpenGL ICD, atio6axx.dll on AMD, nvoglv64.dll on NVIDIA, \
             ig9icd64.dll on Intel. Set \"gpu_backend\" to \"auto\" or \"dx12\" in \
             z2-native.json so the game uses DirectX 12 instead, set the monitor to \
             a standard refresh rate (60, 120, 144 Hz...), and update the graphics \
             driver."
        ));
    }
    match trying {
        Some(t) if !started => Some(format!(
            "The game died while starting graphics backend {t}. With \"gpu_backend\" \
             on \"auto\" it skips {t} next time and tries the next one; updating the \
             graphics driver or turning off overlays may make {t} work again."
        )),
        _ => None,
    }
}

/// A plain-language explanation of a crash exit code, or `None` for an
/// ordinary one.
///
/// Windows reports a process killed by an exception with the NTSTATUS code
/// as its exit code, which `ExitStatus::code` shows as a large negative
/// number (0xC0000005 is -1073741819). 101 is Rust's exit code after a
/// panic, on every platform.
#[must_use]
pub fn explain_exit_code(code: i32) -> Option<&'static str> {
    Some(match code as u32 {
        101 => {
            "The game hit an internal error (a Rust panic). The message is in \
             the log below and in z2-native.log."
        }
        0xC000_0005 => {
            "The game crashed with an access violation (0xC0000005) in native \
             code, most often a graphics, audio or controller driver, or an \
             overlay (Steam, Discord, MSI Afterburner / RivaTuner, OBS, \
             ReShade). z2-native.log says which part was starting and, for a \
             crash, which DLL it was in. Updating the graphics driver or \
             turning overlays off often helps; setting \"gpu_backend\" to \
             \"dx12\" or \"vulkan\" in z2-native.json tries another \
             graphics API."
        }
        0xC000_00FD => "The game ran out of stack space (stack overflow, 0xC00000FD).",
        0xC000_0409 => {
            "The game was stopped by a fatal error check (0xC0000409): an \
             abort after an internal error, or memory corruption."
        }
        0xC000_0135 => {
            "Windows could not find a DLL the game needs (0xC0000135). \
             Reinstall the game, or install the Microsoft Visual C++ \
             Redistributable."
        }
        0xC000_0142 => "A DLL the game needs failed to start (0xC0000142).",
        0xC000_001D => {
            "The game used a CPU instruction this processor does not have \
             (illegal instruction, 0xC000001D)."
        }
        0xC000_0374 => "The game's memory was corrupted (heap corruption, 0xC0000374).",
        _ => return None,
    })
}

/// `exit code N`, with the hex NTSTATUS form for Windows exception codes.
#[must_use]
pub fn format_exit_code(code: i32) -> String {
    if (code as u32) >= 0xC000_0000 {
        format!("exit code {code} / 0x{:08X}", code as u32)
    } else {
        format!("exit code {code}")
    }
}

/// Program names tried next to the launcher, in order.
pub fn game_binary_names() -> Vec<String> {
    let exe = std::env::consts::EXE_SUFFIX;
    vec![format!("z2rs{exe}"), format!("z2-native{exe}")]
}

/// Every path [`resolve_game_binary`] would try for `exe_dir`, in order.
///
/// Release archives and the macOS `.app` bundle (`Contents/MacOS/`) ship the
/// game next to the launcher. In a cargo checkout a debug launcher also looks
/// in the sibling `release` folder, since the debug game is too slow to play.
pub fn candidate_paths(exe_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = game_binary_names()
        .iter()
        .map(|n| exe_dir.join(n))
        .collect();
    if exe_dir.file_name().is_some_and(|n| n == "debug") {
        if let Some(target) = exe_dir.parent() {
            let exe = std::env::consts::EXE_SUFFIX;
            out.push(target.join("release").join(format!("z2-native{exe}")));
        }
    }
    out
}

/// Where the game program is, or an explanation of where we looked.
///
/// Order: the manual override (when non-empty), then `$Z2RS_GAME_BIN`, then
/// [`candidate_paths`] next to the launcher.
pub fn resolve_game_binary(
    override_path: &str,
    env_value: Option<OsString>,
    exe_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let override_path = override_path.trim();
    if !override_path.is_empty() {
        let p = PathBuf::from(override_path);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "The game program set under Advanced was not found: {override_path}"
            ))
        };
    }
    if let Some(v) = env_value.filter(|v| !v.is_empty()) {
        let p = PathBuf::from(&v);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "{GAME_BIN_ENV} is set but that file was not found: {}",
                p.display()
            ))
        };
    }
    let Some(dir) = exe_dir else {
        return Err(
            "Could not tell where the launcher is installed, so the game could not be \
             found. Set the game program under Advanced."
                .to_string(),
        );
    };
    let tried = candidate_paths(dir);
    if let Some(found) = tried.iter().find(|p| p.is_file()) {
        return Ok(found.clone());
    }
    let list = tried
        .iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "The game program was not found. The launcher looked for:\n{list}\n\
         Keep the launcher in the same folder as the game, or set the game program \
         under Advanced."
    ))
}

/// [`resolve_game_binary`] with this process's environment and location.
pub fn resolve_game_binary_here(override_path: &str) -> Result<PathBuf, String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_game_binary(
        override_path,
        std::env::var_os(GAME_BIN_ENV),
        exe_dir.as_deref(),
    )
}

/// A running game process.
pub struct RunningGame {
    child: Child,
    log_path: PathBuf,
}

/// How a finished game ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameExit {
    /// `true` on a zero exit status.
    pub success: bool,
    /// Human-readable status (exit code or signal).
    pub status: String,
    /// What a crash exit code means, when it is a known one
    /// ([`explain_exit_code`]).
    pub explanation: Option<&'static str>,
    /// Last lines of the game's output.
    pub tail: Vec<String>,
    /// For a failed run: what the game's logs say about a graphics driver
    /// crash ([`gpu_crash_hint`]).
    pub gpu_hint: Option<String>,
}

impl RunningGame {
    /// Start `program args`, output to `log_path`.
    pub fn spawn(program: &Path, args: &[String], log_path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::File::create(log_path)?;
        let log_err = log.try_clone()?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        // Run the game from its own folder so it finds anything shipped next
        // to it.
        if let Some(dir) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            /// Keep a console window from flashing up behind the game.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn()?;
        Ok(RunningGame {
            child,
            log_path: log_path.to_path_buf(),
        })
    }

    /// `Some` once the game has exited.
    pub fn poll(&mut self) -> Option<GameExit> {
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => {
                let text = std::fs::read_to_string(&self.log_path).unwrap_or_default();
                let gpu_hint = if status.success() {
                    None
                } else {
                    // The crash logger's FATAL line (faulting DLL) is only in
                    // z2-native.log, not on stderr.
                    let crash_log = self
                        .log_path
                        .parent()
                        .map(|d| d.join(GAME_CRASH_LOG_FILE_NAME))
                        .and_then(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default();
                    gpu_crash_hint(&format!("{text}\n{crash_log}"))
                };
                Some(GameExit {
                    gpu_hint,
                    success: status.success(),
                    status: match status.code() {
                        Some(c) => format_exit_code(c),
                        None => format!("{status}"),
                    },
                    explanation: status.code().and_then(explain_exit_code),
                    tail: last_lines(&text, ERROR_TAIL_LINES),
                })
            }
            Err(e) => Some(GameExit {
                success: false,
                status: format!("could not check the game: {e}"),
                explanation: None,
                gpu_hint: None,
                tail: Vec::new(),
            }),
        }
    }
}

/// The last `n` non-empty lines of `text`.
pub fn last_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// Open `dir` in the system file browser, creating it first.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("z2-launcher-game-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolution_order() {
        let d = temp_dir("resolve");
        let names = game_binary_names();
        // Nothing there: the error lists every place tried.
        let err = resolve_game_binary("", None, Some(&d)).unwrap_err();
        for n in &names {
            assert!(err.contains(n.as_str()), "{err}");
        }
        // z2-native only.
        let native = d.join(&names[1]);
        std::fs::write(&native, b"").unwrap();
        assert_eq!(resolve_game_binary("", None, Some(&d)).unwrap(), native);
        // z2rs wins over z2-native.
        let z2rs = d.join(&names[0]);
        std::fs::write(&z2rs, b"").unwrap();
        assert_eq!(resolve_game_binary("", None, Some(&d)).unwrap(), z2rs);
        // The environment wins over the launcher folder.
        let env_bin = d.join("custom-game");
        std::fs::write(&env_bin, b"").unwrap();
        assert_eq!(
            resolve_game_binary("", Some(env_bin.clone().into()), Some(&d)).unwrap(),
            env_bin
        );
        assert!(resolve_game_binary("", Some(d.join("gone").into()), Some(&d)).is_err());
        // The manual override wins over everything.
        assert_eq!(
            resolve_game_binary(&native.to_string_lossy(), Some(env_bin.into()), Some(&d)).unwrap(),
            native
        );
        assert!(resolve_game_binary("/definitely/not/here", None, Some(&d)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn debug_launcher_also_tries_release_game() {
        let c = candidate_paths(Path::new("/w/target/debug"));
        assert_eq!(c.len(), 3);
        assert!(c[2].starts_with("/w/target/release"));
        assert_eq!(candidate_paths(Path::new("/w/target/release")).len(), 2);
    }

    #[test]
    fn windows_crash_codes_are_explained() {
        // What `ExitStatus::code` reports on Windows for 0xC0000005.
        let av = -1_073_741_819;
        assert!(explain_exit_code(av).unwrap().contains("access violation"));
        assert_eq!(format_exit_code(av), "exit code -1073741819 / 0xC0000005");
        assert!(explain_exit_code(0xC000_00FDu32 as i32)
            .unwrap()
            .contains("stack overflow"));
        assert!(explain_exit_code(0xC000_0135u32 as i32)
            .unwrap()
            .contains("DLL"));
        assert!(explain_exit_code(0xC000_0409u32 as i32).is_some());
        assert!(explain_exit_code(101).unwrap().contains("panic"));
        assert_eq!(explain_exit_code(1), None);
        assert_eq!(explain_exit_code(3), None);
        assert_eq!(format_exit_code(3), "exit code 3");
    }

    #[test]
    fn gpu_guard_notice_explains_the_last_crash() {
        assert_eq!(gpu_guard_notice(""), None);
        let n = gpu_guard_notice("attempting DX12\n").unwrap();
        assert!(n.contains("DX12 crashed the game last time"), "{n}");
        assert!(n.contains("tries Vulkan instead"), "{n}");
        // GL is only ever tried when named, so the advice is to go back
        // to auto.
        let n = gpu_guard_notice("attempting GL\n").unwrap();
        assert!(n.contains("back to \"auto\""), "{n}");
        assert!(n.contains("tries DX12, then Vulkan"), "{n}");
        // A remembered crash with no new one.
        let n = gpu_guard_notice("crashed DX12\n").unwrap();
        assert!(n.contains("Skipping graphics backend DX12"), "{n}");
        assert!(n.contains(GPU_GUARD_FILE_NAME), "{n}");
        // Everything crashed.
        let n = gpu_guard_notice("crashed DX12\nattempting Vulkan\n").unwrap();
        assert!(n.contains("try them all again"), "{n}");
    }

    #[test]
    fn gpu_crash_hint_names_the_gl_icd() {
        // Died inside the AMD GL ICD, as in the v0.4.0 report.
        let log = "[z2] gpu: trying backend GL (Backends(GL)), surface 1x1, texture 1x1\n\
                   [    0.4s] FATAL: unhandled access violation (0xC0000005) at 0x1 in \
                   C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\amdogl.inf_amd64\\atio6axx.dll+0xb1c74a\n";
        let h = gpu_crash_hint(log).unwrap();
        assert!(h.contains("(atio6axx.dll)"), "{h}");
        assert!(h.contains("nvoglv64.dll") && h.contains("\"dx12\""), "{h}");
        // The last breadcrumb was a GL attempt, no crash logger line.
        let h = gpu_crash_hint("[z2] gpu: trying backend GL (Backends(GL))\n").unwrap();
        assert!(h.contains("OpenGL graphics driver"), "{h}");
        // Running on GL, crashed later.
        let h = gpu_crash_hint(
            "[z2] gpu: trying backend WGPU_BACKEND (Backends(GL))\n\
             [z2] gpu: using 'Radeon' via Gl (driver '' ''), surface format X\n\
             [z2] gpu: first frame presented (Ok(()))\n",
        )
        .unwrap();
        assert!(h.contains("OpenGL"), "{h}");
        // Died while DX12 was starting.
        let h = gpu_crash_hint("[z2] gpu: trying backend DX12 (Backends(DX12))\n").unwrap();
        assert!(h.contains("starting graphics backend DX12"), "{h}");
        // DX12 came up fine; the crash was elsewhere.
        assert_eq!(
            gpu_crash_hint(
                "[z2] gpu: trying backend DX12 (Backends(DX12))\n\
                 [z2] gpu: using 'Radeon' via Dx12 (driver '' ''), surface format X\n\
                 [z2] gpu: first frame presented (Ok(()))\n\
                 [z2] audio: opening\n"
            ),
            None
        );
        assert_eq!(gpu_crash_hint(""), None);
    }

    #[test]
    fn tail_keeps_last_non_empty_lines() {
        let text = (1..=30)
            .map(|i| format!("line {i}\n\n"))
            .collect::<String>();
        let t = last_lines(&text, 20);
        assert_eq!(t.len(), 20);
        assert_eq!(t[0], "line 11");
        assert_eq!(t[19], "line 30");
        assert!(last_lines("", 5).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn spawn_captures_output_and_exit_code() {
        let d = temp_dir("spawn");
        let log = d.join("game.log");
        let args = vec![
            "-c".to_string(),
            "echo hello; echo oops >&2; exit 3".to_string(),
        ];
        let mut g = RunningGame::spawn(Path::new("/bin/sh"), &args, &log).unwrap();
        let exit = loop {
            if let Some(e) = g.poll() {
                break e;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(!exit.success);
        assert_eq!(exit.status, "exit code 3");
        assert_eq!(exit.tail, vec!["hello", "oops"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
