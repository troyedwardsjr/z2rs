# Packaging `z2-native`

The desktop app is not packaged yet. The plan is to ship it with [`cargo-dist`](https://github.com/axodotdev/cargo-dist) or an equivalent such as `cargo-bundle` or per-OS installers. Code signing and notarization are deferred: there are no entitlements, certificates or CI signing jobs yet.

## What `cargo-dist` still needs

1. A `[workspace.metadata.dist]` section in the root `Cargo.toml`:
   ```toml
   [workspace.metadata.dist]
   cargo-dist-version = "0.24"
   installers = ["shell", "powershell", "dmg", "msi"]
   targets = ["x86_64-apple-darwin", "aarch64-apple-darwin",
              "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
   ```
2. A `dist` CI workflow (`.github/workflows/dist.yml`) that runs `cargo dist build` on tags.
3. Notes on the display and audio runtime for each OS:
   - Linux needs the X11/Wayland and ALSA/PulseAudio development libraries to build (`libx11-dev libwayland-dev libasound2-dev ...`). The release binary links them dynamically.
   - macOS needs nothing beyond the Xcode command line tools. winit and pixels use Metal, and cpal uses CoreAudio.
   - Windows needs the MSVC toolchain. cpal uses WASAPI and gilrs uses XInput.

## Code signing

macOS Gatekeeper (Developer ID plus notarization), Windows Authenticode and Linux package signatures are not configured. The first signed release is separate work. Until then, anyone who distributes a build should document the warnings an unsigned binary triggers: right-click and Open on macOS, "More info" in Windows SmartScreen, and `chmod +x` on Linux.

## Data files

The installer ships no ROM, movie or snapshot. On launch the app looks for a ROM you supply (`--rom`, then the `rom_path` config key, then `$Z2_ROM`), checks it with the `z2-assets` hash check and loads it directly. `assets.bin` is optional. The extractor can generate it separately, and the app does not need it to start.

The data directory is `$XDG_DATA_HOME/z2rs`, `~/Library/Application Support/z2rs` or `%APPDATA%/z2rs` (see `data_dir` in `src/config.rs`). Only `assets.bin`, `z2-native.json`, `savestate*.z2snap` and `sram.sav` live there.
