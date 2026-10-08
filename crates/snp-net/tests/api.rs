//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.9 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-16 (created by WP-00).

#[cfg(test)]
mod api {
    use std::error::Error;
    use std::io;
    use std::path::{Path, PathBuf};

    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_net::archive::{self, Extract};
    use snp_net::tools::{self, Artifact, Layout, Tool};
    use snp_net::{Checksum, Fetch, HttpFetch, NetError};

    fn send_sync<T: Send + Sync>() {}
    fn error_type<E: Error + Send + Sync + 'static>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The seam's methods, through a trait object.
    fn fetch(f: &dyn Fetch, cancel: &CancelToken) {
        has::<Result<String, NetError>>(f.get_text("https://example.invalid", 1));
        has::<Result<PathBuf, NetError>>(f.get_verified(
            "https://example.invalid",
            &Checksum::Sha256([0; 32]),
            Path::new("/tmp"),
            "x",
            cancel,
        ));
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Checksum>();
        send_sync::<NetError>();
        send_sync::<HttpFetch>();
        send_sync::<Extract>();
        send_sync::<Tool>();
        send_sync::<Artifact>();
        send_sync::<Layout>();
        send_sync::<&dyn Fetch>();
        error_type::<NetError>();
    }

    #[test]
    fn types() {
        has::<[NetError; 9]>([
            NetError::Offline,
            NetError::Http {
                url: String::new(),
                status: 404,
            },
            NetError::Checksum {
                url: String::new(),
                expected: String::new(),
                actual: String::new(),
            },
            NetError::TooLarge {
                url: String::new(),
                limit: 1,
            },
            NetError::Io(io::Error::from(io::ErrorKind::NotFound)),
            NetError::Transport {
                url: String::new(),
                msg: String::new(),
            },
            NetError::UnsafeEntry(String::new()),
            NetError::Insecure(String::new()),
            NetError::Cancelled,
        ]);
        has::<NetError>(NetError::from(io::Error::from(io::ErrorKind::Other)));
        has::<Extract>(Extract {
            strip_components: 1,
            allow_symlinks_inside: false,
            max_total_bytes: 1,
        });
        has::<[Tool; 6]>([
            Tool::Uv,
            Tool::Node,
            Tool::Go,
            Tool::Rustup,
            Tool::RubyBuild,
            Tool::Rbenv,
        ]);
        has::<Artifact>(Artifact {
            url: String::new(),
            checksum: Checksum::Sha256([0; 32]),
            layout: Layout::Executable,
        });
        has::<[Layout; 3]>([
            Layout::TarGz { strip: 1 },
            Layout::TarXz { strip: 1 },
            Layout::Zip { strip: 0 },
        ]);
        has::<&str>(tools::UV_PINNED);
    }

    #[test]
    fn functions() {
        has::<fn(&dyn Fetch, &CancelToken)>(fetch);
        has::<fn(&Env) -> HttpFetch>(HttpFetch::from_env);
        has::<fn(&HttpFetch, &str, u64) -> Result<String, NetError>>(HttpFetch::get_text);
        has::<fn(&Path, &Path, &Extract) -> Result<(), NetError>>(archive::tar_gz);
        has::<fn(&Path, &Path, &Extract) -> Result<(), NetError>>(archive::tar_xz);
        has::<fn(&Path, &Path, &Extract) -> Result<(), NetError>>(archive::zip);
        has::<fn(Tool, &str, &str, &dyn Fetch) -> Result<Artifact, NetError>>(tools::resolve);
        has::<fn() -> &'static str>(tools::host_target);
    }
}
