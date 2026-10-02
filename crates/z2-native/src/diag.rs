//! Crash breadcrumbs for the windowed game: `<data-dir>/z2-native.log`.
//!
//! A native crash (an access violation inside a graphics, audio or
//! controller driver) kills the process without a Rust panic message, so a
//! bug report used to carry nothing but an exit code. The windowed loop now
//! logs each subsystem as it comes up (window, GPU adapter and backend, audio
//! device and format, gamepad backend and connected pads, first input from
//! each pad) to this file, one line at a time and flushed per line, so the
//! last line of the log names where the game died. The previous run's log is
//! kept as `z2-native.prev.log`.
//!
//! Also logged here: every panic (message, thread, backtrace) and, on
//! Windows, any unhandled structured exception with its NTSTATUS code, the
//! faulting address and the module (DLL) it lies in.
//!
//! Every line also goes to stderr, which the launcher saves to
//! `last-game.log` and shows the tail of when the game stops.
//!
//! Headless runs never open the log.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Breadcrumb log file name inside the data dir.
pub const LOG_FILE_NAME: &str = "z2-native.log";
/// The previous run's log (rotated on start).
pub const PREV_LOG_FILE_NAME: &str = "z2-native.prev.log";

struct Log {
    file: Mutex<File>,
    start: Instant,
    path: PathBuf,
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Open the log in `data_dir` (rotating the old one), install the panic
/// hook and, on Windows, the unhandled-exception logger. Returns the log
/// path, or `None` when the file cannot be created (breadcrumbs then go to
/// stderr only). Calling it again is a no-op.
pub fn init(data_dir: &Path) -> Option<PathBuf> {
    if let Some(log) = LOG.get() {
        return Some(log.path.clone());
    }
    let path = data_dir.join(LOG_FILE_NAME);
    let _ = std::fs::rename(&path, data_dir.join(PREV_LOG_FILE_NAME));
    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[z2] could not create {}: {e}", path.display());
            return None;
        }
    };
    let _ = LOG.set(Log {
        file: Mutex::new(file),
        start: Instant::now(),
        path: path.clone(),
    });
    install_panic_hook();
    #[cfg(any(windows, z2_check_win_seh))]
    win::install();
    breadcrumb(format_args!(
        "z2-native {} on {} {} (log: {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        path.display()
    ));
    Some(path)
}

/// Path of the open log, if [`init`] succeeded.
#[must_use]
pub fn log_path() -> Option<PathBuf> {
    LOG.get().map(|l| l.path.clone())
}

/// One log line: seconds since start (millisecond precision), then `msg`.
#[must_use]
pub fn format_line(elapsed_ms: u128, msg: &str) -> String {
    format!(
        "[{:>5}.{:03}s] {msg}\n",
        elapsed_ms / 1000,
        elapsed_ms % 1000
    )
}

/// Log `msg` to stderr and (when open) the log file, flushed immediately.
pub fn breadcrumb(msg: impl std::fmt::Display) {
    let msg = msg.to_string();
    eprintln!("[z2] {msg}");
    write_line(&msg, false);
}

/// Append one line to the log. `try_only` never blocks (crash paths: the
/// faulting thread may already hold the lock).
fn write_line(msg: &str, try_only: bool) {
    let Some(log) = LOG.get() else { return };
    let line = format_line(log.start.elapsed().as_millis(), msg);
    let guard = if try_only {
        match log.file.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    } else {
        Some(
            log.file
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    };
    if let Some(mut f) = guard {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let bt = std::backtrace::Backtrace::force_capture();
        write_line(
            &format!("PANIC on thread '{name}': {info}\nbacktrace:\n{bt}"),
            true,
        );
        prev(info);
    }));
}

/// Short name of a Windows NTSTATUS exception / exit code, if known.
#[must_use]
pub fn ntstatus_name(code: u32) -> Option<&'static str> {
    Some(match code {
        0xC000_0005 => "access violation",
        0xC000_001D => "illegal instruction",
        0xC000_0094 => "integer divide by zero",
        0xC000_00FD => "stack overflow",
        0xC000_0135 => "DLL not found",
        0xC000_0142 => "DLL initialization failed",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "stack buffer overrun / fast fail (abort)",
        0x8000_0003 => "breakpoint",
        _ => return None,
    })
}

/// Unhandled structured-exception logger (Windows).
///
/// Declared by hand against kernel32 (always linked by std) so it has no
/// dependency on any `windows`/`windows-sys` version. `cargo check` with
/// `RUSTFLAGS="--cfg z2_check_win_seh"` type-checks it on other hosts.
#[cfg(any(windows, z2_check_win_seh))]
mod win {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        record: *mut ExceptionRecord,
        address: *mut c_void,
        n_params: u32,
        info: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        record: *const ExceptionRecord,
        context: *mut c_void,
    }

    type Filter = Option<unsafe extern "system" fn(*const ExceptionPointers) -> i32>;

    #[cfg_attr(windows, link(name = "kernel32"))]
    extern "system" {
        fn SetUnhandledExceptionFilter(filter: Filter) -> Filter;
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }

    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_ACCESS_VIOLATION: u32 = 0xC000_0005;
    const STATUS_STACK_OVERFLOW: u32 = 0xC000_00FD;

    static PREV: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn install() {
        // SAFETY: plain kernel32 call; `filter` has the documented
        // LPTOP_LEVEL_EXCEPTION_FILTER signature.
        let prev = unsafe { SetUnhandledExceptionFilter(Some(filter)) };
        PREV.store(prev.map_or(0, |f| f as usize), Ordering::SeqCst);
    }

    /// `module+offset` for `addr`, or `None` outside any loaded module.
    fn module_of(addr: usize) -> Option<String> {
        let mut module: *mut c_void = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS makes `name` an address, not a string;
        // UNCHANGED_REFCOUNT means no FreeLibrary is owed.
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                addr as *const u16,
                &mut module,
            )
        };
        if ok == 0 || module.is_null() {
            return None;
        }
        let mut buf = [0u16; 520];
        // SAFETY: `buf` is valid for `buf.len()` u16s.
        let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) };
        let name = String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())]);
        Some(format!("{name}+0x{:x}", addr.wrapping_sub(module as usize)))
    }

    unsafe extern "system" fn filter(p: *const ExceptionPointers) -> i32 {
        // SAFETY: the OS hands us valid EXCEPTION_POINTERS (null-checked).
        let rec = unsafe { p.as_ref().and_then(|p| p.record.as_ref()) };
        if let Some(rec) = rec {
            let name = super::ntstatus_name(rec.code).unwrap_or("exception");
            if rec.code == STATUS_STACK_OVERFLOW {
                // Little stack left: no module lookup.
                super::write_line("FATAL: stack overflow (0xC00000FD)", true);
            } else {
                let addr = rec.address as usize;
                let module = module_of(addr).unwrap_or_else(|| "<no module>".into());
                let mut msg = format!(
                    "FATAL: unhandled {name} (0x{:08X}) at 0x{addr:x} in {module}",
                    rec.code
                );
                if rec.code == STATUS_ACCESS_VIOLATION && rec.n_params >= 2 {
                    let op = match rec.info[0] {
                        0 => "reading",
                        1 => "writing",
                        8 => "executing",
                        _ => "accessing",
                    };
                    msg.push_str(&format!("; {op} address 0x{:x}", rec.info[1]));
                }
                let thread = std::thread::current();
                msg.push_str(&format!(
                    " (thread '{}')",
                    thread.name().unwrap_or("<unnamed>")
                ));
                super::write_line(&msg, true);
            }
        }
        let prev = PREV.load(Ordering::SeqCst);
        if prev != 0 {
            // SAFETY: `prev` came from SetUnhandledExceptionFilter as a filter.
            let prev: unsafe extern "system" fn(*const ExceptionPointers) -> i32 =
                unsafe { std::mem::transmute(prev) };
            return unsafe { prev(p) };
        }
        EXCEPTION_CONTINUE_SEARCH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format_is_seconds_with_millis() {
        assert_eq!(format_line(0, "hi"), "[    0.000s] hi\n");
        assert_eq!(format_line(12_345, "x"), "[   12.345s] x\n");
    }

    #[test]
    fn known_ntstatus_codes_have_names() {
        assert_eq!(ntstatus_name(0xC000_0005), Some("access violation"));
        assert_eq!(ntstatus_name(0xC000_00FD), Some("stack overflow"));
        assert_eq!(
            ntstatus_name(-1_073_741_819_i32 as u32),
            Some("access violation")
        );
        assert_eq!(ntstatus_name(1), None);
    }
}
