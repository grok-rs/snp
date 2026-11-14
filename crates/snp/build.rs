//! Embeds snp's application icon in the Windows executable (ADR-130).
//!
//! For `*-windows-msvc` targets this compiles `assets/snp.ico` (drawn by `docs/assets/src/pixel_icon.py`) into a
//! resource file in `OUT_DIR` and passes it to the linker of the `snp` binary; every other target builds exactly as
//! before. No resource compiler and no dependency: see `build/winres.rs`.

use std::error::Error;
use std::io::Write as _;
use std::path::PathBuf;

#[path = "build/winres.rs"]
mod winres;

/// A variable cargo sets for build scripts.
#[expect(
    clippy::disallowed_methods,
    reason = "cargo hands a build script its inputs in environment variables"
)]
fn cargo_env(name: &str) -> Result<String, Box<dyn Error>> {
    std::env::var(name).map_err(|e| format!("{name}: {e}").into())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "cargo::rerun-if-changed=assets/snp.ico")?;
    writeln!(out, "cargo::rerun-if-changed=build/winres.rs")?;
    if cargo_env("CARGO_CFG_TARGET_OS")? != "windows"
        || cargo_env("CARGO_CFG_TARGET_ENV")? != "msvc"
    {
        return Ok(());
    }
    let ico = PathBuf::from(cargo_env("CARGO_MANIFEST_DIR")?)
        .join("assets")
        .join("snp.ico");
    let bytes = std::fs::read(&ico).map_err(|e| format!("{}: {e}", ico.display()))?;
    let res = PathBuf::from(cargo_env("OUT_DIR")?).join("snp.res");
    std::fs::write(&res, winres::compile_ico(&bytes)?)?;
    // link.exe and lld-link take a .res file as an input file; only the `snp` binary gets it.
    writeln!(out, "cargo::rustc-link-arg-bins={}", res.display())?;
    Ok(())
}
