//! The per-push guard of the streamed stash patch (ADR-100; ARCHITECTURE §12.1 B8, T6): stashing a patch of more than
//! 20 MB must not grow the resident memory of the process by more than a few MiB. The nightly bench gates B8's dhat heap
//! peak (B3's + 2 MB); this test catches a buffered patch on every push, in the `perf-smoke` job (release mode,
//! `--run-ignored only -E 'test(/perf_/)'`).
//!
//! A resident-memory bound depends on the allocator, the build profile and the machine, so it is `#[ignore]`d in the
//! deterministic default gate, like `snp-proc`'s `perf_output_budget_bounds_resident_memory`. It is the only test of
//! its binary: nextest and `cargo test` alike then run it in a process of its own, whose `VmHWM` nothing else raises.
//!
//! Owner: WP-36.

#[cfg(test)]
#[expect(
    dead_code,
    reason = "shared with the catalog binary; this binary uses a subset"
)]
mod common;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod perf {
    use std::io::{BufWriter, Write as _};
    use std::path::Path;

    use snp_stash::Stashed;

    use crate::common::Sandbox;

    /// The tracked file: 12 MiB of numbered lines. Rewriting every line makes an unstaged patch of about 24 MB.
    const FILE_BYTES: usize = 12 << 20;
    /// The smallest patch the test accepts (the plan's ≥ 20 MB).
    const PATCH_MIN: u64 = 20_000_000;
    /// The resident growth allowed while stashing and restoring. A buffered patch holds at least the patch (24 MB),
    /// a streamed one holds pipe and hash buffers.
    const GROWTH_MAX_KB: u64 = 8 * 1024;

    /// The process's peak resident set (`VmHWM`, kB).
    fn vm_hwm_kb() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        status
            .lines()
            .find_map(|l| l.strip_prefix("VmHWM:"))
            .unwrap()
            .trim()
            .trim_end_matches(" kB")
            .trim()
            .parse()
            .unwrap()
    }

    /// Writes `FILE_BYTES` of lines salted with `salt` in small chunks, so the test itself never holds the file.
    fn write_big(path: &Path, salt: &str) {
        let mut w = BufWriter::with_capacity(64 << 10, std::fs::File::create(path).unwrap());
        let mut written = 0;
        let mut i = 0_u64;
        while written < FILE_BYTES {
            let line = format!("line {i:08} {salt} lorem ipsum dolor sit amet\n");
            w.write_all(line.as_bytes()).unwrap();
            written += line.len();
            i += 1;
        }
        w.flush().unwrap();
    }

    #[test]
    #[ignore = "perf"]
    fn perf_patch_stream_bounds_resident_memory() {
        let sb = Sandbox::new();
        let repo = sb.init("r");
        write_big(&repo.join("big.txt"), "aaaa");
        sb.write(&repo, "small.txt", b"small\n");
        sb.git(&repo, &["add", "big.txt", "small.txt"]);
        sb.commit(&repo, "big");
        // a staged change and a large unstaged rewrite, B8's shape
        sb.write(&repo, "small.txt", b"small, changed\n");
        sb.git(&repo, &["add", "small.txt"]);
        write_big(&repo.join("big.txt"), "bbbb");

        let (cx, _out, _err) = sb.ctx(&repo);
        let before = vm_hwm_kb();
        let guard = snp_stash::enter(cx).unwrap();
        let Stashed::Patch(patch) = guard.stashed().clone() else {
            panic!("nothing stashed: {:?}", guard.stashed())
        };
        guard.restore().unwrap();
        let growth = vm_hwm_kb().saturating_sub(before);

        let patch_len = std::fs::metadata(&patch).unwrap().len();
        eprintln!("patch {patch_len} bytes: VmHWM {before} kB -> +{growth} kB");
        assert!(patch_len >= PATCH_MIN, "the patch is {patch_len} bytes");
        assert!(
            growth <= GROWTH_MAX_KB,
            "VmHWM grew by {growth} kB while stashing a {patch_len}-byte patch (limit {GROWTH_MAX_KB} kB): \
             the patch is buffered instead of streamed"
        );
        // the unstaged rewrite is back
        let restored = std::fs::read(repo.join("big.txt")).unwrap();
        assert!(restored.len() >= FILE_BYTES);
        assert!(restored.starts_with(b"line 00000000 bbbb "));
        assert_eq!(sb.status_of(&repo, "big.txt"), "M");
        assert_eq!(sb.status_of(&repo, "small.txt"), "M");
    }
}
