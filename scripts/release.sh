#!/usr/bin/env bash
#
# Publishes freyja to crates.io, by hand, from a maintainer's machine.
#
# There is no publish job in CI and no registry token in the repository, so
# this script is the only path to crates.io. It exists because publishing this
# workspace is not a single command: `freyja` depends on `freyja-macros`, and
# `cargo package` strips the `path` from that dependency and leaves the
# version, so the macro crate must already be on crates.io before the root
# crate is uploaded.
#
# Usage:
#   scripts/release.sh            # publish the version in Cargo.toml
#   scripts/release.sh --dry-run  # run every check and cargo's own dry run
#
# Requires a crates.io token in the environment or in ~/.cargo/credentials.toml
# (`cargo login`).

set -euo pipefail

cd "$(dirname "$0")/.."

dry_run=false
if [ "${1:-}" = "--dry-run" ]; then
  dry_run=true
elif [ $# -gt 0 ]; then
  echo "usage: $0 [--dry-run]" >&2
  exit 2
fi

root="$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)"
macros="$(grep '^version' macros/Cargo.toml | head -1 | cut -d'"' -f2)"
dep="$(grep '^freyja-macros' Cargo.toml | sed -E 's/.*version = "([^"]+)".*/\1/')"

echo "freyja           $root"
echo "freyja-macros    $macros"
echo "dependency decl  $dep"

# Caught here it costs nothing. Caught during publish it leaves a version on
# crates.io that resolves against one that was never uploaded.
if [ "$root" != "$macros" ] || [ "$root" != "$dep" ]; then
  echo "error: both manifests and the freyja-macros dependency must agree" >&2
  exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "error: working tree is dirty; commit or stash first" >&2
  exit 1
fi

# The published archive is built from the committed tree, so verify that tree
# rather than whatever happens to be in target/.
cargo test --workspace
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets
cargo publish -p freyja-macros --dry-run
cargo publish -p freyja --dry-run

if [ "$dry_run" = true ]; then
  echo "dry run only; nothing was uploaded"
  exit 0
fi

echo
echo "About to publish freyja-macros $macros and freyja $root to crates.io."
echo "crates.io is append-only: neither version can be replaced or reused."
read -r -p "Type the version to confirm: " confirm
if [ "$confirm" != "$root" ]; then
  echo "aborted" >&2
  exit 1
fi

# The macro crate goes first, and an already-published version is not an
# error. A run that uploads it and then fails leaves that version permanently
# taken, so re-running has to resume rather than dead-end on a number that can
# never be reused. Any other failure still stops the release.
if output=$(cargo publish -p freyja-macros 2>&1); then
  echo "$output"
else
  echo "$output"
  echo "$output" | grep -qE 'already (exists|uploaded)' || exit 1
  echo "freyja-macros $macros is already published; continuing."
fi

cargo publish -p freyja

echo
echo "Published. Tag the release if you have not already:"
echo "  git tag v$root && git push origin v$root"
