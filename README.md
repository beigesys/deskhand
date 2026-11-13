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

The dashboard is a React app (`web/`) that `deskhand-server` embeds, so build it first.
[Bun](https://bun.sh) is a build-time tool only, like esbuild; the server's `build.rs`
runs these two commands itself if `web/dist` is missing and `bun` is on `PATH`:

```sh
cd web && bun install --frozen-lockfile && bun run build && cd ..
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

### 3. Agent downloads

The server hands out the agent itself: install commands download from
`https://SERVER/dl/...`, never from anywhere else. It serves the builds in its agents
directory (`DESKHAND_AGENT_DIR`, default `agents/` next to the database), named
`deskhand-agent-linux-x86_64`, `deskhand-agent-linux-aarch64`,
`deskhand-agent-windows-x86_64.exe` and `deskhand-agent-macos-aarch64`.

Fill it either way:

- **Build them:** `scripts/package-agents.sh /path/to/agents` builds every platform this
  machine can (static Linux via zigbuild, Windows via `scripts/build-windows.sh`) and
  copies them in under those names.
- **Let the server fetch them:** a build missing from the directory is downloaded from
  the GitHub Release matching the server's own version (`v{version}` of
  `DESKHAND_AGENT_REPO`, default `beigesys/deskhand`), checked against that release's
  `SHA256SUMS`, and cached there; the server tries all platforms at startup. Agents
  therefore always match the server's version. `DESKHAND_AGENT_SOURCE=local` turns
  fetching off.

The Enrollment page lists which builds are ready and warns about missing ones.

### 4. Enroll a machine

1. In the dashboard, open **Enrollment** and click **New token**; pick a group.
2. Run the command it shows on the machine. They come from this server:

```sh
curl -fsSL https://SERVER/install.sh | sudo sh -s -- TOKEN        # Linux / macOS
irm https://SERVER/install.ps1 | iex; Install-Deskhand -Token TOKEN  # Windows, admin PowerShell
```

On Linux, `install.sh` puts the agent in `/usr/local/bin`, enrolls it with
`/etc/deskhand/agent.json`, and installs a `deskhand-agent` systemd unit. On Windows,
`install.ps1` installs to `C:\Program Files\Deskhand` and starts the agent in your
session (a Windows service comes with service mode; see `docs/windows.md`). By hand:

```sh
deskhand-agent enroll --server https://SERVER --token TOKEN   # saves agent.json in the config dir
DISPLAY=:0 deskhand-agent                                     # connects; voxl flags go after --
```

Linux needs an X11 display; Wayland isn't supported yet. `--config FILE` (or
`DESKHAND_CONFIG`) picks another identity file, so several agents can run on one machine
while developing. The device list flags agents whose version differs from the server's.

### 5. Open a desktop or shell

The device list shows each machine online or offline. **Desktop** opens its screen
(`/d/{id}/`); **Shell** opens a terminal on it (`/d/{id}/shell`: PowerShell on Windows,
`$SHELL` elsewhere). Click the device name for its inventory.

## The web app

- `web/`: React 19, TypeScript, Tailwind v4, TanStack Router and Query, Radix primitives
  styled as Deskhand's own (no stock shadcn look), CodeMirror for scripts. Built with
  bun and Vite into `web/dist`, which `deskhand-server` embeds; the server serves it at
  `/` and answers `/api/v1` (JSON, same cookie sessions and roles).
- Look: `web/src/theme.css` holds every color, font, radius and density value. Palettes
  (Graphite, Harbor, Field, Paper, Navy) each set the sidebar, the accent and the page
  neutrals for light and dark; each user picks one in the user menu (kept in that
  browser). Admins see the tokens and components at `/style`.
- Lists never load everything: every list endpoint takes `?q=&cursor=&limit=&ids=` and
  answers `{items, next_cursor}`, searched and keyset-paged in SQL. Pickers are one
  searchable, infinitely scrolling combobox (`AsyncCombobox`) with recent picks first.
- Phones: a top bar with the page's title and main action, the sidebar as a drawer, and
  tables as stacked cards.
- The server-rendered pages from before the web app are still at `/classic/`.

Develop with the Rust server on 8080 and Vite's dev server in front of it:

```sh
DESKHAND_ADMIN_PASSWORD=admin-password cargo run -p deskhand-server   # terminal 1
cd web && bun run dev                                                 # terminal 2: http://localhost:5173
```

Tests:

```sh
cargo test -p deskhand-server       # unit tests, plus tests/api.rs: the API end to end and 5,000 devices
cd web && bun run typecheck
scripts/e2e-web.sh                  # Playwright: server + agent on Xvfb; sign in, devices, run a script, phone layout, 5,000-device picker
```

`scripts/e2e-web.sh` needs Xvfb and Playwright's Chromium (`bunx playwright install
chromium`, or `PW_CHROMIUM=/path/to/chrome`); `DESKHAND_SHOTS=DIR` saves screenshots.

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
| `DESKHAND_PUBLIC_URL` | unset | The `https://` address users and agents use; install commands and SSO redirects are built from it (else from the request's host) |
| `DESKHAND_AGENT_DIR` | `agents/` beside the DB | Agent builds served at `/dl/...` |
| `DESKHAND_AGENT_SOURCE` | `github` | `local` never fetches missing agent builds |
| `DESKHAND_AGENT_REPO` | `beigesys/deskhand` | GitHub repo whose `v{server version}` release supplies missing agent builds |
| `DESKHAND_LOGIN_RATE` | `5` | Sign-in attempts allowed per IP per minute |
| `DESKHAND_ENTRA_*`, `DESKHAND_GOOGLE_*`, `DESKHAND_GITHUB_*` | unset | Single sign-on with Microsoft Entra ID, Google, GitHub: see [docs/sso.md](docs/sso.md) |
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
