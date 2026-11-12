#!/usr/bin/env bash
# Cross-builds static ithub-agent.exe and ithub-server.exe from Linux with mingw-w64.
# The .exe files import only Windows DLLs (msvcrt.dll included, which ships with Windows).
# libvpx and Opus come from vcpkg; OpenH264 is compiled by its crate; libstdc++ and
# winpthread are linked statically (-static, in .cargo/config.toml).
#
# One-time setup (Debian/Ubuntu):
#   sudo apt install mingw-w64 nasm cmake ninja-build esbuild pkg-config
#   rustup target add x86_64-pc-windows-gnu
#   git clone https://github.com/microsoft/vcpkg ~/vcpkg && ~/vcpkg/bootstrap-vcpkg.sh -disableMetrics
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
export VCPKG_ROOT=${VCPKG_ROOT:-$HOME/vcpkg}
export VCPKGRS_TRIPLET=x64-mingw-static
target=x86_64-pc-windows-gnu

"$VCPKG_ROOT/vcpkg" install libvpx opus --triplet "$VCPKGRS_TRIPLET"
cd "$root"
cargo build --release --target "$target" -p ithub-agent -p ithub-server

# Same rule as CI: nothing but Windows DLLs.
bad=0
for exe in ithub-agent.exe ithub-server.exe; do
	f=${CARGO_TARGET_DIR:-target}/$target/release/$exe
	dlls=$(x86_64-w64-mingw32-objdump -p "$f" | sed -n 's/.*DLL Name: //p' | sort -fu)
	echo "$exe imports: $(echo $dlls)"
	if echo "$dlls" | grep -iqE '^(vcruntime|msvcp|ucrtbase|api-ms-win-crt-|libvpx|vpx|libopus|opus|openh264|libgcc|libstdc|libwinpthread|libc\+\+)'; then
		echo "FAIL $exe: imports a DLL that isn't part of Windows" >&2
		bad=1
	fi
done
exit $bad
