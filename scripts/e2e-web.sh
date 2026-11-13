#!/usr/bin/env bash
# End-to-end check of the web app: starts a server with a fresh database, enrolls and
# runs an agent on a throwaway X display, then runs the Playwright tests in web/e2e.
# Screenshots go to $DESKHAND_SHOTS if set.
#   scripts/e2e-web.sh            (builds debug binaries first)
#   PW_CHROMIUM=/path/to/chrome   use a Chromium other than Playwright's own
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
port=${DESKHAND_E2E_PORT:-18123}
display=:${DESKHAND_E2E_DISPLAY:-93}
pids=()
cleanup() {
	for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
	wait 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT

(cd "$root/web" && bun run build >/dev/null)
cargo build -q -p deskhand-server -p deskhand-agent --manifest-path "$root/Cargo.toml"
bin=$root/target/debug

Xvfb "$display" -screen 0 1280x720x24 >/dev/null 2>&1 & pids+=($!)
sleep 0.5
DISPLAY=$display xclock >/dev/null 2>&1 & pids+=($!)

DESKHAND_ADDR=127.0.0.1:$port DESKHAND_DB=$work/e2e.db DESKHAND_ADMIN_PASSWORD=admin-password DESKHAND_AGENT_SOURCE=local DESKHAND_LOGIN_RATE=100 RUST_LOG=warn \
	"$bin/deskhand-server" >"$work/server.log" 2>&1 & pids+=($!)
base=http://127.0.0.1:$port
for _ in $(seq 50); do curl -sf "$base/api/v1/auth" >/dev/null && break; sleep 0.1; done

jar=$work/cookies
curl -sf -c "$jar" -H 'content-type: application/json' -d '{"username":"admin","password":"admin-password"}' "$base/api/v1/login" >/dev/null
token=$(curl -sf -b "$jar" -H 'content-type: application/json' -d '{"hours":1,"max_uses":1}' "$base/api/v1/enroll-tokens" | sed 's/.*"token":"\([0-9a-f]*\)".*/\1/')
host=e2e-agent
export DESKHAND_CONFIG=$work/agent.json HOSTNAME=$host
"$bin/deskhand-agent" enroll --server "$base" --token "$token" >/dev/null
DISPLAY=$display "$bin/deskhand-agent" >"$work/agent.log" 2>&1 & pids+=($!)
for _ in $(seq 100); do
	curl -sf -b "$jar" "$base/api/v1/devices?status=online" | grep -q "$host" && break
	sleep 0.1
done

cd "$root/web"
DESKHAND_URL=$base DESKHAND_DEVICE=$host bunx playwright test "$@" || { tail -n 20 "$work/server.log" "$work/agent.log"; exit 1; }
