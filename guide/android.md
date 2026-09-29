# Playing on Android

z2rs runs on Android 8.0 and newer, on phones, tablets and handhelds. The app contains nothing from the original game. You pick your own ROM the first time you open it.

## Installing

Download `z2rs-<version>-android.apk` from the [releases page](https://github.com/troyedwardsjr/z2rs/releases) on the device and open it. One APK covers both `arm64-v8a` (phones, tablets and most handhelds) and `x86_64` (the Android emulator and some Chromebooks). The app is not in an app store, so Android asks whether the browser or file manager you opened it from may install apps. Allow it for that app, then install.

A newer APK installs over the old one and keeps your ROM, saves and settings, as long as both are signed with the same key. If Android refuses because the signatures differ (a build you made and signed yourself, for example), uninstall the old one first. Uninstalling deletes the ROM copy and the saves too.

## First run

The launcher opens with a Choose ROM button. It uses Android's own file picker, so the app asks for no storage permission, and you can pick the file from local storage, an SD card or a cloud drive. Use your own Zelda II (USA) dump. The same check as on the desktop applies when the game starts: only the No-Intro USA dump (body CRC32 `BA322865`) is accepted, and with any other file the game screen stays blank. Quit to the launcher and pick the right file.

The app copies the ROM into its private storage, so it can start straight away next time without asking again. Other apps cannot read that copy, it is left out of Android's cloud backup and device-to-device transfer, and it is deleted when you uninstall the app. Replace ROM picks a different file at any time. Press Play to start.

## The on-screen gamepad

A touch gamepad sits over the game: the d-pad on one side, B and A on the other, Select and Start between them. The d-pad has eight directions, and a thumb can slide from B onto A without lifting. In portrait the game sits above the controls. In landscape the game fills the screen and the pad is drawn over it, partly transparent.

The pad is on by default. While a controller is connected it hides itself, and it comes back when the controller disconnects. The launcher's settings change the rest:

- Gamepad size: small, medium or large.
- Gamepad opacity over the game, from 20 to 100 percent.
- Left-handed layout, which puts the d-pad on the right.
- High-contrast gamepad, with solid buttons and heavy outlines.
- Vibrate on press.
- Whether the pad hides while a controller is connected.

You can also turn the pad off completely, or show and hide it for the current session from the pause menu.

## Controllers

Bluetooth and USB controllers work without any setup. The mapping is positional, as on the desktop: the right face button is NES A and the bottom one is NES B, whatever their labels say. The d-pad and the left stick both steer. The controller's Menu button, or its guide button where Android passes that to apps, opens the pause menu.

The first controller is player 1 and the second is player 2. To play local co-op, turn on "Two-player local co-op" in the launcher before pressing Play. See [co-op.md](co-op.md) for what player 2 can and cannot do.

## Keyboard

A physical keyboard (Bluetooth, USB, or a Chromebook's own) uses the desktop keys for player 1: arrow keys for the d-pad, `Z` for A, `X` for B, `Enter` for Start and `Shift` for Select.

## Pause menu

Back (the button, the gesture or the key) opens the pause menu, and so does the pause button in the top corner of the screen. The menu has Resume, Save state and Load state (both use slot 1), Show or Hide on-screen gamepad, and Quit to launcher. The game also stops while the app is in the background and carries on when you return.

Save states are full snapshots of the game, the same as `F5` and `F7` on the desktop. They and the battery save live in the app's private storage and are deleted when you uninstall. Android's backup can include the saves and settings, but never the ROM.

## Settings

Besides the gamepad options, the launcher has:

- Widescreen: off, 16:10, 16:9 or 21:9. Phones held sideways are usually wider than 16:9, so 16:9 or 21:9 uses more of the screen than the original view. See [co-op.md](co-op.md) for how widescreen works.
- Scaling: Fill the screen (the default), or whole multiples only, which is a little sharper but can leave borders.
- Keep the screen on while playing (on by default).

Settings are read when the game starts, so change them in the launcher before pressing Play.

## Accessibility

Each on-screen gamepad button is exposed to TalkBack and Switch Access as its own labelled control ("D-pad up", "A button", "Start" and so on), even though the pad is drawn as one surface. Activating one presses that button briefly. Every launcher control has a spoken label, and TalkBack announces controllers as they connect and disconnect.

Gamepad buttons and the pause button are at least 48dp, Android's recommended minimum, at every gamepad size. Launcher and menu text follows the system font size. The vibration on press follows the system touch feedback setting, so with touch feedback turned off for the device the pad does not vibrate, whatever the setting in the app says. The high-contrast gamepad is described above.

## Limits

- Online co-op is not available on Android yet. The app does not request internet access. Local co-op with two controllers works.
- The app has no HD pack option.
- Only `arm64-v8a` and `x86_64` are built. 32-bit ARM devices are not supported by the release APK.

## Building from source

You need the desktop build's Rust toolchain plus:

- the two Android targets: `rustup target add aarch64-linux-android x86_64-linux-android`
- `cargo-ndk`: `cargo install cargo-ndk`
- the Android SDK, with `ANDROID_HOME` (or `ANDROID_SDK_ROOT`) pointing at it. Android Studio's default locations (`~/Library/Android/sdk` on macOS, `~/Android/Sdk` on Linux) are found without it. With the command-line tools only: `sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"`
- an NDK, r26 or newer. `ANDROID_NDK_HOME` (or `ANDROID_NDK_ROOT`) picks one, otherwise the newest under `$ANDROID_HOME/ndk/` is used.
- JDK 17 for Gradle, on `PATH` or in `JAVA_HOME`

When a piece is missing, the build stops and prints the command that installs it.

```sh
make android           # release Rust library + debug APK
make android-release   # release Rust library + unsigned release APK
make android-install   # adb install the debug APK on the connected device
make android-run       # install, then start the app
make android-log       # logcat for the running app
```

Both build targets run `tools/android/build.sh`. It builds `crates/z2-android` with cargo-ndk for both ABIs into `android/app/src/main/jniLibs/`, then runs Gradle in `android/`. The APK ends up in `android/app/build/outputs/apk/debug/` or `.../release/`. The Rust library is built in release mode even for the debug APK, because a debug build of the game is too slow to play. Pass options through `ANDROID_ARGS`, for example `make android ANDROID_ARGS="--abi arm64-v8a"` to build one ABI only, which roughly halves the build time. `tools/android/build.sh --help` lists the rest.

The device targets use `adb` from `PATH` or from `$ANDROID_HOME/platform-tools`, and need a phone with USB debugging on or a running emulator. The debug APK is signed with Gradle's local debug key and installs as it is.

### Signing a release APK

`make android-release` produces `app-release-unsigned.apk`, which Android will not install. Sign it with `zipalign` and `apksigner` from the SDK's `build-tools/<version>/` directory:

```sh
cd android/app/build/outputs/apk/release
keytool -genkeypair -v -keystore z2rs.jks -alias z2rs -keyalg RSA -keysize 2048 -validity 10000   # once
zipalign -p -f 4 app-release-unsigned.apk z2rs-aligned.apk
apksigner sign --ks z2rs.jks --out z2rs-release.apk z2rs-aligned.apk
adb install -r z2rs-release.apk
```

Keep the keystore somewhere safe and out of the repository. Updates must be signed with the same key, or Android makes you uninstall first, which deletes the ROM copy and the saves.

## How it works

The game runs in a `GameActivity` from the Android games library, through winit's Android backend. `crates/z2-android` builds `libz2rs_android.so`, a thin library around the same Rust game loop, renderer and audio code that the desktop app uses in `crates/z2-native`. The launcher is ordinary Kotlin in `android/`. Android has no command line, so when you press Play the launcher writes the game's options as a list of desktop flags to `launch_args.json` in the app's files directory. The game reads that file at start and parses it with the desktop's own argument parser. The on-screen gamepad, controllers and the pause menu reach the game through a small JNI class, `com.z2rs.game.NativeBridge`, and the game loop picks those values up once per frame.
