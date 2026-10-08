//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.12 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-17 (created by WP-00).

#[cfg(test)]
mod api {
    use snp_hooks_data::{Epoch, Gate, HookClass, NativeSource, TagProfile};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Epoch>();
        send_sync::<HookClass>();
        send_sync::<Gate>();
        send_sync::<TagProfile>();
        send_sync::<NativeSource>();
    }

    #[test]
    fn types() {
        has::<u16>(Epoch(1).0);
        has::<[HookClass; 5]>([
            HookClass::Native,
            HookClass::NativeWithCaveats,
            HookClass::Delegate,
            HookClass::FailLanguage,
            HookClass::Stub,
        ]);
        has::<[Gate; 3]>([Gate::Exact, Gate::Caveat, Gate::Delegate]);
        has::<TagProfile>(TagProfile {
            tag: "v6.0.0",
            commit: "0000000000000000000000000000000000000000",
            manifest_yaml: "",
            hooks: &[("trailing-whitespace", HookClass::Native, Epoch(0))],
        });
        has::<[NativeSource; 2]>([NativeSource::PreCommitHooks, NativeSource::Builtin]);
        has::<&str>(snp_hooks_data::BUILTIN_MANIFEST_YAML);
        has::<&str>(snp_hooks_data::GENERATED_FROM);
    }

    #[test]
    fn functions() {
        has::<fn(&str) -> bool>(snp_hooks_data::is_pre_commit_hooks_url);
        has::<fn(&str) -> Option<&'static TagProfile>>(snp_hooks_data::profile_for_rev);
        has::<fn(&str) -> Option<(&'static str, Epoch)>>(snp_hooks_data::verified_blob);
        has::<fn(&str) -> Option<&'static str>>(snp_hooks_data::module_for_entry);
        has::<fn(&str) -> &'static [&'static str]>(snp_hooks_data::import_closure);
        has::<fn(&str) -> Gate>(snp_hooks_data::parity_gate);
    }
}
