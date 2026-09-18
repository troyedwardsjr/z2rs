//! `z2-native`: native desktop frontend.
//!
//! * [`headless`] — windowless CI surface (`--headless`; the
//!   parity check routes its smoke through [`app`]'s shared `Game` step path).
//! * [`app`] — shared `Game` build/step path + windowed loop (winit/pixels).
//! * [`audio`] — `cpal` plumbing over `z2-apu`'s `PcmFifo` headlines.
//! * [`input`] — keyboard + gamepad → shared-contract pad bytes.
//! * [`config`] — JSON config + platform data dirs.
//! * [`netplay`] — lockstep session driving over `z2-net` (the WebRTC
//!   transport itself is behind the `netplay` cargo feature).

pub mod app;
pub mod audio;
pub mod config;
#[cfg(target_os = "macos")]
pub mod gc_pad;
pub mod headless;
pub mod input;
pub mod netplay;
