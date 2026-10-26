#!/usr/bin/env bash
# Restore the session-archive desktop app from 502827d^ and delete the agent
# orchestrator that 502827d put in its place. Safe to rerun from the repo root:
# every step converges, so a second run leaves `git status` unchanged.
#
# Reviewers can rerun it on main and diff against the commit that added it.
set -euo pipefail

ARCHIVE_REV="502827d^"

if [[ ! -f Cargo.toml || ! -d src-tauri ]]; then
  echo "run from the repo root" >&2
  exit 1
fi

git rev-parse --verify --quiet "${ARCHIVE_REV}^{commit}" >/dev/null || {
  echo "missing ${ARCHIVE_REV}; fetch full history first" >&2
  exit 1
}

ORCHESTRATOR=(
  crates/lore-orchestrator
  src/components/workbench
  src-tauri/src/commands/tasks.rs
  src/mock/ipc-mock.ts
)

ARCHIVE=(
  src-tauri/src/commands/export.rs
  src-tauri/src/commands/folders.rs
  src-tauri/src/commands/mod.rs
  src-tauri/src/commands/repositories.rs
  src-tauri/src/commands/search.rs
  src-tauri/src/commands/sessions.rs
  src-tauri/src/commands/system.rs
  src-tauri/src/lib.rs
  src-tauri/src/state.rs
  src/App.tsx
  src/App.test.tsx
  src/main.tsx
  src/ipc.ts
  src/ipc.test.ts
  src/components
  src/hooks
  src/focus-trap.ts
  src/focus-trap.test.tsx
  src/virtual.ts
  src/virtual.test.ts
  src/styles.css
)

git rm -r -q --ignore-unmatch -- "${ORCHESTRATOR[@]}"
rm -rf -- "${ORCHESTRATOR[@]}"

git checkout "$ARCHIVE_REV" -- "${ARCHIVE[@]}"

# Keep a helper module only while the restored UI still imports it.
for module in diff format; do
  importers=$(grep -rlE "from \"(\./|\.\./)+${module}\"" src --include='*.ts' --include='*.tsx' \
    | grep -vE "^src/${module}(\.test)?\.ts$" || true)
  if [[ -z "$importers" ]]; then
    git rm -q --ignore-unmatch -- "src/${module}.ts" "src/${module}.test.ts"
  fi
done
