#!/usr/bin/env bash
set -euo pipefail

grep -Eq 'termua-relay' packaging/linux/make-appimage.sh
grep -Eq 'termua-relay' packaging/macos/make-dmg.sh
grep -Eq 'termua-relay' packaging/windows/make-msi.ps1
grep -Eq 'termua-relay' termua/Cargo.toml
