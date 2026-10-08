//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.23 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-34 (created by WP-00).

#[cfg(test)]
mod api {
    use std::path::PathBuf;

    use snp_core::failure::Failure;
    use snp_migrate::{Converted, LefthookOpts};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<LefthookOpts>();
        send_sync::<Converted>();
    }

    #[test]
    fn converters() {
        has::<LefthookOpts>(LefthookOpts {
            root: PathBuf::from("/r"),
            config: Some(PathBuf::from("/r/lefthook.yml")),
            output: None,
            portable: false,
            local: true,
            strict: false,
        });
        has::<Converted>(Converted {
            config_yaml: String::from("repos: []\n"),
            overlay_yaml: None,
            report: Vec::new(),
            lossy: false,
        });
        has::<fn(&LefthookOpts) -> Result<Converted, Failure>>(snp_migrate::lefthook);
        has::<fn(&[u8]) -> Result<Vec<u8>, Failure>>(snp_migrate::to_builtin);
        has::<fn(&[u8]) -> Result<Vec<u8>, Failure>>(snp_migrate::to_upstream);
    }
}
