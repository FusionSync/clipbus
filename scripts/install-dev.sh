#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)

prefix=${PREFIX:-/usr/local}
destdir=${DESTDIR:-}
libdir=${LIBDIR:-"$prefix/lib"}
includedir=${INCLUDEDIR:-"$prefix/include"}
pkgconfigdir=${PKGCONFIGDIR:-"$libdir/pkgconfig"}
cmakedir=${CMAKEDIR:-"$libdir/cmake/Clipbus"}
soname_major=${SONAME_MAJOR:-0}
build_profile=${BUILD_PROFILE:-debug}

case "$build_profile" in
    debug)
        cargo_profile_args=()
        artifact_dir=target/debug
        ;;
    release)
        cargo_profile_args=(--release)
        artifact_dir=target/release
        ;;
    *)
        echo "install-dev: BUILD_PROFILE must be debug or release" >&2
        exit 1
        ;;
esac

install_path() {
    printf '%s%s' "$destdir" "$1"
}

install_file() {
    local mode=$1
    local source=$2
    local target=$3
    install -D -m "$mode" "$source" "$(install_path "$target")"
}

install_text_template() {
    local mode=$1
    local source=$2
    local target=$3
    local rendered
    rendered=$(mktemp)
    sed \
        -e "s|@prefix@|$prefix|g" \
        -e "s|@version@|$version|g" \
        -e "s|@includedir@|$includedir|g" \
        -e "s|@libdir@|$libdir|g" \
        "$source" > "$rendered"
    install_file "$mode" "$rendered" "$target"
    rm -f "$rendered"
}

cd "$repo_dir"

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || {
    echo "install-dev: could not read package version" >&2
    exit 1
}

CLIPBUS_SONAME_MAJOR="$soname_major" \
    cargo build --locked "${cargo_profile_args[@]}"

install_file 0644 include/clipbus.h "$includedir/clipbus.h"

versioned_lib="$libdir/libclipbus.so.$version"
install_file 0755 "$artifact_dir/libclipbus.so" "$versioned_lib"
ln -sfn "libclipbus.so.$version" "$(install_path "$libdir/libclipbus.so.$soname_major")"
ln -sfn "libclipbus.so.$soname_major" "$(install_path "$libdir/libclipbus.so")"

install_file 0644 "$artifact_dir/libclipbus.a" "$libdir/libclipbus.a"

install_text_template 0644 pkgconfig/clipbus.pc.in "$pkgconfigdir/clipbus.pc"
install_text_template 0644 cmake/ClipbusConfig.install.cmake.in \
    "$cmakedir/ClipbusConfig.cmake"

echo "installed clipbus $version into $(install_path "$prefix")"
