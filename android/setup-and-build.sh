#!/usr/bin/env bash
# One-shot setup + APK build for the MusicSync companion app on Linux.
#
# Idempotent: re-running skips installs that are already done. Prints a
# summary line per step so you can see where it is. Stops on first error.
#
# Outputs: android/app/build/outputs/apk/debug/app-debug.apk
# Install on a connected phone with `adb install <path>` (adb is in
# $ANDROID_HOME/platform-tools/).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# /tmp is usually tmpfs (RAM-backed) on Mint and can be too small for Gradle
# to extract its ~200 MB distribution into. Route all temp work to ~/.cache
# which lives on real disk.
export TMPDIR="$HOME/.cache/musicsync-tmp"
mkdir -p "$TMPDIR"

ANDROID_HOME_DEFAULT="$HOME/Android/Sdk"
ANDROID_HOME="${ANDROID_HOME:-$ANDROID_HOME_DEFAULT}"
CMDLINE_TOOLS_VERSION="11076708"
CMDLINE_TOOLS_URL="https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS_VERSION}_latest.zip"
SDK_PLATFORM="android-35"
BUILD_TOOLS="34.0.0"

step() { printf "\n\033[1;34m==>\033[0m %s\n" "$*"; }
info() { printf "    %s\n" "$*"; }

# ---------- 1. JDK 17 ----------
step "Checking for JDK 17"
if command -v javac >/dev/null 2>&1 && javac -version 2>&1 | grep -q '^javac 17'; then
    info "JDK 17 already on PATH ($(javac -version 2>&1))."
elif [ -x /usr/lib/jvm/java-17-openjdk-amd64/bin/javac ]; then
    info "JDK 17 installed at /usr/lib/jvm/java-17-openjdk-amd64; will use it."
    export JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
    export PATH="$JAVA_HOME/bin:$PATH"
else
    info "Installing openjdk-17-jdk (will prompt for sudo password)."
    sudo apt-get update
    sudo apt-get install -y openjdk-17-jdk unzip wget
    export JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
    export PATH="$JAVA_HOME/bin:$PATH"
fi
info "java: $(java -version 2>&1 | head -1)"

# ---------- 2. cmdline-tools ----------
step "Setting up Android SDK at $ANDROID_HOME"
mkdir -p "$ANDROID_HOME"
TOOLS_BIN="$ANDROID_HOME/cmdline-tools/latest/bin"
if [ -x "$TOOLS_BIN/sdkmanager" ]; then
    info "cmdline-tools already installed."
else
    info "Downloading command-line tools (~150 MB)…"
    TMP_ZIP="$(mktemp --suffix=.zip)"
    trap 'rm -f "$TMP_ZIP"' EXIT
    wget -q --show-progress -O "$TMP_ZIP" "$CMDLINE_TOOLS_URL"
    info "Extracting…"
    rm -rf "$ANDROID_HOME/cmdline-tools/latest"
    mkdir -p "$ANDROID_HOME/cmdline-tools"
    unzip -q "$TMP_ZIP" -d "$ANDROID_HOME/cmdline-tools"
    # Zip contains a top-level `cmdline-tools` dir; rename to `latest`.
    mv "$ANDROID_HOME/cmdline-tools/cmdline-tools" "$ANDROID_HOME/cmdline-tools/latest"
    rm -f "$TMP_ZIP"
    trap - EXIT
fi
export ANDROID_HOME
export PATH="$TOOLS_BIN:$ANDROID_HOME/platform-tools:$PATH"

# ---------- 3. SDK packages ----------
step "Accepting SDK licences"
# `yes | sdkmanager` is the standard pattern, but `yes` gets SIGPIPE when
# sdkmanager closes stdin and exits with 141. Disable pipefail just for
# this command so the script doesn't abort on that benign signal.
set +o pipefail
yes | sdkmanager --licenses >/dev/null
LIC_STATUS=$?
set -o pipefail
if [ "$LIC_STATUS" -ne 0 ] && [ "$LIC_STATUS" -ne 141 ]; then
    echo "sdkmanager --licenses exited $LIC_STATUS" >&2
    exit 1
fi
info "Licences accepted."

step "Installing SDK packages (platform-tools, $SDK_PLATFORM, build-tools $BUILD_TOOLS)"
sdkmanager --install \
    "platform-tools" \
    "platforms;$SDK_PLATFORM" \
    "build-tools;$BUILD_TOOLS"
info "SDK ready."

# ---------- 4. local.properties ----------
# Gradle needs to know where the SDK is. local.properties is gitignored.
if [ ! -f local.properties ] || ! grep -q "^sdk.dir=" local.properties 2>/dev/null; then
    info "Writing local.properties with sdk.dir=$ANDROID_HOME"
    echo "sdk.dir=$ANDROID_HOME" > local.properties
fi

# ---------- 5. Gradle wrapper ----------
step "Ensuring Gradle wrapper exists"
if [ ! -x gradlew ]; then
    info "Bootstrapping the Gradle wrapper via a one-shot Gradle download."
    GRADLE_VERSION="8.10"
    # Use ~/.cache (not /tmp, which can be small/tmpfs and silently truncate).
    BOOT_DIR="$HOME/.cache/musicsync-gradle-bootstrap"
    rm -rf "$BOOT_DIR"
    mkdir -p "$BOOT_DIR"
    BOOT_ZIP="$BOOT_DIR/gradle.zip"
    info "Downloading gradle-${GRADLE_VERSION}-bin.zip (~140 MB)…"
    # curl with -fL: follow redirects, fail on HTTP errors (so a 404 / 502
    # doesn't pretend to be a successful 0-byte download).
    if ! curl -fL --retry 3 --progress-bar -o "$BOOT_ZIP" \
        "https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip"; then
        echo "Gradle download failed." >&2
        exit 1
    fi
    info "Verifying zip integrity…"
    if ! unzip -tq "$BOOT_ZIP" >/dev/null; then
        echo "Downloaded zip is corrupt — rerun the script." >&2
        rm -f "$BOOT_ZIP"
        exit 1
    fi
    unzip -q "$BOOT_ZIP" -d "$BOOT_DIR"
    BOOT_GRADLE="$BOOT_DIR/gradle-${GRADLE_VERSION}/bin/gradle"
    BOOT_JAR="$BOOT_DIR/gradle-${GRADLE_VERSION}/lib/gradle-runtime-api-info-${GRADLE_VERSION}.jar"
    if [ ! -x "$BOOT_GRADLE" ] || [ ! -f "$BOOT_JAR" ]; then
        echo "Gradle extraction is incomplete (expected $BOOT_JAR)." >&2
        echo "Contents of $BOOT_DIR:" >&2
        ls -la "$BOOT_DIR" >&2 || true
        exit 1
    fi
    info "Generating gradlew via the bootstrap Gradle…"
    "$BOOT_GRADLE" wrapper --gradle-version "$GRADLE_VERSION"
    rm -rf "$BOOT_DIR"
fi

# ---------- 6. Build ----------
# If a previous run's wrapper-cached extract is incomplete, gradlew can
# silently reuse it and fail with the same NoSuchFileException. Detect that
# by checking for the runtime-api-info jar; nuke the dist cache if missing.
WRAPPER_DIST_ROOT="$HOME/.gradle/wrapper/dists/gradle-8.10-bin"
if [ -d "$WRAPPER_DIST_ROOT" ]; then
    if ! find "$WRAPPER_DIST_ROOT" -name "gradle-runtime-api-info-8.10.jar" -print -quit | grep -q .; then
        info "Wrapper dist cache appears incomplete; clearing $WRAPPER_DIST_ROOT"
        rm -rf "$WRAPPER_DIST_ROOT"
    fi
fi

step "Building debug APK"
# Pass the TMPDIR through to the JVM so Gradle's wrapper doesn't fall back
# to /tmp when extracting its bundled distribution.
GRADLE_OPTS="-Djava.io.tmpdir=$TMPDIR" ./gradlew assembleDebug

APK=app/build/outputs/apk/debug/app-debug.apk
if [ -f "$APK" ]; then
    step "Done"
    info "APK: $SCRIPT_DIR/$APK"
    info "Install on a USB-connected phone with:"
    info "    $ANDROID_HOME/platform-tools/adb install -r $SCRIPT_DIR/$APK"
    info "(enable USB debugging on the phone first: Settings → About → tap"
    info " Build Number 7 times, then Developer Options → USB debugging.)"
else
    echo "Build claimed success but $APK was not produced." >&2
    exit 1
fi
