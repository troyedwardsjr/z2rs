//! Give the Windows game binary the same 8 MiB main-thread stack macOS and
//! Linux have.
//!
//! Windows reserves only 1 MiB for the main thread, and the winit event loop
//! (and so every emulator step, save state and netplay session start) runs
//! on it. A rollback session start overflowed that 1 MiB while its peers on
//! macOS and Linux had room to spare: "thread 'main' has overflowed its
//! stack", exit code 0xC00000FD, right after "synchronizing (66%)". The
//! frames involved are much smaller now (`Game::frame` is boxed), and this
//! keeps a margin for whatever grows next. Only the stack *reservation*
//! changes; pages are committed as they are touched.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    const STACK_BYTES: u32 = 8 * 1024 * 1024;
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os != "windows" {
        return;
    }
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/STACK:{STACK_BYTES}");
    } else {
        // MinGW (GNU ld) and the gnullvm targets (lld) take the GNU spelling.
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK_BYTES}");
    }
}
