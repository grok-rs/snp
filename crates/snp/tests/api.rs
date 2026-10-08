//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.24 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. `entry`
//! is only coerced to a `fn` pointer (never called), so the test passes on the W0 stub.
//!
//! Owner: WP-33 (created by WP-00).

#[cfg(test)]
mod api {
    use snp::Exit;

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// `Exit` is exhaustive: this match has no wildcard arm.
    fn raw_code(e: Exit) -> i32 {
        match e {
            Exit::Code(c) => i32::from(c),
            Exit::Raw(r) => r,
        }
    }

    #[test]
    fn entry_and_exit() {
        send_sync::<Exit>();
        has::<fn() -> Exit>(snp::entry);
        assert_eq!(raw_code(Exit::Code(3)), 3);
        assert_eq!(raw_code(Exit::Raw(300)), 300);
        assert_ne!(Exit::Code(1), Exit::Raw(1));
    }
}
