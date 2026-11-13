#!/usr/bin/env bash
# Builds the agent for every platform this machine can build and copies the binaries,
# named the way the server's /dl serves them, into an agents directory:
#   scripts/package-agents.sh /var/lib/deskhand/agents
# The default is ./agents (next to a ./deskhand.db). Platforms that fail to build are
# reported and skipped; the server can still fetch those from the matching GitHub
# release unless it runs with DESKHAND_AGENT_SOURCE=local.
#
# Needs: cargo-zigbuild + zig for the static Linux builds (see README), and for Windows
# the mingw/vcpkg setup in scripts/build-windows.sh.
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "${1:-agents}")
mkdir -p "$out"
target_dir=${CARGO_TARGET_DIR:-$root/target}
built=()
failed=()

copy() { # target-triple binary-name served-name
	install -m 0755 "$target_dir/$1/release/$2" "$out/$3" && built+=("$3")
}

cd "$root"
for t in x86_64-unknown-linux-musl:linux-x86_64 aarch64-unknown-linux-musl:linux-aarch64; do
	triple=${t%%:*}
	name=deskhand-agent-${t##*:}
	echo "== $name ($triple)"
	if rustup target add "$triple" >/dev/null 2>&1 &&
		cargo zigbuild --release --target "$triple" -p deskhand-agent; then
		copy "$triple" deskhand-agent "$name" || failed+=("$name")
	else
		failed+=("$name")
	fi
done

echo "== deskhand-agent-windows-x86_64.exe"
if "$root/scripts/build-windows.sh"; then
	copy x86_64-pc-windows-gnu deskhand-agent.exe deskhand-agent-windows-x86_64.exe || failed+=(deskhand-agent-windows-x86_64.exe)
else
	failed+=(deskhand-agent-windows-x86_64.exe)
fi

# macOS builds only on a Mac.
if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ]; then
	echo "== deskhand-agent-macos-aarch64"
	if cargo build --release -p deskhand-agent; then
		install -m 0755 "$target_dir/release/deskhand-agent" "$out/deskhand-agent-macos-aarch64" && built+=(deskhand-agent-macos-aarch64)
	else
		failed+=(deskhand-agent-macos-aarch64)
	fi
fi

echo
echo "in $out:"
for f in "${built[@]}"; do echo "  ok    $f"; done
for f in "${failed[@]}"; do echo "  FAIL  $f"; done
[ ${#built[@]} -gt 0 ]
