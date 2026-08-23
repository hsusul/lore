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

# Crates that can open an outbound connection and have no local-only purpose:
# HTTP/WebSocket clients and servers, TLS-for-transport, DNS resolvers, QUIC.
# Deliberately NOT listed are low-level IO primitives (mio, socket2, tokio,
# async-io) — they have legitimate non-network uses and appear here anyway via
# the file watcher on Linux (notify -> mio), so matching them would false-fail.
NET_RE='(^| )(reqwest|hyper|h2|ureq|attohttpc|isahc|curl|surf|trust-dns|hickory|quinn|tonic|native-tls|rustls|warp|axum|actix-web|rocket|tungstenite)( |$)'

echo "== egress check 1: no networking crate in lore-core's dependency graph =="
tree="$(cargo tree -p lore-core -e normal --prefix none 2>/dev/null | sed 's/ v[0-9].*//' | sort -u)"
if echo "$tree" | grep -iE "$NET_RE"; then
  echo "FAIL: a networking crate is present in lore-core's normal dependency graph"
  exit 1
fi
echo "ok: none of $(echo "$tree" | wc -l | tr -d ' ') normal deps are networking crates"

echo "== egress check 2: archive test suite runs with network denied =="
# Compile with network available (CI fetches deps in an earlier step) and ask
# cargo for every test executable, then run each with the network taken away.
#
# This must cover the INTEGRATION binaries, not just the lib unit tests: the
# workflow SECURITY.md §1.5 promises to exercise (discover, scan, ingest,
# search, export, forget) lives in crates/lore-core/tests/*.rs, and each of
# those is a separate binary. Covering only --lib would leave the claim
# unsupported by the check that is supposed to prove it.
# Read into an array with a while-loop rather than `mapfile`, which needs bash 4
# (macOS still ships bash 3.2 as /bin/bash).
bins=()
while IFS= read -r line; do
  bins+=("$line")
done < <(cargo test -p lore-core --no-run --message-format=json -q \
  | python3 -c 'import sys, json
for line in sys.stdin:
    try:
        o = json.loads(line)
    except ValueError:
        continue
    if (o.get("reason") == "compiler-artifact"
            and o.get("profile", {}).get("test")
            and o.get("executable")):
        print(o["executable"])' | sort -u)
[ "${#bins[@]}" -gt 0 ] || { echo "FAIL: could not locate any lore-core test binary"; exit 1; }
echo "found ${#bins[@]} test binaries"

# Probe that the OS-level network-deny wrapper is actually permitted here (some
# CI runners restrict user namespaces). If it is not, the static guards above
# still stand; skip the runtime check rather than fail spuriously.
#
# A skip is LOUD, not silent: set LORE_EGRESS_REQUIRE_RUNTIME=1 (CI does) so a
# missing wrapper fails the job. Otherwise a green pipeline would report that a
# check passed when it never ran.
skip_runtime() {
  if [ "${LORE_EGRESS_REQUIRE_RUNTIME:-}" = "1" ]; then
    echo "FAIL: $1, and LORE_EGRESS_REQUIRE_RUNTIME=1 requires the runtime check to run"
    exit 1
  fi
  # Surface it as a CI annotation too, so a skip is visible in the run summary
  # rather than buried in the log.
  [ -n "${GITHUB_ACTIONS:-}" ] && echo "::warning::egress runtime check skipped — $1"
  echo "note: $1; skipping runtime check 2"
  exit 0
}

case "$(uname -s)" in
  Darwin)
    if ! sandbox-exec -p '(version 1)(allow default)' /usr/bin/true 2>/dev/null; then
      skip_runtime "sandbox-exec unavailable"
    fi
    wrapper=(sandbox-exec -p '(version 1)(allow default)(deny network*)')
    ;;
  Linux)
    # Ubuntu 24.04 restricts unprivileged user namespaces via AppArmor, so
    # `unshare -rn` can be unavailable on an otherwise ordinary runner. Fall
    # back to bubblewrap before giving up.
    if unshare -rn /bin/true 2>/dev/null; then
      wrapper=(unshare -rn)
    elif command -v bwrap >/dev/null 2>&1 \
      && bwrap --unshare-net --dev-bind / / /bin/true 2>/dev/null; then
      wrapper=(bwrap --unshare-net --dev-bind / /)
    else
      skip_runtime "no usable network-deny wrapper (tried unshare -rn and bwrap)"
    fi
    ;;
  *)
    skip_runtime "no runtime network-deny wrapper for this OS"
    ;;
esac

for bin in "${bins[@]}"; do
  echo "-- network denied: $(basename "$bin")"
  "${wrapper[@]}" "$bin"
done
echo "ok: all ${#bins[@]} archive test binaries passed with OS-level network access denied"
