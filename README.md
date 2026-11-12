# ithub

Self-hosted remote desktop management, in Rust. Agents on each machine dial out to one
server, so nothing behind NAT needs an open port; you sign in to the server's dashboard
and open any online device in the browser.

Screen streaming is a fork of [voxl](https://github.com/tembo/voxl) in `vendor/voxl`:
VP9 (libvpx, real-time screen-content mode) over a WebSocket, decoded in the browser with
WebCodecs at up to 60 fps. A browser that can't decode VP9 gets H.264 (OpenH264) instead.

Every binary is one static file that needs nothing installed on the host: libvpx,
OpenH264 and Opus are compiled from source and linked in, colour conversion is Rust (no
libyuv), and Linux desktop audio speaks PulseAudio's protocol in Rust (no libpulse).

## How it fits together

- `server/`: dashboard, admin sign-in, and a relay. `/d/{id}/` proxies the browser to
  that device's voxl through the agent.
- `agent/`: runs voxl on a random loopback port and keeps a control socket open to the
  server. For each browser request the server asks for a stream, and the agent opens
  one and pipes it to voxl.
- Inventory: the agent reports OS, hardware, disks, network, installed software and
  security state (BitLocker/LUKS, Defender, firewall, pending reboot) when it connects,
  every 4 hours, and on Refresh. The server keeps the last 30 snapshots per device and
  shows them at `/devices/{id}`; `/devices.csv` exports the device list.
- `proto/`: the control messages they exchange.

## Build (Linux)

```sh
sudo apt install nasm cmake esbuild libclang-dev git make   # build machine only
cargo install cargo-zigbuild                                # and zig (e.g. brew install zig)
cargo zigbuild --release --target x86_64-unknown-linux-musl -p ithub-agent -p ithub-server
```

Both come out statically linked (`file` says so, `ldd` says "not a dynamic executable").
The first build clones libvpx (pinned tag) into the build directory; set `VPX_SRC` to use
a local checkout. Windows: see `scripts/build-windows.sh` and `docs/windows.md`.

## Run

Server:

```sh
./target/release/ithub-server
# listens on 0.0.0.0:8080 and keeps its data in ./ithub.db
```

| Variable | Default | Purpose |
|---|---|---|
| `ITHUB_ADDR` | `0.0.0.0:8080` | Listen address |
| `ITHUB_DB` | `./ithub.db` | SQLite database file |
| `ITHUB_ADMIN_PASSWORD` | unset | With no users yet, creates `admin` with this password |
| `ITHUB_TRUST_PROXY` | unset | `1` to trust `X-Forwarded-Proto`/`-For` from your TLS proxy (sets `Secure` cookies) |
| `ITHUB_PUBLIC_URL`, `ITHUB_ENTRA_*`, `ITHUB_GOOGLE_*`, `ITHUB_GITHUB_*` | unset | Single sign-on with Microsoft Entra ID, Google, GitHub: see [docs/sso.md](docs/sso.md) |
| `ITHUB_SSO_AUTO_PROVISION` | off | `viewer` or `operator`: create users on first SSO sign-in (restricted providers only) |
| `ITHUB_PASSWORD_LOGIN` | `1` | `0` hides password sign-in once an SSO provider is set up |

Without `ITHUB_ADMIN_PASSWORD`, the first start prints a one-time link,
`http://SERVER:8080/setup?token=...`, where you create the first admin.

Enroll a machine:

1. In the dashboard, open **Enroll**, pick a group, and create a token.
2. Run the install command it shows on the machine. By hand, that's:

```sh
ithub-agent enroll --server https://SERVER --token TOKEN   # saves agent.json in the config dir
DISPLAY=:0 ithub-agent                                       # connects; voxl flags go after --
```

Linux needs an X11 display; Wayland isn't supported yet. `--config FILE` (or
`ITHUB_CONFIG`) picks another identity file, so several agents can run on one machine
while developing.

Roles: **viewer** sees the device list, **operator** can also open desktops and shells,
**admin** manages devices, groups, enrollment tokens, users and reads the audit log.

Browsers only allow WebCodecs on HTTPS or localhost, so put the server behind a TLS
proxy (Caddy, nginx), set `ITHUB_TRUST_PROXY=1`, and enroll agents with `https://`.

## License

MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). `vendor/voxl` is MIT (its own
`LICENSE`). The linked codecs are permissive too: libvpx BSD-3-Clause, OpenH264
BSD-2-Clause, Opus BSD-3-Clause.

H.264 is patent-encumbered; Cisco's patent license covers only its prebuilt OpenH264
binaries, not this source build. VP9, the default, is royalty-free.
