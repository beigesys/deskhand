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
ITHUB_ADMIN_PASSWORD=... ITHUB_AGENT_KEY=... ./target/release/ithub-server
# listens on 0.0.0.0:8080; set ITHUB_ADDR to change it
```

Agent, on each machine (Linux needs an X11 display; Wayland isn't supported yet):

```sh
DISPLAY=:0 ./target/release/ithub-agent --server ws://SERVER:8080 --key AGENT_KEY
# or ITHUB_SERVER / ITHUB_AGENT_KEY; voxl flags go after --, e.g. -- --framerate=30
```

Open `http://SERVER:8080`, sign in, and pick a device. Browsers only allow WebCodecs
on HTTPS or localhost, so put the server behind a TLS proxy (Caddy, nginx) and point
agents at `wss://`.

## License

GPL-2.0-or-later. voxl is MIT, but it links x264 (GPL-2.0), so the agent binary is GPL.
