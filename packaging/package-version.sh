#!/usr/bin/env bash

get_termua_package_version() {
  local cargo_toml="${1:-Cargo.toml}"
  local package_version

  package_version="$(
    awk '
      $0 ~ /^\[workspace\.package\]/ { in_workspace_package = 1; next }
      in_workspace_package && $0 ~ /^\[/ { in_workspace_package = 0 }
      in_workspace_package && $0 ~ /^version[[:space:]]*=/ {
        line = $0
        sub(/^[^"]*"/, "", line)
        sub(/".*$/, "", line)
        print line
        exit
      }
    ' "$cargo_toml" 2>/dev/null || true
  )"

  if [[ -z "$package_version" ]]; then
    echo "failed to determine workspace package version from $cargo_toml" >&2
    return 1
  fi

  printf '%s\n' "$package_version"
}
