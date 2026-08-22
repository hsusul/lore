#!/usr/bin/env bash
# Toolchain pin consistency guard.
#
# The Rust version is pinned in two places that cannot reference each other:
#   1. rust-toolchain.toml  — what rustup uses locally and in CI.
#   2. dtolnay/rust-toolchain@<version> refs in .github/workflows/*.yml — what
#      the CI runner pre-installs (the action does not read the toml file).
#
# If those drift, CI silently pre-installs one compiler while rustup builds with
# another. This fails the build instead. Also verifies the active rustc actually
# matches, so a stale local override is caught too.
set -euo pipefail
cd "$(dirname "$0")/.."

fail() { echo "FAIL: $*" >&2; exit 1; }

pinned="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\(.*\)"/\1/p' rust-toolchain.toml)"
[ -n "$pinned" ] || fail "could not read [toolchain] channel from rust-toolchain.toml"
echo "rust-toolchain.toml pins: $pinned"

# Every workflow reference to the action must name the same version.
refs="$(grep -rhoE 'dtolnay/rust-toolchain@[^ ]+' .github/workflows/ | sed 's|.*@||' | sort -u)"
[ -n "$refs" ] || fail "no dtolnay/rust-toolchain reference found in .github/workflows/"
while read -r ref; do
  [ "$ref" = "$pinned" ] || fail "workflow uses dtolnay/rust-toolchain@$ref but rust-toolchain.toml pins $pinned"
done <<< "$refs"
echo "ok: all workflow action refs are @$pinned"

# And the compiler actually in use must be that version.
active="$(rustc --version | awk '{print $2}')"
[ "$active" = "$pinned" ] || fail "active rustc is $active but rust-toolchain.toml pins $pinned"
echo "ok: active rustc is $active"
