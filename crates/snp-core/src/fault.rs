//! Test-only fault injection (ADR-066). Always compiled; [`hit`] is an inlined no-op unless the
//! crate feature `fault-injection` is on (enabled only by `crates/snp`'s `testing` feature for the
//! fault-injection and panic tests, never in releases).
//!
//! `SNP_FAULT=<point>[@n]` arms one point; `<point>` is a [`Point`] name (`PrinterWrite`,
//! `WorkerPartition`, `StashState`, `RestoreCommand`, `InstallCommit`, `StoreMaterialize`),
//! optionally with an argument (`StashState(3)`; without one, any argument matches). The `n`-th hit
//! (default 1) panics with the payload `snp fault injection: <point>`.

use std::fmt;

use crate::env::Env;

/// A panic-injection point.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Point {
    /// Inside the Printer's state lock (S14 test).
    PrinterWrite,
    /// Inside a worker partition (thread holding a pool permit).
    WorkerPartition,
    /// A stash state transition (`n` = the transition's index).
    StashState(u8),
    /// A restore command (`n` = its index in the restore sequence).
    RestoreCommand(u8),
    /// Just before an install's commit marker is written.
    InstallCommit,
    /// While materialising a store entry.
    StoreMaterialize,
}

impl Point {
    const fn name(self) -> &'static str {
        match self {
            Self::PrinterWrite => "PrinterWrite",
            Self::WorkerPartition => "WorkerPartition",
            Self::StashState(_) => "StashState",
            Self::RestoreCommand(_) => "RestoreCommand",
            Self::InstallCommit => "InstallCommit",
            Self::StoreMaterialize => "StoreMaterialize",
        }
    }

    const fn arg(self) -> Option<u8> {
        match self {
            Self::StashState(n) | Self::RestoreCommand(n) => Some(n),
            _ => None,
        }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.arg() {
            Some(n) => write!(f, "{}({n})", self.name()),
            None => f.write_str(self.name()),
        }
    }
}

/// Arms the point named by `SNP_FAULT` in `env` (disarms when unset or malformed). Read once by
/// `snp::entry` from the captured environment.
pub fn arm_from_env(env: &Env) {
    #[cfg(feature = "fault-injection")]
    imp::arm(env.get("SNP_FAULT"));
    #[cfg(not(feature = "fault-injection"))]
    let _: &Env = env;
}

/// Panics with payload `snp fault injection: <point>` on the armed hit; otherwise does nothing.
#[inline]
pub fn hit(p: Point) {
    #[cfg(feature = "fault-injection")]
    imp::hit(p);
    #[cfg(not(feature = "fault-injection"))]
    let _: Point = p;
}

#[cfg(feature = "fault-injection")]
mod imp {
    use std::ffi::OsStr;
    use std::sync::Mutex;

    use super::Point;

    #[derive(Debug)]
    struct Armed {
        name: String,
        arg: Option<u8>,
        nth: u32,
        count: u32,
    }

    static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

    const NAMES: [&str; 6] = [
        "PrinterWrite",
        "WorkerPartition",
        "StashState",
        "RestoreCommand",
        "InstallCommit",
        "StoreMaterialize",
    ];

    /// `<name>[(<arg>)][@<n>]`.
    fn parse(spec: &str) -> Option<Armed> {
        let (point, nth) = match spec.split_once('@') {
            Some((p, n)) => (p, n.parse::<u32>().ok().filter(|n| *n > 0)?),
            None => (spec, 1),
        };
        let (name, arg) = match point.split_once('(') {
            Some((name, rest)) => (name, Some(rest.strip_suffix(')')?.parse::<u8>().ok()?)),
            None => (point, None),
        };
        NAMES.contains(&name).then(|| Armed {
            name: name.to_owned(),
            arg,
            nth,
            count: 0,
        })
    }

    pub(super) fn arm(spec: Option<&OsStr>) {
        let armed = spec.and_then(OsStr::to_str).and_then(parse);
        *crate::sync::lock(&ARMED) = armed;
    }

    pub(super) fn hit(p: Point) {
        let fire = {
            let mut guard = crate::sync::lock(&ARMED);
            match guard.as_mut() {
                Some(a) if a.name == p.name() && (a.arg.is_none() || a.arg == p.arg()) => {
                    a.count = a.count.saturating_add(1);
                    a.count == a.nth
                }
                _ => false,
            }
        };
        if fire {
            inject(p);
        }
    }

    #[cold]
    #[expect(
        clippy::panic,
        reason = "test-only panic injection (feature fault-injection, ADR-066)"
    )]
    fn inject(p: Point) -> ! {
        panic!("snp fault injection: {p}");
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn specs() {
            let a = parse("StashState(3)@2").unwrap();
            assert_eq!((a.name.as_str(), a.arg, a.nth), ("StashState", Some(3), 2));
            let a = parse("PrinterWrite").unwrap();
            assert_eq!((a.arg, a.nth), (None, 1));
            assert!(parse("Nope").is_none());
            assert!(parse("PrinterWrite@0").is_none());
            assert!(parse("StashState(x)").is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_disarmed_hits() {
        assert_eq!(Point::StashState(3).to_string(), "StashState(3)");
        assert_eq!(Point::PrinterWrite.to_string(), "PrinterWrite");
        // Nothing is armed by default: hits never panic.
        arm_from_env(&Env::from_pairs([("OTHER", "1")]));
        hit(Point::InstallCommit);
        hit(Point::RestoreCommand(1));
    }
}
