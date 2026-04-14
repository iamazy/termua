#!/usr/bin/env bash
set -euo pipefail

grep -Eq '\.VolumeIcon\.icns' packaging/macos/make-dmg.sh
grep -Eq 'SetFile' packaging/macos/make-dmg.sh
grep -Eq 'hdiutil attach' packaging/macos/make-dmg.sh
grep -Eq 'hdiutil convert' packaging/macos/make-dmg.sh
