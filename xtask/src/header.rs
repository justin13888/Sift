//! `cargo xtask header` — D-60's generated header, and the drift check.
//!
//! > "A C header is generated from the ABI crate's source and consumed by Swift through a
//! > module map; the generated artefact is committed; **regenerating it in CI and finding a
//! > difference fails the build.**"
//!
//! Committing generated output is widely disliked, and D-60 records why it is done anyway:
//! it keeps the Swift target independent of the Rust toolchain in day-to-day shell work, and
//! it makes **every addition to the boundary visible in review** — which is the mechanism by
//! which D-17's tripwire ("if the surface grows past what one file can hold") actually trips.
//! The tidier alternative catches nothing until the first silent layout mismatch.

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/abi/sift-abi")
}

fn header_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/abi/include/sift.h")
}

fn generate() -> Result<String, String> {
    let config = cbindgen::Config::from_file(crate_dir().join("../cbindgen.toml"))
        .map_err(|e| format!("cannot read cbindgen.toml: {e}"))?;
    let bindings = cbindgen::Builder::new()
        .with_crate(crate_dir())
        .with_config(config)
        .generate()
        .map_err(|e| format!("cannot generate the header: {e}"))?;
    let mut out = Vec::new();
    bindings.write(&mut out);
    String::from_utf8(out).map_err(|e| format!("the header is not UTF-8: {e}"))
}

/// Write the header.
pub(crate) fn bless() -> Result<(), String> {
    let generated = generate()?;
    let path = header_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, generated)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("header: wrote {}", path.display());
    Ok(())
}

/// Check the committed header against a fresh generation.
pub(crate) fn check() -> Result<(), String> {
    let generated = generate()?;
    let path = header_path();
    let committed = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {}: {e}\n\
             If this is a first run, `cargo xtask header --bless` creates it.",
            path.display()
        )
    })?;

    if committed == generated {
        let lines = generated.lines().count();
        println!("header: {lines} lines, no drift");
        return Ok(());
    }

    let mut report = String::from(
        "the committed header does not match the ABI crate.\n\n\
         This is D-60 doing its job: the boundary changed and the artefact the Swift shell \
         compiles against did not.\n\n",
    );
    for (n, (a, b)) in committed.lines().zip(generated.lines()).enumerate() {
        if a != b {
            report.push_str(&format!(
                "  line {}:\n    committed:  {a}\n    generated:  {b}\n",
                n + 1
            ));
            break;
        }
    }
    let (c, g) = (committed.lines().count(), generated.lines().count());
    if c != g {
        report.push_str(&format!("  committed has {c} lines, generated has {g}\n"));
    }
    report.push_str(
        "\nRun `cargo xtask header --bless` and read the diff. A line that changed shape is \
         a boundary change,\nand D-66 requires both shells handle every discriminant \
         exhaustively — so a new one is a build\nfailure on the far side rather than a \
         runtime case.",
    );
    Err(report)
}
