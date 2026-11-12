# Deskhand

Self-hosted remote desktop and device management, in Rust. Agents on each machine dial
out to one server, so nothing behind NAT needs an open port; you sign in to the server's
dashboard and open any online device's desktop or shell in the browser, see its
inventory, and run scripts and power actions across your fleet.

Website: [deskhand.sh](https://deskhand.sh)

Screen streaming is a fork of [voxl](https://github.com/tembo/voxl) in `vendor/voxl`:
VP9 (libvpx, real-time screen-content mode) over a WebSocket, decoded in the browser with
WebCodecs at up to 60 fps. A browser that can't decode VP9 gets H.264 (OpenH264) instead.

Every binary is one static file that needs nothing installed on the host: libvpx,
OpenH264 and Opus are compiled from source and linked in, colour conversion is Rust (no
libyuv), and Linux desktop audio speaks PulseAudio's protocol in Rust (no libpulse).

## Quick start

### 1. Build

Get the source with its submodule (libvpx, pinned to v1.17.0):

```sh
git clone --recursive <repo-url> deskhand && cd deskhand
# or, in an existing checkout: git submodule update --init
```

Linux, static musl binaries:

```sh
sudo apt install nasm cmake esbuild libclang-dev make      # build machine only
cargo install cargo-zigbuild                               # and zig (e.g. pip install ziglang)
cargo zigbuild --release --target x86_64-unknown-linux-musl -p deskhand-agent -p deskhand-server
file target/x86_64-unknown-linux-musl/release/deskhand-*   # "statically linked"
```

Windows `.exe` files, cross-built from Linux with mingw (libvpx and Opus from vcpkg's
static triplet):

```sh
VCPKG_ROOT=~/vcpkg scripts/build-windows.sh
# target/x86_64-pc-windows-gnu/release/deskhand-{agent,server}.exe, checked to import only system DLLs
```

The build downloads nothing beyond `cargo fetch`: libvpx is the `vendor/libvpx`
submodule (set `VPX_SRC` to use another checkout), and OpenH264 and Opus ship their C
sources inside their crates. See `docs/windows.md` for the MSVC/CI build.

### 2. Start the server and create the first admin

```sh
./deskhand-server
# listens on 0.0.0.0:8080 and keeps its data in ./deskhand.db
```

On the first start, with no users yet, it prints a one-time link,
`http://SERVER:8080/setup?token=...`. Open it and create the admin account. Or set
`DESKHAND_ADMIN_PASSWORD` before the first start to create `admin` with that password.

Browsers only allow WebCodecs on HTTPS or localhost, so for real use put the server
behind a TLS proxy (Caddy, nginx), set `DESKHAND_TRUST_PROXY=1`, and use `https://` URLs.

### 3. Enroll a machine

1. In the dashboard, open **Enroll**, pick a group, and create a token.
2. Run the install command it shows on the machine. By hand, that's:

```sh
deskhand-agent enroll --server https://SERVER --token TOKEN   # saves agent.json in the config dir
DISPLAY=:0 deskhand-agent                                     # connects; voxl flags go after --
```

Linux needs an X11 display; Wayland isn't supported yet. `--config FILE` (or
`DESKHAND_CONFIG`) picks another identity file, so several agents can run on one machine
while developing.

### 4. Open a desktop or shell

The device list shows each machine online or offline. **Desktop** opens its screen
(`/d/{id}/`); **Shell** opens a terminal on it (`/d/{id}/shell`: PowerShell on Windows,
`$SHELL` elsewhere). Click the device name for its inventory.

## Jobs and scripts

- **Scripts** (`/scripts`): a library of PowerShell, cmd, bash or sh scripts, each with
  a timeout.
- **Jobs** (`/jobs`): run a script, or a built-in action, on chosen devices, a group or
  every device, either now or on a cron schedule (`*/15 * * * *`). Each device's run is
  kept with its status, exit code, duration and output; output streams live to the job
  page while it runs.
- Offline devices: their runs wait as `queued` and go out when the agent reconnects,
  until the job's expiry (24 hours by default) passes.
- On the agent, a script runs as the agent's user with its timeout; on timeout the whole
  process tree is killed. At most 4 jobs run at once per device.
- Built-in actions: reboot and shutdown (with a delay and an optional message), log off
  and lock (Windows), and wake-on-LAN, which an online agent on the same subnet sends to
  the target's MAC from its inventory. Rebooting or shutting down several devices asks
  you to type the hostname to confirm.
- `DESKHAND_AGENT_DRY_RUN_POWER=1` on an agent logs power actions instead of doing them.

## How it fits together

- `server/`: dashboard, sign-in, and a relay. `/d/{id}/` proxies the browser to that
  device's voxl through the agent.
- `agent/`: runs voxl on a random loopback port and keeps a control socket open to the
  server. For each browser request the server asks for a stream, and the agent opens
  one and pipes it to voxl. It also runs shells, jobs and inventory.
- Inventory: the agent reports OS, hardware, disks, network, installed software and
  security state (BitLocker/LUKS, Defender, firewall, pending reboot) when it connects,
  every 4 hours, and on Refresh. The server keeps the last 30 snapshots per device and
  shows them at `/devices/{id}`; `/devices.csv` exports the device list.
- `proto/`: the control messages they exchange.

## Server settings

| Variable | Default | Purpose |
|---|---|---|
| `DESKHAND_ADDR` | `0.0.0.0:8080` | Listen address |
| `DESKHAND_DB` | `./deskhand.db` | SQLite database file |
| `DESKHAND_ADMIN_PASSWORD` | unset | With no users yet, creates `admin` with this password |
| `DESKHAND_TRUST_PROXY` | unset | `1` to trust `X-Forwarded-Proto`/`-For` from your TLS proxy (sets `Secure` cookies) |
| `DESKHAND_PUBLIC_URL`, `DESKHAND_ENTRA_*`, `DESKHAND_GOOGLE_*`, `DESKHAND_GITHUB_*` | unset | Single sign-on with Microsoft Entra ID, Google, GitHub: see [docs/sso.md](docs/sso.md) |
| `DESKHAND_SSO_AUTO_PROVISION` | off | `viewer` or `operator`: create users on first SSO sign-in (restricted providers only) |
| `DESKHAND_PASSWORD_LOGIN` | `1` | `0` hides password sign-in once an SSO provider is set up |

Roles: **viewer** sees devices, inventory and jobs; **operator** can also open desktops
and shells and run jobs; **admin** manages devices, groups, enrollment tokens, users and
reads the audit log.

## License

MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). `vendor/voxl` is MIT (its own
`LICENSE`). The linked codecs are permissive too: libvpx BSD-3-Clause, OpenH264
BSD-2-Clause, Opus BSD-3-Clause.

H.264 is patent-encumbered; Cisco's patent license covers only its prebuilt OpenH264
binaries, not this source build. VP9, the default, is royalty-free.
