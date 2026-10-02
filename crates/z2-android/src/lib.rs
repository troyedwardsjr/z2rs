//! `z2-android`: the Android entry point for the z2rs native frontend.
//!
//! Built as `libz2rs_android.so` and loaded by the app's GameActivity
//! (`androidx.games:games-activity` 4.x, paired with winit's
//! `android-game-activity` / android-activity 0.6). Two surfaces:
//!
//! * [`android_main`] — the GameActivity entry point. It points the data dir
//!   at the app's internal storage, reads the launcher's argv from
//!   `<internal_data_path>/launch_args.json`, and runs the same windowed loop
//!   as the desktop binary ([`z2_native::app::run_windowed_with`]) on a winit
//!   event loop built for this `AndroidApp`. Suspend/resume (surface
//!   teardown, SRAM save, audio pause) is handled inside that loop.
//! * `Java_com_z2rs_game_NativeBridge_*` — JNI exports for the Kotlin
//!   `object com.z2rs.game.NativeBridge` (`@JvmStatic external fun`, hence the
//!   `JClass` second parameter). The pad and pause ones run on the UI thread
//!   and only store into [`z2_native::external_pad`]'s atomics; the loop reads
//!   them. `checkHdPack` is the exception: the launcher (its own process, no
//!   game loop) calls it on a background thread to validate an imported HD
//!   pack with the game's own loader.
//!
//! Pad bytes use the shared NES contract, LSB-first: A=0, B=1, Select=2,
//! Start=3, Up=4, Down=5, Left=6, Right=7.
//!
//! Everything is `cfg(target_os = "android")`: a desktop workspace build
//! compiles this crate to an empty library.

#![cfg(target_os = "android")]

use std::io::{BufRead, BufReader};
use std::os::fd::FromRawFd;
use std::path::Path;

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_FALSE};
use jni::JNIEnv;
use winit::event_loop::EventLoop;
use winit::platform::android::activity::AndroidApp;
use winit::platform::android::EventLoopBuilderExtAndroid;
use z2_native::external_pad;

/// Logcat tag for everything this library logs.
const LOG_TAG: &str = "z2rs";
/// Launcher-written argv file inside the internal data dir.
const LAUNCH_ARGS_FILE: &str = "launch_args.json";
/// argv[0] used when the launch file is missing or unreadable.
const DEFAULT_ARGV0: &str = "z2-native";
/// Why the last game could not start (one line), for the launcher to show:
/// the game process just ends on such an error, and logcat is out of sight.
/// The launcher reads and deletes it when it comes back to the front.
const LAST_ERROR_FILE: &str = "last_game_error.txt";

/// GameActivity entry point (called by android-activity on its own thread).
///
/// Returns only when the loop exits (a quit request) or cannot start. winit
/// allows one event loop per process, so a returning `android_main` also
/// ends the process: a relaunch into the same process would otherwise get
/// `RecreationAttempt` and a black screen.
#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            // wgpu logs its adapter and downlevel report at info/warn on
            // every surface creation; keep only its errors.
            .with_filter(
                android_logger::FilterBuilder::new()
                    .parse("info,wgpu_core=error,wgpu_hal=error,naga=error")
                    .build(),
            )
            .with_tag(LOG_TAG),
    );
    // z2-native reports through eprintln! (ROM load, audio fallback, save
    // results, netplay...), which goes nowhere on Android without this.
    forward_stdio_to_logcat();

    let Some(data) = app.internal_data_path() else {
        log::error!("no internal data path; cannot start");
        return;
    };
    // Must happen before anything reads config: `config::data_dir()` honours
    // XDG_DATA_HOME first, so config, SRAM, save states and assets.bin all
    // land in `<internal>/z2rs/`. Set once, before any other thread in this
    // library reads the environment.
    std::env::set_var("XDG_DATA_HOME", &data);
    let _ = std::fs::remove_file(data.join(LAST_ERROR_FILE));

    let argv = read_launch_args(&data.join(LAUNCH_ARGS_FILE));
    log::info!("launch args: {argv:?}");
    let args = match z2_native::app::parse_native_args(&argv) {
        Ok(a) => a,
        Err(msg) => {
            log::error!("launch args rejected, starting with defaults: {msg}");
            match z2_native::app::parse_native_args(&[DEFAULT_ARGV0.to_string()]) {
                Ok(a) => a,
                Err(msg) => {
                    log::error!("default args rejected: {msg}");
                    return;
                }
            }
        }
    };
    match &args.rom {
        Some(rom) if Path::new(rom).is_file() => log::info!("ROM: {rom}"),
        Some(rom) => log::warn!("ROM path does not exist: {rom}"),
        None => log::warn!("no --rom in launch args; the game will show an empty frame"),
    }

    let event_loop = match EventLoop::builder().with_android_app(app).build() {
        Ok(l) => l,
        Err(e) => {
            log::error!("event loop: {e}");
            std::process::exit(1);
        }
    };
    log::info!("windowed loop starting");
    match z2_native::app::run_windowed_with(&args, event_loop) {
        Ok(()) => log::info!("windowed loop exited"),
        Err(e) => {
            log::error!("windowed loop failed: {e}");
            if let Err(w) = std::fs::write(data.join(LAST_ERROR_FILE), e.to_string()) {
                log::warn!("{LAST_ERROR_FILE}: {w}");
            }
        }
    }
    std::process::exit(0);
}

/// Read the launcher's argv (a JSON array of strings including argv[0]).
/// A missing or malformed file yields just `[argv0]`, i.e. config defaults.
fn read_launch_args(path: &Path) -> Vec<String> {
    let parsed = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).map_err(|e| e.to_string()));
    match parsed {
        Ok(argv) if !argv.is_empty() => argv,
        Ok(_) => vec![DEFAULT_ARGV0.to_string()],
        Err(e) => {
            log::warn!("{}: {e}; using defaults", path.display());
            vec![DEFAULT_ARGV0.to_string()]
        }
    }
}

/// Point stdout and stderr at a pipe whose lines a background thread logs to
/// logcat (Android discards both by default). Best effort: any failure just
/// leaves stdio where it was.
fn forward_stdio_to_logcat() {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: plain POSIX calls on descriptors this function owns; `fds` is a
    // valid two-element buffer for `pipe`.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        let ok = libc::dup2(fds[1], libc::STDOUT_FILENO) >= 0
            && libc::dup2(fds[1], libc::STDERR_FILENO) >= 0;
        libc::close(fds[1]);
        if !ok {
            libc::close(fds[0]);
            return;
        }
    }
    // SAFETY: `fds[0]` is the read end just created and owned by nobody else.
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let _ = std::thread::Builder::new()
        .name("z2rs-stdio".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(l) => log::info!("{l}"),
                    Err(_) => break,
                }
            }
        });
}

/// Clamp a Java int to a pad byte (the low eight bits are the buttons).
fn pad_byte(mask: jint) -> u8 {
    (mask & 0xFF) as u8
}

/// Clamp a Java int to a save-state slot (0..=9, like the desktop hotkeys);
/// `None` for anything outside.
fn slot(slot: jint) -> Option<u8> {
    u8::try_from(slot)
        .ok()
        .filter(|s| *s < z2_native::app::SAVESTATE_SLOTS)
}

/// `NativeBridge.setTouchPad(mask: Int)`: the on-screen pad (player 1).
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_setTouchPad(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    mask: jint,
) {
    external_pad::set_touch_mask(pad_byte(mask));
}

/// `NativeBridge.setHardwarePad(player: Int, mask: Int)`: a hardware gamepad
/// for player 0 (P1) or 1 (P2); other indices are ignored.
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_setHardwarePad(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    player: jint,
    mask: jint,
) {
    if let Ok(player) = usize::try_from(player) {
        external_pad::set_hw_mask(player, pad_byte(mask));
    }
}

/// `NativeBridge.setPaused(paused: Boolean)`: the app's pause button / Back.
///
/// Note the loop also pauses by itself on window focus loss (the desktop
/// `pause_on_focus_loss` config, default on: backgrounding, a dialog, the
/// notification shade) and does NOT unpause on regaining focus, so the app
/// must call `setPaused(false)` to resume play after such an interruption.
/// There is no title bar on Android, so the paused state is only visible
/// through whatever the app draws over the game.
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_setPaused(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    paused: jboolean,
) {
    external_pad::request_pause(paused != JNI_FALSE);
}

/// `NativeBridge.requestSaveState(slot: Int)`: write save-state `slot` (0-9)
/// on the loop's next iteration.
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_requestSaveState(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    slot_index: jint,
) {
    match slot(slot_index) {
        Some(s) => external_pad::request_save_state(s),
        None => log::warn!("requestSaveState: slot {slot_index} out of range"),
    }
}

/// `NativeBridge.requestLoadState(slot: Int)`: load save-state `slot` (0-9)
/// on the loop's next iteration.
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_requestLoadState(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    slot_index: jint,
) {
    match slot(slot_index) {
        Some(s) => external_pad::request_load_state(s),
        None => log::warn!("requestLoadState: slot {slot_index} out of range"),
    }
}

/// `NativeBridge.checkHdPack(dir: String): String`: load the HD pack in `dir`
/// with the game's loader and answer in [`z2_native::hd_check::check_reply`]'s
/// one-line format (`ok\t<name>\t<scale>\t<tiles>\t<layers>` or
/// `error\t<message>`). Blocking (it decodes every sheet): the launcher calls
/// it off the UI thread. Returns null only if the reply string cannot be
/// created, which the Kotlin side treats as "could not check".
#[no_mangle]
pub extern "system" fn Java_com_z2rs_game_NativeBridge_checkHdPack<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    dir: JString<'local>,
) -> jstring {
    let reply = match env.get_string(&dir) {
        Ok(s) => {
            let dir: String = s.into();
            std::panic::catch_unwind(|| z2_native::hd_check::check_reply(Path::new(&dir)))
                .unwrap_or_else(|_| "error\tthe pack check failed unexpectedly".to_string())
        }
        Err(e) => format!("error\tunreadable path: {e}"),
    };
    match env.new_string(reply) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
