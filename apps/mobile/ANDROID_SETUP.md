# Android build setup (MedMe mobile, Tauri v2)

Local toolchain for building the Android app on macOS (Apple Silicon).
Base tools installed via Homebrew: `brew install openjdk@17` and
`brew install --cask android-commandlinetools`.

## Required environment variables

```sh
export JAVA_HOME=/opt/homebrew/opt/openjdk@17
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
export NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
```

(`brew --prefix openjdk@17` = `/opt/homebrew/opt/openjdk@17`;
`$(brew --prefix)/share/android-commandlinetools` = `ANDROID_HOME`.)

## Installed SDK components

Installed with `$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager`:

- `platform-tools`
- `platforms;android-34`
- `build-tools;34.0.0`
- `ndk;27.1.12297006` (NDK r27b — Tauri v2 compatible)

Licenses accepted with `yes | sdkmanager --sdk_root="$ANDROID_HOME" --licenses`.

## Rust targets

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi \
  i686-linux-android x86_64-linux-android
```

## Build

```sh
# One-time project generation (already committed under src-tauri/gen/android):
pnpm -C apps/mobile tauri android init

# Debug APK, arm64 only (fastest; proves the toolchain):
pnpm -C apps/mobile tauri android build -t aarch64 --apk --debug

# All ABIs:
pnpm -C apps/mobile tauri android build --apk --debug
```

Output APK: `apps/mobile/src-tauri/gen/android/app/build/outputs/apk/`.

## OCR note

On Android, image OCR is not yet wired up. iOS uses Apple Vision
(`src-tauri/src/vision.rs`, `#[cfg(target_os = "ios")]`) instead of the
desktop `oar-ocr` path. The shared `oar-ocr` crate still *compiles* for
Android (it is a build-time dependency of `pipeline`), but at runtime it
would try to auto-download a PP-OCR model into a home dir the Android
sandbox does not provide, so images fall through to the `StoredNoText`
path (original bytes preserved, no text). A future release should route
Android image OCR through Google ML Kit, mirroring the iOS Vision bridge.
