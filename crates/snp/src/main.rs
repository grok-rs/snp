//! The `snp` binary (ARCHITECTURE §1.4.24, §10.1): the only `main`, and the only place that may call
//! `std::process::exit`. `ExitCode` cannot carry a code > 255 on stable Rust (Windows `ExitCodeExt::from_raw` is
//! unstable in 1.99.0), so `entry` returns `Exit`.
//!
//! Owner: WP-33 (W0 skeleton by WP-00).

fn main() -> std::process::ExitCode {
    // every guard and destructor of `entry` has already run
    match snp::entry() {
        snp::Exit::Code(c) => std::process::ExitCode::from(c),
        // clippy's `exit` lint never fires inside `main`; the clippy.toml `disallowed-methods` entry does.
        #[expect(
            clippy::disallowed_methods,
            reason = "Windows hook-impl `legacy | run` > 255 (14 A.7.4)"
        )]
        snp::Exit::Raw(r) => std::process::exit(r),
    }
}
