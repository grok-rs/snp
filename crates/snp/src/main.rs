//! The `snp` binary (ARCHITECTURE §1.4.24, §10.1): the only `main`, and the only place that may call
//! `std::process::exit`. `ExitCode` cannot carry a code > 255 on stable Rust (Windows `ExitCodeExt::from_raw` is
//! unstable in 1.99.0), so `entry` returns `Exit`.
//!
//! The global allocator is the system allocator (ARCHITECTURE §12.2 T17), with two build-time exceptions measured by
//! `cargo xtask bench` (WP-36): feature `dhat-heap` installs dhat's counting allocator for the B3 allocation budget
//! (never in releases), and feature `musl-mimalloc` installs mimalloc on musl targets only (ADR-079).
//!
//! Owner: WP-33 (W0 skeleton by WP-00; the allocator items by WP-36).

/// dhat's heap profiler: total bytes/blocks allocated, written to `dhat-heap.json` in the working directory at exit.
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// mimalloc on musl, iff ADR-079's measurement adopts it (`bench/musl-allocator.md`).
#[cfg(all(
    target_env = "musl",
    feature = "musl-mimalloc",
    not(feature = "dhat-heap")
))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    #[cfg(feature = "dhat-heap")]
    let profiler = dhat::Profiler::new_heap();
    // every guard and destructor of `entry` has already run
    let exit = snp::entry();
    // the profile covers the whole program and is written before a raw exit can skip destructors
    #[cfg(feature = "dhat-heap")]
    drop(profiler);
    match exit {
        snp::Exit::Code(c) => std::process::ExitCode::from(c),
        // clippy's `exit` lint never fires inside `main`; the clippy.toml `disallowed-methods` entry does.
        #[expect(
            clippy::disallowed_methods,
            reason = "Windows hook-impl `legacy | run` > 255 (14 A.7.4)"
        )]
        snp::Exit::Raw(r) => std::process::exit(r),
    }
}
