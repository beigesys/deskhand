//! Builds libvpx (VP9 encoder) from source as a static library, generates its Rust
//! bindings, and bundles the web client. Nothing here links a shared library: OpenH264
//! and Opus come from crates that compile their bundled sources (on Windows, libvpx and
//! Opus come from vcpkg's static triplet instead).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The libvpx release built when VPX_SRC doesn't point at a source tree.
const LIBVPX_TAG: &str = "v1.17.0";
const LIBVPX_GIT: &str = "https://chromium.googlesource.com/webm/libvpx";

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let include = if target_os == "windows" {
        windows_libs()
    } else {
        build_libvpx(&out)
    };
    let mut b = bindgen::Builder::default()
        .header_contents(
            "wrap.h",
            "#include <vpx/vpx_encoder.h>\n#include <vpx/vp8cx.h>\n",
        )
        .allowlist_function("vpx_.*")
        .allowlist_type("vpx_.*|vp8e_.*|vp9e_.*|VP8E_.*|VP9E_.*")
        .allowlist_var("VPX_.*|VP8_.*|VP9E_.*|VP8E_.*")
        .layout_tests(false);
    for p in &include {
        b = b.clang_arg(format!("-I{}", p.display()));
    }
    let code = b.generate().expect("bindgen libvpx").to_string();
    std::fs::write(out.join("vpx.rs"), code).unwrap();
    bundle_web(&out);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VPX_SRC");
}

/// Configures and builds libvpx's VP9 encoder as libvpx.a with the target's C compiler
/// (under cargo-zigbuild, zig for musl); returns the include dir.
fn build_libvpx(out: &Path) -> Vec<PathBuf> {
    let prefix = out.join("libvpx");
    let lib = prefix.join("lib/libvpx.a");
    if !lib.exists() {
        let src = match std::env::var("VPX_SRC") {
            Ok(s) => PathBuf::from(s),
            Err(_) => {
                let s = out.join("libvpx-src");
                if !s.join("configure").exists() {
                    let _ = std::fs::remove_dir_all(&s);
                    run(Command::new("git").args([
                        "clone",
                        "--depth=1",
                        "--branch",
                        LIBVPX_TAG,
                        LIBVPX_GIT,
                    ])
                    .arg(&s));
                }
                s
            }
        };
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
        let target = match (arch.as_str(), os.as_str()) {
            ("x86_64", "linux") => "x86_64-linux-gcc",
            ("aarch64", "linux") => "arm64-linux-gcc",
            ("x86_64", "macos") => "x86_64-darwin20-gcc",
            ("aarch64", "macos") => "arm64-darwin20-gcc",
            _ => "generic-gnu",
        };
        let cmd = |cpp: bool| {
            let c = cc::Build::new().cpp(cpp).get_compiler();
            let mut s = c.path().display().to_string();
            for a in c.args() {
                s.push(' ');
                s.push_str(&a.to_string_lossy());
            }
            s
        };
        let (cc_cmd, cxx_cmd) = (cmd(false), cmd(true));
        let ar = cc::Build::new().get_archiver();
        let mut ar_cmd = ar.get_program().to_string_lossy().into_owned();
        for a in ar.get_args() {
            ar_cmd.push(' ');
            ar_cmd.push_str(&a.to_string_lossy());
        }
        let build = out.join("libvpx-build");
        std::fs::create_dir_all(&build).unwrap();
        let mut conf = Command::new(src.join("configure"));
        conf.current_dir(&build)
            .env("CC", &cc_cmd)
            .env("CXX", &cxx_cmd)
            .env("LD", &cxx_cmd)
            .env("AR", &ar_cmd)
            .arg(format!("--target={target}"))
            .arg(format!("--prefix={}", prefix.display()))
            .args([
                "--enable-static",
                "--disable-shared",
                "--enable-pic",
                "--disable-examples",
                "--disable-tools",
                "--disable-docs",
                "--disable-unit-tests",
                "--disable-webm-io",
                "--disable-libyuv",
                "--enable-realtime-only",
                "--disable-vp8",
                "--enable-vp9-encoder",
                "--disable-vp9-decoder",
            ]);
        if arch == "x86_64" {
            conf.arg("--as=nasm");
        }
        run(&mut conf);
        let jobs = std::env::var("NUM_JOBS").unwrap_or_else(|_| "4".into());
        run(Command::new("make")
            .current_dir(&build)
            .env("CC", &cc_cmd)
            .env("CXX", &cxx_cmd)
            .env("AR", &ar_cmd)
            .arg(format!("-j{jobs}"))
            .arg("install"));
    }
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=vpx");
    vec![prefix.join("include")]
}

/// libvpx and Opus from vcpkg (`vcpkg install libvpx opus --triplet x64-windows-static`,
/// triplet from VCPKGRS_TRIPLET); returns libvpx's include paths. Capture, audio and
/// input use system DLLs.
fn windows_libs() -> Vec<PathBuf> {
    let vpx = vcpkg::Config::new()
        .find_package("libvpx")
        .expect("libvpx from vcpkg");
    vcpkg::Config::new()
        .find_package("opus")
        .unwrap_or_else(|e| panic!("opus from vcpkg: {e}"));
    println!("cargo:rerun-if-env-changed=VCPKGRS_TRIPLET");
    vpx.include_paths
}

fn run(cmd: &mut Command) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("running {cmd:?} failed: {e}"));
    assert!(status.success(), "{cmd:?} failed");
}

/// Bundles the TypeScript web client (web/src) into OUT_DIR/web, which the server
/// embeds. Needs esbuild on PATH (or ESBUILD); shell.nix and default.nix provide it.
fn bundle_web(out: &Path) {
    // npm installs esbuild on Windows as a .cmd shim.
    let default = if cfg!(windows) {
        "esbuild.cmd"
    } else {
        "esbuild"
    };
    let esbuild = std::env::var("ESBUILD").unwrap_or_else(|_| default.into());
    let status = Command::new(&esbuild)
        .args([
            "client=web/src/client/main.ts",
            "worker=web/src/worker.ts",
            "audio=web/src/worklets/audio.ts",
            "mic=web/src/worklets/mic.ts",
            "--bundle",
            "--format=iife",
            "--target=es2022",
            "--minify",
            "--log-level=warning",
        ])
        .arg(format!("--outdir={}", out.join("web").display()))
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "running {esbuild} to bundle web/src failed ({e}); use nix-shell, or set ESBUILD"
            )
        });
    assert!(status.success(), "esbuild failed to bundle web/src");
    println!("cargo:rerun-if-changed=web/src");
    println!("cargo:rerun-if-env-changed=ESBUILD");
}
