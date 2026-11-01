//! The NATIVE-WITH-CAVEATS hooks against the Python hooks (WP-27 acceptance): the per-epoch goldens of
//! `cases.json`, the Appendix A argparse goldens, and the corpus comparison behind the parity gates.
//!
//! The corpus (`corpus/goldens.json`, recorded by `cargo xtask pch-gates corpus` under the v6.0.0 venv) runs one
//! file per process: the yaml-test-suite, toml-test, xmlconf and `configparser` inputs of snp-pyparsers' committed
//! vectors (`crates/snp-pyparsers/tests/vectors/`, check-yaml in its three modes) plus the real repository files
//! of `corpus/real/` (copied from the pre-commit, lefthook, prek, identify, cfgv and pre-commit-hooks sources, all
//! MIT-licensed, and from snp itself). `caveat_corpus_gates` compares every run natively: an *acceptance*
//! difference is a different exit status or crash, a *message* difference the same verdict with other output
//! (detect-aws-credentials' findings are compared as a set: Python iterates a `set`). The committed
//! `snp_hooks_data::parity_gate` of every caveat hook must be the gate the comparison implies; with
//! `SNP_PCH_GATES_REPORT` set the test writes its report there instead (`cargo xtask pch-gates table`).
//!
//! Owner: WP-27.
#![cfg(test)]

#[path = "../framework/support.rs"]
mod support;

#[path = "../p1/golden.rs"]
mod golden;

use std::collections::BTreeMap;
use std::ffi::OsStr;

use snp_hooks::{Epoch, NativeSource};
use snp_hooks_data::Gate;
use support::{Ctx, Sandbox, latin1, show};

/// The caveat hooks and their console scripts.
const CAVEATS: [(&str, &str); 4] = [
    ("check-yaml", "check-yaml"),
    ("check-toml", "check-toml"),
    ("check-xml", "check-xml"),
    ("detect-aws-credentials", "detect-aws-credentials"),
];

#[cfg(unix)]
#[test]
fn caveat_goldens_per_epoch() {
    let checked = golden::replay("caveats");
    eprintln!("caveats: {checked} golden cases");
    assert!(checked > 70, "only {checked} golden cases ran");
}

#[cfg(unix)]
#[test]
fn caveat_appendix_a_goldens() {
    let checked = golden::appendix_a(&CAVEATS);
    eprintln!("caveats: {checked} Appendix A texts");
    assert!(checked >= 100, "only {checked} Appendix A texts compared");
}

#[test]
fn caveat_registry_and_preflight() {
    golden::registry(&CAVEATS);
    // check-toml before v4.3.0 used the `toml` PyPI package: delegated.
    assert!(golden::native_epoch("v4.2.0", "check-toml", &[]).is_none());
    assert!(golden::native_epoch("v4.3.0", "check-toml", &[]).is_some());
}

/// Standard base64 (the vectors' `b64` fields).
fn b64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let mut acc = 0_u32;
    let mut bits = 0;
    for c in s.bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).unwrap());
        }
    }
    out
}

/// The bytes of a corpus entry's source.
fn source_bytes(
    source: &serde_json::Value,
    vectors: &mut BTreeMap<String, serde_json::Value>,
) -> Vec<u8> {
    if let Some(real) = source.get("real").and_then(|r| r.as_str()) {
        let p = support::crate_dir()
            .join("tests/caveats/corpus/real")
            .join(real);
        return std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    }
    let file = source["vector"].as_str().unwrap();
    let v = vectors.entry(file.to_owned()).or_insert_with(|| {
        let p = support::crate_dir()
            .join("../snp-pyparsers/tests/vectors")
            .join(file);
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
    });
    let case = &v["cases"][usize::try_from(source["index"].as_u64().unwrap()).unwrap()];
    assert_eq!(
        case["name"], source["name"],
        "stale corpus goldens: rerun `cargo xtask pch-gates corpus`"
    );
    if let Some(b) = case.get("b64").and_then(|b| b.as_str()) {
        b64(b)
    } else {
        case["text"].as_str().unwrap().as_bytes().to_vec()
    }
}

/// Lines of `out`, sorted.
fn sorted_lines(out: &[u8]) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = out.split(|&b| b == b'\n').collect();
    v.sort_unstable();
    v
}

/// The per-hook comparison counts.
#[derive(Default, Debug)]
struct Report {
    cases: u64,
    acceptance: u64,
    message: u64,
    /// Message differences that are only a different line order.
    order_only: u64,
    examples: Vec<String>,
}

/// The gate a report implies (the `xtask pch-gates table` rule).
fn gate_of(r: &Report) -> Gate {
    if r.cases == 0 || r.acceptance > 0 {
        Gate::Delegate
    } else if r.message > 0 {
        Gate::Caveat
    } else {
        Gate::Exact
    }
}

/// Runs the whole corpus natively and compares it with the Python results; checks (or, with
/// `SNP_PCH_GATES_REPORT`, reports) the parity gates.
#[cfg(unix)]
#[test]
fn caveat_corpus_gates() {
    let goldens = support::json("caveats/corpus/goldens.json");
    let mut vectors = BTreeMap::new();
    let ctx = Ctx::new(Sandbox::new(true));
    let mut reports: BTreeMap<String, Report> = BTreeMap::new();
    for (id, _) in CAVEATS {
        reports.insert(id.to_owned(), Report::default());
    }
    for e in goldens["entries"].as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        let hook_id = e["hook"].as_str().unwrap();
        let name = e["name"].as_str().unwrap();
        let want = &goldens["results"][id];
        let data = source_bytes(&e["source"], &mut vectors);
        std::fs::write(ctx.sb.root.join(name), &data).unwrap();
        let mut argv: Vec<&OsStr> = e["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| OsStr::new(a.as_str().unwrap()))
            .collect();
        argv.push(OsStr::new(name));
        let epoch = golden::native_epoch("v6.0.0", hook_id, &[]).unwrap();
        let hook = snp_hooks::native(NativeSource::PreCommitHooks, hook_id).unwrap();
        let got = hook.run(&ctx.cx(epoch), &argv);
        let r = reports.get_mut(hook_id).unwrap();
        r.cases += 1;
        let want_out = latin1(want["out"].as_str().unwrap());
        let same_verdict = i64::from(got.code) == want["code"].as_i64().unwrap()
            && got.crashed == want["crashed"].as_bool().unwrap();
        // detect-aws-credentials reports its findings in a deterministic order (sorted), where Python iterates a
        // set (an order that varies with the hash seed): an order-only difference is a message difference, not
        // an acceptance one, and is counted as such (`order_only` in the report).
        let same_text = got.output == want_out;
        if !same_text && sorted_lines(&got.output) == sorted_lines(&want_out) {
            r.order_only += 1;
        }
        if !same_verdict {
            r.acceptance += 1;
        } else if !same_text {
            r.message += 1;
        }
        if (!same_verdict || !same_text) && r.examples.len() < 8 {
            r.examples.push(format!(
                "{id}: code {} crashed {} (want {} {})\n--- got\n{}\n--- want\n{}",
                got.code,
                got.crashed,
                want["code"],
                want["crashed"],
                show(&got.output),
                show(&want_out)
            ));
        }
    }
    let summary: Vec<String> = reports
        .iter()
        .map(|(id, r)| {
            format!(
                "{id}: {} runs, {} acceptance, {} message ({} order only)",
                r.cases, r.acceptance, r.message, r.order_only
            )
        })
        .collect();
    eprintln!("{}", summary.join("\n"));
    let process = snp_core::env::Env::capture_process();
    if let Some(path) = process.get("SNP_PCH_GATES_REPORT") {
        let json: serde_json::Map<String, serde_json::Value> = reports
            .iter()
            .map(|(id, r)| {
                (
                    id.clone(),
                    serde_json::json!({
                        "cases": r.cases,
                        "acceptance": r.acceptance,
                        "message": r.message,
                        "order_only": r.order_only,
                        "examples": r.examples,
                    }),
                )
            })
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
        return;
    }
    let mut failures = Vec::new();
    for (id, r) in &reports {
        let want = gate_of(r);
        let have = snp_hooks_data::parity_gate(id);
        if want != have {
            failures.push(format!(
                "{id}: the committed gate is {have:?} but the corpus implies {want:?} (rerun `cargo xtask pch-gates \
                 table`)\n{}",
                r.examples.join("\n")
            ));
        }
        // A gate that lets the native run in `auto` never hides an acceptance difference.
        assert!(
            r.acceptance == 0 || have == Gate::Delegate,
            "{id}: {} acceptance differences:\n{}",
            r.acceptance,
            r.examples.join("\n")
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    let total: u64 = reports.values().map(|r| r.cases).sum();
    assert!(total > 5000, "only {total} corpus runs");
}

/// A partition of check-yaml shares one ruamel `YAML` object: a `%YAML 1.1` directive leaks into the next file.
#[test]
fn check_yaml_state_carries_over() {
    let sb = Sandbox::new(true);
    sb.write("a.yaml", b"%YAML 1.1\n---\na: 1\n");
    sb.write("b.yaml", b"yes: 1\ntrue: 2\n");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "check-yaml").unwrap();
    let alone = h.run(&ctx.cx(Epoch(2)), &[OsStr::new("b.yaml")]);
    assert_eq!(alone.code, 0);
    let both = h.run(
        &ctx.cx(Epoch(2)),
        &[OsStr::new("a.yaml"), OsStr::new("b.yaml")],
    );
    assert_eq!(both.code, 1, "{}", show(&both.output));
}

/// ruamel's warnings reach the output unless `SNP_PYTHON_WARNINGS=0`.
#[test]
fn check_yaml_warnings() {
    let sb = Sandbox::new(true);
    sb.write("a.yaml", b"a: &x 1\nb: &x 2\n");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "check-yaml").unwrap();
    let o = h.run(&ctx.cx(Epoch(2)), &[OsStr::new("a.yaml")]);
    assert_eq!(o.code, 0);
    assert!(
        show(&o.output).starts_with("site-packages/ruamel/yaml/composer.py:"),
        "{}",
        show(&o.output)
    );
    let quiet = ctx
        .sb
        .env
        .patched(&snp_core::env::EnvPatch::default().set("SNP_PYTHON_WARNINGS", "0"));
    let cx = snp_hooks::HookCx {
        env: &quiet,
        ..ctx.cx(Epoch(2))
    };
    let o = h.run(&cx, &[OsStr::new("a.yaml")]);
    assert_eq!((o.code, o.output.as_slice()), (0, &b""[..]));
}
