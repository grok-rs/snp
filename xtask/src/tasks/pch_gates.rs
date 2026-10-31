//! `cargo xtask pch-gates`: the WP-27 goldens and the CI parity gates of the NATIVE-WITH-CAVEATS hooks (09 §9.9).
//!
//! Everything Python-side runs the pinned upstream console scripts of the per-tag venvs `cargo xtask pch-goldens
//! venvs` builds (default `target/pch-oracle`, CPython 3.13, `ruamel.yaml==0.19.1`), through an embedded driver
//! ([`DRIVER`], run with `python -I`) — never by hand and never in CI:
//!
//! * `goldens [--dir DIR]` runs `crates/snp-hooks/tests/{p1,p2,caveats}/cases.json` under the venvs of each file's
//!   golden tags and writes `tests/<group>/goldens/<tag>.json` ((argv, files before) → (exit, output, files after)),
//!   which `tests/{p1,p2,caveats}` replay natively.
//! * `corpus [--dir DIR] [--jobs N]` runs the caveat corpus — the yaml-test-suite, toml-test, xmlconf and
//!   `configparser` inputs of snp-pyparsers' committed vectors (`crates/snp-pyparsers/tests/vectors/`) plus the real
//!   repository files under `crates/snp-hooks/tests/caveats/corpus/real/` — one file per process under the v6.0.0
//!   venv (check-yaml in its three modes) and writes `tests/caveats/corpus/goldens.json`.
//! * `table` runs `tests/caveats`' `caveat_corpus_gates` (native vs the corpus goldens), which reports acceptance
//!   differences (exit status, crash) and message differences per caveat hook, and regenerates
//!   `crates/snp-hooks-data/src/generated/parity_gates.rs`: `Exact` for every NATIVE id of the registry (their
//!   goldens are exact by construction), and for a caveat hook `Exact` with no difference, `Caveat` with message
//!   differences only, `Delegate` with any acceptance difference. Sorted, deterministic: the same goldens give the
//!   same file.
//! * no subcommand (or `all`): `goldens`, `corpus`, `table`.
//!
//! Every run happens in a fresh directory with the hermetic environment the replaying tests build (fixed git
//! identity and dates, `master`, `PATH=/usr/bin:/bin`, `LC_ALL=C.UTF-8`, `PYTHONHASHSEED=0`); a traceback is
//! reduced to its last line (what the natives print, 09 §2.6); the work tree, `$HOME` and the venv's
//! `site-packages` are written as `<root>`, `<home>` and `site-packages`.
//!
//! Owner: WP-27.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};

use crate::workspace;

/// The snp-hooks test directory.
const HOOKS: &str = "crates/snp-hooks/tests";

/// The case groups `goldens` runs.
const GROUPS: [&str; 3] = ["p1", "p2", "caveats"];

/// The vectors the caveat corpus reuses (file, hook, file name in the sandbox, extra argv per run).
const VECTORS: [(&str, &str, &str); 4] = [
    ("yaml_313.json", "check-yaml", "f.yaml"),
    ("toml_313.json", "check-toml", "f.toml"),
    ("xml.json", "check-xml", "f.xml"),
    ("ini.json", "detect-aws-credentials", "creds.ini"),
];

/// check-yaml's three loader modes.
const YAML_MODES: [(&str, &[&str]); 3] =
    [("safe", &[]), ("multi", &["-m"]), ("unsafe", &["--unsafe"])];

/// The generated table.
const TABLE: &str = "crates/snp-hooks-data/src/generated/parity_gates.rs";

/// The Python driver (written to `<dir>/pch_gates_driver.py` and run with `-I`).
const DRIVER: &str = r#"
"""Golden generator of `cargo xtask pch-gates` (WP-27); embedded in pch_gates.rs and written to a temp file.

    cases  --cases FILE --tag T --out FILE   tests/{p1,p2,caveats}/cases.json under this venv's console scripts
    corpus --corpus FILE --out FILE --jobs N one corpus file per process (caveat parity gates)

Byte strings are latin-1 JSON strings. Every run happens in a fresh directory with a hermetic environment (the
same one tests/framework/support.rs builds); a traceback is reduced to its last line (`crashed: true`).
"""
from __future__ import annotations

import argparse
import base64
import concurrent.futures
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile

DATE = 1577836800
SITE = re.compile(rb'/[^\s:\'"]*/lib/python3\.[0-9]+/site-packages')


def b(s: str) -> bytes:
    return s.encode('latin-1')


def s(x: bytes) -> str:
    return x.decode('latin-1')


def hermetic_env(home: str) -> dict[str, str]:
    os.makedirs(home, exist_ok=True)
    gitconfig = os.path.join(home, '.gitconfig')
    with open(gitconfig, 'w') as f:
        f.write(
            '[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n'
            '[commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n'
            '[advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n',
        )
    template = os.path.join(home, 'git-template')
    os.makedirs(template, exist_ok=True)
    return {
        'PATH': '/usr/bin:/bin',
        'HOME': home,
        'USERPROFILE': home,
        'GIT_CONFIG_GLOBAL': gitconfig,
        'GIT_CONFIG_NOSYSTEM': '1',
        'GIT_TEMPLATE_DIR': template,
        'GIT_AUTHOR_NAME': 'test',
        'GIT_COMMITTER_NAME': 'test',
        'GIT_AUTHOR_EMAIL': 'test@example.com',
        'GIT_COMMITTER_EMAIL': 'test@example.com',
        'GIT_AUTHOR_DATE': f'{DATE} +0000',
        'GIT_COMMITTER_DATE': f'{DATE} +0000',
        'LC_ALL': 'C.UTF-8',
        'LANG': 'C.UTF-8',
        'TZ': 'UTC',
        'PYTHONHASHSEED': '0',
        'PRE_COMMIT': '1',
    }


def strip_traceback(out: bytes) -> tuple[bytes, bool]:
    """The output before the (first) traceback plus the final exception's text, which may span lines."""
    marker = b'Traceback (most recent call last):\n'
    i = out.find(marker)
    if i < 0:
        return out, False
    lines = out[out.rfind(marker) + len(marker):].splitlines(keepends=True)
    k = 0
    while k < len(lines) and lines[k].startswith(b' '):
        k += 1
    return out[:i] + b''.join(lines[k:]), True


def normalise(out: bytes, root: str, home: str) -> bytes:
    out = SITE.sub(b'site-packages', out)
    out = out.replace(os.fsencode(home), b'<home>')
    return out.replace(os.fsencode(root), b'<root>')


def run(argv: list[str], cwd: str, env: dict[str, str]) -> tuple[int, bytes]:
    p = subprocess.run(argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                       stderr=subprocess.STDOUT)
    return p.returncode, p.stdout


def apply_steps(root: str, home: str, steps: list[list[str]], env: dict[str, str]) -> list[str]:
    written = []
    for step in steps:
        op, *rest = step
        if op in ('w', 'home'):
            base = root if op == 'w' else home
            path = os.path.join(base, os.fsdecode(b(rest[0])))
            os.makedirs(os.path.dirname(path) or base, exist_ok=True)
            with open(path, 'wb') as f:
                f.write(b(rest[1]))
            if op == 'w' and rest[0] not in written:
                written.append(rest[0])
        elif op == 'x':
            os.chmod(os.path.join(root, os.fsdecode(b(rest[0]))), 0o755)
        elif op == 'ro':
            os.chmod(os.path.join(root, os.fsdecode(b(rest[0]))), 0o444)
        elif op == 'mkdir':
            os.makedirs(os.path.join(root, os.fsdecode(b(rest[0]))), exist_ok=True)
        elif op == 'rm':
            os.unlink(os.path.join(root, os.fsdecode(b(rest[0]))))
        elif op == 'git':
            subprocess.run(['git', *[os.fsdecode(b(a)) for a in rest]], cwd=root, env=env, check=True,
                           stdout=subprocess.DEVNULL)
        elif op == 'gitfile':
            path = os.path.join(root, '.git', rest[0])
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, 'wb') as f:
                f.write(b(rest[1]))
        elif op == 'ln':
            os.symlink(rest[0], os.path.join(root, rest[1]))
        else:
            raise SystemExit(f'unknown step {op!r}')
    return written


def snapshot(root: str, paths: list[str]) -> dict[str, str | None]:
    out: dict[str, str | None] = {}
    for p in paths:
        full = os.path.join(root, os.fsdecode(b(p)))
        try:
            with open(full, 'rb') as f:
                out[p] = s(f.read())
        except OSError:
            out[p] = None
    return out


def cleanup(root: str) -> None:
    def onexc(func, path, exc):  # read-only files and directories
        os.chmod(path, stat.S_IRWXU)
        func(path)
    shutil.rmtree(root, onexc=onexc)


def mode_cases(cases_file: str, tag: str, out_file: str) -> None:
    from importlib.metadata import distribution
    with open(cases_file) as f:
        cases = json.load(f)
    dist = distribution('pre-commit-hooks')
    scripts = {ep.name for ep in dist.entry_points if ep.group == 'console_scripts'}
    bindir = os.path.dirname(sys.executable)
    results = {}
    for case in cases['cases']:
        if case.get('tags') and tag not in case['tags']:
            continue
        if case['script'] not in scripts:
            continue
        tmp = tempfile.mkdtemp()
        try:
            root = os.path.join(tmp, 'repo')
            home = os.path.join(tmp, 'home')
            os.makedirs(root)
            env = hermetic_env(home)
            subprocess.run(['git', 'init', '-q', '.'], cwd=root, env=env, check=True)
            written = apply_steps(root, home, case['steps'], env)
            big = case.get('big_then')
            if big:
                with open(os.path.join(root, big['path']), 'wb') as f:
                    f.write(b'a' * big['fill'] + b(big['tail']))
            env.update(case.get('env', {}))
            exe = os.path.join(bindir, case['script'])
            if 'partition' in case:
                from pre_commit import lang_base
                from pre_commit import xargs
                part = case['partition']
                shuffled = lang_base._shuffled(part['files'])
                code, out, crashed = 0, b'', False
                for files in xargs.partition((), shuffled, part['jobs'], 131072):
                    c, o = run([exe, *case['argv'], *files], root, env)
                    o, cr = strip_traceback(o)
                    crashed = crashed or cr
                    if abs(c) > abs(code):
                        code = c
                    out += o
            else:
                code, out = run([exe, *case['argv']], root, env)
                out, crashed = strip_traceback(out)
            results[case['name']] = {
                'code': code,
                'out': s(normalise(out, root, home)),
                'crashed': crashed,
                'files': snapshot(root, case.get('files', written)),
            }
        finally:
            cleanup(tmp)
    result = {'tag': tag, 'version': dist.version, 'python': sys.version.split()[0], 'results': results}
    with open(out_file, 'w') as f:
        json.dump(result, f, indent=1, sort_keys=True, ensure_ascii=True)
        f.write('\n')


def corpus_one(entry: dict) -> tuple[str, dict]:
    tmp = tempfile.mkdtemp()
    try:
        root = os.path.join(tmp, 'w')
        home = os.path.join(tmp, 'home')
        os.makedirs(root)
        env = hermetic_env(home)
        if 'b64' in entry:
            data = base64.b64decode(entry['b64'])
        elif 'text' in entry:
            data = entry['text'].encode('utf-8', 'surrogatepass')
        else:
            with open(entry['path'], 'rb') as f:
                data = f.read()
        with open(os.path.join(root, entry['name']), 'wb') as f:
            f.write(data)
        exe = os.path.join(os.path.dirname(sys.executable), entry['script'])
        code, out = run([exe, *entry['args'], entry['name']], root, env)
        out, crashed = strip_traceback(out)
        return entry['id'], {'code': code, 'out': s(normalise(out, root, home)), 'crashed': crashed}
    finally:
        cleanup(tmp)


def mode_corpus(corpus_file: str, out_file: str, jobs: int) -> None:
    with open(corpus_file) as f:
        corpus = json.load(f)
    results = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as ex:
        for key, res in ex.map(corpus_one, corpus['entries']):
            results[key] = res
    entries = [{k: v for k, v in e.items() if k not in ('b64', 'text', 'path')} for e in corpus['entries']]
    result = {'python': sys.version.split()[0], 'entries': entries, 'results': results}
    with open(out_file, 'w') as f:
        json.dump(result, f, indent=0, sort_keys=True, ensure_ascii=True)
        f.write('\n')


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=('cases', 'corpus'))
    parser.add_argument('--cases')
    parser.add_argument('--corpus')
    parser.add_argument('--tag')
    parser.add_argument('--jobs', type=int, default=6)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    if args.mode == 'cases':
        mode_cases(args.cases, args.tag, args.out)
    else:
        mode_corpus(args.corpus, args.out, args.jobs)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
"#;

/// Parsed options.
#[derive(Debug)]
struct Opts {
    /// The venvs.
    dir: PathBuf,
    /// Scratch files (driver, corpus manifest, report).
    work: PathBuf,
    jobs: usize,
}

/// The `python` of a venv.
fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// Runs `cmd`, failing with its output on a non-zero exit.
fn run_ok(cmd: &mut Command, what: &str) -> anyhow::Result<()> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    ensure!(
        out.status.success(),
        "{what} failed ({}):\n{}{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

/// Writes the driver next to the venvs.
fn driver(o: &Opts) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(&o.work)?;
    let path = o.work.join("pch_gates_driver.py");
    std::fs::write(&path, DRIVER.trim_start())?;
    Ok(path)
}

/// Runs the driver with `python -I` of the venv of `tag`.
fn drive(o: &Opts, tag: &str, args: &[&OsStr], what: &str) -> anyhow::Result<()> {
    let python = venv_python(&o.dir.join(tag));
    ensure!(
        python.exists(),
        "{} is missing: run `cargo xtask pch-goldens venvs` first",
        python.display()
    );
    let drv = driver(o)?;
    run_ok(
        Command::new(python)
            .arg("-I")
            .arg(drv)
            .args(args)
            .env("PYTHONHASHSEED", "0"),
        what,
    )
}

/// The golden tags of a `cases.json`.
fn golden_tags(cases: &Path) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cases)?)?;
    let tags = v
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .with_context(|| format!("{}: no `tags`", cases.display()))?;
    Ok(tags
        .iter()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect())
}

/// `goldens`: per-epoch goldens of every group.
fn goldens(root: &Path, o: &Opts) -> anyhow::Result<()> {
    for group in GROUPS {
        let cases = root.join(HOOKS).join(group).join("cases.json");
        let out_dir = root.join(HOOKS).join(group).join("goldens");
        std::fs::create_dir_all(&out_dir)?;
        for tag in golden_tags(&cases)? {
            let out = out_dir.join(format!("{tag}.json"));
            drive(
                o,
                &tag,
                &[
                    "cases".as_ref(),
                    "--cases".as_ref(),
                    cases.as_os_str(),
                    "--tag".as_ref(),
                    tag.as_ref(),
                    "--out".as_ref(),
                    out.as_os_str(),
                ],
                &format!("{group} goldens {tag}"),
            )?;
        }
        workspace::say(&format!(
            "pch-gates: {group} goldens in {}",
            workspace::rel(root, &out_dir)
        ));
    }
    Ok(())
}

/// One corpus run.
fn entry(
    id: String,
    script: &str,
    args: &[&str],
    name: &str,
    source: serde_json::Value,
    data: (&str, serde_json::Value),
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("id".to_owned(), serde_json::Value::String(id));
    m.insert("script".to_owned(), script.into());
    m.insert("hook".to_owned(), script.into());
    m.insert("args".to_owned(), args.into());
    m.insert("name".to_owned(), name.into());
    m.insert("source".to_owned(), source);
    m.insert(data.0.to_owned(), data.1);
    serde_json::Value::Object(m)
}

/// The runs of one input for `hook` (check-yaml: once per loader mode).
fn runs(hook: &str) -> Vec<(String, Vec<&'static str>)> {
    if hook == "check-yaml" {
        YAML_MODES
            .iter()
            .map(|(m, a)| ((*m).to_owned(), a.to_vec()))
            .collect()
    } else if hook == "detect-aws-credentials" {
        vec![("scan".to_owned(), vec!["--credentials-file", "creds.ini"])]
    } else {
        vec![("check".to_owned(), Vec::new())]
    }
}

/// The real repository files of the corpus: `corpus/real/<kind>/<file>` (kind = yaml, toml, xml, ini).
fn real_files(root: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
    let dir = root.join(HOOKS).join("caveats/corpus/real");
    let mut out = Vec::new();
    for kind in ["yaml", "toml", "xml", "ini"] {
        let d = dir.join(kind);
        if !d.is_dir() {
            continue;
        }
        let mut names: Vec<PathBuf> = std::fs::read_dir(&d)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        names.sort();
        for p in names {
            out.push((kind.to_owned(), p));
        }
    }
    Ok(out)
}

/// The corpus entries (deterministic order).
fn corpus_entries(root: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let vectors = root.join("crates/snp-pyparsers/tests/vectors");
    let mut entries = Vec::new();
    for (file, hook, name) in VECTORS {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(vectors.join(file))?)
                .with_context(|| format!("parsing {file}"))?;
        let cases = v
            .get("cases")
            .and_then(serde_json::Value::as_array)
            .with_context(|| format!("{file}: no `cases`"))?;
        for (i, c) in cases.iter().enumerate() {
            let cname = c
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let data = if let Some(b64) = c.get("b64") {
                ("b64", b64.clone())
            } else if let Some(t) = c.get("text") {
                ("text", t.clone())
            } else {
                continue;
            };
            for (mode, args) in runs(hook) {
                entries.push(entry(
                    format!("{hook}:{file}:{i}:{mode}"),
                    hook,
                    &args,
                    name,
                    serde_json::json!({"vector": file, "index": i, "name": cname}),
                    data.clone(),
                ));
            }
        }
    }
    for (kind, path) in real_files(root)? {
        let (hook, name) = match kind.as_str() {
            "yaml" => ("check-yaml", "f.yaml"),
            "toml" => ("check-toml", "f.toml"),
            "xml" => ("check-xml", "f.xml"),
            _ => ("detect-aws-credentials", "creds.ini"),
        };
        let rel = workspace::rel(&root.join(HOOKS).join("caveats/corpus/real"), &path);
        for (mode, args) in runs(hook) {
            entries.push(entry(
                format!("{hook}:real/{rel}:{mode}"),
                hook,
                &args,
                name,
                serde_json::json!({"real": rel}),
                (
                    "path",
                    serde_json::Value::String(path.display().to_string()),
                ),
            ));
        }
    }
    Ok(entries)
}

/// `corpus`: the caveat corpus goldens under the v6.0.0 venv.
fn corpus(root: &Path, o: &Opts) -> anyhow::Result<()> {
    let entries = corpus_entries(root)?;
    let n = entries.len();
    std::fs::create_dir_all(&o.work)?;
    let manifest = o.work.join("pch_gates_corpus.json");
    std::fs::write(
        &manifest,
        serde_json::to_string(&serde_json::json!({ "entries": entries }))?,
    )?;
    let out = root.join(HOOKS).join("caveats/corpus/goldens.json");
    drive(
        o,
        "v6.0.0",
        &[
            "corpus".as_ref(),
            "--corpus".as_ref(),
            manifest.as_os_str(),
            "--out".as_ref(),
            out.as_os_str(),
            "--jobs".as_ref(),
            o.jobs.to_string().as_ref(),
        ],
        "caveat corpus",
    )?;
    workspace::say(&format!(
        "pch-gates: {n} corpus runs in {}",
        workspace::rel(root, &out)
    ));
    Ok(())
}

/// The gate of a caveat hook from its corpus report.
fn caveat_gate(report: &serde_json::Value) -> &'static str {
    let count = |k: &str| {
        report
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(u64::MAX)
    };
    if report
        .get("cases")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        == 0
        || count("acceptance") != 0
    {
        "Delegate"
    } else if count("message") != 0 {
        "Caveat"
    } else {
        "Exact"
    }
}

/// The ids of a registry group.
fn ids(group: &'static [&'static dyn snp_hooks::NativeHook]) -> impl Iterator<Item = &'static str> {
    group.iter().map(|h| h.id())
}

/// Renders `parity_gates.rs` from the caveat reports.
fn render_table(reports: &serde_json::Value) -> anyhow::Result<String> {
    let mut gates: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    for id in ids(snp_hooks::pch::p0::HOOKS)
        .chain(ids(snp_hooks::pch::p1::HOOKS))
        .chain(ids(snp_hooks::pch::p2::HOOKS))
        .chain(ids(snp_hooks::pch::stubs::HOOKS))
    {
        gates.insert(id, ("Exact", "NATIVE".to_owned()));
    }
    for id in ids(snp_hooks::pch::caveats::HOOKS) {
        let r = reports
            .get(id)
            .with_context(|| format!("the corpus report has no `{id}`"))?;
        let num = |k: &str| r.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        gates.insert(
            id,
            (
                caveat_gate(r),
                format!(
                    "{} corpus runs, {} acceptance and {} message differences",
                    num("cases"),
                    num("acceptance"),
                    num("message")
                ),
            ),
        );
    }
    let mut s = String::new();
    s.push_str(
        "// @generated by `cargo xtask pch-gates table` (WP-27) from the caveat corpus comparison of \
         crates/snp-hooks/tests/caveats.\n\
         //! CI parity-gate table (09 §9.9): (hook id, gate), sorted by id for binary search. `Exact` for the NATIVE\n\
         //! ids of the snp-hooks registry (removed stubs included); for a NATIVE-WITH-CAVEATS id `Exact` when its\n\
         //! corpus shows no difference, `Caveat` with message differences only (eligible for `auto`), `Delegate` with\n\
         //! any acceptance difference; ids that are never native are absent (they delegate).\n\
         //!\n\
         //! Owner: WP-17 (W1), WP-27 (W2).\n\n\
         use crate::Gate;\n\n\
         /// Sorted by hook id.\n\
         pub(crate) static PARITY_GATES: &[(&str, Gate)] = &[\n",
    );
    for (id, (gate, why)) in &gates {
        if why != "NATIVE" {
            writeln!(s, "    // {why}.")?;
        }
        writeln!(s, "    (\"{id}\", Gate::{gate}),")?;
    }
    s.push_str("];\n");
    Ok(s)
}

/// `table`: runs the corpus comparison and regenerates the gate table.
fn table(root: &Path, o: &Opts) -> anyhow::Result<()> {
    std::fs::create_dir_all(&o.work)?;
    let report = o.work.join("pch_gates_report.json");
    let _ignored = std::fs::remove_file(&report);
    run_ok(
        Command::new("cargo")
            .current_dir(root)
            .args([
                "test",
                "-p",
                "snp-hooks",
                "--test",
                "caveats",
                "--",
                "--exact",
                "caveat_corpus_gates",
            ])
            .env("SNP_PCH_GATES_REPORT", &report),
        "the caveat corpus comparison",
    )?;
    let reports: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&report)
            .with_context(|| format!("reading {}", report.display()))?,
    )?;
    let text = render_table(&reports)?;
    std::fs::write(root.join(TABLE), text)?;
    run_ok(
        Command::new("rustfmt")
            .args(["--edition", "2024"])
            .arg(root.join(TABLE)),
        "rustfmt parity_gates.rs",
    )?;
    workspace::say(&format!("pch-gates: {TABLE}"));
    Ok(())
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut o = Opts {
        dir: root.join("target/pch-oracle"),
        work: root.join("target/pch-gates"),
        jobs: 6,
    };
    let mut what: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_string_lossy().into_owned();
        match a.as_str() {
            "--dir" => {
                o.dir = PathBuf::from(it.next().context("--dir needs a value")?);
            }
            "--jobs" => {
                o.jobs = it
                    .next()
                    .context("--jobs needs a value")?
                    .to_string_lossy()
                    .parse()
                    .context("--jobs: not a number")?;
            }
            "goldens" | "corpus" | "table" | "all" if what.is_none() => what = Some(a),
            _ => bail!(
                "usage: cargo xtask pch-gates [goldens|corpus|table|all] [--dir DIR] [--jobs N]"
            ),
        }
    }
    match what.as_deref().unwrap_or("all") {
        "goldens" => goldens(&root, &o),
        "corpus" => corpus(&root, &o),
        "table" => table(&root, &o),
        _ => {
            goldens(&root, &o)?;
            corpus(&root, &o)?;
            table(&root, &o)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates_from_reports() {
        let r =
            |c: u64, a: u64, m: u64| serde_json::json!({"cases": c, "acceptance": a, "message": m});
        assert_eq!(caveat_gate(&r(10, 0, 0)), "Exact");
        assert_eq!(caveat_gate(&r(10, 0, 3)), "Caveat");
        assert_eq!(caveat_gate(&r(10, 1, 0)), "Delegate");
        assert_eq!(caveat_gate(&r(0, 0, 0)), "Delegate");
        assert!(DRIVER.contains("def mode_corpus("));
    }

    #[test]
    fn table_lists_every_registry_id_sorted() {
        let mut reports = serde_json::Map::new();
        for id in ids(snp_hooks::pch::caveats::HOOKS) {
            reports.insert(
                id.to_owned(),
                serde_json::json!({"cases": 1, "acceptance": 0, "message": 0}),
            );
        }
        let t = render_table(&serde_json::Value::Object(reports)).unwrap();
        let rows: Vec<&str> = t
            .lines()
            .filter_map(|l| l.trim().strip_prefix("(\""))
            .filter_map(|l| l.split('"').next())
            .collect();
        let mut sorted = rows.clone();
        sorted.sort_unstable();
        assert_eq!(rows, sorted);
        assert!(rows.contains(&"check-yaml"));
        assert!(rows.contains(&"pretty-format-json"));
    }
}
