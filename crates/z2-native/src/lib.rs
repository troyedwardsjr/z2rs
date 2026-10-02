//! `z2-native`: native desktop frontend.
//!
//! * [`headless`] — windowless CI surface (`--headless`; the
//!   parity check routes its smoke through [`app`]'s shared `Game` step path).
//! * [`app`] — shared `Game` build/step path + windowed loop (winit/pixels).
//! * [`audio`] — `cpal` plumbing over `z2-apu`'s `PcmFifo` headlines.
//! * [`input`] — keyboard + gamepad → shared-contract pad bytes.
//! * [`external_pad`] — process-global pad/request mailboxes a non-winit
//!   host (the Android app, over JNI) writes and the windowed loop reads.
//! * [`config`] — JSON config + platform data dirs.
//! * [`diag`] — startup breadcrumb / crash log (`<data-dir>/z2-native.log`).
//! * [`hd_check`] — up-front HD pack check for launchers that import packs
//!   (the Android app, over JNI).
//! * [`display_enh`] — display-only enhancements: flash colour, screen
//!   shake, post effects, quest timer, dev overlays, volumes (see
//!   README.md).
//! * [`overlay`] — in-game egui options menu (`O`, or LB + RB + Y).
//! * [`netplay`] — lockstep session driving over `z2-net` (the WebRTC
//!   transport itself is behind the `netplay` cargo feature).
//! * [`rando`] — randomizer seam (`--seed`, `--rando-flags`): the vanilla
//!   body is randomized by `z2-rando` before every emulator build.

pub mod app;
pub mod audio;
pub mod config;
pub mod diag;
pub mod display_enh;
pub mod external_pad;
#[cfg(target_os = "macos")]
pub mod gc_pad;
pub mod gpu_guard;
pub mod gpu_present;
pub mod hd_check;
pub mod headless;
pub mod input;
pub mod netplay;
pub mod overlay;
pub mod rando;
