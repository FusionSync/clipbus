#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
cc_bin=${CC:-cc}
pkg_config_bin=${PKG_CONFIG:-pkg-config}
readelf_bin=${READELF:-readelf}
soname_major=${SONAME_MAJOR:-0}

fail() {
    echo "error: $*" >&2
    exit 1
}

command -v "$cc_bin" >/dev/null || fail "cc is required"
command -v "$pkg_config_bin" >/dev/null || fail "pkg-config is required"
command -v "$readelf_bin" >/dev/null || fail "readelf is required"

unset PKG_CONFIG_SYSROOT_DIR

work_dir=$(mktemp -d)
cleanup() {
    rm -rf "$work_dir"
}
trap cleanup EXIT

prefix="$work_dir/prefix"
build_dir="$work_dir/build"
mkdir -p "$build_dir"

cd "$repo_dir"
PREFIX="$prefix" scripts/install-dev.sh > "$work_dir/install.log"

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || fail "could not read package version"

test -f "$prefix/include/clipbus.h" || fail "public header was not installed"
test -f "$prefix/lib/libclipbus.so.$version" \
    || fail "versioned shared library was not installed"
"$readelf_bin" -d "$prefix/lib/libclipbus.so.$version" \
    | grep -q "SONAME.*libclipbus.so.$soname_major" \
    || fail "shared library SONAME is not libclipbus.so.$soname_major"
test -L "$prefix/lib/libclipbus.so.$soname_major" \
    || fail "soname-major shared library link was not installed"
test -L "$prefix/lib/libclipbus.so" \
    || fail "linker shared library link was not installed"
test -f "$prefix/lib/libclipbus.a" \
    || fail "static library was not installed"
test -f "$prefix/lib/pkgconfig/clipbus.pc" \
    || fail "pkg-config file was not installed"

grep -q "^Version: $version$" "$prefix/lib/pkgconfig/clipbus.pc" \
    || fail "pkg-config version mismatch"
grep -q "^includedir=$prefix/include$" "$prefix/lib/pkgconfig/clipbus.pc" \
    || fail "pkg-config includedir mismatch"
grep -q "^libdir=$prefix/lib$" "$prefix/lib/pkgconfig/clipbus.pc" \
    || fail "pkg-config libdir mismatch"

export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
"$pkg_config_bin" --exists clipbus \
    || fail "installed pkg-config metadata is not discoverable"
pkg_flags=$("$pkg_config_bin" --cflags --libs clipbus)
case "$pkg_flags" in
    *"-I$prefix/include"*"-L$prefix/lib"*"-lclipbus"*) ;;
    *) fail "pkg-config flags do not reference installed public ABI" ;;
esac

cp examples/minimal_owner.c "$build_dir/"
"$cc_bin" -std=c11 -Wall -Wextra -Werror \
    "$build_dir/minimal_owner.c" $pkg_flags \
    "-Wl,-rpath,$prefix/lib" \
    -o "$build_dir/minimal_owner"
"$readelf_bin" -d "$build_dir/minimal_owner" \
    | grep -q "NEEDED.*libclipbus.so.$soname_major" \
    || fail "example does not depend on libclipbus.so.$soname_major"

stage="$work_dir/stage"
staged_prefix=/usr
DESTDIR="$stage" PREFIX="$staged_prefix" scripts/install-dev.sh \
    > "$work_dir/staged-install.log"
test -f "$stage$staged_prefix/include/clipbus.h" \
    || fail "DESTDIR public header was not staged"
test -f "$stage$staged_prefix/lib/libclipbus.so.$version" \
    || fail "DESTDIR shared library was not staged"
grep -q "^includedir=$staged_prefix/include$" \
    "$stage$staged_prefix/lib/pkgconfig/clipbus.pc" \
    || fail "DESTDIR pkg-config includedir incorrectly includes staging root"
grep -q "^libdir=$staged_prefix/lib$" \
    "$stage$staged_prefix/lib/pkgconfig/clipbus.pc" \
    || fail "DESTDIR pkg-config libdir incorrectly includes staging root"

echo "install metadata passed"

