#!/usr/bin/env bash
set -euo pipefail

grep -Eq '^build = "build\.rs"$' termua/Cargo.toml
grep -Eq 'termua\.ico' termua/build.rs
