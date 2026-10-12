#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf 'This script builds a local macOS test app.\n' >&2
  exit 1
fi

task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$task_root/src-tauri"

if cargo tauri --version >/dev/null 2>&1; then
  task_cli=(cargo tauri)
elif command -v npx >/dev/null 2>&1; then
  task_cli=(npx --yes @tauri-apps/cli@2)
else
  printf 'Install Tauri CLI 2 or Node.js/npm before building the test app.\n' >&2
  exit 1
fi

# Local testing only: rebuild the entire bundle and bind Info.plist/resources
# with an ad-hoc signature. Replacing only its executable invalidates signing.
"${task_cli[@]}" build --bundles app \
  --config '{"bundle":{"macOS":{"signingIdentity":"-"}}}' -- --locked

task_target_dir="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
task_triple_dir="${CARGO_BUILD_TARGET:+/$CARGO_BUILD_TARGET}"
task_app="$task_target_dir$task_triple_dir/release/bundle/macos/lotion-rs.app"
codesign --verify --deep --strict "$task_app"
printf 'Verified local test app: %s\n' "$task_app"
