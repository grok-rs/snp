//! `cargo xtask release hardening <target> <binary>...`: the hardening gate of the release artifacts (ADR-137).
//!
//! Reads each binary's headers (ELF, PE or Mach-O; no tool, no dependency) and checks the protections the target's
//! artifact must carry. Every failed check of every binary is reported before the command fails. The expectations,
//! per target triple (ARCHITECTURE §11.7, "Artifact hardening"):
//!
//! | target | expected |
//! |---|---|
//! | `x86_64`/`aarch64-unknown-linux-gnu` | a PIE (`ET_DYN` with `DF_1_PIE` and an interpreter), `BIND_NOW`, a `PT_GNU_RELRO` segment, a non-executable `PT_GNU_STACK`, no text relocations |
//! | `x86_64-unknown-linux-musl` | static-pie: `ET_DYN` with `DF_1_PIE`, no interpreter, no `DT_NEEDED`, a non-executable `PT_GNU_STACK`, no text relocations |
//! | `aarch64-unknown-linux-musl` | static, **not** PIE (`ET_EXEC`, no interpreter, no `DT_NEEDED`), a non-executable `PT_GNU_STACK`, no text relocations |
//! | `x86_64`/`aarch64-apple-darwin` | a 64-bit `MH_EXECUTE` with `MH_PIE` and without `MH_ALLOW_STACK_EXECUTION` |
//! | `x86_64`/`aarch64-pc-windows-msvc` | a PE32+ image with `DYNAMIC_BASE`, `HIGH_ENTROPY_VA` and `NX_COMPAT`, relocations kept |
//!
//! Every format also checks the machine (a binary of another architecture fails). Why aarch64-musl is static
//! non-PIE: rustc 1.100's target specification of `aarch64-unknown-linux-musl` lacks
//! `static-position-independent-executables` (x86_64-musl, x86_64-gnu and aarch64-gnu have it; read with
//! `RUSTC_BOOTSTRAP=1 rustc -Z unstable-options --print target-spec-json --target aarch64-unknown-linux-musl`), so
//! rustc's `link_output_kind` turns the requested static PIC executable into a static non-PIC one. That is the
//! toolchain's limitation, not snp's choice (ADR-137 rejects non-PIE builds): the gate states the fact exactly, so a
//! toolchain that enables static-pie for the triple fails here, and the expectation (with ADR-137 and §11.7) moves
//! to static-pie in that change. musl applies no `PT_GNU_RELRO` in static binaries (the segment stays writable at run
//! time) and `BIND_NOW` means nothing without a dynamic loader, so neither is checked for the musl targets.
//!
//! Owner: WP-37.

use std::fmt::Write as _;

use anyhow::{Context as _, bail};

/// The protections a target's artifact must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Expect {
    /// A dynamically linked ELF PIE with full RELRO and an NX stack (linux-gnu).
    GnuPie(Machine),
    /// A static-pie ELF with an NX stack (x86_64-musl).
    StaticPie(Machine),
    /// A static non-PIE ELF with an NX stack (aarch64-musl: the toolchain cannot link static-pie for it).
    StaticNoPie(Machine),
    /// A Mach-O PIE executable without an executable stack (macOS).
    MachOPie(Machine),
    /// A PE32+ image with high-entropy ASLR and DEP (Windows).
    PeAslr(Machine),
}

/// The CPU architecture of a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Machine {
    X86_64,
    Aarch64,
}

impl Expect {
    /// The expectation of a release target triple; an unknown triple is an error, never a pass.
    pub(super) fn of(target: &str) -> anyhow::Result<Self> {
        Ok(match target {
            "x86_64-unknown-linux-gnu" => Self::GnuPie(Machine::X86_64),
            "aarch64-unknown-linux-gnu" => Self::GnuPie(Machine::Aarch64),
            "x86_64-unknown-linux-musl" => Self::StaticPie(Machine::X86_64),
            // rustc 1.100: no `static-position-independent-executables` in this target's spec (module docs)
            "aarch64-unknown-linux-musl" => Self::StaticNoPie(Machine::Aarch64),
            "x86_64-apple-darwin" => Self::MachOPie(Machine::X86_64),
            "aarch64-apple-darwin" => Self::MachOPie(Machine::Aarch64),
            "x86_64-pc-windows-msvc" => Self::PeAslr(Machine::X86_64),
            "aarch64-pc-windows-msvc" => Self::PeAslr(Machine::Aarch64),
            _ => bail!("no hardening expectation for the target {target} (ADR-137)"),
        })
    }

    /// What the expectation means, for the report.
    fn describe(self) -> &'static str {
        match self {
            Self::GnuPie(_) => "PIE, BIND_NOW, GNU_RELRO, NX stack, no TEXTREL",
            Self::StaticPie(_) => "static-pie, NX stack, no TEXTREL",
            Self::StaticNoPie(_) => {
                "static non-PIE (no static-pie in rustc's target spec), NX stack, no TEXTREL"
            }
            Self::MachOPie(_) => "MH_PIE, no MH_ALLOW_STACK_EXECUTION",
            Self::PeAslr(_) => "DYNAMIC_BASE, HIGH_ENTROPY_VA, NX_COMPAT, relocations kept",
        }
    }
}

/// Checks every binary against the target's expectation; reports each binary and fails if any check failed.
pub(super) fn run(args: &[String]) -> anyhow::Result<()> {
    let [target, binaries @ ..] = args else {
        bail!("usage: cargo xtask release hardening <target> <binary>...");
    };
    if binaries.is_empty() {
        bail!("usage: cargo xtask release hardening <target> <binary>...");
    }
    let expect = Expect::of(target)?;
    let mut failed = 0_usize;
    for bin in binaries {
        let bytes = std::fs::read(bin).with_context(|| format!("reading {bin}"))?;
        let problems = check(expect, &bytes);
        if problems.is_empty() {
            crate::workspace::say(&format!("ok    {target}: {bin} ({})", expect.describe()));
        } else {
            failed = failed.saturating_add(1);
            let mut line = format!("FAIL  {target}: {bin} (expected {})", expect.describe());
            for p in &problems {
                write!(line, "\n        - {p}").unwrap_or_default();
            }
            crate::workspace::say(&line);
        }
    }
    if failed > 0 {
        bail!(
            "{failed} of {} binaries lack the hardening of {target} (ADR-137)",
            binaries.len()
        );
    }
    Ok(())
}

/// Every way `bytes` misses `expect`; empty when it meets it. A malformed or truncated header is a problem, never a
/// pass.
pub(super) fn check(expect: Expect, bytes: &[u8]) -> Vec<String> {
    let result = match expect {
        Expect::GnuPie(m) | Expect::StaticPie(m) | Expect::StaticNoPie(m) => {
            Elf::parse(bytes).map(|elf| elf.problems(expect, m))
        }
        Expect::MachOPie(m) => MachO::parse(bytes).map(|macho| macho.problems(m)),
        Expect::PeAslr(m) => Pe::parse(bytes).map(|pe| pe.problems(m)),
    };
    result.unwrap_or_else(|e| vec![e])
}

// ----- byte readers --------------------------------------------------------------------------------------------

fn u16_at(b: &[u8], off: usize) -> Result<u16, String> {
    let end = off.checked_add(2).ok_or("offset overflow")?;
    let s = b
        .get(off..end)
        .ok_or_else(|| format!("truncated at byte {off}"))?;
    Ok(u16::from_le_bytes([
        *s.first().ok_or("short")?,
        *s.get(1).ok_or("short")?,
    ]))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, String> {
    let end = off.checked_add(4).ok_or("offset overflow")?;
    let s: [u8; 4] = b
        .get(off..end)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| format!("truncated at byte {off}"))?;
    Ok(u32::from_le_bytes(s))
}

fn u64_at(b: &[u8], off: usize) -> Result<u64, String> {
    let end = off.checked_add(8).ok_or("offset overflow")?;
    let s: [u8; 8] = b
        .get(off..end)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| format!("truncated at byte {off}"))?;
    Ok(u64::from_le_bytes(s))
}

fn usize_of(v: u64) -> Result<usize, String> {
    usize::try_from(v).map_err(|_| format!("offset {v} out of range"))
}

// ----- ELF -----------------------------------------------------------------------------------------------------

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_GNU_RELRO: u32 = 0x6474_e552;
const PF_X: u32 = 1;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_TEXTREL: u64 = 22;
const DT_BIND_NOW: u64 = 24;
const DT_FLAGS: u64 = 30;
const DT_FLAGS_1: u64 = 0x6fff_fffb;
const DF_TEXTREL: u64 = 0x4;
const DF_BIND_NOW: u64 = 0x8;
const DF_1_NOW: u64 = 0x1;
const DF_1_PIE: u64 = 0x0800_0000;

/// The facts of an ELF64 little-endian executable the gate reads.
#[derive(Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent fact of the headers per field"
)]
struct Elf {
    e_type: u16,
    machine: u16,
    interp: bool,
    dynamic: bool,
    needed: usize,
    bind_now: bool,
    pie_flag: bool,
    textrel: bool,
    relro: bool,
    /// The flags of `PT_GNU_STACK`, if the segment exists.
    stack: Option<u32>,
}

impl Elf {
    fn parse(b: &[u8]) -> Result<Self, String> {
        if b.get(..4) != Some(b"\x7fELF") {
            return Err("not an ELF file".to_owned());
        }
        if b.get(4) != Some(&2) || b.get(5) != Some(&1) {
            return Err("not a 64-bit little-endian ELF file".to_owned());
        }
        let mut elf = Self {
            e_type: u16_at(b, 16)?,
            machine: u16_at(b, 18)?,
            ..Self::default()
        };
        let phoff = usize_of(u64_at(b, 32)?)?;
        let phentsize = usize::from(u16_at(b, 54)?);
        let phnum = usize::from(u16_at(b, 56)?);
        if phentsize < 56 {
            return Err(format!("program header entries of {phentsize} bytes"));
        }
        let mut dynamic = None;
        for i in 0..phnum {
            let ph = i
                .checked_mul(phentsize)
                .and_then(|o| o.checked_add(phoff))
                .ok_or("program header offset overflow")?;
            let p_type = u32_at(b, ph)?;
            let p_flags = u32_at(b, ph.checked_add(4).ok_or("overflow")?)?;
            match p_type {
                PT_INTERP => elf.interp = true,
                PT_GNU_RELRO => elf.relro = true,
                PT_GNU_STACK => elf.stack = Some(p_flags),
                PT_DYNAMIC => {
                    let off = usize_of(u64_at(b, ph.checked_add(8).ok_or("overflow")?)?)?;
                    let size = usize_of(u64_at(b, ph.checked_add(32).ok_or("overflow")?)?)?;
                    dynamic = Some((off, size));
                }
                _ => {}
            }
        }
        if let Some((off, size)) = dynamic {
            elf.dynamic = true;
            let end = off.checked_add(size).ok_or("dynamic section overflow")?;
            let section = b
                .get(off..end)
                .ok_or("the dynamic segment lies outside the file")?;
            let (entries, _) = section.as_chunks::<16>();
            for entry in entries {
                let tag = u64_at(entry, 0)?;
                let val = u64_at(entry, 8)?;
                match tag {
                    DT_NULL => break,
                    DT_NEEDED => elf.needed = elf.needed.saturating_add(1),
                    DT_TEXTREL => elf.textrel = true,
                    DT_BIND_NOW => elf.bind_now = true,
                    DT_FLAGS => {
                        elf.bind_now |= val & DF_BIND_NOW != 0;
                        elf.textrel |= val & DF_TEXTREL != 0;
                    }
                    DT_FLAGS_1 => {
                        elf.bind_now |= val & DF_1_NOW != 0;
                        elf.pie_flag |= val & DF_1_PIE != 0;
                    }
                    _ => {}
                }
            }
        }
        Ok(elf)
    }

    fn problems(&self, expect: Expect, m: Machine) -> Vec<String> {
        let mut p = Vec::new();
        let machine = match m {
            Machine::X86_64 => EM_X86_64,
            Machine::Aarch64 => EM_AARCH64,
        };
        if self.machine != machine {
            p.push(format!(
                "e_machine is {}, not {machine} ({m:?})",
                self.machine
            ));
        }
        let pie = self.e_type == ET_DYN && self.pie_flag;
        match expect {
            Expect::GnuPie(_) => {
                if !pie {
                    p.push(format!(
                        "not a PIE (e_type {}, DF_1_PIE {})",
                        self.e_type, self.pie_flag
                    ));
                }
                if !self.interp {
                    p.push("no PT_INTERP: not dynamically linked".to_owned());
                }
                if !self.bind_now {
                    p.push("no BIND_NOW (DT_BIND_NOW, DF_BIND_NOW or DF_1_NOW)".to_owned());
                }
                if !self.relro {
                    p.push("no PT_GNU_RELRO segment".to_owned());
                }
            }
            Expect::StaticPie(_) => {
                if !pie {
                    p.push(format!(
                        "not static-pie (e_type {}, DF_1_PIE {})",
                        self.e_type, self.pie_flag
                    ));
                }
                self.static_problems(&mut p);
            }
            Expect::StaticNoPie(_) => {
                if self.e_type != ET_EXEC {
                    p.push(format!(
                        "e_type {} is not ET_EXEC: if the toolchain now links static-pie for this target, move \
                         the expectation to static-pie (ADR-137, ARCHITECTURE §11.7)",
                        self.e_type
                    ));
                }
                self.static_problems(&mut p);
            }
            Expect::MachOPie(_) | Expect::PeAslr(_) => p.push("not an ELF expectation".to_owned()),
        }
        match self.stack {
            None => p.push("no PT_GNU_STACK segment".to_owned()),
            Some(f) if f & PF_X != 0 => p.push("PT_GNU_STACK is executable".to_owned()),
            Some(_) => {}
        }
        if self.textrel {
            p.push("text relocations (DT_TEXTREL or DF_TEXTREL)".to_owned());
        }
        p
    }

    fn static_problems(&self, p: &mut Vec<String>) {
        if self.interp {
            p.push("has PT_INTERP: not statically linked".to_owned());
        }
        if self.needed > 0 {
            p.push(format!(
                "{} DT_NEEDED entries: not statically linked",
                self.needed
            ));
        }
    }
}

// ----- Mach-O --------------------------------------------------------------------------------------------------

const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_EXECUTE: u32 = 2;
const MH_ALLOW_STACK_EXECUTION: u32 = 0x2_0000;
const MH_PIE: u32 = 0x20_0000;
const CPU_TYPE_X86_64: u32 = 0x0100_0007;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;

/// The facts of a thin 64-bit little-endian Mach-O file the gate reads.
#[derive(Debug, PartialEq, Eq)]
struct MachO {
    cputype: u32,
    filetype: u32,
    flags: u32,
}

impl MachO {
    fn parse(b: &[u8]) -> Result<Self, String> {
        if u32_at(b, 0)? != MH_MAGIC_64 {
            return Err("not a thin 64-bit little-endian Mach-O file".to_owned());
        }
        Ok(Self {
            cputype: u32_at(b, 4)?,
            filetype: u32_at(b, 12)?,
            flags: u32_at(b, 24)?,
        })
    }

    fn problems(&self, m: Machine) -> Vec<String> {
        let mut p = Vec::new();
        let cpu = match m {
            Machine::X86_64 => CPU_TYPE_X86_64,
            Machine::Aarch64 => CPU_TYPE_ARM64,
        };
        if self.cputype != cpu {
            p.push(format!("cputype {:#x}, not {cpu:#x} ({m:?})", self.cputype));
        }
        if self.filetype != MH_EXECUTE {
            p.push(format!("filetype {} is not MH_EXECUTE", self.filetype));
        }
        if self.flags & MH_PIE == 0 {
            p.push("no MH_PIE".to_owned());
        }
        if self.flags & MH_ALLOW_STACK_EXECUTION != 0 {
            p.push("MH_ALLOW_STACK_EXECUTION".to_owned());
        }
        p
    }
}

// ----- PE ------------------------------------------------------------------------------------------------------

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
const IMAGE_FILE_RELOCS_STRIPPED: u16 = 0x1;
const PE32_PLUS: u16 = 0x20b;
const DLL_HIGH_ENTROPY_VA: u16 = 0x20;
const DLL_DYNAMIC_BASE: u16 = 0x40;
const DLL_NX_COMPAT: u16 = 0x100;

/// The facts of a PE image the gate reads.
#[derive(Debug, PartialEq, Eq)]
struct Pe {
    machine: u16,
    characteristics: u16,
    magic: u16,
    dll_characteristics: u16,
}

impl Pe {
    fn parse(b: &[u8]) -> Result<Self, String> {
        if b.get(..2) != Some(b"MZ") {
            return Err("not a PE file (no MZ header)".to_owned());
        }
        let pe = usize_of(u64::from(u32_at(b, 0x3c)?))?;
        let sig_end = pe.checked_add(4).ok_or("overflow")?;
        if b.get(pe..sig_end) != Some(b"PE\0\0") {
            return Err("not a PE file (no PE signature)".to_owned());
        }
        let coff = sig_end;
        let opt = coff.checked_add(20).ok_or("overflow")?;
        Ok(Self {
            machine: u16_at(b, coff)?,
            characteristics: u16_at(b, coff.checked_add(18).ok_or("overflow")?)?,
            magic: u16_at(b, opt)?,
            // DllCharacteristics sits at offset 70 of both the PE32 and the PE32+ optional header
            dll_characteristics: u16_at(b, opt.checked_add(70).ok_or("overflow")?)?,
        })
    }

    fn problems(&self, m: Machine) -> Vec<String> {
        let mut p = Vec::new();
        let machine = match m {
            Machine::X86_64 => IMAGE_FILE_MACHINE_AMD64,
            Machine::Aarch64 => IMAGE_FILE_MACHINE_ARM64,
        };
        if self.machine != machine {
            p.push(format!(
                "machine {:#x}, not {machine:#x} ({m:?})",
                self.machine
            ));
        }
        if self.magic != PE32_PLUS {
            p.push(format!(
                "optional header magic {:#x} is not PE32+",
                self.magic
            ));
        }
        for (flag, name) in [
            (DLL_DYNAMIC_BASE, "DYNAMIC_BASE"),
            (DLL_HIGH_ENTROPY_VA, "HIGH_ENTROPY_VA"),
            (DLL_NX_COMPAT, "NX_COMPAT"),
        ] {
            if self.dll_characteristics & flag == 0 {
                p.push(format!("no {name}"));
            }
        }
        if self.characteristics & IMAGE_FILE_RELOCS_STRIPPED != 0 {
            p.push("relocations stripped: the image cannot be rebased".to_owned());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// No problems.
    const NONE: [String; 0] = [];

    /// Writes `src` into `b` at `off`.
    fn put(b: &mut [u8], off: usize, src: &[u8]) {
        assert!(b.len() >= off + src.len());
        for (d, s) in b[off..].iter_mut().zip(src) {
            *d = *s;
        }
    }

    /// A minimal ELF64 little-endian file: the header, `phdrs` (type, flags), and a dynamic segment of `dyn_entries`
    /// when given.
    fn elf(e_type: u16, machine: u16, phdrs: &[(u32, u32)], dyn_entries: &[(u64, u64)]) -> Vec<u8> {
        let mut b = vec![0_u8; 64];
        put(&mut b, 0, b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        put(&mut b, 16, &e_type.to_le_bytes());
        put(&mut b, 18, &machine.to_le_bytes());
        let mut phdrs = phdrs.to_vec();
        if !dyn_entries.is_empty() {
            phdrs.push((PT_DYNAMIC, 6));
        }
        put(&mut b, 32, &64_u64.to_le_bytes());
        put(&mut b, 54, &56_u16.to_le_bytes());
        put(
            &mut b,
            56,
            &u16::try_from(phdrs.len()).unwrap().to_le_bytes(),
        );
        let dyn_off = 64 + 56 * phdrs.len();
        let dyn_size = 16 * (dyn_entries.len() + 1);
        for (t, f) in &phdrs {
            let mut ph = vec![0_u8; 56];
            put(&mut ph, 0, &t.to_le_bytes());
            put(&mut ph, 4, &f.to_le_bytes());
            if *t == PT_DYNAMIC {
                put(&mut ph, 8, &u64::try_from(dyn_off).unwrap().to_le_bytes());
                put(&mut ph, 32, &u64::try_from(dyn_size).unwrap().to_le_bytes());
            }
            b.extend(ph);
        }
        for (tag, val) in dyn_entries.iter().chain([&(DT_NULL, 0)]) {
            b.extend(tag.to_le_bytes());
            b.extend(val.to_le_bytes());
        }
        b
    }

    const RW: u32 = 6;

    fn gnu_pie() -> Vec<u8> {
        elf(
            ET_DYN,
            EM_X86_64,
            &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
            &[
                (DT_NEEDED, 1),
                (DT_NEEDED, 2),
                (DT_FLAGS, DF_BIND_NOW),
                (DT_FLAGS_1, DF_1_NOW | DF_1_PIE),
            ],
        )
    }

    const GNU: Expect = Expect::GnuPie(Machine::X86_64);

    /// Program headers, dynamic entries and the problem they cause.
    type Case = (&'static [(u32, u32)], &'static [(u64, u64)], &'static str);

    #[test]
    fn every_release_target_has_an_expectation() {
        for t in [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ] {
            Expect::of(t).unwrap();
            assert!(
                crate::workspace::root()
                    .map(|r| std::fs::read_to_string(r.join("dist-workspace.toml")).unwrap())
                    .unwrap()
                    .contains(&format!("\"{t}\"")),
                "{t} is not a dist target"
            );
        }
        assert!(Expect::of("riscv64gc-unknown-linux-gnu").is_err());
        assert_eq!(
            Expect::of("aarch64-unknown-linux-musl").unwrap(),
            Expect::StaticNoPie(Machine::Aarch64)
        );
    }

    #[test]
    fn a_gnu_pie_passes_and_each_missing_protection_fails() {
        assert_eq!(check(GNU, &gnu_pie()), NONE);
        // -C relocation-model=static (the rejected O5): ET_EXEC, no DF_1_PIE, everything else kept
        let nopie = elf(
            ET_EXEC,
            EM_X86_64,
            &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
            &[
                (DT_NEEDED, 1),
                (DT_FLAGS, DF_BIND_NOW),
                (DT_FLAGS_1, DF_1_NOW),
            ],
        );
        let p = check(GNU, &nopie);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("not a PIE"), "{p:?}");
        // a shared library has ET_DYN but no DF_1_PIE
        let lib = elf(
            ET_DYN,
            EM_X86_64,
            &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
            &[(DT_FLAGS, DF_BIND_NOW)],
        );
        assert!(check(GNU, &lib)[0].starts_with("not a PIE"));
        // lazy binding, partial RELRO, an executable or missing stack, text relocations: one problem each
        let cases: [Case; 6] = [
            (
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE)],
                "no BIND_NOW",
            ),
            (
                &[(PT_INTERP, 4), (PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE | DF_1_NOW)],
                "no PT_GNU_RELRO",
            ),
            (
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW | PF_X)],
                &[(DT_FLAGS_1, DF_1_PIE | DF_1_NOW)],
                "PT_GNU_STACK is executable",
            ),
            (
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4)],
                &[(DT_FLAGS_1, DF_1_PIE | DF_1_NOW)],
                "no PT_GNU_STACK",
            ),
            (
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE | DF_1_NOW), (DT_TEXTREL, 0)],
                "text relocations",
            ),
            (
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE | DF_1_NOW), (DT_FLAGS, DF_TEXTREL)],
                "text relocations",
            ),
        ];
        for (phdrs, dyns, want) in cases {
            let p = check(GNU, &elf(ET_DYN, EM_X86_64, phdrs, dyns));
            assert_eq!(p.len(), 1, "{want}: {p:?}");
            assert!(p[0].starts_with(want), "{want}: {p:?}");
        }
        // DT_BIND_NOW alone is BIND_NOW too
        let p = check(
            GNU,
            &elf(
                ET_DYN,
                EM_X86_64,
                &[(PT_INTERP, 4), (PT_GNU_RELRO, 4), (PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE), (DT_BIND_NOW, 0)],
            ),
        );
        assert!(p.is_empty(), "{p:?}");
        // the wrong architecture
        let p = check(Expect::GnuPie(Machine::Aarch64), &gnu_pie());
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("e_machine is 62"), "{p:?}");
    }

    #[test]
    fn musl_static_pie_and_static_non_pie() {
        let static_pie = elf(
            ET_DYN,
            EM_X86_64,
            &[(PT_GNU_STACK, RW), (PT_GNU_RELRO, 4)],
            &[(DT_FLAGS, DF_BIND_NOW), (DT_FLAGS_1, DF_1_NOW | DF_1_PIE)],
        );
        let static_exec = elf(ET_EXEC, EM_AARCH64, &[(PT_GNU_STACK, RW)], &[]);
        let x86 = Expect::StaticPie(Machine::X86_64);
        let arm = Expect::StaticNoPie(Machine::Aarch64);
        assert_eq!(check(x86, &static_pie), NONE);
        assert_eq!(check(arm, &static_exec), NONE);
        // x86_64-musl linked non-PIE fails; a dynamic binary fails both
        let p = check(x86, &elf(ET_EXEC, EM_X86_64, &[(PT_GNU_STACK, RW)], &[]));
        assert!(p.len() == 1 && p[0].starts_with("not static-pie"), "{p:?}");
        let p = check(x86, &gnu_pie());
        assert!(p.iter().any(|s| s.starts_with("has PT_INTERP")), "{p:?}");
        assert!(p.iter().any(|s| s.starts_with("2 DT_NEEDED")), "{p:?}");
        // aarch64-musl turning static-pie (a future rustc) fails and says what to change
        let p = check(
            arm,
            &elf(
                ET_DYN,
                EM_AARCH64,
                &[(PT_GNU_STACK, RW)],
                &[(DT_FLAGS_1, DF_1_PIE)],
            ),
        );
        assert!(
            p.len() == 1 && p[0].contains("move the expectation to static-pie"),
            "{p:?}"
        );
        let p = check(
            arm,
            &elf(ET_EXEC, EM_AARCH64, &[(PT_GNU_STACK, RW | PF_X)], &[]),
        );
        assert_eq!(p, ["PT_GNU_STACK is executable"]);
    }

    #[test]
    fn malformed_files_fail_closed() {
        for bytes in [
            &b""[..],
            b"\x7fELF",
            b"MZ",
            b"\xcf\xfa\xed\xfe",
            b"#!/bin/sh\n",
        ] {
            for e in [
                GNU,
                Expect::StaticPie(Machine::X86_64),
                Expect::MachOPie(Machine::Aarch64),
                Expect::PeAslr(Machine::X86_64),
            ] {
                assert!(!check(e, bytes).is_empty(), "{e:?} {bytes:?}");
            }
        }
        // a 32-bit ELF, and program headers or a dynamic segment past the end of the file
        let mut b = gnu_pie();
        b[4] = 1;
        assert!(check(GNU, &b)[0].contains("64-bit"));
        let mut b = gnu_pie();
        put(&mut b, 56, &500_u16.to_le_bytes());
        assert!(check(GNU, &b)[0].starts_with("truncated"));
        let b = gnu_pie();
        assert_ne!(check(GNU, &b[..b.len() - 20]), NONE);
    }

    fn macho(cpu: u32, filetype: u32, flags: u32) -> Vec<u8> {
        let mut b = vec![0_u8; 32];
        put(&mut b, 0, &MH_MAGIC_64.to_le_bytes());
        put(&mut b, 4, &cpu.to_le_bytes());
        put(&mut b, 12, &filetype.to_le_bytes());
        put(&mut b, 24, &flags.to_le_bytes());
        b
    }

    #[test]
    fn mach_o_needs_pie_and_no_executable_stack() {
        let arm = Expect::MachOPie(Machine::Aarch64);
        // MH_NOUNDEFS | MH_DYLDLINK | MH_TWOLEVEL | MH_PIE, as ld64 writes them
        assert_eq!(
            check(arm, &macho(CPU_TYPE_ARM64, MH_EXECUTE, 0x0020_0085)),
            NONE
        );
        assert_eq!(
            check(arm, &macho(CPU_TYPE_ARM64, MH_EXECUTE, 0x85)),
            ["no MH_PIE"]
        );
        assert_eq!(
            check(
                arm,
                &macho(
                    CPU_TYPE_ARM64,
                    MH_EXECUTE,
                    MH_PIE | MH_ALLOW_STACK_EXECUTION
                )
            ),
            ["MH_ALLOW_STACK_EXECUTION"]
        );
        assert_eq!(
            check(arm, &macho(CPU_TYPE_X86_64, MH_EXECUTE, MH_PIE)).len(),
            1
        );
        assert_eq!(check(arm, &macho(CPU_TYPE_ARM64, 6, MH_PIE)).len(), 1);
    }

    fn pe(machine: u16, characteristics: u16, magic: u16, dll: u16) -> Vec<u8> {
        let mut b = vec![0_u8; 0x80 + 4 + 20 + 112];
        put(&mut b, 0, b"MZ");
        put(&mut b, 0x3c, &0x80_u32.to_le_bytes());
        put(&mut b, 0x80, b"PE\0\0");
        put(&mut b, 0x84, &machine.to_le_bytes());
        put(&mut b, 0x96, &characteristics.to_le_bytes());
        put(&mut b, 0x98, &magic.to_le_bytes());
        put(&mut b, 0x98 + 70, &dll.to_le_bytes());
        b
    }

    #[test]
    fn pe_needs_high_entropy_aslr_and_dep() {
        let x64 = Expect::PeAslr(Machine::X86_64);
        // link.exe's defaults for an x64 executable: DYNAMIC_BASE, HIGH_ENTROPY_VA, NX_COMPAT, TERMINAL_SERVER_AWARE
        let all = 0x8160;
        assert_eq!(
            check(x64, &pe(IMAGE_FILE_MACHINE_AMD64, 0x22, PE32_PLUS, all)),
            NONE
        );
        assert_eq!(
            check(
                x64,
                &pe(
                    IMAGE_FILE_MACHINE_AMD64,
                    0x22,
                    PE32_PLUS,
                    all & !DLL_HIGH_ENTROPY_VA
                )
            ),
            ["no HIGH_ENTROPY_VA"]
        );
        assert_eq!(
            check(x64, &pe(IMAGE_FILE_MACHINE_AMD64, 0x22, PE32_PLUS, 0)),
            ["no DYNAMIC_BASE", "no HIGH_ENTROPY_VA", "no NX_COMPAT"]
        );
        assert_eq!(
            check(x64, &pe(IMAGE_FILE_MACHINE_AMD64, 0x23, PE32_PLUS, all)),
            ["relocations stripped: the image cannot be rebased"]
        );
        assert_eq!(
            check(x64, &pe(IMAGE_FILE_MACHINE_ARM64, 0x22, PE32_PLUS, all)).len(),
            1
        );
        assert_eq!(
            check(x64, &pe(IMAGE_FILE_MACHINE_AMD64, 0x22, 0x10b, all)).len(),
            1
        );
    }

    /// This test binary is a linux-gnu PIE linked by the pinned toolchain with its defaults (full RELRO, `-z now`):
    /// the same link flags the release artifacts get.
    #[test]
    #[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))]
    fn this_test_binary_is_a_hardened_gnu_pie() {
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(check(GNU, &exe), NONE);
        let elf = Elf::parse(&exe).unwrap();
        assert!(elf.needed > 0 && elf.interp && elf.dynamic, "{elf:?}");
        // and it is not a static or a non-PIE binary
        assert_ne!(check(Expect::StaticPie(Machine::X86_64), &exe), NONE);
        assert_ne!(check(Expect::StaticNoPie(Machine::X86_64), &exe), NONE);
    }

    #[test]
    fn the_command_reports_every_binary_and_fails_on_any() {
        let dir = super::super::private_scratch("snp-hardening-test").unwrap();
        let good = dir.join("good");
        let bad = dir.join("bad");
        std::fs::write(&good, gnu_pie()).unwrap();
        std::fs::write(&bad, elf(ET_EXEC, EM_X86_64, &[(PT_GNU_STACK, RW)], &[])).unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_owned();
        let t = "x86_64-unknown-linux-gnu".to_owned();
        run(&[t.clone(), s(&good)]).unwrap();
        let e = run(&[t.clone(), s(&good), s(&bad)]).unwrap_err();
        assert!(format!("{e}").starts_with("1 of 2 binaries"), "{e}");
        assert!(run(std::slice::from_ref(&t)).is_err());
        assert!(run(&[t, s(&dir.join("missing"))]).is_err());
        assert!(run(&["sparc-sun-solaris".to_owned(), s(&good)]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
