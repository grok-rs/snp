# Differential tests (snp vs pre-commit 4.7.0)

The differential harness (`snp-difftest`, ARCHITECTURE §13.2, research 10 §7.4) runs every scenario against snp and
against the pre-commit 4.7.0 oracle and compares everything a user can observe: stdout, stderr and exit code of each
step, the working tree (bytes, exec bits, symlink targets), the index (`git ls-files -s`), refs, the stash list,
`.pre-commit-config.yaml` and `.git/config` bytes, the installed hook scripts (by name and mode; their bytes differ by
design) and the logical store contents. Every scenario runs in both brands: the **compat** brand compares bytes with
the oracle directly; the **snp** brand compares with the oracle whose argparse `prog` is patched to `snp`, after
`brand_map.yaml` is applied to its non-argparse texts (ADR-058, COMPATIBILITY §3.3).

| Path | What | Owner |
|---|---|---|
| `scenarios/**/*.yaml` | the curated corpus (this file documents the schema) | WP-35 |
| `brand_map.yaml` | snp-brand substitutions for non-argparse texts (machine form of COMPATIBILITY §3.3) | WP-35 |
| `harness/` | harness fixtures (`planted-divergence.sh`, the fake snp of the shrinker acceptance test) | WP-35 |
| `divergences/<ID>.yaml` | one scenario per register ID of COMPATIBILITY §5 that must keep diverging | the first WP of the ID's register row |

## Running

```sh
cargo xtask oracle install                         # target/oracle/pc470-py312 and pc470-py313 (once)
cargo xtask difftest                               # curated corpus, release binaries, both brands
cargo xtask difftest --suite all                   # curated corpus + divergence files (the CI job)
cargo xtask difftest --scenario run --brand compat # every scenario whose name starts with `run/`
cargo xtask difftest --suite seed --self-check     # the oracle against itself (harness self-consistency)
cargo xtask difftest --suite generated --cases 2000 --seed 42   # the nightly generated suite
cargo xtask difftest --suite generated --seed 42 --case 17      # replay one generated case
cargo xtask difftest register --strict             # the divergence-register report
cargo xtask difftest shrink --file failing.yaml    # shrink a diverging scenario, write repro.sh
cargo xtask scenario-gen --cases 20 --seed 42      # write generated cases as YAML (to promote one)
cargo xtask divergence-book                        # docs/divergences/*.md → target/docs/DIVERGENCES.md
cargo xtask catalog-status --wave W3 --check       # ported vs total catalog rows per owner
```

`cargo xtask difftest` builds `snp`, `snp-probe` and `snp-difftest` (release; `--debug` for the dev profile,
`--no-build` to skip) and runs `snp-difftest`, passing `--snp-bin`. `snp-difftest run` options: `--suite
seed|curated|divergences|all|generated`, `--scenario NAME` (a name or a name prefix ending at `/`), `--file PATH`,
`--tag T`, `--brand compat|snp`, `--net` (allow scenarios that need the network), `--keep` (keep the roots),
`--self-check`, `--jobs N`, `--no-taskset`, `--no-shrink`, `--max-shrink-runs N`, `--root DIR`, `--out DIR`,
`--oracle DIR`, `--cli-oracle DIR`, `--probe P` / `--no-probe`. Exit status: 0 = every scenario passed (identical,
diverging exactly as registered, or skipped with a reason), 1 = a failure, 2 = usage error.

A failing scenario writes `<out>/<name>/` (default `target/difftest/out`): `report.txt`, the scenario as run, a
`repro.sh`, and per brand `diff.txt` plus `oracle/` and `snp/` with `step-N/{stdout,stderr,exit,tree.txt,index.txt,
refs.txt,…}`. Generated failures are shrunk first (`<name>.shrunk/{scenario.yaml,repro.sh}`).

## Protocol

For each scenario, brand and (for `strict: both`) `SNP_COMPAT_STRICT` variant, each side runs **sequentially** at the
same absolute root `R = <root>/<hash of the name>`: `R` is wiped (only if it carries the harness marker), the hook
repositories and the work repository are materialised there with fixed `GIT_*_DATE`s (identical SHAs), and every
step runs in a hermetic environment (`HOME`, `PRE_COMMIT_HOME`, `SNP_HOME`, `TMPDIR`, `XDG_CACHE_HOME` under `R`;
`GIT_CONFIG_NOSYSTEM`, a fixed global gitconfig, `GIT_CEILING_DIRECTORIES` = the parent of `R`, `LC_ALL=C.UTF-8`,
`TZ=UTC`, `PYTHONHASHSEED=0`) under `taskset -c 0-3` on Linux, so CPU-count-derived job numbers agree. The side's
entry point is linked as `R/bin/pre-commit` (compat) or `R/bin/snp` (snp brand) and `R/bin` leads `PATH`, so
installed hook scripts call back into the same side.

## Normalisation tiers (COMPATIBILITY §2)

| Tier | Applied to | Rules |
|---|---|---|
| A (`raw`) | — | none: byte equality |
| C (`exact`, the default) | both sides | `- duration: …s` → `[D]s`; stash patch names `patch<epoch>-<pid>` → `patch[TP]`; random `repo…`/`tmp…` suffixes; snp's store root ↔ pre-commit's store root; installer chatter between `[INFO] Installing environment for …` and the next status line; the `rev:` of try-repo's `shadow-repo` (committed by pre-commit without the fixed dates) |
| B (`tierB`) | both sides | Tier C plus: Python tracebacks (including exception chains) → their final line; argparse usage blocks → `usage: [USAGE]` + the `error:` line; libyaml mark lines; localized `[WinError N]` texts; version numbers |

`lines-unordered` compares the multiset of Tier-C lines (N-1 merge-conflict sets, `autoupdate -j>1`). In snp-brand
runs the brand map is applied to the oracle's non-argparse text first; usage blocks of the parsers with snp-only
options (`snp_only_option_parsers` in `brand_map.yaml`) are reduced to their `error:` lines on both sides (WP-12's
`crates/snp-cli/tests/goldens/snp.json` pins their full text). snp's class-DIAG stderr lines are stripped before
comparing unless the scenario names that DIAG ID.

## Verdicts and the divergence register

* No `expect_divergence`: any difference fails (**unlisted difference**).
* `expect_divergence: D-…`: the sides must still differ and the scenario's `expected:` checks must hold; a scenario
  that stopped diverging fails (**stale divergence**). A class-DIAG ID may differ on stderr only. The strict variant
  of a class-EXT scenario (`strict: both`) must be identical to the oracle.
* `status: stub` (a W0 placeholder) and `status: unit` (pinned by the listed `unit_tests:`, which must exist) are not
  run; `cargo xtask difftest register` lists them per owner and fails on a register ID without its files, a file
  without a register ID, or a scenario naming another ID. `--strict` also fails on remaining stubs.
* `--self-check` runs the oracle on both sides (registered expectations are ignored): the harness's own
  determinism check.

## The generated suite

Generated scenarios run with the same protocol and verdicts, but the suite exists to find the difference nobody has
seen, so it must stay green on the ones everybody has. Exactly these are taken out, and only for scenarios tagged
`generated` (a curated scenario sees them all):

| Difference | What the suite does | What still checks it |
|---|---|---|
| D-MISC-09: pygrep fails to `open()` a file (random `args` read as file names) | `reduce_generated` collapses the traceback to its uncoloured exception line, in streams and worktree files (`log_file`) | that final line and the exit code here, `divergences/D-MISC-09` for the traceback |
| CPython 3.12.3's argparse stops filling `nargs='*'` at an unknown option, so `pygrep.py: error: unrecognized arguments:` lists more than on 3.13.14+, the canonical profile (`-dash.txt` as a file name) | compat brand: the list is masked | the snp brand compares the line byte for byte against the 3.13 oracle |
| D-EXT-07: `snp run` takes several hook ids, so `snp: error: unrecognized arguments:` lists fewer names | snp brand: the list is masked | the compat brand compares the line byte for byte |
| D-DATA-12: an intent-to-add file paired with an unstaged deletion as a rename crashes pre-commit | the generator sets `diff.renames=false` on exactly those scenarios, so the git state stays under test without the crash | `divergences/D-DATA-12`, `hazard_b_intent_to_add_paired_as_a_rename` |

Every other traceback, argparse line and state difference stays a failure. A new class gets a register ID (or a
reason it is not a divergence), a row here and a test next to `reduce_generated`, never a wider reduction.

## Scenario schema

Research 10 §7.4.3 plus the extensions below. Unknown keys are errors, except keys the harness knows but cannot
execute on this host, which skip the scenario with the reason (so nothing passes silently).

```yaml
name: run/example                 # unique; must start with the directory (`run/`) in the curated corpus
description: what it shows        # free text
covers: [T10-RUN-12, X13-EXE-01]  # catalog rows it ports (must exist in tests/catalog/index.tsv)
tags: [seed]                      # seed, crash-recipe, platform, migration, fast-path, generated, …
since: "4.1.0"
expect_divergence: D-CRASH-04     # a COMPATIBILITY §5 ID (see Verdicts)
brands: [compat, snp]             # default both
strict: both                      # also run snp with SNP_COMPAT_STRICT=1 (extension scenarios)
requires:
  git: ">=2.31, <3"               # host git version constraint
  os: [linux, macos, windows]     # default every OS
  net: false                      # true: skipped unless --net (also implied by an external repo)
  tools: [python3]                # executables that must be on PATH
  pty: false                      # posix pty available
  probe: false                    # snp-probe needed (implied by `${probe}`)
  not_root: true                  # or `root: true`
  logical_cpus_min: 4
  console: pipe                   # pipe | pty | conpty (Windows ConPTY: not executable yet → skipped)
  launcher: direct                # git-cmd | git-pwsh | git-bash (14 §A.6.1: not executable yet → skipped)
  path_profile: runner            # user-minimal | no-usr-bin: not executable yet → skipped
  manual: "reason"                # a setup the harness cannot build: always skipped with this reason
repos:                            # hook repositories, materialised at R/repos/<name> (or `path:`)
  hooks:
    fixture: script_hooks_repo    # a crates/snp-testkit/resources/<name> recreation of pre-commit's fixtures
    files: {bin/x.sh: "#!/bin/sh\n"}
    exec: [bin/x.sh]
    tags: [v1.0.0]                # tags on the first commit
    then:                         # operations after the first commit (see Operations)
      - write: {x: "1\n"}
      - commit: second
      - tag: v1.1.0
    bare: false                   # an empty bare repository (a push target)
    object_format: sha256
  remote: {url: "https://github.com/pre-commit/pre-commit-hooks", rev: v6.0.0}   # external, needs the network
work:                             # the repository both tools run in (R/work, or `dir:`)
  files: {a.txt: "a\n", "bad\\xff.txt": "", img.jpg: !!binary "/9j/4AA="}   # `\xNN` in names = raw bytes
  files_bytes: {}                 # same, bytes only
  generated: {count: 400, pattern: "f{i:05}.txt", content: ""}             # (also inside `files:`)
  exec: [a.txt]
  symlinks: {link: target}        # posix
  fifos: [ff]                     # posix
  config: |                       # .pre-commit-config.yaml (null: no config file)
    repos: []
  commit: [.pre-commit-config.yaml]   # committed before the steps (empty: no commit)
  modes: {a.txt: "0o000"}         # applied after the commit, before staging
  stage: [a.txt]                  # `git add` after the commit ("." = everything)
  unstaged: {a.txt: "edit\n"}     # post-staging edits (null: delete)
  chmod: {a.txt: "0o444"}         # applied last
  git_config: {core.autocrlf: "true"}
  setup: [...]                    # further operations, in order
  git: true                       # false: not a git repository
  dir: real/repo                  # the work directory relative to R
projects: [one/p, two/p]          # the work block materialised in each directory; steps run in each
steps:
  - tool: [run, --all-files]      # the side's tool (after the program name)
    env: {SKIP: x, VAR: null, PATH_PREPEND: "${root}/bin"}   # null unsets
    stdin: "text"                 # or stdin_bytes: "<hex>"
    tty: false                    # run under a pty (posix; streams arrive merged on stdout)
    timeout: 5s                   # default 120 s; a timed-out step is killed and recorded
    cwd: subdir                   # relative to the work directory
    signal: {name: SIGINT, after: .hook-started.ready}   # or `after: 1.5` (seconds)
  - git: [commit, -m, msg]        # git; installed hooks call back into the side's tool
  - shell: '"$PRE_COMMIT" run | head -c 20'   # sh -c; $PRE_COMMIT = the side's tool
compare:
  stdout: exact                   # raw | exact | lines-unordered | tierB | ignore
  stderr: exact
  exit: exact                     # exact | nonzero-equal | ignore
  tree: true
  index: true
  refs: true                      # for-each-ref, HEAD, stash list
  hooks_dir: false                # true / behavior: .git/hooks names and modes
  config_file: exact              # .pre-commit-config.yaml after each step
  git_config: false
  store: false                    # true / logical: the set of (repo, rev) entries
  eol: insensitive                # \r\n == \n (14 §A.6.4); `windows`: on Windows only (D-DATA-09 rewrites)
expected:                         # divergence files: how each side must behave (crates/snp-difftest/src/expect.rs)
  oracle: [{exit: 3, stdout_contains: "OverflowError"}]
  snp: [{exit: 1, stderr: ""}]
```

**Operations** (`then:` and `setup:`): `git: [args]`, `write: {path: bytes}`, `remove: path`, `shell: script`,
`commit: message` (`git add -A` first), `tag: name`.

**Substitutions** in every text value: `${root}`, `${work}`, `${home}`, `${store}` (`PRE_COMMIT_HOME`),
`${snp_store}` (`SNP_HOME`), `${repo:NAME}` (path), `${url:NAME}` (`file://…`), `${rev:NAME}` (HEAD of the hook
repository after `then:`), `${head}` (the work repository's HEAD after setup), `${probe}` (shell-quoted snp-probe),
`${probe_path}`, `${tool}` (the side's entry point), `${oracle}` (the plain pre-commit 4.7.0 oracle on **both**
sides, shell-quoted: interop scenarios run it next to the tool under test), `${z40}`, `${z64}`, `${PATH}`. An
unknown name is an error; write shell variables without braces (`$VAR`).

## Resource limits

The harness runs untrusted-by-construction inputs (generated cases, a snp under development), so every resource it
spends is bounded and every limit fails loudly instead of exhausting the machine:

* **YAML** (scenarios, divergence files, `brand_map.yaml`): aliases are expanded with a budget of 1,000,000 nodes and
  64 MiB of scalar text (every alias use counts), nesting ≤ 256. A "billion laughs" document is an error in
  milliseconds.
* **Generated work files**: one `{generate}` file ≤ 64 MiB, `count` in `0..=64 Mi`; `generated:` ≤ 100,000 files;
  all generated bytes of a scenario together ≤ 256 MiB, charged before anything is built.
* **Names and paths**: `name`, `work.dir`, `repos.*.path` and `projects` are relative, without `.`/`..` components
  or backslashes; every file a scenario writes, removes, chmods or links (`work.files`, `ops`, `unstaged`, …) must
  stay inside the scenario root. Artifact directories under `--out` are replaced only when they carry the harness's
  `.snp-difftest-artifacts` marker.
* **Processes**: each captured stream keeps at most 64 MiB; past that the process group is killed and the step is
  reported as a difference (`output exceeded …`), never as equal. On posix every tool runs in its own process group,
  so a deadline kills hooks and their children too, and the readers are joined with a 2 s grace after the kill (a
  process that left the group is abandoned and the step counts as timed out). Windows kills only the direct child.
* **`*_matches`**: the matcher has a step budget, a nesting limit (quantified single characters run as loops) and
  searches at most 16 MiB of text; exceeding any is an expectation error.

## Corpus layout

| Directory | Source |
|---|---|
| `seed/` | 20 basic scenarios: the oracle-vs-oracle self-consistency set |
| `config/`, `migrate/`, `run/`, `stash/`, `xargs/`, `install/`, `git/`, `store/`, `repo/`, `autoupdate/`, `try-repo/`, `gc/`, `meta/`, `cli/` | the CLI-reachable rows of research 10 §5 (`covers: [T10-…]`) |
| `crash/` | the reproduction recipes c01…c24 of research 13 X.10 and the X.4 rows they exercise (`X13-…`), each with the register ID where snp deliberately differs |
| `platform/` | research 14 §A.6.4 W01–W27 and M01–M07 (Windows/macOS scenarios skip elsewhere; setups the harness cannot build are `requires.manual`) |
| `interop/` | research 11 §A.20 M11-MIG-01…24 (WP-33 owns their acceptance; snp-only commands are `requires.manual`) |
| `fast-path/` | cold and warm transcripts (local repos; pre-commit-hooks and pygrep-hooks need `--net`) |

To promote a generated or shrunk failure into the corpus, copy its `scenario.yaml` into the right directory, give
it a name, `covers:` and a description, and check it with `cargo xtask difftest --file <it> --self-check`.
