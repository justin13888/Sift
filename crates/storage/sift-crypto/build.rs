//! D-75 — compile the macOS page cipher's CryptoKit shim.
//!
//! CryptoKit has no C interface, so `platform/cryptokit.swift` gives it one, and this script
//! compiles that file into a static archive `page.rs` links on macOS. Every other target
//! does nothing here and uses the vendored implementation of the same construction.
//!
//! The platform's own toolchain compiles it (`xcrun swiftc`) into this crate's own archive,
//! which is the bridge exception D-61 records: it builds object code and never assembles,
//! bundles or signs anything. The object carries Swift's autolink entries for
//! CryptoKit, Foundation and the Swift runtime, so a binary that links this crate — a test,
//! the harness, or the shell's `libsift_abi.a` inside Xcode — needs nothing further on its
//! link line. The runtime compatibility shims are turned off: the shim uses nothing they
//! back-deploy, and D-46's macOS 13 floor ships the runtime itself.

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// D-46's floor, used when the build does not state one. One of the places the floor is
/// stated; `docs/build/workspace.md` lists them all, and they change together.
const DEFAULT_DEPLOYMENT_TARGET: &str = "13.0";

fn main() {
    println!("cargo:rerun-if-changed=platform/cryptokit.swift");
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("sift-crypto: no CryptoKit shim for macOS on {other:?}"),
    };
    let floor = env::var("MACOSX_DEPLOYMENT_TARGET")
        .unwrap_or_else(|_| DEFAULT_DEPLOYMENT_TARGET.to_owned());
    let target = format!("{arch}-apple-macos{floor}");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let object = out.join("sift_cryptokit.o");
    let archive = out.join("libsift_cryptokit.a");
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("platform/cryptokit.swift");

    run(Command::new("xcrun")
        .args(["--sdk", "macosx", "swiftc"])
        .args([
            "-parse-as-library",
            "-emit-object",
            "-O",
            "-whole-module-optimization",
        ])
        .args(["-module-name", "SiftCryptoKit"])
        .args(["-target", &target])
        .args(["-runtime-compatibility-version", "none"])
        .arg(&source)
        .arg("-o")
        .arg(&object));

    // Replaced rather than appended to: `libtool -static` writes a fresh archive.
    run(Command::new("xcrun")
        .args(["libtool", "-static", "-o"])
        .arg(&archive)
        .arg(&object));

    // The Swift runtime's link stubs live in the SDK; the autolink entries name the
    // libraries, and this tells the linker where they are.
    let sdk = capture(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-lib=static=sift_cryptokit");
    println!("cargo:rustc-link-lib=framework=CryptoKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
}

fn run(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| {
        panic!(
            "sift-crypto: {cmd:?} did not start ({e}). D-61's bridge exception needs the Xcode toolchain on macOS."
        )
    });
    assert!(
        status.success(),
        "sift-crypto: {cmd:?} failed with {status}"
    );
}

fn capture(cmd: &mut Command) -> String {
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("sift-crypto: {cmd:?} did not start ({e})"));
    assert!(output.status.success(), "sift-crypto: {cmd:?} failed");
    String::from_utf8(output.stdout)
        .expect("utf-8 SDK path")
        .trim()
        .to_owned()
}
