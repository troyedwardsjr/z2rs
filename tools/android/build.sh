#!/usr/bin/env bash
# Build the z2rs Android app: the Rust library (crates/z2-android, built with
# cargo-ndk into android/app/src/main/jniLibs/<abi>/) and then the APK (Gradle
# in android/). See README.md.
#
#   tools/android/build.sh                  release .so + debug APK (make android)
#   tools/android/build.sh --apk release    release .so + release APK (make android-release)
#   tools/android/build.sh --debug          debug .so (slow game, faster build)
#   tools/android/build.sh --apk none       the .so files only
#   tools/android/build.sh --abi arm64-v8a  one ABI (comma-separated list)
#
# Tool locations come from the environment, never from this file:
#   ANDROID_HOME / ANDROID_SDK_ROOT   the Android SDK
#   ANDROID_NDK_HOME / ANDROID_NDK_ROOT   the NDK (default: newest $SDK/ndk/*)
#   ANDROID_ABIS, ANDROID_API          defaults for --abi / --api
#   Z2RS_VERSION                       app version (e.g. 0.4.0); sets the APK's
#                                      versionName and versionCode
#
# No ROM is involved at any point: the APK never contains one, and the player
# picks their own dump inside the app (LEGAL.md).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ANDROID_DIR="$ROOT/android"
JNI_LIBS="$ANDROID_DIR/app/src/main/jniLibs"
CRATE="z2-android"
LIB_NAME="libz2rs_android.so"

PROFILE="release"
APK="debug"
ABIS="${ANDROID_ABIS:-arm64-v8a,x86_64}"
API="${ANDROID_API:-26}"

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
  echo "android build: $1" >&2
  shift
  for line in "$@"; do echo "  $line" >&2; done
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --debug) PROFILE="debug" ;;
    --release) PROFILE="release" ;;
    --apk)
      [ $# -ge 2 ] || die "--apk needs debug, release or none"
      APK="$2"
      shift
      ;;
    --apk=*) APK="${1#--apk=}" ;;
    --abi)
      [ $# -ge 2 ] || die "--abi needs a comma-separated ABI list"
      ABIS="$2"
      shift
      ;;
    --abi=*) ABIS="${1#--abi=}" ;;
    --api)
      [ $# -ge 2 ] || die "--api needs an API level"
      API="$2"
      shift
      ;;
    --api=*) API="${1#--api=}" ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) die "unknown argument '$1' (try --help)" ;;
  esac
  shift
done

case "$APK" in
  debug | release | none) ;;
  *) die "--apk must be debug, release or none, not '$APK'" ;;
esac

# --- ABIs ---------------------------------------------------------------------

abi_triple() {
  case "$1" in
    arm64-v8a) echo aarch64-linux-android ;;
    x86_64) echo x86_64-linux-android ;;
    armeabi-v7a) echo armv7-linux-androideabi ;;
    x86) echo i686-linux-android ;;
    *) return 1 ;;
  esac
}

IFS=',' read -r -a ABI_LIST <<<"$ABIS"
[ "${#ABI_LIST[@]}" -gt 0 ] || die "no ABIs selected"
for abi in "${ABI_LIST[@]}"; do
  abi_triple "$abi" >/dev/null || die "unknown ABI '$abi'" "Use arm64-v8a, x86_64, armeabi-v7a or x86."
done

# --- Rust toolchain -------------------------------------------------------------

command -v cargo >/dev/null 2>&1 || die "cargo not found." "Install Rust from https://rustup.rs, then retry."

if ! cargo ndk --version >/dev/null 2>&1; then
  die "cargo-ndk not found." \
    "Install it with:  cargo install cargo-ndk" \
    "(it must be on PATH, usually ~/.cargo/bin)"
fi

if command -v rustup >/dev/null 2>&1; then
  installed="$(rustup target list --installed)"
  missing=()
  for abi in "${ABI_LIST[@]}"; do
    t="$(abi_triple "$abi")"
    grep -qx "$t" <<<"$installed" || missing+=("$t")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    die "Rust targets not installed: ${missing[*]}" "Install them with:  rustup target add ${missing[*]}"
  fi
fi

# --- Android SDK and NDK --------------------------------------------------------

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [ -z "$SDK" ]; then
  # Android Studio's default install locations.
  for guess in "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
    if [ -d "$guess" ]; then
      SDK="$guess"
      break
    fi
  done
fi
[ -n "$SDK" ] || die "Android SDK not found." \
  "Set ANDROID_HOME to your SDK (Android Studio: Settings > Android SDK shows the path)," \
  "or install the command-line tools and then:" \
  "  sdkmanager \"platform-tools\" \"platforms;android-35\" \"build-tools;35.0.0\" \"ndk;27.2.12479018\""
[ -d "$SDK" ] || die "ANDROID_HOME='$SDK' is not a directory."
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"

# Newest NDK under $SDK/ndk, by numeric version (portable: no sort -V).
newest_ndk() {
  [ -d "$SDK/ndk" ] || return 0
  find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d -exec basename {} \; |
    grep -E '^[0-9]+(\.[0-9]+)*$' |
    sort -t. -k1,1n -k2,2n -k3,3n |
    tail -n 1
}

NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [ -z "$NDK" ]; then
  v="$(newest_ndk)"
  [ -n "$v" ] && NDK="$SDK/ndk/$v"
fi
[ -n "$NDK" ] || die "Android NDK not found under $SDK/ndk." \
  "Install it with:  sdkmanager \"ndk;27.2.12479018\"" \
  "or point ANDROID_NDK_HOME at an existing NDK (r26 or newer)."
[ -f "$NDK/source.properties" ] || die "ANDROID_NDK_HOME='$NDK' does not look like an NDK (no source.properties)."
export ANDROID_NDK_HOME="$NDK"

case "$(uname -s)" in
  Darwin) HOST_TAG="darwin-x86_64" ;; # the NDK ships universal binaries under this name
  Linux) HOST_TAG="linux-x86_64" ;;
  MINGW* | MSYS* | CYGWIN*) HOST_TAG="windows-x86_64" ;;
  *) die "unsupported host OS '$(uname -s)'" ;;
esac
NDK_SYSROOT_LIB="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/sysroot/usr/lib"
READELF="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin/llvm-readelf"
[ -x "$READELF" ] || READELF="$READELF.exe"
[ -x "$READELF" ] || die "llvm-readelf not found in the NDK ($NDK); is the NDK install complete?"

[ -f "$ROOT/crates/$CRATE/Cargo.toml" ] || die "crates/$CRATE is missing from this checkout."

# --- Rust library ---------------------------------------------------------------

echo "android build: $CRATE ($PROFILE) for ${ABI_LIST[*]}, API $API"
echo "  SDK $SDK"
echo "  NDK $NDK"

ndk_args=(-P "$API" -o "$JNI_LIBS")
for abi in "${ABI_LIST[@]}"; do ndk_args+=(-t "$abi"); done

# oboe (cpal's Android audio backend) links the shared C++ runtime, so
# libc++_shared.so has to ship next to our library. Recent cargo-ndk copies it
# itself; older ones get the manual copy below.
if cargo ndk --help 2>/dev/null | grep -q -- '--link-libcxx-shared'; then
  ndk_args+=(--link-libcxx-shared)
fi

cargo_args=(build -p "$CRATE")
[ "$PROFILE" = "release" ] && cargo_args+=(--release)

(cd "$ROOT" && cargo ndk "${ndk_args[@]}" "${cargo_args[@]}")

for abi in "${ABI_LIST[@]}"; do
  out="$JNI_LIBS/$abi"
  [ -f "$out/$LIB_NAME" ] || die "cargo-ndk finished but $out/$LIB_NAME is missing." \
    "Check that crates/$CRATE builds a cdylib named z2rs_android."
  if [ ! -f "$out/libc++_shared.so" ]; then
    src="$NDK_SYSROOT_LIB/$(abi_triple "$abi")/libc++_shared.so"
    [ -f "$src" ] || die "libc++_shared.so not found at $src."
    cp "$src" "$out/"
  fi
  # cargo-ndk copies every .so the build produced, including dylibs of
  # dependencies (e.g. libtetanes_core-<hash>.so) that our library does not
  # load. Keep only what libz2rs_android.so actually needs, so the APK does
  # not carry dead weight and stale hash-suffixed copies do not pile up.
  needed="$("$READELF" -d "$out/$LIB_NAME" | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p')"
  for f in "$out"/*.so; do
    base="$(basename "$f")"
    [ "$base" = "$LIB_NAME" ] && continue
    grep -qx "$base" <<<"$needed" || rm -f "$f"
  done
  echo "  $abi: $(du -h "$out/$LIB_NAME" | cut -f1) $LIB_NAME + libc++_shared.so"
done

# --- APK ------------------------------------------------------------------------

[ "$APK" = "none" ] && exit 0

[ -d "$ANDROID_DIR" ] || die "android/ (the Gradle project) is missing from this checkout."
command -v java >/dev/null 2>&1 || [ -n "${JAVA_HOME:-}" ] ||
  die "no Java found for Gradle." "Install a JDK 17 (for example Temurin or Zulu) or set JAVA_HOME."

if [ -x "$ANDROID_DIR/gradlew" ]; then
  GRADLE=("$ANDROID_DIR/gradlew")
elif command -v gradle >/dev/null 2>&1; then
  GRADLE=(gradle)
  echo "android build: no android/gradlew, using $(command -v gradle)." >&2
  echo "  If the Android Gradle plugin rejects this Gradle version, create the wrapper" >&2
  echo "  once with a version the plugin supports:  (cd android && gradle wrapper --gradle-version <ver>)" >&2
else
  die "no Gradle: android/gradlew is missing and gradle is not on PATH." \
    "Install Gradle (brew install gradle, sdk install gradle), or run 'gradle wrapper' in android/."
fi

case "$APK" in
  debug) task=assembleDebug ;;
  release) task=assembleRelease ;;
esac

gradle_args=("$task")
[ -n "${Z2RS_VERSION:-}" ] && gradle_args+=("-Pz2rsVersion=$Z2RS_VERSION")

echo "android build: ${GRADLE[*]} ${gradle_args[*]}"
(cd "$ANDROID_DIR" && "${GRADLE[@]}" "${gradle_args[@]}")

apk_dir="$ANDROID_DIR/app/build/outputs/apk/$APK"
found=0
if [ -d "$apk_dir" ]; then
  for f in "$apk_dir"/*.apk; do
    [ -f "$f" ] || continue
    echo "android build: APK $f"
    found=1
  done
fi
[ "$found" = 1 ] || die "Gradle finished but no APK is in $apk_dir."
if [ "$APK" = "release" ] && ls "$apk_dir"/*-unsigned.apk >/dev/null 2>&1; then
  echo "  (unsigned: sign it with apksigner before installing, see README.md)"
fi
