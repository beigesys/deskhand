//! The web app in ../web/dist is embedded into the binary (src/web.rs). Build it first with
//! `cd web && bun install && bun run build`; if it's missing and bun is on PATH (or BUN
//! points at it), this does that.

use std::path::Path;
use std::process::Command;

fn main() {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web");
    let dist = web.join("dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=BUN");
    if dist.join("index.html").exists() {
        return;
    }
    let bun = std::env::var("BUN").unwrap_or_else(|_| "bun".into());
    let run = |args: &[&str]| {
        Command::new(&bun)
            .args(args)
            .current_dir(&web)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !(run(&["install", "--frozen-lockfile"]) && run(&["run", "build"])) || !dist.join("index.html").exists() {
        panic!(
            "web/dist is missing and building it failed. Install bun (https://bun.sh) and run \
             `cd web && bun install && bun run build`, or set BUN to bun's path."
        );
    }
}
