#!/usr/bin/env bash
# Generate a memory vault of N valid notes for the `lorectl mem` perf budget.
#
#   scripts/gen-vault.sh 5000 /tmp/lore-program/m1/perf-vault
#
# The notes are written directly as files and committed once, so the script
# measures nothing but itself. Word choice is seeded, so two runs with the same
# N produce the same vault. Refuses to write into a directory that exists.
set -euo pipefail

count="${1:?usage: gen-vault.sh <count> <vault-dir>}"
vault="${2:?usage: gen-vault.sh <count> <vault-dir>}"
case "$vault" in
  /*) ;;
  *) echo "gen-vault: the vault directory must be absolute" >&2; exit 1 ;;
esac
[ -e "$vault" ] && { echo "gen-vault: $vault already exists" >&2; exit 1; }

words=(worktree branch commit rebase index lock sqlite vault note anchor
  session archive agent hook recall search ranking token migration schema
  cache build release toolchain clippy format test fixture parser render
  frontmatter scope repo origin remote fetch merge conflict stash patch
  diff blame bisect tag signing identity runner sandbox network egress
  deadline budget latency throughput memory freshness provenance timeline)
types=(convention decision fact howto pitfall)
repos=(3f9a1c0b a71e9d24c3 5be0c1d2e3)

RANDOM=42
n=${#words[@]}

mkdir -p "$vault/global"
for repo in "${repos[@]}"; do mkdir -p "$vault/repos/$repo"; done

for ((i = 1; i <= count; i++)); do
  title="${words[RANDOM % n]} ${words[RANDOM % n]} ${words[RANDOM % n]} ${words[RANDOM % n]} $i"
  slug="${title// /-}"
  type="${types[i % 5]}"
  if ((i % 4 == 0)); then
    scope="global"
    dir="global"
  else
    repo="${repos[i % 3]}"
    scope="repo:$repo"
    dir="repos/$repo"
  fi
  body=""
  for ((w = 0; w < 60; w++)); do body+="${words[RANDOM % n]} "; done
  printf -- '---\nid: 01JD%022d\ntype: %s\ntitle: %s\nscope: %s\nstatus: active\ncreated: 2026-10-%02d\n---\n%s\n\n## Timeline\n- 2026-10-%02d generated\n' \
    "$i" "$type" "$title" "$scope" $((i % 28 + 1)) "$body" $((i % 28 + 1)) \
    >"$vault/$dir/$slug.md"
done

printf '.lore/\n_inbox/\n' >"$vault/.gitignore"
git -C "$vault" init -q
git -C "$vault" add -A
git -C "$vault" -c user.name=Lore -c user.email=lore@localhost commit -q -m "mem: generate $count notes"
echo "generated $count notes in $vault"
