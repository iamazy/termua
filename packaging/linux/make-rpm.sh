#!/usr/bin/env bash
set -euo pipefail

# Build an .rpm for termua on Linux using cargo-generate-rpm.
#
# Prereqs:
#   - Rust toolchain (cargo)
#   - rpmbuild (usually from the `rpm` package)
#
# Usage:
#   packaging/linux/make-rpm.sh
#   ARCH=x86_64 packaging/linux/make-rpm.sh
#   ARCH=aarch64 packaging/linux/make-rpm.sh
#   OUT_DIR=target/rpm/x86_64 packaging/linux/make-rpm.sh
#
# Output:
#   - Copies the newest .rpm from target/generate-rpm/ into OUT_DIR.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "make-rpm.sh must run on Linux." >&2
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

out_dir="${OUT_DIR:-target/rpm/$arch}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "missing cargo; install Rust toolchain first" >&2
  exit 1
fi

if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "missing rpmbuild; on Ubuntu: sudo apt-get install -y rpm" >&2
  exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cargo-generate-rpm >/dev/null 2>&1; then
  echo "==> Installing cargo-generate-rpm (missing)"
  cargo install cargo-generate-rpm --locked
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v cargo-generate-rpm >/dev/null 2>&1; then
    echo "cargo-generate-rpm install finished but cargo-generate-rpm is still not in PATH" >&2
    echo "try: export PATH=\"$HOME/.cargo/bin:$PATH\"" >&2
    exit 1
  fi
fi

echo "==> Building termua (release)"
if [[ "$explicit_target" -eq 1 ]]; then
  cargo build -p termua --release --target "$target"
else
  cargo build -p termua --release
fi

echo "==> Packaging .rpm (cargo generate-rpm)"
if [[ "$explicit_target" -eq 1 ]]; then
  # termua/Cargo.toml rpm metadata currently references ../target/release/termua
  # Ensure the expected path exists even when building with --target.
  work="$(mktemp -d)"
  cleanup() {
    if [[ -f "$work/termua.bak" ]]; then
      mkdir -p target/release
      mv -f "$work/termua.bak" target/release/termua
    else
      rm -f target/release/termua
    fi
    rm -rf "$work"
  }
  trap cleanup EXIT

  built_bin="target/$target/release/termua"
  expected_bin="target/release/termua"
  mkdir -p target/release
  if [[ -f "$expected_bin" ]]; then
    mv -f "$expected_bin" "$work/termua.bak"
  fi
  cp "$built_bin" "$expected_bin"
  chmod +x "$expected_bin"

  # cargo-generate-rpm writes into target/generate-rpm by default.
  cargo generate-rpm -p termua --target "$target"
else
  cargo generate-rpm -p termua
fi

rpm_path="$(
  ls -1t target/generate-rpm/*.rpm target/generate-rpm/rpms/*.rpm 2>/dev/null | head -n 1 || true
)"
if [[ -z "${rpm_path}" || ! -f "${rpm_path}" ]]; then
  echo "failed to locate built .rpm under target/generate-rpm" >&2
  exit 1
fi

mkdir -p "${out_dir}"
rpm_name="$(basename "${rpm_path}")"
if [[ "${rpm_name}" != *"${arch}"* ]]; then
  rpm_name="${rpm_name%.rpm}-${arch}.rpm"
fi
cp "${rpm_path}" "${out_dir}/${rpm_name}"

echo "==> Wrote: ${out_dir}/${rpm_name}"
