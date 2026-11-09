#!/usr/bin/env bash
# Builds Ghostty's libghostty-vt as a static library for macOS arm64 at the revision pinned in
# vendor/libghostty-vt/VERSION, and puts the .a and the headers in vendor/libghostty-vt
# (ADR-199). The `ghostty` cargo feature links it; without the feature nothing here is needed.
#
# Zig 0.15.2 is downloaded into ~/.cache/sirus when no matching `zig` is on PATH (the same
# approach as T3 Code's build-libghostty-wasm.sh). Network is needed the first time.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPIKE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR_DIR="${SPIKE_DIR}/vendor/libghostty-vt"

GHOSTTY_REVISION="$(tr -d '[:space:]' < "${VENDOR_DIR}/VERSION")"
CACHE_ROOT="${SIRUS_CACHE_DIR:-${HOME}/.cache/sirus}"
GHOSTTY_SOURCE_DIR="${GHOSTTY_SOURCE_DIR:-${CACHE_ROOT}/ghostty-${GHOSTTY_REVISION:0:8}}"
GHOSTTY_ZIG_VERSION="${GHOSTTY_ZIG_VERSION:-0.15.2}"
GHOSTTY_ZIG="${GHOSTTY_ZIG:-}"
GHOSTTY_TARGET="${GHOSTTY_TARGET:-aarch64-macos}"

log() {
  printf '[libghostty-vt] %s\n' "$*"
}

die() {
  printf '[libghostty-vt] error: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

ensure_zig() {
  if [[ -n "${GHOSTTY_ZIG}" ]]; then
    [[ -x "${GHOSTTY_ZIG}" ]] || die "GHOSTTY_ZIG is not executable: ${GHOSTTY_ZIG}"
    return
  fi
  if command -v zig >/dev/null 2>&1 && [[ "$(zig version)" == "${GHOSTTY_ZIG_VERSION}" ]]; then
    GHOSTTY_ZIG="$(command -v zig)"
    return
  fi

  local host_os host_arch cache_dir
  host_os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  host_arch="$(uname -m)"
  case "${host_os}" in
    darwin) host_os="macos" ;;
    linux) ;;
    *) die "unsupported host OS for Zig download: ${host_os}" ;;
  esac
  case "${host_arch}" in
    arm64) host_arch="aarch64" ;;
    aarch64 | x86_64) ;;
    *) die "unsupported host architecture: ${host_arch}" ;;
  esac

  cache_dir="${CACHE_ROOT}/zig-${GHOSTTY_ZIG_VERSION}"
  GHOSTTY_ZIG="${cache_dir}/zig"
  if [[ -x "${GHOSTTY_ZIG}" ]]; then
    return
  fi

  require_cmd curl
  require_cmd tar
  mkdir -p "${cache_dir}"
  log "downloading Zig ${GHOSTTY_ZIG_VERSION}"
  curl -fsSL \
    "https://ziglang.org/download/${GHOSTTY_ZIG_VERSION}/zig-${host_arch}-${host_os}-${GHOSTTY_ZIG_VERSION}.tar.xz" \
    | tar -xJ --strip-components=1 -C "${cache_dir}"
}

ensure_ghostty_source() {
  require_cmd git
  if [[ ! -d "${GHOSTTY_SOURCE_DIR}/.git" ]]; then
    mkdir -p "$(dirname "${GHOSTTY_SOURCE_DIR}")"
    log "cloning Ghostty ${GHOSTTY_REVISION}"
    git clone --filter=blob:none --no-checkout https://github.com/ghostty-org/ghostty.git \
      "${GHOSTTY_SOURCE_DIR}"
  fi

  # A cached checkout may still be on a previously pinned revision; converge on the pinned one.
  local actual_revision
  actual_revision="$(git -C "${GHOSTTY_SOURCE_DIR}" rev-parse HEAD 2>/dev/null || echo none)"
  if [[ "${actual_revision}" != "${GHOSTTY_REVISION}" ]]; then
    log "checking out Ghostty ${GHOSTTY_REVISION}"
    git -C "${GHOSTTY_SOURCE_DIR}" fetch --depth=1 origin "${GHOSTTY_REVISION}"
    git -C "${GHOSTTY_SOURCE_DIR}" checkout --detach "${GHOSTTY_REVISION}"
  fi

  actual_revision="$(git -C "${GHOSTTY_SOURCE_DIR}" rev-parse HEAD)"
  [[ "${actual_revision}" == "${GHOSTTY_REVISION}" ]] || \
    die "expected Ghostty ${GHOSTTY_REVISION}, found ${actual_revision}"
}

# The libSystem stubs of recent macOS SDKs (26.x, 27.x) list only arm64e slices, so Zig 0.15.2
# cannot link its own build runner against them (undefined `_abort`, `_malloc_size`, …). And the
# 27.x headers define INFINITY through clang's newer float.h (`__need_infinity_nan`), which Zig's
# clang lacks, so its libc++ (needed for the shared libghostty-vt the build also emits) does not
# compile. When either bites, point Zig at a shim SDK: the headers and frameworks of the newest
# installed SDK whose math.h still defines INFINITY itself (or GHOSTTY_MACOS_SDK), with the
# libSystem stubs Zig ships. Sirus links only the static library, so nothing it runs comes from
# the shim.
sdk_headers_ok() {
  [[ -f "$1/usr/include/math.h" ]] && ! grep -q '__need_infinity_nan' "$1/usr/include/math.h"
}

pick_header_sdk() {
  if [[ -n "${GHOSTTY_MACOS_SDK:-}" ]]; then
    echo "${GHOSTTY_MACOS_SDK}"
    return
  fi
  local candidate
  candidate="$(/usr/bin/xcrun --sdk macosx --show-sdk-path)"
  if sdk_headers_ok "${candidate}"; then
    echo "${candidate}"
    return
  fi
  # Newest first, by the version in the folder's name.
  for candidate in $(
    for sdk in /Library/Developer/CommandLineTools/SDKs/MacOSX[0-9]*.sdk \
      /Applications/Xcode*.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX[0-9]*.sdk; do
      [[ -d "${sdk}" ]] || continue
      version="$(basename "${sdk}" .sdk)"
      printf '%s %s\n' "${version#MacOSX}" "${sdk}"
    done | sort -V -r | cut -d' ' -f2-
  ); do
    if sdk_headers_ok "${candidate}"; then
      echo "${candidate}"
      return
    fi
  done
  /usr/bin/xcrun --sdk macosx --show-sdk-path
}

work_around_new_macos_sdk() {
  [[ "$(uname -s)" == "Darwin" ]] || return 0
  local probe="${build_root}/probe"
  mkdir -p "${probe}/bin"
  printf 'pub fn main() void {}\n' > "${probe}/probe.zig"
  if (cd "${probe}" && "${GHOSTTY_ZIG}" build-exe probe.zig -lc >/dev/null 2>&1) && \
    sdk_headers_ok "$(/usr/bin/xcrun --sdk macosx --show-sdk-path)"; then
    return 0
  fi
  local real_sdk zig_lib shim
  real_sdk="$(pick_header_sdk)"
  zig_lib="$(dirname "${GHOSTTY_ZIG}")/lib"
  shim="${build_root}/sdk"
  log "using a shim SDK with the headers of ${real_sdk} and Zig's libSystem stubs"
  mkdir -p "${shim}/usr/lib"
  ln -s "${real_sdk}/usr/include" "${shim}/usr/include"
  ln -s "${real_sdk}/System" "${shim}/System"
  cp "${zig_lib}/libc/darwin/libSystem.tbd" "${shim}/usr/lib/libSystem.tbd"
  cp "${zig_lib}/libc/darwin/SDKSettings.json" "${shim}/SDKSettings.json"
  cat > "${probe}/bin/xcrun" <<SHIM
#!/bin/sh
case "\$*" in
  *--show-sdk-path*) echo "${shim}" ;;
  *) exec /usr/bin/xcrun "\$@" ;;
esac
SHIM
  chmod +x "${probe}/bin/xcrun"
  export PATH="${probe}/bin:${PATH}"
  (cd "${probe}" && "${GHOSTTY_ZIG}" build-exe probe.zig -lc >/dev/null) || \
    die "Zig ${GHOSTTY_ZIG_VERSION} cannot link on this Mac even with the shim SDK"
}

ensure_zig
ensure_ghostty_source

build_root="$(mktemp -d)"
trap 'rm -rf "${build_root}"' EXIT
work_around_new_macos_sdk

log "building ${GHOSTTY_REVISION} for ${GHOSTTY_TARGET}"
(
  cd "${GHOSTTY_SOURCE_DIR}"
  # The pinned revision rides along as semver build metadata, so the library names its own
  # provenance through ghostty_build_info().
  "${GHOSTTY_ZIG}" build \
    -Demit-lib-vt \
    -Demit-xcframework=false \
    -Dtarget="${GHOSTTY_TARGET}" \
    -Doptimize=ReleaseFast \
    -Dlib-version-string="0.1.0-dev+${GHOSTTY_REVISION}" \
    -p "${build_root}"
)

static_lib="$(find "${build_root}/lib" -name 'libghostty-vt*.a' -print -quit)"
[[ -n "${static_lib}" ]] || die "no static library in ${build_root}/lib: $(ls "${build_root}/lib")"

mkdir -p "${VENDOR_DIR}/lib"
cp "${static_lib}" "${VENDOR_DIR}/lib/libghostty-vt.a"
rm -rf "${VENDOR_DIR}/include/ghostty"
mkdir -p "${VENDOR_DIR}/include"
cp -R "${build_root}/include/ghostty" "${VENDOR_DIR}/include/ghostty"
cp "${GHOSTTY_SOURCE_DIR}/LICENSE" "${VENDOR_DIR}/LICENSE"
log "wrote ${VENDOR_DIR}/lib/libghostty-vt.a"
