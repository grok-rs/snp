//! `cargo xtask pch-goldens`: per-epoch pre-commit-hooks goldens and the Appendix A help goldens.
//!
//! The goldens snp-hooks replays come from the real Python, run by an embedded driver ([`DRIVER`]) under pinned
//! interpreters (CPython 3.13, ADR-058) — never by hand and never in CI:
//!
//! * `venvs [--dir DIR] [--python PY]` creates one venv per supported tag (v3.0.0…v6.0.0) in `DIR` (default
//!   `target/pch-oracle`) with `uv`: `pre-commit-hooks==<tag>` from PyPI plus `ruamel.yaml==0.19.1`,
//!   `toml==0.10.2` (the v3.x–v4.2.0 check-toml import) and `pre-commit==4.6.2` (its `_shuffled` and
//!   `xargs.partition` emulate partitions).
//! * `help [--dir DIR]` regenerates the lost 09 Appendix A: `crates/snp-hooks/tests/goldens/appendix_a/<tag>.json`
//!   holds, for every console script of every tag (P0, P1, P2 and caveat hooks alike), the bytes and exit code of
//!   `<script> -h` and of `<script> --snp-no-such-option` with `COLUMNS=80` (WP-27 reads them).
//! * `p0 [--dir DIR]` runs `crates/snp-hooks/tests/p0/cases.json` under the venvs of the golden tags and writes
//!   `crates/snp-hooks/tests/p0/goldens/<tag>.json` ((argv, files before) → (exit, output, files after)).
//! * `pygrep [--pc462 PY]` / `hazmat [--pc462 PY]` run `tests/{pygrep,hazmat}/cases.json` through
//!   `python -m pre_commit.languages.pygrep` / `python -m pre_commit.commands.hazmat` of the pre-commit 4.6.2
//!   oracle (default `target/oracle/pc462-py313/bin/python`) and write `goldens.json` next to the cases.
//! * no subcommand (or `all`): everything above, in that order.
//!
//! Every run happens in a fresh directory with the hermetic environment the replaying tests build (fixed git
//! identity, dates, `master`, `PATH=/usr/bin:/bin`, `LC_ALL=C.UTF-8`, `PYTHONHASHSEED=0`); a Python traceback is
//! reduced to its last line, which is what the natives print (09 §2.6). The output is deterministic for a given
//! interpreter.
//!
//! Owner: WP-26.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail, ensure};
use snp_hooks as _;

use crate::workspace;

/// The supported pre-commit-hooks tags (09 §5.1).
const TAGS: [&str; 16] = [
    "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
    "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
];

/// Extra packages of every tag venv.
const EXTRA: [&str; 3] = ["ruamel.yaml==0.19.1", "toml==0.10.2", "pre-commit==4.6.2"];

/// The crate whose tests read the goldens.
const HOOKS: &str = "crates/snp-hooks/tests";

/// The Python driver (written to `<dir>/pch_goldens_driver.py` and run with `-I`).
const DRIVER: &str = r#"
"""Golden generator of `cargo xtask pch-goldens` (WP-26); embedded in pch_goldens.rs and written to a temp file.

Runs the pinned upstream Python (pre-commit-hooks console scripts from PyPI, pre-commit 4.6.2) and records what the
processes produce, for snp-hooks' native ports to replay byte for byte:

    help   --out FILE                       every pre-commit-hooks console script of this venv: `-h` and an
                                            unknown option (Appendix A of 09), COLUMNS=80
    p0     --cases FILE --tag T --out FILE  tests/p0/cases.json under this venv's console scripts
    pygrep --cases FILE --out FILE          `python -m pre_commit.languages.pygrep`
    hazmat --cases FILE --out FILE          `python -m pre_commit.commands.hazmat`

Byte strings are latin-1 JSON strings. Every run happens in a fresh directory with a hermetic environment (the
same one tests/framework/support.rs builds); a traceback is reduced to its last line (`crashed: true`), which is
what the natives print instead.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile

DATE = 1577836800


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
    marker = b'Traceback (most recent call last):\n'
    i = out.find(marker)
    if i < 0:
        return out, False
    lines = out[i:].splitlines(keepends=True)
    return out[:i] + lines[-1], True


def run(argv: list[str], cwd: str, env: dict[str, str]) -> tuple[int, bytes]:
    p = subprocess.run(argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                       stderr=subprocess.STDOUT)
    return p.returncode, p.stdout


def apply_steps(root: str, steps: list[list[str]], env: dict[str, str]) -> list[str]:
    written = []
    for step in steps:
        op, *rest = step
        if op == 'w':
            path = os.path.join(root, os.fsdecode(b(rest[0])))
            os.makedirs(os.path.dirname(path) or root, exist_ok=True)
            with open(path, 'wb') as f:
                f.write(b(rest[1]))
            if rest[0] not in written:
                written.append(rest[0])
        elif op == 'x':
            os.chmod(os.path.join(root, os.fsdecode(b(rest[0]))), 0o755)
        elif op == 'ro':
            os.chmod(os.path.join(root, os.fsdecode(b(rest[0]))), 0o444)
        elif op == 'mkdir':
            os.makedirs(os.path.join(root, os.fsdecode(b(rest[0]))), exist_ok=True)
        elif op == 'git':
            subprocess.run(['git', *rest], cwd=root, env=env, check=True, stdout=subprocess.DEVNULL)
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


def mode_help(out_file: str) -> None:
    from importlib.metadata import distribution
    dist = distribution('pre-commit-hooks')
    bindir = os.path.dirname(sys.executable)
    scripts = sorted(ep.name for ep in dist.entry_points if ep.group == 'console_scripts')
    result = {'version': dist.version, 'python': sys.version.split()[0], 'scripts': {}}
    with tempfile.TemporaryDirectory() as tmp:
        env = hermetic_env(os.path.join(tmp, 'home'))
        env['COLUMNS'] = '80'
        for name in scripts:
            if name == 'pre-commit-hooks-removed':
                continue  # no argparse
            exe = os.path.join(bindir, name)
            entry = {}
            for kind, argv in (('help', ['-h']), ('usage', ['--snp-no-such-option'])):
                code, out = run([exe, *argv], tmp, env)
                out, crashed = strip_traceback(out)
                out = out.replace(os.fsencode(sys.prefix), b'<venv>')
                entry[kind] = {'code': code, 'out': s(out), 'crashed': crashed}
            result['scripts'][name] = entry
    with open(out_file, 'w') as f:
        json.dump(result, f, indent=1, sort_keys=True, ensure_ascii=True)
        f.write('\n')


def mode_p0(cases_file: str, tag: str, out_file: str) -> None:
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
            os.makedirs(root)
            env = hermetic_env(os.path.join(tmp, 'home'))
            subprocess.run(['git', 'init', '-q', '.'], cwd=root, env=env, check=True)
            written = apply_steps(root, case['steps'], env)
            for path, size in case.get('big', {}).items():
                with open(os.path.join(root, os.fsdecode(b(path))), 'wb') as f:
                    f.write(b'B' * size)
            for path, digits in case.get('bigint', {}).items():
                with open(os.path.join(root, os.fsdecode(b(path))), 'wb') as f:
                    f.write(b'1' * digits)
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
                'out': s(out),
                'crashed': crashed,
                'files': snapshot(root, case.get('files', written)),
            }
        finally:
            cleanup(tmp)
    result = {'tag': tag, 'version': dist.version, 'python': sys.version.split()[0], 'results': results}
    with open(out_file, 'w') as f:
        json.dump(result, f, indent=1, sort_keys=True, ensure_ascii=True)
        f.write('\n')


def write_fixtures(root: str, fixtures: dict[str, str], execs: list[str]) -> None:
    for path, content in fixtures.items():
        full = os.path.join(root, os.fsdecode(b(path)))
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'wb') as f:
            f.write(b(content))
    for path in execs:
        os.chmod(os.path.join(root, path), 0o755)


def mode_module(module: str, cases_file: str, out_file: str) -> None:
    with open(cases_file) as f:
        cases = json.load(f)
    results = {}
    for case in cases['cases']:
        tmp = tempfile.mkdtemp()
        try:
            root = os.path.join(tmp, 'w')
            os.makedirs(root)
            env = hermetic_env(os.path.join(tmp, 'home'))
            write_fixtures(root, cases['fixtures'], cases.get('exec', []))
            if module == 'pygrep':
                argv = [*case['args'], case['entry'], *case['files']]
                mod = 'pre_commit.languages.pygrep'
            else:
                argv = case['argv']
                mod = 'pre_commit.commands.hazmat'
            code, out = run([sys.executable, '-m', mod, *argv], root, env)
            out, crashed = strip_traceback(out)
            out = out.replace(os.fsencode(root), b'<root>')
            results[case['name']] = {'code': code, 'out': s(out), 'crashed': crashed}
        finally:
            cleanup(tmp)
    result = {'python': sys.version.split()[0], 'results': results}
    with open(out_file, 'w') as f:
        json.dump(result, f, indent=1, sort_keys=True, ensure_ascii=True)
        f.write('\n')


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=('help', 'p0', 'pygrep', 'hazmat'))
    parser.add_argument('--cases')
    parser.add_argument('--tag')
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    if args.mode == 'help':
        mode_help(args.out)
    elif args.mode == 'p0':
        mode_p0(args.cases, args.tag, args.out)
    else:
        mode_module(args.mode, args.cases, args.out)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
"#;

/// Parsed options.
#[derive(Debug)]
struct Opts {
    dir: PathBuf,
    python: PathBuf,
    pc462: PathBuf,
}

/// The `bin` directory of a venv.
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

/// `venvs`: one venv per tag (kept when it already has the right pre-commit-hooks).
fn venvs(o: &Opts) -> anyhow::Result<()> {
    std::fs::create_dir_all(&o.dir)?;
    for tag in TAGS {
        let venv = o.dir.join(tag);
        let marker = venv.join(".snp-pch-goldens");
        if marker.exists() {
            continue;
        }
        run_ok(
            Command::new("uv")
                .args(["venv", "--quiet", "--python"])
                .arg(&o.python)
                .arg(&venv),
            &format!("uv venv {tag}"),
        )?;
        let version = tag.trim_start_matches('v');
        run_ok(
            Command::new("uv")
                .args(["pip", "install", "--quiet", "--python"])
                .arg(venv_python(&venv))
                .arg(format!("pre-commit-hooks=={version}"))
                .args(EXTRA),
            &format!("uv pip install pre-commit-hooks=={version}"),
        )?;
        std::fs::write(&marker, tag)?;
        workspace::say(&format!("pch-goldens: venv {}", venv.display()));
    }
    Ok(())
}

/// Writes the driver next to the venvs.
fn driver(o: &Opts) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(&o.dir)?;
    let path = o.dir.join("pch_goldens_driver.py");
    std::fs::write(&path, DRIVER.trim_start())?;
    Ok(path)
}

/// Runs the driver with `python -I`.
fn drive(
    python: &Path,
    driver: &Path,
    args: &[&std::ffi::OsStr],
    what: &str,
) -> anyhow::Result<()> {
    run_ok(
        Command::new(python)
            .arg("-I")
            .arg(driver)
            .args(args)
            .env("PYTHONHASHSEED", "0"),
        what,
    )
}

/// `help`: Appendix A for every tag.
fn help(root: &Path, o: &Opts) -> anyhow::Result<()> {
    let drv = driver(o)?;
    let out_dir = root.join(HOOKS).join("goldens/appendix_a");
    std::fs::create_dir_all(&out_dir)?;
    for tag in TAGS {
        let out = out_dir.join(format!("{tag}.json"));
        drive(
            &venv_python(&o.dir.join(tag)),
            &drv,
            &["help".as_ref(), "--out".as_ref(), out.as_os_str()],
            &format!("help goldens {tag}"),
        )?;
    }
    workspace::say(&format!(
        "pch-goldens: Appendix A goldens in {}",
        workspace::rel(root, &out_dir)
    ));
    Ok(())
}

/// The golden tags of `tests/p0/cases.json`.
fn golden_tags(cases: &Path) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(cases)?)?;
    let tags = v
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .context("cases.json: no `tags`")?;
    Ok(tags
        .iter()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect())
}

/// `p0`: per-epoch goldens of the P0 cases.
fn p0(root: &Path, o: &Opts) -> anyhow::Result<()> {
    let drv = driver(o)?;
    let cases = root.join(HOOKS).join("p0/cases.json");
    let out_dir = root.join(HOOKS).join("p0/goldens");
    std::fs::create_dir_all(&out_dir)?;
    for tag in golden_tags(&cases)? {
        ensure!(TAGS.contains(&tag.as_str()), "unknown golden tag {tag}");
        let out = out_dir.join(format!("{tag}.json"));
        drive(
            &venv_python(&o.dir.join(&tag)),
            &drv,
            &[
                "p0".as_ref(),
                "--cases".as_ref(),
                cases.as_os_str(),
                "--tag".as_ref(),
                tag.as_ref(),
                "--out".as_ref(),
                out.as_os_str(),
            ],
            &format!("p0 goldens {tag}"),
        )?;
    }
    workspace::say(&format!(
        "pch-goldens: P0 goldens in {}",
        workspace::rel(root, &out_dir)
    ));
    Ok(())
}

/// `pygrep` / `hazmat`: the module goldens under the pre-commit 4.6.2 oracle.
fn module(root: &Path, o: &Opts, which: &str) -> anyhow::Result<()> {
    let drv = driver(o)?;
    let dir = root.join(HOOKS).join(which);
    let cases = dir.join("cases.json");
    let out = dir.join("goldens.json");
    drive(
        &o.pc462,
        &drv,
        &[
            which.as_ref(),
            "--cases".as_ref(),
            cases.as_os_str(),
            "--out".as_ref(),
            out.as_os_str(),
        ],
        &format!("{which} goldens"),
    )?;
    workspace::say(&format!("pch-goldens: {}", workspace::rel(root, &out)));
    Ok(())
}

/// Runs the task with the arguments that follow its name.
pub(crate) fn run(args: &[OsString]) -> anyhow::Result<()> {
    let root = workspace::root()?;
    let mut o = Opts {
        dir: root.join("target/pch-oracle"),
        python: PathBuf::from("/usr/bin/python3.13"),
        pc462: venv_python(&root.join("target/oracle/pc462-py313")),
    };
    let mut what: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_string_lossy().into_owned();
        let slot = match a.as_str() {
            "--dir" => &mut o.dir,
            "--python" => &mut o.python,
            "--pc462" => &mut o.pc462,
            "venvs" | "help" | "p0" | "pygrep" | "hazmat" | "all" if what.is_none() => {
                what = Some(a);
                continue;
            }
            _ => bail!(
                "usage: cargo xtask pch-goldens [venvs|help|p0|pygrep|hazmat|all] [--dir DIR] \
                 [--python PY] [--pc462 PY]"
            ),
        };
        *slot = PathBuf::from(it.next().with_context(|| format!("{a} needs a value"))?);
    }
    match what.as_deref().unwrap_or("all") {
        "venvs" => venvs(&o),
        "help" => help(&root, &o),
        "p0" => p0(&root, &o),
        "pygrep" => module(&root, &o, "pygrep"),
        "hazmat" => module(&root, &o, "hazmat"),
        _ => {
            venvs(&o)?;
            help(&root, &o)?;
            p0(&root, &o)?;
            module(&root, &o, "pygrep")?;
            module(&root, &o, "hazmat")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_is_python_and_tags_are_ordered() {
        assert!(DRIVER.contains("def mode_help("));
        assert!(DRIVER.contains("def mode_p0("));
        assert_eq!(TAGS.first(), Some(&"v3.0.0"));
        assert_eq!(TAGS.last(), Some(&"v6.0.0"));
    }
}
