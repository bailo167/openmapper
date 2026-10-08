#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Builds the LGPL FFmpeg shared libraries that OpenMapper releases bundle
# (docs/media/ffmpeg.md, D-031). This script *is* the published build recipe:
# it is shipped in every release archive next to the libraries it produced.
#
#   tools/ffmpeg/build.sh [PREFIX]        (default: target/ffmpeg)
#
# Runs on Linux, macOS and Windows (MSYS2 UCRT64 shell with the
# mingw-w64-ucrt-x86_64 toolchain, nasm, make and diffutils installed).
#
# Environment:
#   FFMPEG_SOURCE_DIR   use this FFmpeg checkout instead of cloning
#   OM_FFMPEG_BUILD_DIR scratch directory for the clone and objects
#                       (default: PREFIX-build)
#   OM_FFMPEG_JOBS      parallel make jobs (default: all CPUs)
#   OM_FFMPEG_SOURCE_ARCHIVE=1  also write the complete corresponding
#                       source archive (git archive of the pinned commit)
#
# The source is pinned by commit, which identifies it cryptographically; the
# release attaches the matching source archive (LGPL-2.1 §6d).
set -euo pipefail

FFMPEG_GIT="https://github.com/FFmpeg/FFmpeg"
FFMPEG_TAG="n9.0.2"
FFMPEG_COMMIT="946fcce07b6dcd0331c8cc609192aeff5e1924f8"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
prefix="${1:-$repo/target/ffmpeg}"
mkdir -p "$prefix"
prefix="$(cd "$prefix" && pwd)"
build="${OM_FFMPEG_BUILD_DIR:-$prefix-build}"
mkdir -p "$build"

say() { echo "ffmpeg-build: $*" >&2; }

# --- source -----------------------------------------------------------------
src="${FFMPEG_SOURCE_DIR:-$build/src}"
if [ ! -f "$src/configure" ]; then
    say "cloning $FFMPEG_GIT at $FFMPEG_TAG"
    rm -rf "$src"
    # LF line endings even on Windows: configure is a shell script.
    git clone --quiet --depth 1 --branch "$FFMPEG_TAG" -c core.autocrlf=false "$FFMPEG_GIT" "$src"
fi
head="$(git -C "$src" rev-parse HEAD)"
if [ "$head" != "$FFMPEG_COMMIT" ]; then
    say "source is at $head, expected $FFMPEG_COMMIT ($FFMPEG_TAG)"
    exit 1
fi

# --- platform ----------------------------------------------------------------
case "$(uname -s)" in
    Linux)
        platform=linux
        platform_flags=(--enable-indev=v4l2)
        ;;
    Darwin)
        platform=macos
        # @rpath install names: the app finds the libraries through its own
        # rpath (lib/ next to the executable), wherever the archive is unpacked.
        platform_flags=(
            --install-name-dir=@rpath
            --enable-avfoundation --enable-indev=avfoundation
            --enable-securetransport
        )
        ;;
    MINGW*|MSYS*|CYGWIN*)
        platform=windows
        # Win32 threads (no libwinpthread DLL) and a static libgcc, so the
        # DLLs depend only on system libraries.
        platform_flags=(
            --target-os=mingw64
            --enable-indev=dshow
            --enable-schannel
            --disable-pthreads --enable-w32threads
            --extra-ldflags=-static-libgcc
        )
        ;;
    *)
        say "unsupported platform $(uname -s)"
        exit 1
        ;;
esac

# --- components --------------------------------------------------------------
components=()
while IFS= read -r line; do
    line="${line%%#*}"
    line="${line//[[:space:]]/}"
    [ -n "$line" ] && components+=("$line")
done < "$here/components.txt"

# No GPL or non-free parts (the defaults), no autodetected external
# libraries, no programs, no documentation, shared libraries only.
flags=(
    --prefix="$prefix"
    --disable-everything
    --disable-autodetect
    --disable-programs
    --disable-doc
    --disable-static --enable-shared
    --disable-avfilter
    --disable-debug
    --enable-pic
    --extra-version=openmapper
    "${components[@]}"
    "${platform_flags[@]}"
)

say "configuring in $build ($platform)"
mkdir -p "$build/out"
(
    cd "$build/out"
    "$src/configure" "${flags[@]}" > configure.log 2>&1 || {
        cat configure.log >&2
        [ -f ffbuild/config.log ] && tail -40 ffbuild/config.log >&2
        exit 1
    }
    jobs="${OM_FFMPEG_JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)}"
    say "building with $jobs jobs"
    make -j"$jobs" > make.log 2>&1 || { tail -60 make.log >&2; exit 1; }
    make install > install.log 2>&1 || { tail -40 install.log >&2; exit 1; }
)

# Windows: configure installs the MSVC import libraries (*.lib) next to the
# DLLs; the Rust bindings look for them in lib/ (FFMPEG_DIR layout).
if [ "$platform" = windows ]; then
    for f in "$prefix"/bin/*.lib; do
        [ -f "$f" ] && mv -f "$f" "$prefix/lib/"
    done
fi

# --- compliance record -------------------------------------------------------
# Everything a recipient needs to know what they got and how to rebuild it.
rec="$prefix/share/openmapper-ffmpeg"
mkdir -p "$rec"
cp "$src/COPYING.LGPLv2.1" "$src/LICENSE.md" "$rec/"
cp "$here/build.sh" "$here/components.txt" "$rec/"
{
    echo "FFmpeg shared libraries bundled with OpenMapper"
    echo
    echo "Version:   $FFMPEG_TAG (version string suffix: openmapper)"
    echo "Source:    $FFMPEG_GIT"
    echo "Commit:    $FFMPEG_COMMIT"
    echo "Licence:   LGPL-2.1-or-later (COPYING.LGPLv2.1; see LICENSE.md)."
    echo "           Built without --enable-gpl, --enable-nonfree or any"
    echo "           external library."
    echo "Platform:  $platform ($(uname -m))"
    echo "Recipe:    build.sh with components.txt (this folder), i.e."
    echo
    printf '  configure'
    for f in "${flags[@]}"; do
        case "$f" in
            --prefix=*) ;;
            *) printf ' %s' "$f" ;;
        esac
    done
    echo
    echo
    echo "The complete corresponding source is the commit above; a source"
    echo "archive (ffmpeg-$FFMPEG_TAG-src.tar.gz) is published with every"
    echo "OpenMapper release that contains these libraries. The libraries are"
    echo "ordinary shared libraries loaded from lib/ (or next to the"
    echo "executables on Windows) and may be replaced by any ABI-compatible"
    echo "FFmpeg $FFMPEG_TAG build (docs/media/ffmpeg.md)."
} > "$rec/SOURCE.txt"

if [ "${OM_FFMPEG_SOURCE_ARCHIVE:-0}" = 1 ]; then
    say "writing source archive"
    git -C "$src" archive --format=tar.gz --prefix="ffmpeg-$FFMPEG_TAG/" \
        -o "$rec/ffmpeg-$FFMPEG_TAG-src.tar.gz" "$FFMPEG_COMMIT"
fi

say "installed in $prefix"
ls "$prefix/lib" "$prefix/bin" 2>/dev/null | grep -Ei '\.(so\.[0-9]+|[0-9]+\.dylib|dll)$' | sed 's/^/  /' >&2 || true
