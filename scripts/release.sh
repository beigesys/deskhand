#!/usr/bin/env bash
# Bumps the workspace version to X.Y.Z, commits "Release vX.Y.Z" and tags it.
# Pushing the tag (printed at the end) starts .github/workflows/release.yml.
set -euo pipefail
v=${1:?usage: scripts/release.sh X.Y.Z}
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "not a version: $v" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean" >&2; exit 1; }
git rev-parse -q --verify "refs/tags/v$v" >/dev/null && { echo "tag v$v exists" >&2; exit 1; }
# Only the version line inside [workspace.package].
sed -i "/^\[workspace.package\]/,/^\[/s/^version = \".*\"/version = \"$v\"/" Cargo.toml
grep -q "^version = \"$v\"" Cargo.toml || { echo "couldn't set the version in Cargo.toml" >&2; exit 1; }
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null
git commit -q -m "Release v$v" -- Cargo.toml Cargo.lock
git tag -a "v$v" -m "Deskhand v$v"
echo "Tagged v$v. To publish:"
echo "  git push origin main v$v"
