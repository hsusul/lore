#!/usr/bin/env bash
# OS-level & dependency egress guard for the archive module (`lore-core`).
#
# The archive must have no network capability (AGENTS.md; SECURITY.md §7). Two
# static call-site guards already assert the source references no networking
# APIs (`no_network_in_archive`, `no_egress`). This adds the two checks those
# cannot make:
#   1. Dependency graph — no networking crate is present in lore-core's normal
#      dependency tree, so a transitive dependency cannot phone home.
#   2. Runtime — lore-core's test suite (the archive workflow: discover, scan,
#      ingest, search, forget) runs to completion with OS-level network access
#      denied, proving it needs and attempts zero outbound connections.
#
# Runs on macOS (sandbox-exec) and Linux (unshare -rn / CI). Exit non-zero on
# any violation.
set -euo pipefail
cd "$(dirname "$0")/.."

# Networking crates that must never appear in the archive's normal deps. Include
# async-net runtimes (tokio/mio/async-std): lore-core is fully synchronous, so
# their presence would itself signal a networking dependency creeping in.
NET_RE='(^| )(reqwest|hyper|h2|http-body|ureq|attohttpc|isahc|curl|surf|native-tls|rustls|openssl|openssl-sys|socket2|trust-dns|hickory|quinn|tonic|tokio|mio|async-std|async-io|smol)( |$)'

echo "== egress check 1: no networking crate in lore-core's dependency graph =="
tree="$(cargo tree -p lore-core -e normal --prefix none 2>/dev/null | sed 's/ v[0-9].*//' | sort -u)"
if echo "$tree" | grep -iE "$NET_RE"; then
  echo "FAIL: a networking crate is present in lore-core's normal dependency graph"
  exit 1
fi
echo "ok: none of $(echo "$tree" | wc -l | tr -d ' ') normal deps are networking crates"

echo "== egress check 2: archive test suite runs with network denied =="
# Compile with network available (CI fetches deps in an earlier step) and ask
# cargo for the exact test executable, then run it with the network taken away.
bin="$(cargo test -p lore-core --lib --no-run --message-format=json -q \
  | python3 -c 'import sys, json
for line in sys.stdin:
    try:
        o = json.loads(line)
    except ValueError:
        continue
    t = o.get("target", {})
    if (o.get("reason") == "compiler-artifact" and t.get("name") == "lore_core"
            and "lib" in t.get("kind", []) and o.get("profile", {}).get("test")
            and o.get("executable")):
        print(o["executable"])' | tail -1)"
[ -n "$bin" ] || { echo "FAIL: could not locate the lore-core test binary"; exit 1; }

# Probe that the OS-level network-deny wrapper is actually permitted here (some
# CI runners restrict user namespaces). If it is not, the static guards above
# still stand; skip the runtime check rather than fail spuriously.
case "$(uname -s)" in
  Darwin)
    if ! sandbox-exec -p '(version 1)(allow default)' /usr/bin/true 2>/dev/null; then
      echo "note: sandbox-exec unavailable; skipping runtime check 2"; exit 0
    fi
    wrapper=(sandbox-exec -p '(version 1)(allow default)(deny network*)')
    ;;
  Linux)
    if ! unshare -rn /bin/true 2>/dev/null; then
      echo "note: unshare -rn not permitted here; skipping runtime check 2"; exit 0
    fi
    wrapper=(unshare -rn)
    ;;
  *)
    echo "note: no runtime network-deny wrapper for this OS; skipping check 2"
    exit 0
    ;;
esac

echo "running the archive test suite with network denied: $bin"
"${wrapper[@]}" "$bin"
echo "ok: the archive test suite passed with OS-level network access denied"
