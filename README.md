# ithub

Self-hosted remote desktop management, in Rust. Agents on each machine dial out to one
server, so nothing behind NAT needs an open port; you sign in to the server's dashboard
and open any online device in the browser.

Screen streaming is [voxl](https://github.com/tembo/voxl): H.264 over a WebSocket,
decoded in the browser with WebCodecs, at up to 60 fps.

## How it fits together

- `server/`: dashboard, admin sign-in, and a relay. `/d/{id}/` proxies the browser to
  that device's voxl through the agent.
- `agent/`: runs voxl on a random loopback port and keeps a control socket open to the
  server. For each browser request the server asks for a stream, and the agent opens
  one and pipes it to voxl.
- `proto/`: the control messages they exchange.

## Build (Linux)

```sh
sudo apt install libx264-dev libopus-dev libpulse-dev libyuv-dev esbuild pkg-config libclang-dev
cargo build --release
```

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

GPL-2.0-or-later. voxl is MIT, but it links x264 (GPL-2.0), so the agent binary is GPL.
