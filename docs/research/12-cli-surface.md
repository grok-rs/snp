# 12 — CLI surface of pre-commit 4.6.2 (snp spec)

This file collects the CLI-surface specification for **snp**. The baseline CLI facts live in 02 §2 (run options), 03 §1 (shared
CLI frame, other subcommands) and 01 §8.1 (`validate-config`). The addenda below extend and, where stated, **correct** them.

---

## Addendum: CLI surface parity: verbatim help, usage and argparse error routing

### A.0 Provenance and method

| Item | Value |
|---|---|
| Reference source | `refs/pre-commit` (4.6.2): `pre_commit/main.py:202-375` (parser), `pre_commit/color.py:84-109` (`use_color`, `--color`), `pre_commit/commands/hazmat.py:10-91`, `pre_commit/clientlib.py:26-39,64-72` (HOOK_TYPES, STAGES, `transform_stage`), `pre_commit/error_handler.py:70-81`, `pre_commit/commands/hook_impl.py:189-256`, `pre_commit/lang_base.py:174-178` (`hook_cmd`) |
| argparse source | CPython `Lib/argparse.py` and `Lib/textwrap.py` from the interpreters below; line numbers cite **3.13.16** unless marked |
| Oracles run | pre-commit **4.6.2** under CPython **3.12.3** (`tmp-test-strategy/venv462`, Ubuntu 24.04 build), **3.12.15**, **3.13.6** (uv), **3.13.16**, **3.14.8**, **3.15.0rc3** (python-build-standalone release `20261003`, venvs in `scratchpad/tmp-cli/venv-<ver>`) |
| Capture environment | `env -i PATH=/usr/bin:/bin HOME=<tmp> PRE_COMMIT_HOME=<tmp> COLUMNS=80 LANG=C.UTF-8`, stdin `/dev/null`, stdout and stderr captured as **separate pipes** (non-TTY) |
| Raw captures | `scratchpad/tmp-cli/out/{help,err,prec,dd}-<pyver>.txt` (≈170 argv cases × 6 Python versions); scripts in `scratchpad/tmp-cli/scripts/` |
| clap probe | `scratchpad/tmp-cli/claprobe` (clap **4.6.7**, rustc 1.98.0), used to verify every clap claim in §A.12 |

Everything marked "observed" was run. Everything else is derived from the cited source.

### A.1 TL;DR — the facts that matter most

1. **Two error prefixes, chosen by *which* parser fails.** Unrecognized arguments are collected from every level and reported by
   the **top-level** parser *after* the whole parse has succeeded: top-level usage block plus
   `pre-commit: error: unrecognized arguments: <tokens joined by ' '>`. All other parse errors are raised by the parser that
   owns the offending argument: `pre-commit <cmd>: error: …` or `pre-commit hazmat <tool>: error: …`, each preceded by
   **that** parser's usage block. Exit **2**, stderr. (02 §2.2 is wrong on this point; 10 CLI-07 is right.)
2. **Precedence is "first error in consumption order wins".** An unknown option never stops parsing. Later errors in the same
   (sub)parser still win: `run --bogus --hook-stage bogus` reports the `--hook-stage` error. Parsing stops at the first
   *raised* error. `-h`/`-V` exit 0 at the point where they are consumed.
3. **`PRE_COMMIT_COLOR` is a raw string default that is type-converted only if `--color` was not given**, at the *end* of that
   subparser's parse. So `PRE_COMMIT_COLOR=bogus pre-commit sample-config --color never` exits 0. `PRE_COMMIT_COLOR=bogus`
   with any argv that gets as far as the end of a subparser's parse (including bare `pre-commit` → `run`) exits 2 with
   `pre-commit <cmd>: error: argument --color: invalid use_color value: 'bogus'`. `PRE_COMMIT_COLOR=` (empty) is invalid in
   the same way (`''`). Help, version and `help …` are unaffected. (02 §2.2, 03 §1.1 and 10 RUN-59 / §7.2 corrected.)
4. **Subcommand names are never abbreviated** (`ru` → invalid choice). Long options are abbreviated by unique prefix *within
   the parser that owns them* (`run --all`, `--hook-st=commit`, `sample-config --c never`). Ambiguity is an error that lists
   every candidate in declaration order. Example: `pre-commit run: error: ambiguous option: --co could match --color, --config, --commit-msg-filename, --commit-object-name`.
5. **REMAINDER is not "everything after `--`".** For `hook-impl` `rest` and the hazmat `cmd`, argparse keeps a literal `--` in
   the list. `hook-impl` then *unconditionally* drops `rest[0]` (`main.py:408`), which is normally the `--` from the hook
   script. `-h` after `--` reaches `rest` (observed: `expected 0 arguments but got 1: ['-h']`). An option-like token before
   the first REMAINDER value is *not* captured (`hazmat n1 -n echo -- a` → `unrecognized arguments: -n`). After a preceding
   positional it is captured (`hazmat cd sub -h` → `cmd=['-h']`).
6. **argparse output depends on the Python version.** Choice quoting in `invalid choice` has flip-flopped:
   `'a', 'b'` on ≤3.12.7, 3.13.0, ≥3.13.14 and ≥3.14.5; `a, b` on 3.12.8–3.12.x, 3.13.1–3.13.13 and 3.14.0–3.14.4. The help
   layout changed in 3.13 (`-c, --config CONFIG` versus `-c CONFIG, --config CONFIG`). Parse semantics changed between
   ≤3.12.6/3.13.0 and ≥3.12.7/≥3.13.1 (`--` before a subcommand, ambiguity timing, positional trimming). 3.14 colourises usage and help on a
   TTY, and 3.15 adds "maybe you meant" suggestions.
   **Decision: snp's canonical reference is pre-commit 4.6.2 on CPython ≥ 3.13.14 / ≥ 3.14.5, with colour off and no
   suggestions** (§A.2).
7. **Width:** help and usage wrap at `shutil.get_terminal_size().columns - 2`. That size comes from `COLUMNS` if it is a
   positive Python-`int`, else from the size of the terminal on **stdout** (not stderr), else 80. So piped output is always
   80-wide unless `COLUMNS` is set. Usage errors on stderr also use the width of stdout.
8. **Exit codes are always fixed:** help and version → 0 on stdout; usage error → 2 on stderr. Errors raised after parsing go
   through `error_handler`: FatalError → 1, unexpected exception → 3, plus the crash log. `SystemExit("msg")` (hazmat,
   hook-impl) → message on stderr, exit 1, **no** log.
9. **hazmat has two invocation paths with different exit codes.** A direct `pre-commit hazmat …` runs under `error_handler`
   (exit 3 for ExecutableNotFoundError). Inside a hook `entry`, `pre-commit hazmat` is rewritten to
   `python -m pre_commit.commands.hazmat` (`lang_base.py:176-177`). There is no `error_handler` and no `--color` there: a raw
   traceback gives exit **1**, and argparse errors give exit 2 with prog `hazmat.py` (≤3.13) or
   `python -m pre_commit.commands.hazmat` (3.14+).
10. **clap cannot reproduce argparse acceptance semantics** (verified with clap 4.6.7: 13 residual divergences, C1–C13 in §A.12.3: deferred
    unknown arguments, env-default ordering, REMAINDER/`--`, negative numbers, `"--x y"` positionals, short clusters).
    **Recommendation:** a small, table-driven, dependency-free argparse-compatible parser and help formatter in `snp-cli`
    (§A.13). clap is kept only to generate shell completions at build time, if at all. This revises 07 D9.

### A.2 Python-version matrix and the canonical choice

Observed with pre-commit 4.6.2 on 3.12.3, 3.12.15, 3.13.6, 3.13.16, 3.14.8 and 3.15.0rc3. The version boundaries were
bisected from CPython tags (`raw.githubusercontent.com/python/cpython/vX.Y.Z/Lib/argparse.py`). Two independent change
sets matter:
- **"new parse semantics"** landed in **3.12.7** and **3.13.1**. They are absent in ≤3.12.6 (including Ubuntu's 3.12.3) and in
  3.13.0.
- **choice quoting** flipped to unquoted in **3.12.8** and **3.13.1**, and back to quoted in **3.13.14** and **3.14.5**.

| Behaviour | Old semantics: ≤ 3.12.6, 3.13.0 | New semantics: ≥ 3.12.7, ≥ 3.13.1, 3.14, 3.15 |
|---|---|---|
| Ambiguous-prefix error | raised while **classifying** argv, before anything else (`run -h --co x` → exit 2; `run --hook-stage bogus --co x` → ambiguous) | raised when the token is consumed (`run -h --co x` → help, exit 0; `run --hook-stage bogus --co x` → invalid choice) |
| `pre-commit -- sample-config` | `argument command: invalid choice: '--'` (exit 2) | runs sample-config (PARSER strips a leading `--`) |
| `try-repo REPO -a HOOK`, `try-repo REPO --ref X HOOK` | **`unrecognized arguments: HOOK`** (exit 2): the `?` positional is consumed as empty | accepted (trailing zero-length positional matches are kept for later) |
| `hazmat cd` (no args) | `the following arguments are required: subdir, cmd` | `… required: subdir` (REMAINDER is no longer required) |
| mutex "present" test | `value is not default` | `has option strings or got ≥1 string` (same observable result for pre-commit) |

| Behaviour | ≤3.12.7, 3.13.0 | 3.12.8 – 3.12.x | 3.13.1 – 3.13.13 | **3.13.14+, 3.14.5+** | 3.14.0 – 3.14.4 | 3.15.0rc3 |
|---|---|---|---|---|---|---|
| `invalid choice` list | quoted `'a', 'b'` | **unquoted** `a, b` | **unquoted** | quoted | unquoted | quoted |
| Offending value (`%r`) | repr | repr | repr | repr | repr | repr |
| "maybe you meant" | no | no | no | no (3.14 `suggest_on_error=False`) | no | **yes**: `invalid choice: 'ru', maybe you meant 'run'? (choose from …)` |

| Behaviour | 3.12.x | 3.13.x | 3.14.x | 3.15.0rc3 |
|---|---|---|---|---|
| Help: option with value and aliases | `-c CONFIG, --config CONFIG` (help on the next line) | `-c, --config CONFIG   Path …` | as 3.13 | as 3.13 |
| Usage: subparsers positional | `{…}` and `...` split onto separate lines | `{…} ...` on one line | as 3.13 | as 3.13 |
| Usage: mutex group may split across lines | no (`[--all-files \| --files [FILES ...]]` stays whole) | yes (`[--all-files \|` / `--files [FILES ...]]`) | yes | yes |
| Colour in usage/help | never | never | **when `_colorize.can_colorize()`**: `PYTHON_COLORS`, `NO_COLOR`, `FORCE_COLOR`, `TERM=dumb`, and `sys.stdout` isatty, even when printing to stderr. The `error:` line itself stays plain | coloured, and the `error:` line is coloured too |
| prog of `python -m pre_commit.commands.hazmat` | `hazmat.py` | `hazmat.py` | `python -m pre_commit.commands.hazmat` (basename of `sys.executable` + ` -m ` + module) | as 3.14 |
| Negative-number token (→ positional) | `^-\d+$\|^-\d*\.\d+$` | same | `match(r'-\.?\d')` (also `-1x`, `-1e5`) | as 3.14 |

3.14 colour bytes (observed with `FORCE_COLOR=1`, Tier C, do not reproduce):
`\x1b[1;34musage: \x1b[0m\x1b[1;35mpre-commit sample-config\x1b[0m [\x1b[32m-h\x1b[0m] [\x1b[36m--color \x1b[33m{auto,always,never}\x1b[0m]`.
Headings use `\x1b[1;34m`, long options in the list use `\x1b[1;36m`, short ones `\x1b[1;32m` and metavars `\x1b[1;33m`.
3.15 additionally renders `error:` as `\x1b[1;35merror:\x1b[0m \x1b[35m<message>\x1b[0m`.

Observed points: 3.12.3 = old semantics, quoted. 3.12.15 = new semantics, unquoted. 3.13.6 = new semantics, unquoted, 3.13
layout. 3.13.16 = 3.14.8 = new semantics, quoted, 3.13 layout (**byte-identical** help and errors in non-TTY mode).
3.15.0rc3 = 3.14.8 plus suggestions and coloured errors when forced.

**Canonical reference for snp (decision): pre-commit 4.6.2 on CPython ≥ 3.13.14 or ≥ 3.14.5, non-TTY, `NO_COLOR` set.**
- Its `invalid choice` wording is identical to the existing 3.12.3 oracle (quoted).
- Its parse semantics are the ones every maintained CPython line now has (since 3.12.7 / 3.13.1).
- Its help layout is the one that current installs (uv/pipx pick the newest Python) print.
- Verified: the canonical help and error texts are **byte-identical between 3.13.16 and 3.14.8** (non-TTY). 3.13.6 differs
  only in choice quoting.
- Against the 3.12.3 oracle (`venv462`), only the rows of the first table above and the help layout differ. Error lines
  for single-error argv are otherwise identical, because 3.12.3 also quotes choices.

### A.3 Output contract

| Event | Stream | Bytes | Exit |
|---|---|---|---|
| `-h`/`--help` (any level), `help`, `help <cmd>` | stdout | `format_help()`: usage block, blank line, sections; ends with exactly one `\n` | 0 |
| `-V`/`--version` (top level only) | stdout | `pre-commit 4.6.2\n` | 0 |
| argparse usage error | stderr | `format_usage()` of the **reporting** parser (`usage: …\n`, wrapped), then **immediately** `{prog}: error: {message}\n`. No blank line between them; nothing on stdout | 2 |
| Error after a successful parse | see §A.6.4 | | 1 / 3 / 130 / child code |

- Each message is written with one `write`. argparse **swallows** `OSError` and `AttributeError` on write
  (`_print_message`, `argparse.py:2658-2668`), so a closed or broken stdout still exits 0 or 2.
- **Windows:** Python's `sys.stdout` and `sys.stderr` are text streams with `newline=None`, which translates `\n` → `\r\n`
  (CPython `Python/pylifecycle.c` `create_stdio`, verified in source). So on Windows **argparse help, usage and errors end
  lines with CRLF**. pre-commit's own output (`output.write_line` writes bytes to `sys.stdout.buffer`) stays LF. snp must do
  the same: CRLF only for argparse-originated text, `print()`-originated text (sample-config, validate-*) and hazmat or
  hook-impl `SystemExit` messages.
- Non-UTF-8 argv (POSIX) is decoded with `surrogateescape`. In `%r` messages a raw byte `0xFF` shows as `'\udcff'`. In
  `unrecognized arguments:` (no repr) stderr's `backslashreplace` also shows `\udcff` (observed).
- `prog` is the literal `pre-commit` (`main.py:204`), regardless of `argv[0]`. Subparser prog = `pre-commit <name>`.
  hazmat tools: `pre-commit hazmat <tool>`.

**Terminal width** (`argparse.py:171` with `shutil.get_terminal_size`):
1. `COLUMNS`, parsed with Python `int()` (surrounding whitespace allowed, `_` digit separators, Unicode digits). It is used
   only if > 0. `abc`, `0`, `-5` and unset all fall through. `" 100 "` is accepted (observed).
2. Otherwise `os.get_terminal_size(sys.__stdout__.fileno())`: ioctl `TIOCGWINSZ` on fd 1 on POSIX,
   `GetConsoleScreenBufferInfo` window width on Windows. A reported width of 0 → 80.
3. Otherwise (stdout not a terminal, i.e. piped or redirected) → **80**.
4. Formatter width `W = columns - 2`. The help column is `max_help_position = min(24, max(W - 20, 4))`.

### A.4 Verbatim help (canonical, `COLUMNS=80`, non-TTY)

Byte-identical on CPython 3.13.16 and 3.14.8. The error usage block of each parser (§A.6) is exactly the first paragraph of
its help (all lines before the first blank line) plus `\n`. The `--color` help text embeds the **raw** env value:
``Defaults to `bogus`.`` when `PRE_COMMIT_COLOR=bogus`, ```Defaults to ``.``` when it is empty. It is shown even though
that value is invalid, because help exits before the default is converted (observed). `-c` never shows its default
(its help string has no `%(default)s`). `-j` shows `(default 1)`.
`hook-impl` is absent from the command list but present in the `{…}` choices (it has no `help=`).

#### `pre-commit -h`  (exit 0, stdout; identical for `--help`, `help`, `help ""`, `-h <anything>`)
```text
usage: pre-commit [-h] [-V]
                  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...

positional arguments:
  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl}
    autoupdate          Auto-update pre-commit config to the latest repos'
                        versions.
    clean               Clean out pre-commit files.
    gc                  Clean unused cached repos.
    hazmat              Composable tools for rare use in hook `entry`.
    init-templatedir    Install hook script in a directory intended for use
                        with `git config init.templateDir`.
    install             Install the pre-commit script.
    install-hooks       Install hook environments for all environments in the
                        config file. You may find `pre-commit install
                        --install-hooks` more useful.
    migrate-config      Migrate list configuration to new map configuration.
    run                 Run hooks.
    sample-config       Produce a sample .pre-commit-config.yaml file
    try-repo            Try the hooks in a repository, useful for developing
                        new hooks.
    uninstall           Uninstall the pre-commit script.
    validate-config     Validate .pre-commit-config.yaml files
    validate-manifest   Validate .pre-commit-hooks.yaml files
    help                Show help for a specific command.

options:
  -h, --help            show this help message and exit
  -V, --version         show program's version number and exit
```

#### `pre-commit help -h`  (exit 0, stdout; identical for `help --help`, `help help`)
```text
usage: pre-commit help [-h] [help_cmd]

positional arguments:
  help_cmd    Command to show help for.

options:
  -h, --help  show this help message and exit
```

#### `pre-commit autoupdate -h`  (exit 0, stdout; identical for `autoupdate --help`, `help autoupdate`)
```text
usage: pre-commit autoupdate [-h] [--color {auto,always,never}] [-c CONFIG]
                             [--bleeding-edge] [--freeze] [--repo REPO]
                             [-j JOBS]

options:
  -h, --help            show this help message and exit
  --color {auto,always,never}
                        Whether to use color in output. Defaults to `auto`.
  -c, --config CONFIG   Path to alternate config file
  --bleeding-edge       Update to the bleeding edge of `HEAD` instead of the
                        latest tagged version (the default behavior).
  --freeze              Store "frozen" hashes in `rev` instead of tag names
  --repo REPO           Only update this repository -- may be specified
                        multiple times.
  -j, --jobs JOBS       Number of threads to use. (default 1).
```

#### `pre-commit clean -h`  (exit 0, stdout; identical for `clean --help`, `help clean`)
```text
usage: pre-commit clean [-h] [--color {auto,always,never}]

options:
  -h, --help            show this help message and exit
  --color {auto,always,never}
                        Whether to use color in output. Defaults to `auto`.
```

#### `pre-commit gc -h`  (exit 0, stdout; identical for `gc --help`, `help gc`)
```text
usage: pre-commit gc [-h] [--color {auto,always,never}]

options:
  -h, --help            show this help message and exit
  --color {auto,always,never}
                        Whether to use color in output. Defaults to `auto`.
```

#### `pre-commit hazmat -h`  (exit 0, stdout; identical for `hazmat --help`, `help hazmat`)
```text
usage: pre-commit hazmat [-h] [--color {auto,always,never}]
                         {cd,ignore-exit-code,n1} ...

positional arguments:
  {cd,ignore-exit-code,n1}
    cd                  cd to a subdir and run the command
    ignore-exit-code    run the command but ignore the exit code
    n1                  run the command once per filename

options:
  -h, --help            show this help message and exit
  --color {auto,always,never}
                        Whether to use color in output. Defaults to `auto`.
```

#### `pre-commit init-templatedir -h`  (exit 0, stdout; identical for `init-templatedir --help`, `help init-templatedir`)
```text
usage: pre-commit init-templatedir [-h] [--color {auto,always,never}]
                                   [-c CONFIG] [--no-allow-missing-config]
[[RECOVERY GAP: 339 original line(s) 261-599 could not be recovered after the /tmp wipe]]
  -h, --help  show this help message and exit
```

#### A.4.1 CPython 3.12.x rendering deltas (the 4.6.2 oracle `venv462` is 3.12.3)

All other lines are identical at `COLUMNS=80`.

| Location | 3.12.x | Canonical (3.13+) |
|---|---|---|
| top-level usage (also in every top-level error) | line 2 = `{autoupdate,…,hook-impl}`, line 3 = `...` (both indented 18) | line 2 = `{autoupdate,…,hook-impl} ...` (indented 18) |
| `-c/--config` (every command that has it) | `-c CONFIG, --config CONFIG` (indent 2), help on the next line at column 24 | `-c, --config CONFIG` + 3 spaces + `Path to alternate config file` (help at column 24) |
| `-j/--jobs` | `-j JOBS, --jobs JOBS` + 2 spaces + help | `-j, --jobs JOBS` + 7 spaces + help |
| `-t/--hook-type` | `-t {…}, --hook-type {…}` (no help text) | `-t, --hook-type {…}` |
| `--from-ref` | `--from-ref FROM_REF, --source FROM_REF, -s FROM_REF` | `--from-ref, --source, -s FROM_REF` |
| `--to-ref` | `--to-ref TO_REF, --origin TO_REF, -o TO_REF` | `--to-ref, --origin, -o TO_REF` |
| try-repo `--ref` | `--ref REF, --rev REF` + 2 spaces + help | `--ref, --rev REF` + 6 spaces + help |
| try-repo usage lines 2-4 | `[--ref REF] [--verbose]` / `[--all-files \| --files [FILES ...]]` / `[--show-diff-on-failure] [--fail-fast]` | `[--ref REF] [--verbose] [--all-files \|` / `--files [FILES ...]] [--show-diff-on-failure]` / `[--fail-fast]` |

### A.5 `help` subcommand and help-like argv (`main.py:350-354, 372-375`)

- The `help` parser has **no `--color`** and an optional positional `help_cmd`. After a successful parse:
  `help_cmd` truthy → `parser.parse_args([help_cmd, '--help'])`; otherwise → `parser.parse_args(['--help'])`. This is a real
  re-parse, so every outcome follows from the normal rules (all observed on 3.13.16):

| argv | Result |
|---|---|
| `help`, `help ""` | top-level help, exit 0 (an empty `help_cmd` is falsy) |
| `help run`, `help -- run`, `help hook-impl`, `help hazmat`, `help help` | that command's help, exit 0 (hidden `hook-impl` included) |
| `help bogus`, `help ru` | top-level usage + `pre-commit: error: argument command: invalid choice: 'bogus' (choose from …)`, exit 2 |
| `help run extra`, `help hazmat cd` | `pre-commit: error: unrecognized arguments: extra` / `…: cd`, exit 2. The first parse fails; `help` takes one positional |
| `help --color never`, `help --bogus` | `pre-commit: error: unrecognized arguments: --color` / `--bogus`, exit 2 |
| `help -- --bogus` | top-level help, exit 0: re-parse of `['--bogus','--help']`, where `--help` exits before extras are reported |
| `help -- -V` | `pre-commit 4.6.2`, exit 0 |
| `PRE_COMMIT_COLOR=bogus help run` | run help (with ``Defaults to `bogus`.``), exit 0 |

- Help-like argv elsewhere: `-h <anything>` → top-level help. `--he`, `--hel` (prefixes) → help. `--v`, `--vers` → version.
  `--version run --bogus` → version, exit 0. `run -h --bogus` and `run --bogus -h` → run help, exit 0 (the unknown option is
  only *collected*). `run --hook-stage bogus -h` → exit 2 (the error comes first). `run --hel` → run help. `run --ver -a` =
  `--verbose` (top-level `--version` is not visible inside `run`). `run -V` / `run --version` / `run --vers` →
  `pre-commit: error: unrecognized arguments: -V` (or the respective token), exit 2.
- No argv at all → `['run']` (`main.py:368-369`). Outside a repo:
  `An error has occurred: FatalError: git failed. Is it installed, and are you in a Git repository directory?` +
  `Check the log at <PRE_COMMIT_HOME>/pre-commit.log` on **stdout**, exit **1**. The store dir, `README`, `db.db` and
  `pre-commit.log` are created first.

### A.6 Error routing (canonical strings; exit 2 and stderr unless stated)

Notation: `U(top)` = top-level usage block (§A.4, first paragraph); `U(<cmd>)` = that subparser's usage block. Every message
below is the **last line of stderr**, preceded by the usage block named in the subsection heading (or in the row). Choices
appear in declaration order.

#### A.6.1 Reported by the top-level parser (prog `pre-commit`, `U(top)`)

| # | Trigger (observed argv) | Last stderr line |
|---|---|---|
| T1 | unknown option at top level: `--bogus`, `-v`, `---` | `pre-commit: error: unrecognized arguments: --bogus` |
| T2 | unknown option in **any** subparser or hazmat tool, collected and reported at the end: `run --bogus`, `run --bogus=1`, `run -x`, `validate-config --colour never`, `hazmat cd --foo sub`, `hazmat n1 -n echo -- a`, `hazmat n1 --color never echo -- a` | `pre-commit: error: unrecognized arguments: --bogus=1` (raw tokens, **no repr**, joined by one space, in argv order across all levels) |
| T3 | several unknown tokens: `run --bogus -y zz` | `pre-commit: error: unrecognized arguments: --bogus -y` (`zz` became the `hook` positional) |
| T4 | extra positionals: `run a b`, `sample-config extra`, `clean extra`, `help run extra`, `validate-config a --color never b` | `pre-commit: error: unrecognized arguments: b` |
| T5 | leftover from a short-flag cluster: `run -v1`, `run -all-files`, `run -c=x.yaml -v1` | `pre-commit: error: unrecognized arguments: -1`, `… -ll-files` |
| T6 | `--` alone: `pre-commit --` | `pre-commit: error: unrecognized arguments: --` |
| T7 | `-V`/`--version` after a subcommand: `run -V` | `pre-commit: error: unrecognized arguments: -V` |
| T8 | invalid subcommand, including prefixes and option values that land in its slot: `bogus`, `ru`, `sample`, `""`, `--color never run` (`never`), `-c foo.yaml run` (`foo.yaml`), `help ru` | `pre-commit: error: argument command: invalid choice: 'ru' (choose from 'autoupdate', 'clean', 'gc', 'hazmat', 'init-templatedir', 'install', 'install-hooks', 'migrate-config', 'run', 'sample-config', 'try-repo', 'uninstall', 'validate-config', 'validate-manifest', 'help', 'hook-impl')` |

#### A.6.2 Reported by a subparser (prog `pre-commit <cmd>`, `U(<cmd>)`)

| # | Category | Observed argv → last stderr line |
|---|---|---|
| S1 | invalid choice (`--hook-stage`, after `transform_stage`) | `run --hook-stage bogus` → `pre-commit run: error: argument --hook-stage: invalid choice: 'bogus' (choose from 'commit-msg', 'post-checkout', 'post-commit', 'post-merge', 'post-rewrite', 'pre-commit', 'pre-merge-commit', 'pre-push', 'pre-rebase', 'prepare-commit-msg', 'manual')`. Same template for `Manual` (case-sensitive), `--hook-stage=` (`''`), `"it's"` (→ `"it's"`, double quotes by repr) and `$'a\tb\x01'` (→ `'a\tb\x01'`). Legacy `commit`/`push`/`merge-commit` are accepted silently |
| S2 | invalid choice (`-t/--hook-type`) | `install -t bogus` → `pre-commit install: error: argument -t/--hook-type: invalid choice: 'bogus' (choose from 'commit-msg', 'post-checkout', 'post-commit', 'post-merge', 'post-rewrite', 'pre-commit', 'pre-merge-commit', 'pre-push', 'pre-rebase', 'prepare-commit-msg')`. `manual` is rejected the same way |
| S3 | type error, `use_color` | `run --color bogus` → `pre-commit run: error: argument --color: invalid use_color value: 'bogus'` (also `Never`, `--color=` → `''`) |
| S4 | type error, `int` | `autoupdate -j x` → `pre-commit autoupdate: error: argument -j/--jobs: invalid int value: 'x'` (also `--jobs=1.5` → `'1.5'`). `-j " 2"`, `-j 1_0` and `-j -1` are **accepted** (Python `int()`; `-1` is a negative number, so a value, not an option) |
| S5 | missing value | `run --hook-stage` → `pre-commit run: error: argument --hook-stage: expected one argument`; `run -c` → `argument -c/--config: expected one argument`; `run -s` → `argument --from-ref/--source/-s: expected one argument`; `install -t`, `install -ft`, `autoupdate -j`, `autoupdate --repo`, `hook-impl --hook-type`, `run --color` (even with `PRE_COMMIT_COLOR=bogus`) |
| S6 | ambiguous prefix (raw token, `=value` included; candidates in option-string declaration order, **all** option strings of all matching actions) | `run --co x` → `pre-commit run: error: ambiguous option: --co could match --color, --config, --commit-msg-filename, --commit-object-name`; `run --co=never` → `ambiguous option: --co=never could match …`; `run --c never` → `… --c could match --color, --config, --commit-msg-filename, --commit-object-name, --checkout-type`; `run --s x` → `… --s could match --show-diff-on-failure, --source`; `try-repo r --re x` → `… --re could match --ref, --rev, --remote-branch, --remote-name, --remote-url, --rewrite-command` |
| S7 | explicit argument on a flag | `run --verbose=1` → `pre-commit run: error: argument --verbose/-v: ignored explicit argument '1'`; `--all-files=` → `argument --all-files/-a: ignored explicit argument ''`; `-a=1` → `… '1'`; `-a-v` → `… '-v'`; `hook-impl --hook-type x --skip=1` → `argument --skip-on-missing-config: ignored explicit argument '1'` |
| S8 | mutually exclusive (named in **argv order**: the later one is "argument", the earlier one is "not allowed with") | `run -a --files x` → `pre-commit run: error: argument --files: not allowed with argument --all-files/-a`; `run --files x -a`, `run --files -a`, `run --files a --files b -a` → `argument --all-files/-a: not allowed with argument --files`; `try-repo r -a --files x` → `pre-commit try-repo: error: argument --files: …`. A bare `--files` (zero values) still counts |
| S9 | required missing | `init-templatedir` → `pre-commit init-templatedir: error: the following arguments are required: directory`; `try-repo` → `…: repo`; `hook-impl` and `hook-impl -- --hook-type=post-commit` → `pre-commit hook-impl: error: the following arguments are required: --hook-type` |
| S10 | env default conversion (only if `--color` absent) | `PRE_COMMIT_COLOR=bogus sample-config` → `pre-commit sample-config: error: argument --color: invalid use_color value: 'bogus'`; with `=` → `''`, `ALWAYS` → `'ALWAYS'`, `"never "` → `'never '`. Bare `pre-commit` → `pre-commit run: error: …` (with `U(run)`) |

#### A.6.3 hazmat (prog `pre-commit hazmat` / `pre-commit hazmat <tool>`)

| Argv | Result |
|---|---|
| `hazmat bogus`, `hazmat ignore` (no prefix matching) | `U(hazmat)` + `pre-commit hazmat: error: argument tool: invalid choice: 'bogus' (choose from 'cd', 'ignore-exit-code', 'n1')` |
| `hazmat cd` | `U(hazmat cd)` = `usage: pre-commit hazmat cd [-h] subdir ...` + `pre-commit hazmat cd: error: the following arguments are required: subdir` (≤3.12.6 and 3.13.0: `subdir, cmd`) |
| `PRE_COMMIT_COLOR=bogus hazmat n1 echo -- a` | `U(hazmat)` + `pre-commit hazmat: error: argument --color: invalid use_color value: 'bogus'` (after the tool parser has succeeded) |
| `PRE_COMMIT_COLOR=bogus hazmat cd` | the `cd` required error wins (the tool parser finishes first) |
| `PRE_COMMIT_COLOR=bogus hazmat cd --help`, `hazmat ignore-exit-code -h` | help, exit 0 |
| `hazmat --color never n1 echo -- a` | OK: `a` on stdout, exit 0 (`--color` belongs to `hazmat`, before the tool) |
| `hazmat` (no tool) | stdout `An unexpected error has occurred: AttributeError: 'Namespace' object has no attribute 'cmd'` + `Check the log at …`, exit **3** (`hazmat.py:75` runs before the tool check) |
| `hazmat cd sub`, `hazmat cd sub -h` | stderr ``hazmat entry must end with `--` ``, exit **1**, no log (`-h` became `cmd`) |
| `hazmat n1 -- echo -- a` | `cmd=['--','echo','--','a']` → stdout ``An unexpected error has occurred: ExecutableNotFoundError: Executable `--` not found``, exit 3 |
| `hazmat ignore-exit-code -- false` | same, ``Executable `--` not found``, exit 3 |
| `hazmat cd . definitely-not-a-cmd-xyz --` | ``An unexpected error has occurred: ExecutableNotFoundError: Executable `definitely-not-a-cmd-xyz` not found``, exit 3 |
| `hazmat cd -- sub echo --` | `subdir='sub'` (the first `--` is stripped for a non-REMAINDER positional) → `An unexpected error has occurred: FileNotFoundError: [Errno 2] No such file or directory: 'sub'`, exit 3 |
| `hazmat n1 echo -- a b` / `hazmat n1 echo -n -- a b` | `a\nb\n` / `ab` (no newline), exit 0 |

#### A.6.4 Errors after a successful parse (not argparse; for completeness)

| Source | Stream | Text | Exit |
|---|---|---|---|
| FatalError (not a git repo, invalid config, …) | stdout | `An error has occurred: <Type>: <msg>` + `Check the log at <store>/pre-commit.log` | 1 |
| other exception (`AssertionError: unexpected hook type: bogus` from `hook-impl --hook-type=bogus --`, `NotImplementedError`, `ExecutableNotFoundError`, …) | stdout | `An unexpected error has occurred: <Type>: <msg>` + log line | 3 |
| `SystemExit(str)`: hazmat (``hazmat entry must end with `--` ``, `unexpected file without prefix='sub/': <f>`), hook-impl (`hook-impl for post-commit expected 0 arguments but got 2: ['a', 'b']`, `… expected 1 argument but got 0: []`, `… expected 1, 2, or 3 arguments …`, `… expected 1 or 2 arguments …`) | **stderr** | message + `\n` | 1 |
| KeyboardInterrupt | stdout | `Interrupted (^C): KeyboardInterrupt: ` + log line | 130 |

The **hook-entry path** of hazmat (`entry: pre-commit hazmat …`, rewritten to `<sys.executable> -m pre_commit.commands.hazmat`)
has no `error_handler`. Observed inside `pre-commit run` (the hook's captured output):
- `entry: pre-commit hazmat bogus` → `- exit code: 2`, followed by
  `usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...` and
  `hazmat.py: error: argument tool: invalid choice: 'bogus' (choose from 'cd', 'ignore-exit-code', 'n1')`. On 3.14+ the prog
  is `python -m pre_commit.commands.hazmat` (basename of `sys.executable`).
- `entry: pre-commit hazmat n1 definitely-not-xyz --` → `- exit code: 1`, a full Python traceback ending
  ``pre_commit.parse_shebang.ExecutableNotFoundError: Executable `definitely-not-xyz` not found``.
- `entry: pre-commit hazmat n1 echo` (no `--`) → `- exit code: 1`, ``hazmat entry must end with `--` ``.
- The standalone parser has **no `--color`**: `hazmat --color never n1 true --` → `hazmat.py: error: argument tool: invalid choice: 'never' …`,
  exit 2. `PRE_COMMIT_COLOR` has no effect on this path.

### A.7 Message catalogue and formatting rules

Templates (`argparse.py` 3.13.16, line in parentheses):

| Id | Template | Raised by |
|---|---|---|
| M1 | `unrecognized arguments: %s` — `' '.join(tokens)`, tokens verbatim (1928) | top-level `parse_args`, after everything else succeeded |
| M2 | `argument %(name)s: invalid choice: %(value)r (choose from %(choices)s)`; choices = `', '.join(repr(str(c)))` (2596-2606) | owner parser; `name` per M-name; for subparsers `name` = dest (`command`, `tool`) |
| M3 | `argument %(name)s: invalid %(type)s value: %(value)r`; `type` = `__name__` of the converter (`use_color`, `int`) (2571-2594) | owner parser |
| M4 | `argument %(name)s: expected one argument` (2321) | owner parser |
| M5 | `ambiguous option: %(option)s could match %(matches)s`; `option` = raw argv token; `matches` = `', '.join(option strings)` (2047-2055) | owner parser |
| M6 | `argument %(name)s: ignored explicit argument %r` (2085, 2117) | owner parser |
| M7 | `argument %(name)s: not allowed with argument %(other)s` (2024-2037) | owner parser |
| M8 | `the following arguments are required: %s` — `', '.join(names)` in declaration order (2262-2263) | owner parser, at end of its parse |
| M9 | `%(prog)s: error: %(message)s\n` after `format_usage()` (2675-2686) | always |

- **Action name (M-name, `_get_action_name`, 728-746):** options → `'/'.join(option_strings)` in declaration order:
  `--verbose/-v`, `--all-files/-a`, `-c/--config`, `-t/--hook-type`, `-j/--jobs`, `--from-ref/--source/-s`,
  `--to-ref/--origin/-o`, `--ref/--rev`, `-f/--overwrite`, `--files`, `--color`, `--hook-stage`, `--repo`,
  `--hook-type` (hook-impl), `--skip-on-missing-config`. Positionals → `dest` (`directory`, `repo`, `subdir`).
  Subparser actions → `dest` (`command`, `tool`).
- **`%r` = Python `str.__repr__`:** use `"` if the string contains `'` and no `"`, else `'`. Escape `\` → `\\`, the chosen
  quote, `\t` `\n` `\r`. Non-printable characters (Python `isprintable()` false: categories Cc, Cf, Cs, Co, Cn, Zl, Zp and Zs
  other than U+0020) become `\xhh` (< 0x100), `\uhhhh` (< 0x10000) or `\Uhhhhhhhh`, in lowercase hex. Printable non-ASCII
  stays literal (`'é'`). Surrogate-escaped bytes → `\udc80`–`\udcff`.
  Observed: `'it\'s "q"'` (contains both quote types, so `'` is used and escaped), `"it's"`, `'a\tb\x01'`, `'\udcff'`.
- M1 tokens are **not** repr'd. Non-UTF-8 bytes there print as `\udcXX` (stderr `backslashreplace`).

### A.8 Parse semantics (the contract snp's parser must implement)

#### A.8.1 Parser tree (declaration order matters: help listing, usage, ambiguity lists, `required` lists, default conversion)

```
pre-commit                         [-h/--help] [-V/--version] {command: PARSER, dest=command, required=False}
  autoupdate        --color  -c/--config  --bleeding-edge  --freeze  --repo(append, metavar REPO)  -j/--jobs(int, default 1)
  clean             --color
  gc                --color
  hazmat            --color  {tool: PARSER, dest=tool, required=False}
     cd             subdir(1)  cmd(REMAINDER)
     ignore-exit-code cmd(REMAINDER)
     n1             cmd(REMAINDER)
  init-templatedir  --color  -c/--config  directory(1)  --no-allow-missing-config(store_false→allow_missing_config)  -t/--hook-type(append, choices HOOK_TYPES)
  install           --color  -c/--config  -f/--overwrite  --install-hooks  -t/--hook-type  --allow-missing-config
  install-hooks     --color  -c/--config
  migrate-config    --color  -c/--config
  run               --color  -c/--config  hook(?)  --verbose/-v  [mutex: --all-files/-a | --files(*)]  --show-diff-on-failure
                    --fail-fast  --hook-stage(type transform_stage, choices STAGES, default 'pre-commit')  --remote-branch
                    --local-branch  --from-ref/--source/-s  --to-ref/--origin/-o  --pre-rebase-upstream  --pre-rebase-branch
                    --commit-msg-filename  --prepare-commit-message-source  --commit-object-name  --remote-name
                    --remote-url  --checkout-type  --is-squash-merge  --rewrite-command
  sample-config     --color
  try-repo          --color  -c/--config  repo(1)  --ref/--rev  <all run options, same order as run, starting at hook(?)>
  uninstall         --color  -c/--config  -t/--hook-type
  validate-config   --color  filenames(*)
  validate-manifest --color  filenames(*)
  help              help_cmd(?)                                   (no --color)
  hook-impl         --color  -c/--config  --hook-type(1, REQUIRED, free string)  --hook-dir  --skip-on-missing-config  rest(REMAINDER)
```
Every subparser also has `-h/--help` first (added implicitly). The help texts are in §A.4. The `--color` default is the raw
string `os.environ.get('PRE_COMMIT_COLOR', 'auto')`, read when the parser is built. The `-c` default is
`.pre-commit-config.yaml`. The `--files` default is `[]`. `--repo` defaults to `[]`; `-t` defaults to `None` (append).

#### A.8.2 Algorithm (one parser; recursion via the PARSER positional). Equivalent to `argparse.py:1938-2280` (3.13.16)

1. **Defaults.** Initialise every dest with its raw default (strings are *not* converted yet).
2. **Classify** each token (1993-2016 and `_parse_optional` 2352-2396), left to right. The first `--` → `-`, and every later token → `A`
   (including further `--`). Otherwise the token is `A` (positional) if it is empty, does not start with `-`, is exactly `-`,
   contains a space (and matched no option), or is a negative number. Negative number: `^-\d+$|^-\d*\.\d+$` (3.14:
   `^-\.?\d`) and the parser has no option that looks like a negative number (none in pre-commit). Anything else is `O` and
   gets one or more candidate tuples `(action|None, option_string, sep, explicit_arg)`:
   - exact match of the whole token → that option;
   - `--opt=value` / `-c=value` where the part before the first `=` is an exact option → explicit arg = after `=`;
   - two-dash token (`--…`): **prefix match** of the part before `=` against every option string of this parser
     (`_get_option_tuples`, 2398-2440). There is one tuple per matching option string, so an action with two matching
     aliases yields two tuples (`--re` → `--ref, --rev, …`);
   - single-dash token (`-xyz…`): the option whose string equals the first two chars, with explicit arg = the rest (`-cfoo`,
     `-ftbogus`, `-v1`), **plus** every option string that starts with the part before `=` (only multi-char single-dash
     options could match; pre-commit has none);
   - no candidates → `[(None, token)]` = unknown option.

   Each parser classifies **its own slice** of argv against **its own** option table. The top-level parser therefore also
   classifies the tokens that later go to a subparser (as `O` or `A`). This never raises at top level: the only top-level
   long options are `--help` and `--version`, so no prefix can be ambiguous there. The PARSER positional pattern takes `O`
   tokens anyway.
3. **Consume** alternately (2183-2216): while an `O` remains at or after the cursor:
   a. If positional tokens precede the next `O`, run **consume_positionals** (step 5) on the pattern from the cursor. If it
      consumed anything, continue the loop.
   b. Tokens that no positional could take go to **extras** (unknown positionals).
   c. **consume_optional** at the `O`:
      - more than one tuple → **M5 ambiguous** (raised *now*);
      - `None` action → token to **extras**, next;
      - explicit arg present: a flag (nargs 0) given as a single-dash cluster (`-av`) → apply the flag, then look up
        `-<next char>`. If that exists, continue with the rest (a following `=` marks an explicit arg; an empty rest → no
        explicit arg). If not, append `-<rest>` to **extras** and stop (`-v1` → extras `-1`; `-all-files` → `-a`, extras
        `-ll-files`). A flag with `=x`, or whose rest starts with `-` → **M6**. A one-value option takes the explicit arg
        (even `''`). `nargs='*'` (`--files=a`) takes **exactly** that one value. Otherwise → **M6**.
      - no explicit arg: nargs `None` needs exactly one following `A` (else **M4**). `*` takes all consecutive `A`s (zero
        allowed; stops at `O` or `-`). 0 takes none.
      - **take_action**: convert each value (type → choices; **M3/M2**, raised now). Mutex check: the action counts as
        "present" if it is an option or got ≥1 string. A present conflicting action → **M7** naming *this* action and the
        earlier one. Then store / append / store_true / store_false / help (print, **exit 0 now**) / version (print,
        **exit 0 now**) / subparser invocation.
4. After the last `O`: consume_positionals once more. Everything left over goes to **extras**.
5. **consume_positionals** (`_match_arguments_partial`, 2335-2350). Greedily fit the remaining positionals, left to right,
   with the regexes below, and drop positionals from the right until the whole prefix matches. If the match stops right
   before an `O`, drop **trailing zero-length** matches so those positionals stay available for later (this is why
   `try-repo r -a myhook` works on ≥3.12.7 and ≥3.13.1). For each matched positional: strip the first `--` from its strings unless it is
   REMAINDER. A PARSER strips `--` only if it is its first token. Then take_action.

   | nargs | positional regex over pattern chars | used by |
   |---|---|---|
   | 1 (None) | `-*A-*` | `directory`, `repo`, `subdir` |
   | `?` | `-*A?-*` | `hook`, `help_cmd` |
   | `*` | `-*[A-]*` | `filenames` |
   | REMAINDER | `.*` (takes `O` and `-` too, **keeps** `--`) | `rest`, `cmd` |
   | PARSER | `-*A[-AO]*` | `command`, `tool` |

6. **PARSER (subcommand) action:** the first value is the name. Exact match only, so `ru` → **M2** with name
   `command`/`tool`. Set `dest=name`, run that subparser's *full* parse (steps 1–8) on the remaining values, copy its results
   into the namespace, and append its **extras** to a shared "unrecognized" list. Subparser errors exit immediately with
   *the subparser's* prog and usage.
7. **End of parse** (2242-2263), in **action declaration order**: for each action never taken, either collect it as required
   (→ names list) or, if its default is a `str` still identical to the stored default, **convert it now** with type (and
   *not* choices). This is where `PRE_COMMIT_COLOR` is validated (**M3**, raised immediately; `--color` is declared before
   every required positional/option, so it wins over M8). Then, if any required were collected → **M8**.
8. **Top level only** (`parse_args`, 1925-1933): extras of all levels (top-level ones first, in consumption order, then those
   appended by subparsers) → **M1** with the top-level prog and usage.

Precedence examples (observed, canonical):

| argv (env) | Winner |
|---|---|
| `run --bogus --hook-stage bogus` | S1 (unknown only collected) |
| `--bogus run --hook-stage bogus` | S1 (`pre-commit run:` prog), not T1 |
| `run -a --files x --hook-stage bogus` | S8 |
| `run --hook-stage bogus -a --files x` | S1 |
| `run --hook-stage bogus --co x` | S1 (≤3.12.6 and 3.13.0: S6, raised at classification) |
| `run -h --co x` | help, exit 0 (≤3.12.6 and 3.13.0: S6) |
| `run --hook-stage bogus -h` | S1 |
| `install -t bogus -t` | S2 |
| `autoupdate -j x -j 2`, `run --color bogus --color never` | first occurrence's M3 |
| `try-repo --hook-stage bogus` | S1 beats S9 |
| `PRE_COMMIT_COLOR=bogus run --bogus` | S10 (`pre-commit run:`), beats T2 |
| `PRE_COMMIT_COLOR=bogus run -a --files x` | S8 (raised during consumption, before end-of-parse) |
| `PRE_COMMIT_COLOR=bogus run --hook-stage bogus` | S1 |
| `PRE_COMMIT_COLOR=bogus try-repo` | S10 beats S9 |
| `PRE_COMMIT_COLOR=bogus run --color` | S5 `argument --color: expected one argument` |

#### A.8.3 Value semantics

- Repeated store options: the **last wins** (`--color`, `-c`, `--hook-stage`, `--files a --files b` → `['b']`, observed).
  Append options accumulate (`-t`, `--repo`), duplicates kept. Flags are idempotent.
- `--files`: `--files a.txt hook` swallows `hook` (files=`[a.txt, hook]`). `--files=a.txt hook` → files=`[a.txt]`,
  hook=`hook`. `--files a.txt -- hook` → files=`[a.txt]`, hook=`hook`. `--files -- hook` → `[]`, hook. `--files -1 -a` → files
  `['-1']`, then S8. `run -- echo --files a.txt` → hook `echo`, then `unrecognized arguments: --files a.txt`.
  `run --files a.txt -- echo extra` → `unrecognized arguments: extra`. All identical on every version (observed, two-hook
  repo).
- `hook` (`?`): `run -- -a` → hook `-a` → ``No hook with id `-a` in stage `pre-commit` `` (stdout), exit 1. `run "--all files"` → hook
  `--all files` (positional because of the space). `run -` → hook `-`.
- `--hook-stage`: `transform_stage` maps `commit`→`pre-commit`, `merge-commit`→`pre-merge-commit`, `push`→`pre-push`, then
  the choices check runs on the **transformed** value (the error shows the transformed value, which only differs for those
  three legal names).
- `use_color(s)`: valid iff `s ∈ {auto, always, never}` (case-sensitive, no trimming). Result: bool (`color.py:84-100`).
- `int` (for `-j`): Python `int(str)`. Strip Unicode whitespace; optional `+`/`-`; decimal digits (any Unicode `Nd`, e.g.
  `٣`) with single `_` only *between* digits; leading zeros allowed; at most 4300 digits (`sys.int_info.str_digits_check_threshold`,
  else ValueError → M3). `1.5`, `0x10`, `1e3` → M3. Values are unbounded in Python but are clamped by `autoupdate` (`min`
  with the repo count, `max(…, 1)`, `0` → cpu count), so snp may saturate to `i64`.
- `hook-impl rest`: the code uses `rest[1:]`. Observed (all versions identical):

| argv after `hook-impl --hook-type=post-commit` | `rest` | args used |
|---|---|---|
| `-- a b` | `['--','a','b']` | `['a','b']` |
| `a b` | `['a','b']` | `['b']` |
| `-- -h` | `['--','-h']` | `['-h']` (no help) |
| `-- --color never` | `['--','--color','never']` | `['--color','never']` |
| `-- -- x` | `['--','--','x']` | `['--','x']` |
| `x --hook-dir y` | `['x','--hook-dir','y']` | `['--hook-dir','y']` (REMAINDER swallows later options) |
| `-h` | — | hook-impl help, exit 0 (option before REMAINDER) |
| `--` | `['--']` | `[]` |
| (`hook-impl -- --hook-type=x`) | `['--','--hook-type=x']` | S9 required `--hook-type` |

  `--hook-type commit-msg --hook-dir .git/hooks .git/COMMIT_EDITMSG` (no `--`) → `hook-impl for commit-msg expected 1 argument but got 0: []`, exit 1.
- hazmat `cmd`: `cd sub -h` → `['-h']`; `cd sub --foo x --` → `['--foo','x','--']` (observed:
  ``Executable `--foo` not found``, exit 3); `n1 -n echo -- a` → **unknown `-n`**
  (an option *before* the first REMAINDER value is consumed as an option); `n1 -- echo -- a` → `['--','echo','--','a']`;
  `ignore-exit-code -h` → help.

### A.9 `PRE_COMMIT_COLOR` — resolved semantics (observed, all versions)

| Env | argv | Exit | Output |
|---|---|---|---|
| `bogus` | `sample-config --color never` / `--color=always` | 0 | normal |
| `bogus` | `sample-config`, `run`, `hook-impl --hook-type=pre-commit --`, `try-repo` (missing repo), bare `pre-commit` | 2 | `pre-commit <cmd>: error: argument --color: invalid use_color value: 'bogus'` |
| `""` (set, empty) | `sample-config` | 2 | `… invalid use_color value: ''` |
| `ALWAYS`, `"never "` | `sample-config` | 2 | `'ALWAYS'`, `'never '` |
| `bogus` | `-V`, `-h`, `help`, `help run`, `run --help`, `validate-config -h`, `hazmat cd --help` | 0 | help shows ``Defaults to `bogus`.`` |
| `bogus` | `hazmat n1 echo -- a` | 2 | `pre-commit hazmat: error: argument --color: …` |
| `bogus` | in a hook `entry: pre-commit hazmat …` | — | no effect (the standalone parser has no `--color`) |
| `bogus` | git hook (`hook-impl` from the template, which never passes `--color`) | 2 | every `git commit` aborts with the hook-impl usage + error |
| `always` | `sample-config` | 0 | (sample-config prints plain text) |

Non-UTF-8 values are invalid too: `os.environ` uses surrogateescape, so the error shows `'\udcff'`. pre-commit ignores
`NO_COLOR` and `FORCE_COLOR`. Only CPython 3.14+ argparse reads them, for colouring its own usage and help.

### A.10 hazmat exit codes — resolved

| Path | argparse error | `SystemExit("…")` | normalize_cmd / OS error | no tool |
|---|---|---|---|---|
| CLI: `pre-commit hazmat …` (main.py, inside `error_handler`) | 2, `pre-commit hazmat[ <tool>]:` prefix | 1 (stderr) | **3** (`An unexpected error has occurred: ExecutableNotFoundError: …` + log) | 3 (`AttributeError`) |
| Hook entry: `pre-commit hazmat …` → `python -m pre_commit.commands.hazmat` | 2, prog `hazmat.py` (≤3.13) / `python -m pre_commit.commands.hazmat` (3.14+) | 1 | **1** (uncaught traceback, no log) | 1 (traceback) |

03 §15's "surface as exit 3" holds only for the CLI path. The documented use (in `entry`) gives exit 1.

### A.11 Help-formatter algorithm (for widths other than 80; port of `HelpFormatter`, `argparse.py:155-726`, and `textwrap`)

State: `W` (§A.3), `MHP = min(24, max(W-20, 4))`, `indent` (0 at top, +2 per section, +2 for sub-actions).

1. **Usage** (`_format_usage`, 298-405). `prefix = "usage: "`. Parts, in declaration order with **all optionals first,
   then positionals**:
   - an option → `[` + its *first* option string + (` ` + args if it takes a value) + `]` (no brackets if required:
     `--hook-type HOOK_TYPE`);
   - a mutex group of adjacent actions → first part gets `[`, the last part gets `]`, every part but the last gets ` |`
     appended, and the inner brackets are dropped: `[--all-files |`, `--files [FILES ...]]`;
   - positionals: `dest` (nargs 1), `[dest]` (`?`), `[dest ...]` (`*`), `...` (REMAINDER), `{choices} ...` (PARSER, one part).
   Args strings: nargs 1 → `METAVAR`; `*` → `[METAVAR ...]`. METAVAR = explicit metavar (`{auto,always,never}`, `REPO`) else
   `{c1,c2,…}` if choices, else `DEST.upper()`.
   `full = prog + " " + parts.join(" ")`. If `len(prefix) + len(full) <= W` → one line. Otherwise, with
   `get_lines(parts, indent, prefix?)` = greedy fill where a part starts a new line iff `line_len + 1 + len(part) > W` and the
   line is non-empty (start `line_len = len(prefix)-1` if a prefix is given, else `len(indent)-1`; after each part
   `line_len += len(part)+1`; lines are `indent + parts.join(" ")`; with a prefix the first line drops its indent):
   - if `len(prefix)+len(prog) <= 0.75*W`: `indent = len(prefix)+len(prog)+1` spaces; lines =
     `get_lines([prog]+opt_parts, indent, prefix)` + `get_lines(pos_parts, indent)` (positionals **always** start a new
     line). If there are no optionals: `get_lines([prog]+pos_parts, indent, prefix)`.
   - else: `indent = len(prefix)` spaces; if `get_lines(opt+pos, indent)` gives > 1 line, use
     `get_lines(opt) + get_lines(pos)`; prepend a line containing only `prog`.
   Output: `prefix + lines.join("\n") + "\n"`.
2. **Sections:** `positional arguments:` then `options:`, each only if non-empty, with items indented 2. Sub-actions (the
   subcommand choices that have help) are indented 4. help = usage + `\n` + sections joined by `\n`; each section is the
   heading line plus item lines. Final text: runs of 3+ `\n` collapse to 2, trim, one trailing `\n`.
3. **Items:** `action_max_length = max(len(invocation) + its indent)` over **all** items in all sections (sub-actions
   included). `help_position = min(action_max_length + 2, MHP)`. `help_width = max(W - help_position, 11)`.
   `action_width = help_position - indent - 2`.
   - Invocation: positional → metavar/choices/dest (`{autoupdate,…,hook-impl}` for the subparser action, the name for a
     sub-action). Flag → `', '.join(option_strings)`. Valued option → `', '.join(option_strings) + ' ' + ARGS`
     (3.12: `', '.join(f'{o} {ARGS}')`).
   - No help → `indent + invocation + "\n"`. If `len(invocation) <= action_width` →
     `indent + invocation.ljust(action_width) + "  " + line1`. Else `indent + invocation + "\n"` and line1 at
     `help_position`. Further help lines are at `help_position`.
   - Help text: expand `%(default)s` (`str(default)`; `--color` → the raw env string), `%(choices)s`
     (`', '.join(choices)`), `%(prog)s`, `%%`. Collapse ASCII whitespace runs to one space, strip, then
     `textwrap.wrap(text, help_width)`.
4. **textwrap** (`Lib/textwrap.py`, 3.13.16: `_split` 157-195, `_handle_long_word` 197-236, `_wrap_chunks` 238-307,
   `wordsep_re` 74-95). Defaults: `break_long_words=True`, `break_on_hyphens=True`, `drop_whitespace=True`.
   - Chunking regex (Unicode `\w`):
     `( [ws]+ | (?<=[\w!"'&.,?]) -{2,} (?=\w) | [^ws]+? (?: -(?: (?<=[^\d\W]{2}-) | (?<=[^\d\W]-[^\d\W]-)) (?=[^\d\W] -? [^\d\W]) | (?=[ws]|\Z) | (?<=[\w!"'&.,?]) (?=-{2,}\w) ) )`
     with `ws = \t\n\x0b\x0c\r ` (split keeps the delimiters, empty chunks dropped). A hyphen breaks only after ≥2 letters
     (or letter-hyphen-letter) and before letter(-)letter: `Auto-update` → `Auto-` `update`, `pre-commit` → `pre-` `commit`,
     but `--install-hooks` → `--install-` `hooks` (observed `Auto-update pre-` / `commit config to the` at COLUMNS=40).
   - Fill: for each line, drop a leading whitespace chunk (except on the first line). Append chunks while
     `cur_len + len(chunk) <= width`. If the next chunk alone is longer than `width`, split it at
     `space_left = width - cur_len` (minimum 1). Prefer the last `-` before that point, if one exists at index > 0 and is
     preceded by a non-`-` char. Drop a trailing whitespace chunk; emit.
   Verified vectors: `COLUMNS=40` and `COLUMNS=10` `sample-config -h` (below), and `COLUMNS=120` and `COLUMNS=" 100 "` top-level.

```text
$ COLUMNS=40 pre-commit sample-config -h          $ COLUMNS=10 pre-commit sample-config -h
usage: pre-commit sample-config                   usage: pre-commit sample-config
       [-h]                                              [-h]
       [--color {auto,always,never}]                     [--color {auto,always,never}]

options:                                          options:
  -h, --help      show this help                    -h, --help
                  message and exit                    show this
  --color {auto,always,never}                         help
                  Whether to use color                message and
                  in output. Defaults                 exit
                  to `auto`.                        --color {auto,always,never}
                                                      Whether to
                                                      use color
                                                      in output.
                                                      Defaults to
                                                      `auto`.
```

### A.12 clap configuration and custom error renderer (as requested), with verified limits

#### A.12.1 Configuration (clap 4.6.7, builder API; equivalent derive attributes are fine)

```rust
use clap::{Arg, ArgAction, Command, builder::PossibleValuesParser};

fn color() -> Arg {
    Arg::new("color").long("color").value_name("{auto,always,never}")
        .env("PRE_COMMIT_COLOR")          // verified: clap reads env ONLY when the flag is absent
        .default_value("auto")
        .value_parser(PossibleValuesParser::new(["auto", "always", "never"]))
        .hide_possible_values(true).hide_env(true).hide_default_value(true)
}
fn config() -> Arg { Arg::new("config").short('c').long("config").default_value(".pre-commit-config.yaml") }

Command::new("pre-commit")
    .infer_long_args(true)            // argparse allow_abbrev (long options only)
    .infer_subcommands(false)         // argparse never abbreviates subcommand names
    .disable_help_subcommand(true)    // pre-commit's own `help` subcommand
    .disable_help_flag(true).disable_version_flag(true)   // re-add -h/--help (ArgAction::Help) on every
                                                          // command and -V/--version (ArgAction::Version) on the top level only
    .args_override_self(true)         // last occurrence wins
    .subcommand_required(false)       // argv == [] is rewritten to ["run"] BEFORE clap
    .help_template("{usage}")         // never let clap print help; render argparse help ourselves
    // run / try-repo:
    //   Arg "files": .long("files").num_args(0..).action(ArgAction::Set).allow_negative_numbers(true)
    //               .conflicts_with("all_files")
    //   Arg "all_files": .long("all-files").short('a').action(ArgAction::SetTrue)
    //   Arg "hook": positional, optional;  "hook_stage": custom value_parser = transform_stage + choices
    //   Arg "from_ref": .long("from-ref").visible_alias("source").short('s')   (same pattern for to_ref/origin/-o, ref/rev)
    // hook-impl: .hide(true); Arg "rest": .num_args(0..).trailing_var_arg(true).allow_hyphen_values(true)
    // hazmat cd: Arg "subdir" required; Arg "cmd": .num_args(0..).trailing_var_arg(true).allow_hyphen_values(true)
```

#### A.12.2 Error renderer: `clap::Error` → argparse text (always exit 2 to stderr)

The renderer needs the raw argv to find the failing (sub)command path and the raw token, because `clap::Error` carries
neither.

| `ErrorKind` (context) | Render |
|---|---|
| `InvalidSubcommand` (`InvalidSubcommand`) | `U(parent)` + M2 with name `command` (or `tool` under hazmat), value = raw token, choices = all names incl. `hook-impl` |
| `UnknownArgument` (`InvalidArg`) where the raw token's pre-`=` part is a prefix of ≥2 long options of the current command | `U(cmd)` + M5 with the raw token (incl. `=value`) and every matching option string in declaration order |
| `UnknownArgument` otherwise | `U(top)` + M1 with the raw token (**only the first**; see divergences) |
| `InvalidValue` with empty value and the flag present in argv | `U(cmd)` + M4 |
| `InvalidValue` (for `--color`, also from env) | `U(cmd)` + M3 `invalid use_color value: %r` |
| `InvalidValue` / `ValueValidation` (choices args) | M2 with the transformed value |
| `ValueValidation` from the `-j` parser | M3 `invalid int value: %r` |
| `TooManyValues` (flag with `=value`) | M6 with the explicit value |
| `ArgumentConflict` (`InvalidArg`=earlier, `PriorArg`=later — verified for both orders) | M7 `argument <later>: not allowed with argument <earlier>` (names via §A.7) |
| `MissingRequiredArgument` | M8 with names in declaration order |
| `DisplayHelp` / `DisplayVersion` | ignore clap's text; print the argparse help (§A.4/§A.11) of the command path, or `pre-commit 4.6.2`; exit 0 |

#### A.12.3 Residual divergences (each verified with the clap 4.6.7 probe; ✗ = observable difference)

| # | argv (env) | argparse (canonical) | clap 4.6.7 + renderer | Impact |
|---|---|---|---|---|
| C1 | `run --bogus -y zz` | lists `--bogus -y` | stops at first: `--bogus` | ✗ text |
| C2 | `run --bogus --hook-stage bogus` | S1 wins (unknown deferred) | UnknownArgument first | ✗ which error |
| C3 | `PRE_COMMIT_COLOR=bogus run --bogus` | S10 | UnknownArgument | ✗ which error |
| C4 | `PRE_COMMIT_COLOR=bogus run -a --files x` | S8 | env `InvalidValue` first | ✗ which error |
| C5 | `run -a --files x --hook-stage` / `run --files x -a --bogus` | S8 (raised mid-parse) | the value / unknown error wins (conflicts are validated after parsing) | ✗ which error |
| C6 | `hook-impl --hook-type=T -- a b` vs `… a b` | args `['a','b']` vs `['b']` | both `rest=['a','b']` (clap eats `--`) | ✗ **behaviour**; needs a raw-argv check |
| C7 | `hazmat cd sub -h` | `cmd=['-h']` → exit 1 | clap prints help, exit 0 | ✗ **exit code** |
| C8 | `hazmat n1 -n echo -- a`, `hazmat n1 --color never echo -- a` | exit 2 (unknown option) | accepted as `cmd` | ✗ **exit code** |
| C9 | `hazmat n1 -- echo -- a` (same pattern: `hazmat ignore-exit-code -- false`) | `cmd` keeps the leading `--` → exit 3 | `--` stripped (`cmd=['echo','--','a']`), runs `echo` | ✗ **behaviour** |
| C10 | `-- sample-config` | runs (≥3.12.7, ≥3.13.1) | `unexpected argument 'sample-config'` | ✗ **exit code** |
| C11 | `run "--all files"` | hook id `--all files` (exit 1 later) | UnknownArgument, exit 2 | ✗ exit code |
| C12 | `run -all-files`, `run -a=1`, `run -a-v` | `-ll-files` extra / M6 `'1'` / M6 `'-v'` | `-l` / `-=` / `--` unknown | ✗ text |
| C13 | `PRE_COMMIT_COLOR=` | M3 `''` | `InvalidValue` with `''`, indistinguishable from a missing value without an argv check | renderer must special-case |
| C14 | `run --files -1 -a` without `allow_negative_numbers` | files `['-1']` | `-1` unknown | fixed by the setting above |

Kind-level parity is also impossible for `UnknownArgument` versus `InvalidSubcommand` ordering across levels: argparse
checks the subparser *before* reporting top-level unknowns (`--bogus run --hook-stage bogus` → S1).
**Verdict:** clap + renderer reproduces exit code 2 and the single-error message texts. It does **not** reproduce
acceptance in C6–C11 (Tier A) and cannot reproduce precedence C1–C5. So clap does not meet the drop-in bar.

### A.13 Recommended implementation: `snp-cli::pyargparse` (argparse-compatible engine)

- **Scope:** exactly the argparse features pre-commit uses. Actions: `store` (nargs 1 / `*`), `store_true`, `store_false`,
  `append`, `help`, `version`, positional (1 / `?` / `*` / REMAINDER), subparsers (PARSER), one mutex group, `type`
  (`use_color`, `transform_stage`, `int`), `choices`, `required`, `metavar`, `dest`, env-backed string default. About
  900–1200 LOC including the help formatter and the textwrap port, `#![forbid(unsafe_code)]`, **no dependencies**.
- **Data model** (static tables, no allocation for specs):
  ```rust
  pub enum Nargs { Zero, One, Optional, ZeroOrMore, Remainder, Parser }
  pub enum Kind { Store, StoreTrue, StoreFalse, Append, Help, Version,
                  SubParsers(&'static [(&'static str /*name*/, &'static ParserSpec)]) }
  pub enum Conv { Identity, UseColor, Stage, Int }          // Conv::name() -> "use_color" | "int" for M3
  pub enum Default { None, Str(&'static str), Env { var: &'static str, fallback: &'static str }, False, True, Int(i64), EmptyList }
  pub struct ActionSpec { opts: &'static [&'static str], dest: &'static str, nargs: Nargs, kind: Kind, conv: Conv,
                          choices: Option<&'static [&'static str]>, default: Default, required: bool,
                          help: Option<&'static str>, metavar: Option<&'static str>, mutex: Option<u8> }
  pub struct ParserSpec { prog_suffix: &'static str, actions: &'static [ActionSpec], help: Option<&'static str> }
  pub enum Value { Unset, Bool(bool), Str(OsString), List(Vec<OsString>), Int(i64), Sub(&'static str, Box<Namespace>) }
  pub enum Outcome { Parsed(Namespace), Help(ParserPath), Version, Usage { parser: ParserPath, message: String } }
  ```
  Parse `Vec<OsString>` (`std::env::args_os`). Never `String`: `--files` paths and `rest` must round-trip non-UTF-8 bytes.
  Do prefix and `=` splitting on `OsStr::as_encoded_bytes()` (ASCII-only operations, valid on WTF-8 too). `py_repr`
  converts invalid UTF-8 bytes to `\udcXX`; on Windows, unpaired surrogates (from WTF-8) become `\udXXX`.
- **Typed façade:** convert `Namespace` into `enum Cli { Run(RunArgs), TryRepo(..), HookImpl{ rest: Vec<OsString>, .. }, Hazmat(HazmatArgs), Help(Option<OsString>), .. }`.
  Keep `rest` raw and apply `rest[1..]` (saturating) in hook-impl, exactly as `main.py:408`.
- **`help` command:** after `Parsed(Help(cmd))`, call `parse([cmd, "--help"])` or `parse(["--help"])` again. That reproduces
  §A.5 for free.
- **Two hazmat front-ends:** `pre-commit hazmat …` (prog `pre-commit hazmat`, with `--color`, errors → error handler, exit 3)
  and the hook-entry variant used when snp rewrites `entry: pre-commit hazmat …`. The entry variant has no `--color`, uses
  prog `hazmat.py` for Tier-B text, gives exit 1 for exec failures and SystemExit, and exit 2 for usage errors.
- **Rendering:** one `String` per message, one `write_all` to a locked handle. Ignore `ErrorKind::BrokenPipe` and other write
  errors (argparse does), then exit 0 or 2. On Windows, convert `\n` → `\r\n` for argparse, print() and SystemExit text only.
  Width from §A.3: `COLUMNS` via a Python-`int` parser, else the terminal size of **stdout** (use
  `rustix::termios::tcgetwinsize(stdout)` on Unix and `GetConsoleScreenBufferInfo` on Windows, e.g. through a small
  `terminal_size` dependency), else 80.
- **Never colour** usage or help in compat mode, and ignore `NO_COLOR`, `FORCE_COLOR` and `PYTHON_COLORS` for it (3.14 TTY
  colour is Tier B and not worth reproducing).
- **Completions/man pages:** if wanted, an `xtask` builds a `clap::Command` *from the same `ParserSpec` tables* and runs
  `clap_complete`/`clap_mangen` at build time. The runtime binary does not link clap. A unit test asserts that the two
  option sets are equal.

### A.14 Tier classification (per doc 10 §1 / §7.4.5) — proposed update to the "argparse vs clap" normalization row

| Item | Tier | Comparison rule |
|---|---|---|
| Exit code of every argv (0 help/version, 2 usage, 1/3 post-parse) | **A** | exact |
| Stream (help/version → stdout only; usage error → stderr only, stdout empty) | **A** | exact |
| Acceptance and parsed values (every row of §A.8.3, `--files` swallowing, `rest[1:]`, hazmat `cmd`) | **A** | exact, via behaviour |
| `-V` output `pre-commit 4.6.2\n` (compat mode) | **A** | exact (see Q2 of doc 10 and §A.18 Q3) |
| Last stderr line `{prog}: error: {message}` (M1–M8) | **A** against the canonical oracle (CPython ≥3.13.14/≥3.14.5) | exact. Against a 3.12.8–3.12.x / 3.13.1–3.13.13 / 3.14.0–3.14.4 oracle, first normalise `(choose from a, b)` → `(choose from 'a', 'b')`. Against a ≤3.12.6 or 3.13.0 oracle (e.g. `venv462` = 3.12.3), also exempt the old-semantics rows of §A.2 |
| Which error wins when an argv contains several errors (§A.8.2 table) | **B** in difftest | snp implements the canonical precedence, and golden tests assert it exactly |
| Usage block (lines before the error line) | **B** | require the first line to start with `usage: {prog} ` and the line count ≥ 1. Exact at `COLUMNS=80` against the canonical oracle (golden test) |
| `--help` text | **B** | golden-exact against the canonical oracle at `COLUMNS=80` (§A.4). Difftest: the set of option strings and the `usage: {prog}` prefix |
| `SystemExit` messages (hazmat, hook-impl) | **A** | exact (Python list repr `['a', 'b']` via `py_repr`) |
| hazmat entry-path argparse text / traceback | **B** | exit code A; text: last line only |
| Colour in 3.14+ usage/help; 3.15 suggestions | **C** | ignore |
| CRLF on Windows | **A** on Windows runners | exact bytes |

### A.15 Contradictions resolved (evidence: §A.6, §A.9, §A.10, §A.12)

1. **Argparse error prefix:** 02 §2.2 ("Usage errors print `pre-commit run: error: <msg>`") is incomplete. Unrecognized
   arguments from any level go to the top-level parser (`U(top)` + `pre-commit: error: unrecognized arguments: …`). Every
   other parse error uses the owning parser's prog. 10 CLI-07 is correct. 03 §1.1's install example is correct (an S2 row).
2. **`PRE_COMMIT_COLOR`:** 02 §2.2 ("Invalid value or invalid env value → exit 2"), 03 §1.1 ("even from the env var"),
   the parenthetical rule in 10 RUN-59 (the test case itself is right) and the 10 §7.2 `snp-cli` row ("validated also when
   coming from env") are imprecise. The env
   value is converted only if `--color` is absent, at the end of that subparser's parse. It is never converted for `help`,
   `-h`, `-V`, or the hazmat entry path. 02 §21.2 "Validate env values with the same error" must be implemented as §A.8.2
   step 7, **not** eagerly at startup.
3. **hazmat failure exit code:** 03 §15 "surface as exit 3" applies only to the direct CLI. The hook-entry path gives exit 1
   (traceback), exit 2 for argparse errors (prog `hazmat.py` / `python -m …`), and exit 1 for SystemExit. Also new: `hazmat`
   with no tool → exit 3 (`AttributeError`, CLI) or 1 (entry).
4. **07 D9 (clap with `infer_long_args` + a custom error renderer):** insufficient for drop-in parity (§A.12.3 C6–C11 change
   exit codes or behaviour). Replace it with §A.13. clap can stay as a build-time completion generator.
5. **10 Q10 / §7.4.5 "argparse vs clap usage/help" row:** replace it with §A.14. The error line becomes Tier A against a
   canonical-Python oracle, and the usage and help text stay Tier B. Add an oracle venv `pc-4.6.2-py3.13`
   (python-build-standalone `cpython-3.13.16+20261003`) for CLI scenarios. Keep the 3.12 oracle for everything else.
6. **02 §2.3 hook-impl:** "unconditionally drops the first remainder token" is confirmed. The addition is that `-h` after
   `--` is data, and REMAINDER swallows later options (`x --hook-dir y`).
7. The other reported contradictions (hook stdin, YAML error text, regex engine, async runtime, toolchain pin, env-dir naming,
   install-state files, store location, crash log, executable lookup, CPU count, fsmonitor, stash restore, emulation
   granularity, pre-push stdin) do not touch the CLI parsing surface and are left to their owning addenda.

### A.16 Test plan for this surface

- **Goldens** (`crates/snp-cli/tests/golden/*.txt`): every block of §A.4, and every row of §A.5, §A.6, §A.8.2, §A.8.3 and
  §A.9 as `(argv, env) → (exit, stdout, stderr)` at `COLUMNS=80`. Add `COLUMNS ∈ {10, 40, 60, 100, 120, 200}` for
  `-h`, `run -h`, `try-repo -h`, `install -h`, `sample-config -h`, `hazmat cd -h`. Generate them from the canonical oracle
  with `cargo xtask cli-goldens` (driver: `env -i … COLUMNS=N <oracle>/bin/pre-commit <argv>`, stdin `/dev/null`).
- **Differential fuzz (nightly):** random argv from a grammar (option strings, their prefixes, `=` forms, clusters, `--`,
  negative numbers, empty strings, spaces, non-UTF-8 bytes, subcommand names and near-misses), plus random
  `PRE_COMMIT_COLOR`. Compare snp's parse against the oracle's. Oracle driver: a 15-line Python shim that wraps
  `argparse.ArgumentParser.parse_args` to dump the first successful `Namespace` as JSON and `os._exit(0)` before any
  command runs. Errors compare exit code and the last stderr line.
- **Property tests:** the parser never panics (`cargo fuzz` target `pyargparse_parse`); `parse` is deterministic; the help
  formatter never emits a line longer than `max(W, longest unbreakable token + indent)`; textwrap port ≡ Python on random
  ASCII and Unicode text (nightly, oracle).
- **Prefix-stability test:** for every subparser, compute the map *unique prefix → option*. Any snp extension option must
  leave that map unchanged (§A.17 item 6).

### A.17 Implementation notes for snp

1. Rewrite `argv == []` → `["run"]` *before* parsing (`main.py:368-369`), and only for an empty argv.
2. Read `PRE_COMMIT_COLOR` with `env::var_os` when the parser tables are instantiated. Keep it raw. Convert at end-of-parse
   only if `--color` was not consumed. The same raw string feeds `%(default)s` in help.
3. Keep the top-level `-V/--version` and `-h/--help` the only top-level options. `--color` and `-c` before the subcommand must
   keep failing (T8: `--color never run` → `invalid choice: 'never'`).
4. Implement *both* error-timing rules: ambiguity at consumption (canonical) and unknown options deferred to the top level.
   Do not short-circuit on the first unknown option; it is the most user-visible argparse quirk.
5. `hook-impl`: never strip `--` before `rest`; drop `rest[0]` unconditionally. The installed hook scripts always pass
   `--hook-dir "$HERE" -- "$@"` (`resources/hook-tmpl:11`), so this is the hot path of every `git commit`. Parsing must stay
   O(argv) with no regex compilation: precompute pattern decisions; the five positional regexes reduce to simple counting
   rules over the `O/A/-` string.
6. **Extension options (lefthook-inspired flags) must not break existing abbreviations.** Adding a long option that shares a
   currently-unique prefix makes a working command ambiguous (e.g. a new `run --all-hooks` would turn the common
   `pre-commit run --all` into `ambiguous option`). Either use a reserved prefix that no pre-commit option shares
   (e.g. `--snp-…`), or enforce the prefix-stability test (§A.16).
7. Implement `py_repr` once (shared with 02 §21.3 and 09) with the Unicode `isprintable` table pinned to the Unicode version of
   the canonical Python (3.13: Unicode 15.1). Implement Python-`int()` parsing once (shared with `COLUMNS`).
8. Usage and help are computed lazily (only on error/help). Building the tables costs nothing at startup (statics). Target:
   parsing adds < 5 µs and 0 heap allocations beyond the `Vec<OsString>` of values.
9. Exit codes: map `Outcome::Help`/`Version` → 0 and `Usage` → 2, *before* the store is touched. pre-commit creates the store
   only after parsing (`main.py:380`), so `--help` and usage errors leave no `PRE_COMMIT_HOME` side effects.
10. Windows: Rust's `args_os` parses `GetCommandLineW` with MSVC-compatible rules, as the CRT does for Python's `sys.argv`.
    Exotic doubled-quote edge cases may differ between CRT versions (Tier C). Write CRLF as in §A.3. The console width comes
    from the stdout handle, not stderr.

### A.18 Open questions

1. **Canonical Python for argparse wording** (§A.2): confirm CPython ≥ 3.13.14 / ≥ 3.14.5 (quoted choices, 3.13 help layout).
   The alternative is the 3.12.3 oracle (old semantics), which has a different help layout, eager ambiguity, the
   `try-repo REPO -a HOOK` failure and `-- cmd` rejection.
2. **prog when invoked as `snp`:** use `snp` in usage and error lines (Tier B), or always `pre-commit`? Proposal:
   `pre-commit` when `argv[0]`'s file stem is `pre-commit`, else `snp`. The difftest invokes snp through a `pre-commit`
   symlink.
3. **`--version` text:** `pre-commit 4.6.2` (compat, Tier A for scripts that parse it) versus doc 10 Q2's
   `snp X.Y.Z (pre-commit 4.6.2 compatible)`. Proposal: compat mode prints exactly `pre-commit 4.6.2`, and
   `snp --version` (argv[0]=snp) prints the snp string.
4. **Bug-compat depth:** reproduce `hazmat` without a tool → exit 3 with an `AttributeError` text, and the
   hook-entry hazmat traceback (exit 1)? Proposal: exit codes yes (Tier A); texts are short snp equivalents (Tier B).
5. **Entry-path hazmat prog** (`hazmat.py` vs `python -m pre_commit.commands.hazmat` vs `pre-commit hazmat`): Tier B. Pick
   `hazmat.py` (3.12/3.13 oracles) unless the canonical oracle becomes 3.14.
6. **3.15 "maybe you meant" suggestions:** ignore (proposal) until pre-commit pins or 3.15 becomes the common runtime.
7. **EPIPE:** Python exits **120** when flushing stdout fails at interpreter shutdown. Observed:
   `pre-commit hazmat cd . --foo x -- | head -1` → exit 120 instead of 3. The same applies to large help output into a
   closed pipe. Proposal: snp keeps the semantic exit code (0/2/3) and treats this as Tier C.
8. **Do snp-only subcommands or flags exist in compat mode at all?** Every added subcommand name changes the `invalid choice`
   list (Tier A under §A.14). Proposal: in compat mode (argv[0] = `pre-commit`) the choices list is exactly pre-commit's 16;
   snp extensions are only visible when invoked as `snp`.
