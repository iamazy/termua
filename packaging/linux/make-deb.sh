#!/usr/bin/env bash
set -euo pipefail

# Build a .deb for termua on Debian/Ubuntu using cargo-deb.
#
# Prereqs (Ubuntu):
#   - Rust toolchain (cargo)
#   - dpkg (dpkg-deb)
#   - cargo-deb (cargo install cargo-deb --locked)
#
# Usage:
#   packaging/linux/make-deb.sh
#   ARCH=x86_64 packaging/linux/make-deb.sh
#   ARCH=aarch64 packaging/linux/make-deb.sh
#   OUT_DIR=target/deb/x86_64 packaging/linux/make-deb.sh

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "make-deb.sh must run on Linux." >&2
  exit 1
fi

host_machine="$(uname -m)"
case "$host_machine" in
  x86_64) host_arch="x86_64" ;;
  aarch64|arm64) host_arch="aarch64" ;;
  *)
    echo "unsupported arch: $host_machine" >&2
    exit 1
    ;;
esac

explicit_target=0
if [[ -n "${ARCH-}" || -n "${TARGET-}" ]]; then
  explicit_target=1
fi

arch="${ARCH:-$host_arch}"
if [[ "$arch" != "$host_arch" ]]; then
  echo "ARCH=$arch does not match host arch ($host_arch). Cross-packaging is not supported by this script." >&2
  exit 1
fi

target=""
if [[ "$explicit_target" -eq 1 ]]; then
  default_target=""
  case "$arch" in
    x86_64) default_target="x86_64-unknown-linux-gnu" ;;
    aarch64) default_target="aarch64-unknown-linux-gnu" ;;
  esac
  target="${TARGET:-$default_target}"
fi

out_dir="${OUT_DIR:-target/deb/$arch}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "missing cargo; install Rust toolchain first" >&2
  exit 1
fi

if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "missing dpkg-deb; on Ubuntu: sudo apt-get install -y dpkg" >&2
  exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo-deb >/dev/null 2>&1; then
  echo "==> Installing cargo-deb (missing)"
  cargo install cargo-deb --locked
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v cargo-deb >/dev/null 2>&1; then
    echo "cargo-deb install finished but cargo-deb is still not in PATH" >&2
    echo "try: export PATH=\"$HOME/.cargo/bin:$PATH\"" >&2
    exit 1
  fi
fi

echo "==> Building termua + termua-relay (release)"
if [[ "$explicit_target" -eq 1 ]]; then
  cargo build -p termua --release --target "$target"
  cargo build -p termua_relay --release --target "$target"
else
  cargo build -p termua --release
  cargo build -p termua_relay --release
fi

echo "==> Packaging .deb (cargo deb)"
package_out_dir="$(mktemp -d)"
cleanup_package_out_dir() {
  rm -rf "$package_out_dir"
}
if [[ "$explicit_target" -eq 1 ]]; then
  work="$(mktemp -d)"
  cleanup() {
    if [[ -f "$work/termua.bak" ]]; then
      mkdir -p target/release
      mv -f "$work/termua.bak" target/release/termua
    else
      rm -f target/release/termua
    fi
    if [[ -f "$work/termua-relay.bak" ]]; then
      mkdir -p target/release
      mv -f "$work/termua-relay.bak" target/release/termua-relay
    else
      rm -f target/release/termua-relay
    fi
    rm -rf "$work"
    cleanup_package_out_dir
  }
  trap cleanup EXIT

  # termua/Cargo.toml deb metadata currently references ../target/release/termua
  # Ensure the expected path exists even when building with --target.
  built_bin="target/$target/release/termua"
  built_relay_bin="target/$target/release/termua-relay"
  expected_bin="target/release/termua"
  expected_relay_bin="target/release/termua-relay"
  mkdir -p target/release
  if [[ -f "$expected_bin" ]]; then
    mv -f "$expected_bin" "$work/termua.bak"
  fi
  if [[ -f "$expected_relay_bin" ]]; then
    mv -f "$expected_relay_bin" "$work/termua-relay.bak"
  fi
  cp "$built_bin" "$expected_bin"
  cp "$built_relay_bin" "$expected_relay_bin"
  chmod +x "$expected_bin"
  chmod +x "$expected_relay_bin"

  cargo deb -p termua --no-build --target "$target" --output "$package_out_dir"
else
  trap cleanup_package_out_dir EXIT
  cargo deb -p termua --no-build --output "$package_out_dir"
fi

deb_path="$(ls -1t "$package_out_dir"/*.deb 2>/dev/null | head -n 1 || true)"
if [[ -z "${deb_path}" || ! -f "${deb_path}" ]]; then
  echo "failed to locate built .deb in $package_out_dir" >&2
  exit 1
fi

mkdir -p "${out_dir}"
deb_name="$(basename "${deb_path}")"
debian_arch="$arch"
case "$arch" in
  x86_64) debian_arch="amd64" ;;
  aarch64) debian_arch="arm64" ;;
esac

# cargo-deb uses Debian arch names in the filename (e.g. amd64/arm64). Avoid appending our
# internal arch name (x86_64/aarch64) and ending up with both.
if [[ "${deb_name}" != *"${debian_arch}"* && "${deb_name}" != *"${arch}"* ]]; then
  deb_name="${deb_name%.deb}-${debian_arch}.deb"
fi
cp "${deb_path}" "${out_dir}/${deb_name}"

echo "==> Wrote: ${out_dir}/${deb_name}"
