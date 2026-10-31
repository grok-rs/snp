//! Removed-hook stubs: check-byte-order-marker and fix-encoding-pragma (v6.0.0+), autopep8-wrapper, flake8,
//! pyflakes (v3.x) (ARCHITECTURE §7.4, 09 §1).
//!
//! Their manifest entry is `pre-commit-hooks-removed <hookid> <new_hookid> <url>` (`removed.py`): `main` takes
//! `hookid, new_hookid, url = argv[:3]` (epoch 2, v3.1.0+; epoch 1 = v3.0.x unpacks the whole argv, so any hook
//! arg or file name is a `ValueError` crash) and raises `SystemExit` with
//! `` `{hookid}` has been removed -- use `{new_hookid}` from {url} `` (stderr, exit 1). The runner's argv is
//! `args + files`; the stub prepends the three entry tokens itself.
//!
//! `check-byte-order-marker` and `fix-encoding-pragma` were real hooks up to v5.0.0, implemented by WP-27 under the
//! same ids: the stub forwards the real hook's epochs to it. `fix-encoding-pragma`'s epoch 2 is ambiguous in the
//! data (the real v4.6.0–v5.0.0 hook and the v6.0.0 stub, `removed` epoch 2, share `Epoch(2)`), so the stub
//! makes the caller delegate there (preflight error).
//!
//! Owner: WP-26.

use std::ffi::OsStr;
use std::sync::Arc;

use snp_core::failure::PyExc;
use snp_hooks_data::Epoch;

use crate::framework::{self, PyOut};
use crate::{HookCx, NativeHook, Outcome};

/// One removed hook.
#[derive(Debug)]
pub(crate) struct Removed {
    id: &'static str,
    new_hookid: &'static str,
    url: &'static str,
    /// The real hook's epochs (≤ v5.0.0) that run the WP-27 native instead.
    real_epochs: &'static [u16],
    /// Epochs the data cannot tell apart from the real hook's: delegated.
    ambiguous_epochs: &'static [u16],
}

static CHECK_BOM: Removed = Removed {
    id: "check-byte-order-marker",
    new_hookid: "fix-byte-order-marker",
    url: "https://github.com/pre-commit/pre-commit-hooks",
    real_epochs: &[1],
    ambiguous_epochs: &[],
};
static FIX_ENCODING_PRAGMA: Removed = Removed {
    id: "fix-encoding-pragma",
    new_hookid: "pyupgrade",
    url: "https://github.com/asottile/pyupgrade",
    real_epochs: &[1],
    ambiguous_epochs: &[2],
};
static AUTOPEP8: Removed = Removed {
    id: "autopep8-wrapper",
    new_hookid: "autopep8",
    url: "https://github.com/pre-commit/mirrors-autopep8",
    real_epochs: &[],
    ambiguous_epochs: &[],
};
static FLAKE8: Removed = Removed {
    id: "flake8",
    new_hookid: "flake8",
    url: "https://gitlab.com/pycqa/flake8",
    real_epochs: &[],
    ambiguous_epochs: &[],
};
static PYFLAKES: Removed = Removed {
    id: "pyflakes",
    new_hookid: "flake8",
    url: "https://gitlab.com/pycqa/flake8",
    real_epochs: &[],
    ambiguous_epochs: &[],
};

/// This group's registry; [`crate::native`] concatenates the groups.
pub static HOOKS: &[&dyn NativeHook] = &[
    &CHECK_BOM,
    &FIX_ENCODING_PRAGMA,
    &AUTOPEP8,
    &FLAKE8,
    &PYFLAKES,
];

impl Removed {
    /// The real native for an epoch of the real hook.
    fn real(&self, epoch: Epoch) -> Option<&'static dyn NativeHook> {
        if self.real_epochs.contains(&epoch.0) {
            crate::registry::real_pch(self.id)
        } else {
            None
        }
    }

    /// `removed.main(argv)` for the full `argv` (entry tokens first).
    fn stub(&self, epoch: Epoch, argv: &[&OsStr]) -> Outcome {
        let mut out = PyOut::default();
        if epoch.0 <= 1 && !argv.is_empty() {
            return framework::crashed(
                out,
                &PyExc::new("ValueError", "too many values to unpack (expected 3)"),
            );
        }
        out.eprint(&format!(
            "`{}` has been removed -- use `{}` from {}",
            self.id, self.new_hookid, self.url
        ));
        Outcome {
            code: 1,
            output: out.finish(),
            reads_only: true,
            ..Outcome::default()
        }
    }
}

impl NativeHook for Removed {
    fn id(&self) -> &'static str {
        self.id
    }

    fn prog(&self) -> &'static str {
        "pre-commit-hooks-removed"
    }

    fn preflight(&self, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String> {
        if self.ambiguous_epochs.contains(&epoch.0) {
            return Err(format!(
                "{} epoch {} is both the real hook and the removed stub",
                self.id, epoch.0
            ));
        }
        if self.real_epochs.contains(&epoch.0) {
            return match self.real(epoch) {
                Some(h) => h.preflight(epoch, args),
                None => Err(format!("{} epoch {} has no native port", self.id, epoch.0)),
            };
        }
        if (1..=2).contains(&epoch.0) {
            Ok(())
        } else {
            Err(format!("removed-hook epoch {} is not reproduced", epoch.0))
        }
    }

    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome {
        match self.real(cx.epoch) {
            Some(h) => h.run(cx, argv),
            None => self.stub(cx.epoch, argv),
        }
    }

    fn reports_modifications_exactly(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_messages() {
        let o = CHECK_BOM.stub(Epoch(2), &[OsStr::new("--x"), OsStr::new("a.txt")]);
        assert_eq!(o.code, 1);
        let nl = if cfg!(windows) { "\r\n" } else { "\n" };
        assert_eq!(
            String::from_utf8(o.output).unwrap(),
            format!(
                "`check-byte-order-marker` has been removed -- use `fix-byte-order-marker` from \
                 https://github.com/pre-commit/pre-commit-hooks{nl}"
            )
        );
        let o = FIX_ENCODING_PRAGMA.stub(Epoch(2), &[]);
        assert!(String::from_utf8(o.output).unwrap().starts_with(
            "`fix-encoding-pragma` has been removed -- use `pyupgrade` from https://github.com/asottile/pyupgrade"
        ));
        // v3.0.x: `hookid, new_hookid, url = argv` with extra items.
        let o = FLAKE8.stub(Epoch(1), &[OsStr::new("a.py")]);
        assert!(o.crashed);
        assert_eq!(
            String::from_utf8(o.output).unwrap(),
            format!("ValueError: too many values to unpack (expected 3){nl}")
        );
        let o = PYFLAKES.stub(Epoch(1), &[]);
        assert_eq!(o.code, 1);
        assert!(!o.crashed);
    }

    #[test]
    fn preflight_decisions() {
        assert!(FIX_ENCODING_PRAGMA.preflight(Epoch(2), &[]).is_err());
        assert!(AUTOPEP8.preflight(Epoch(1), &[]).is_ok());
        assert!(AUTOPEP8.preflight(Epoch(2), &[]).is_ok());
        assert!(AUTOPEP8.preflight(Epoch(0), &[]).is_err());
        // Epoch 2 of check-byte-order-marker is the v6.0.0 stub.
        assert!(CHECK_BOM.preflight(Epoch(2), &[]).is_ok());
    }
}
