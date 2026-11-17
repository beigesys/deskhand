//! Shell streams: a terminal (ConPTY on Windows, a pty elsewhere) piped to the server's
//! socket. Binary frames carry the terminal's bytes both ways; text frames from the server
//! are [`ShellControl`] JSON.

use futures_util::{SinkExt, StreamExt};
use deskhand_proto::ShellControl;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

fn default_shell() -> CommandBuilder {
    if cfg!(windows) {
        CommandBuilder::new("powershell.exe")
    } else {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        cmd
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.max(1),
        rows: rows.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub async fn run(ws: Ws, cols: u16, rows: u16) -> Result<(), String> {
    let pair = native_pty_system()
        .openpty(size(cols, rows))
        .map_err(|e| format!("openpty: {e}"))?;
    let mut cmd = default_shell();
    if let Some(home) = dirs::home_dir() {
        cmd.cwd(home);
    }
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell: {e}"))?;
    // The child holds its own handle; ours would keep the pty open after it exits.
    drop(pair.slave);
    let master = pair.master;
    let mut reader = master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = master.take_writer().map_err(|e| e.to_string())?;

    // The pty's reads and writes block, so they get threads of their own.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        while let Some(b) = in_rx.blocking_recv() {
            if writer.write_all(&b).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });

    let (mut wtx, mut wrx) = ws.split();
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(b) => {
                    if wtx.send(Message::Binary(b.into())).await.is_err() { break }
                }
                // The shell exited.
                None => break,
            },
            m = wrx.next() => match m {
                Some(Ok(Message::Binary(b))) => {
                    if in_tx.send(b.to_vec()).await.is_err() { break }
                }
                Some(Ok(Message::Text(t))) => {
                    if let Ok(ShellControl { resize: Some((c, r)) }) = serde_json::from_str(&t) {
                        let _ = master.resize(size(c, r));
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
        }
    }
    let _ = wtx.close().await;
    let _ = child.kill();
    tokio::task::spawn_blocking(move || child.wait()).await.ok();
    Ok(())
}
