//! nodeenv 1.11.0's version ranges (`_is_exact_version`, `parse_node_range`, `_parse_comparator`,
//! `_comparator_constraints`, `_pad_version`, `match_node_range`, `resolve_node_version`): `20`, `18.19`, `^20.1`,
//! `20.x`, `>=18 <20`, `4.3.1 - 6.2.0`, `a || b` resolve to the highest matching release of `index.json`.
//!
//! Digits are ASCII (Python's `\d` also accepts other Unicode decimal digits; such a version names no release
//! either way). Numbers saturate at `u64::MAX` (Python integers are unbounded; no release is that large).
//!
//! Owner: WP-24.

/// A version triple.
pub(crate) type Triple = [u64; 3];

/// One constraint `(operator, version)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// `>=`.
    Ge,
    /// `>`.
    Gt,
    /// `<=`.
    Le,
    /// `<`.
    Lt,
}

/// A parsed range: alternatives (`||`), each a list of constraints that must all hold.
pub(crate) type Ranges = Vec<Vec<(Op, Triple)>>;

/// `str` without one trailing `\n` (Python's `$` also matches before a final newline).
fn dollar(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

/// A run of ASCII digits at the start of `s`: `(value, rest)`.
fn digits(s: &str) -> Option<(u64, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    let (d, rest) = s.split_at(n);
    let v = d.bytes().fold(0_u64, |acc, b| {
        acc.saturating_mul(10)
            .saturating_add(u64::from(b.saturating_sub(b'0')))
    });
    Some((v, rest))
}

/// `_EXACT_VERSION_RE = ^v?\d+\.\d+\.\d+(\+\S*)?$`.
pub(crate) fn is_exact(version: &str) -> bool {
    let s = dollar(version);
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut rest = s;
    for i in 0..3 {
        let Some((_, r)) = digits(rest) else {
            return false;
        };
        rest = r;
        if i < 2 {
            let Some(r) = rest.strip_prefix('.') else {
                return false;
            };
            rest = r;
        }
    }
    match rest.strip_prefix('+') {
        None => rest.is_empty(),
        Some(build) => !build.chars().any(snp_pycompat::chars::is_space),
    }
}

/// A version part: a number, or a wildcard (`x`, `X`, `*`).
#[derive(Copy, Clone)]
enum Part {
    /// A number.
    Num(u64),
    /// A wildcard.
    Wild,
}

/// `(?P<part>\d+|[xX*])` at the start of `s`.
fn part(s: &str) -> Option<(Part, &str)> {
    if let Some((v, rest)) = digits(s) {
        return Some((Part::Num(v), rest));
    }
    let rest = s.strip_prefix(['x', 'X', '*'])?;
    Some((Part::Wild, rest))
}

/// The comparator operator.
#[derive(Copy, Clone, PartialEq, Eq)]
enum CmpOp {
    /// `^`.
    Caret,
    /// `~`.
    Tilde,
    /// `>=`.
    Ge,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `<`.
    Lt,
    /// `=` (also no operator).
    Eq,
}

/// `_parse_comparator(token)`: `None` when the token is not a comparator.
fn parse_comparator(token: &str) -> Option<Vec<(Op, Triple)>> {
    let s = dollar(token);
    let (op, rest) = [
        ("^", CmpOp::Caret),
        ("~", CmpOp::Tilde),
        (">=", CmpOp::Ge),
        ("<=", CmpOp::Le),
        (">", CmpOp::Gt),
        ("<", CmpOp::Lt),
        ("=", CmpOp::Eq),
    ]
    .into_iter()
    .find_map(|(p, op)| s.strip_prefix(p).map(|r| (op, r)))
    .unwrap_or((CmpOp::Eq, s));
    let rest = rest.trim_start_matches(snp_pycompat::chars::is_space);
    let rest = rest.strip_prefix('v').unwrap_or(rest);
    let (major, mut rest) = part(rest)?;
    let mut minor = None;
    let mut patch = None;
    if let Some(r) = rest.strip_prefix('.')
        && let Some((p, r)) = part(r)
    {
        minor = Some(p);
        rest = r;
        if let Some(r) = rest.strip_prefix('.')
            && let Some((p, r)) = part(r)
        {
            patch = Some(p);
            rest = r;
        }
    }
    if !rest.is_empty() {
        return None;
    }
    Some(constraints(op, major, minor, patch))
}

/// `_comparator_constraints`: partial versions round up to the next release, as npm does.
fn constraints(
    op: CmpOp,
    major: Part,
    minor: Option<Part>,
    patch: Option<Part>,
) -> Vec<(Op, Triple)> {
    let Part::Num(major) = major else {
        return Vec::new();
    };
    let num = |p: Option<Part>| match p {
        Some(Part::Num(n)) => Some(n),
        _ => None,
    };
    let has_minor = num(minor).is_some();
    let has_patch = has_minor && num(patch).is_some();
    let minor = num(minor).unwrap_or(0);
    let patch = if has_patch {
        num(patch).unwrap_or(0)
    } else {
        0
    };
    let low = [major, minor, patch];
    let next_major = [major.saturating_add(1), 0, 0];
    let next_minor = [major, minor.saturating_add(1), 0];
    match op {
        CmpOp::Caret => {
            if !has_minor || major > 0 {
                vec![(Op::Ge, low), (Op::Lt, next_major)]
            } else if !has_patch || minor > 0 {
                vec![(Op::Ge, low), (Op::Lt, next_minor)]
            } else {
                vec![(Op::Ge, low), (Op::Lt, [0, 0, patch.saturating_add(1)])]
            }
        }
        CmpOp::Tilde => {
            if has_minor {
                vec![(Op::Ge, low), (Op::Lt, next_minor)]
            } else {
                vec![(Op::Ge, low), (Op::Lt, next_major)]
            }
        }
        CmpOp::Eq => {
            if !has_minor {
                vec![(Op::Ge, low), (Op::Lt, next_major)]
            } else if !has_patch {
                vec![(Op::Ge, low), (Op::Lt, next_minor)]
            } else {
                vec![(Op::Ge, low), (Op::Le, low)]
            }
        }
        CmpOp::Gt => {
            if !has_minor {
                vec![(Op::Ge, next_major)]
            } else if !has_patch {
                vec![(Op::Ge, next_minor)]
            } else {
                vec![(Op::Gt, low)]
            }
        }
        CmpOp::Le => {
            if !has_minor {
                vec![(Op::Lt, next_major)]
            } else if !has_patch {
                vec![(Op::Lt, next_minor)]
            } else {
                vec![(Op::Le, low)]
            }
        }
        CmpOp::Ge => vec![(Op::Ge, low)],
        CmpOp::Lt => vec![(Op::Lt, low)],
    }
}

/// `parse_node_range(spec)`: `None` when `spec` is not a range.
pub(crate) fn parse_range(spec: &str) -> Option<Ranges> {
    if spec.is_empty() {
        return None;
    }
    let mut ranges: Ranges = Vec::new();
    for alternative in spec.split("||") {
        let tokens: Vec<&str> = snp_pycompat::text::split_whitespace(alternative, None).collect();
        if tokens.is_empty() {
            return None;
        }
        let groups = if tokens.contains(&"-") {
            let (Some(lo), Some(&"-"), Some(hi), 3) =
                (tokens.first(), tokens.get(1), tokens.get(2), tokens.len())
            else {
                return None;
            };
            vec![
                parse_comparator(&format!(">={lo}")),
                parse_comparator(&format!("<={hi}")),
            ]
        } else {
            tokens.iter().map(|t| parse_comparator(t)).collect()
        };
        let mut all = Vec::new();
        for g in groups {
            all.extend(g?);
        }
        ranges.push(all);
    }
    Some(ranges)
}

/// `match_node_range(version, ranges)`.
pub(crate) fn matches(version: Triple, ranges: &Ranges) -> bool {
    ranges.iter().any(|alt| {
        alt.iter().all(|(op, other)| match op {
            Op::Ge => version >= *other,
            Op::Gt => version > *other,
            Op::Le => version <= *other,
            Op::Lt => version < *other,
        })
    })
}

/// nodeenv's `parse_version` + `_pad_version` of an `index.json` version (`v20.11.1` → `[20, 11, 1]`); `None` when a
/// part is not a number (nodeenv would crash with `ValueError`).
pub(crate) fn index_version(v: &str) -> Option<Triple> {
    let v = v.replace('v', "");
    let mut out = [0_u64; 3];
    for (i, p) in v.split('.').take(3).enumerate() {
        let p = if i == 2 {
            p.split('+').next().unwrap_or(p)
        } else {
            p
        };
        let (n, rest) = digits(p)?;
        if !rest.is_empty() {
            return None;
        }
        if let Some(slot) = out.get_mut(i) {
            *slot = n;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_versions() {
        for v in ["20.11.1", "v20.11.1", "20.11.1+build", "1.2.3\n"] {
            assert!(is_exact(v), "{v}");
        }
        for v in [
            "20",
            "20.11",
            "^20.1.0",
            "20.11.1-rc",
            "20.11.1+a b",
            "lts",
            "",
        ] {
            assert!(!is_exact(v), "{v}");
        }
    }

    /// The values printed by nodeenv 1.11.0's `parse_node_range` (oracle venv, CPython 3.12).
    #[test]
    fn ranges_like_nodeenv() {
        let r = |s: &str| parse_range(s);
        assert_eq!(
            r("20"),
            Some(vec![vec![(Op::Ge, [20, 0, 0]), (Op::Lt, [21, 0, 0])]])
        );
        assert_eq!(
            r("18.19"),
            Some(vec![vec![(Op::Ge, [18, 19, 0]), (Op::Lt, [18, 20, 0])]])
        );
        assert_eq!(
            r("^20.1"),
            Some(vec![vec![(Op::Ge, [20, 1, 0]), (Op::Lt, [21, 0, 0])]])
        );
        assert_eq!(
            r("^0.1.2"),
            Some(vec![vec![(Op::Ge, [0, 1, 2]), (Op::Lt, [0, 2, 0])]])
        );
        assert_eq!(
            r("^0.0.2"),
            Some(vec![vec![(Op::Ge, [0, 0, 2]), (Op::Lt, [0, 0, 3])]])
        );
        assert_eq!(
            r("20.x"),
            Some(vec![vec![(Op::Ge, [20, 0, 0]), (Op::Lt, [21, 0, 0])]])
        );
        assert_eq!(r("*"), Some(vec![vec![]]));
        assert_eq!(
            r(">=18 <20"),
            Some(vec![vec![(Op::Ge, [18, 0, 0]), (Op::Lt, [20, 0, 0])]])
        );
        assert_eq!(r(">4.3"), Some(vec![vec![(Op::Ge, [4, 4, 0])]]));
        assert_eq!(r("<=4.3"), Some(vec![vec![(Op::Lt, [4, 4, 0])]]));
        assert_eq!(
            r("4.3.1 - 6.2.0"),
            Some(vec![vec![(Op::Ge, [4, 3, 1]), (Op::Le, [6, 2, 0])]])
        );
        assert_eq!(
            r("16 || >=20"),
            Some(vec![
                vec![(Op::Ge, [16, 0, 0]), (Op::Lt, [17, 0, 0])],
                vec![(Op::Ge, [20, 0, 0])]
            ])
        );
        assert_eq!(r("v20"), r("20"));
        assert_eq!(
            r("=v1.2"),
            Some(vec![vec![(Op::Ge, [1, 2, 0]), (Op::Lt, [1, 3, 0])]])
        );
        assert_eq!(
            r("~1"),
            Some(vec![vec![(Op::Ge, [1, 0, 0]), (Op::Lt, [2, 0, 0])]])
        );
        assert_eq!(
            r("~1.2.3"),
            Some(vec![vec![(Op::Ge, [1, 2, 3]), (Op::Lt, [1, 3, 0])]])
        );
        assert_eq!(r("x.1.2"), Some(vec![vec![]]));
        assert_eq!(r(">=1.x"), Some(vec![vec![(Op::Ge, [1, 0, 0])]]));
        for bad in [
            "",
            "lts/iron",
            "20 -",
            "a - b",
            "1 || ",
            "20.1.2.3",
            "node",
            "16 || >= 20",
            "1.2.",
        ] {
            assert_eq!(r(bad), None, "{bad}");
        }
    }

    #[test]
    fn matching_and_index_versions() {
        let r = parse_range(">=18 <20").unwrap();
        assert!(matches([18, 19, 1], &r));
        assert!(!matches([20, 0, 0], &r));
        assert_eq!(index_version("v20.11.1"), Some([20, 11, 1]));
        assert_eq!(index_version("v20.11"), Some([20, 11, 0]));
        assert_eq!(index_version("v20.11.1+x"), Some([20, 11, 1]));
        assert_eq!(index_version("v20.11.1-rc.1"), None);
    }

    /// Differential against nodeenv 1.11.0 itself (the oracle venv's `nodeenv` module, `$SNP_ORACLE_PY312`):
    /// random specs built from the atoms the grammar cares about, compared on `_is_exact_version` and
    /// `parse_node_range`.
    #[test]
    #[ignore = "oracle"]
    fn oracle_ranges_match_nodeenv() {
        const ATOMS: [&str; 22] = [
            "1", "20", "0", ".", "x", "*", "X", "v", "^", "~", ">", ">=", "<", "<=", "=", " ",
            "  ", "-", " - ", "||", "+b", "\n",
        ];
        #[expect(
            clippy::disallowed_methods,
            reason = "test switch selecting the oracle venv"
        )]
        let root = std::env::var_os("SNP_ORACLE_PY312").map_or_else(
            || std::path::PathBuf::from("../../target/oracle/pc462-py312"),
            Into::into,
        );
        let python = root.join("bin/python");
        assert!(python.exists(), "set SNP_ORACLE_PY312");
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let specs: Vec<String> = (0..5000)
            .map(|_| {
                (0..=(next() % 8))
                    .map(|_| ATOMS[usize::try_from(next() % 22).unwrap_or(0)])
                    .collect()
            })
            .collect();
        let script = "import json, sys, nodeenv\n\
            out = []\n\
            for s in json.load(sys.stdin):\n\
            \x20   r = nodeenv.parse_node_range(s)\n\
            \x20   out.append([nodeenv._is_exact_version(s), r and [[[o, list(v)] for o, v in a] for a in r]])\n\
            print(json.dumps(out))\n";
        let mut child = std::process::Command::new(&python)
            .args(["-I", "-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            &mut child.stdin.take().unwrap(),
            serde_json::to_string(&specs).unwrap().as_bytes(),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let want: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
        let op = |o: Op| match o {
            Op::Ge => ">=",
            Op::Gt => ">",
            Op::Le => "<=",
            Op::Lt => "<",
        };
        for (spec, want) in specs.iter().zip(want) {
            let ranges = parse_range(spec).map(|r| {
                r.iter()
                    .map(|a| {
                        a.iter()
                            .map(|(o, v)| serde_json::json!([op(*o), v]))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            });
            let got = serde_json::json!([is_exact(spec), ranges]);
            assert_eq!(got, want, "spec {spec:?}");
        }
    }
}
