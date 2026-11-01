# Troubleshooting

## Start with `snp doctor`

`snp doctor` inspects snp, the cache, git and every hook script of the current repository without changing anything.
Each line is `level subject: message` with `level` one of `ok`, `info`, `warn`, `error`; the exit code is 1 when there
is an error. An example (the wording of the lines is informative, not a stable interface):

```console
$ snp doctor
info  snp: 2.0.0, /home/me/.cargo/bin/snp, invoked as snp
info  store: /home/me/.cache/snp (from default)
info  pre-commit cache: /home/me/.cache/pre-commit exists; snp does not use it
info  pre-commit on PATH: /home/me/.local/bin/pre-commit (Python pre-commit 4.6.2)
info  git: 2.43.0
warn  pre-commit: .git/hooks/pre-commit runs Python pre-commit, not snp
ok    pre-push: .git/hooks/pre-push is snp's
2 problem(s): 0 error(s), 1 warning(s).
Run `snp doctor --fix` to repair this repository's hook scripts.
```

What it checks:

| Check | Typical findings |
|---|---|
| snp itself | version, path, the name it was invoked under |
| cache | location and where it came from (`SNP_HOME`, `XDG_CACHE_HOME`, default); not writable; leftovers of snp 1.x (`snp gc` removes them) |
| pre-commit's cache | it exists and is unused by snp (with the command to remove it) |
| `pre-commit` on `PATH` | Python pre-commit, the snp alias (this snp or another version), a stale Windows hard link, nothing |
| `snp` on `PATH` | the first `snp` on `PATH` is not the one running (for example an old `~/.cargo/bin/snp` 1.x) |
| git | version, and the warning band below 2.31 |
| `core.hooksPath` | set (install refuses, as pre-commit does); husky detected |
| each of the ten hook types | which tool the script runs, and whether a commit would work: a pre-commit script whose `INSTALL_PYTHON` can no longer import `pre_commit` is an **error** (commits fail) |
| `.legacy` files | a hook-manager script left in `.legacy` is an **error** (it would run a second tool) |
| git config hooks (git ≥ 2.54) | entries calling `hook-impl`; an event with both a config hook and a hooks-directory script is an **error** |
| `init.templateDir` | the scripts future clones will get |
| config | `entry: pre-commit …` hooks that need a `pre-commit` executable; `minimum_pre_commit_version` above 4.6.2 |

`snp doctor --fix` switches every hook type of the repository to snp (the same as `snp install` for each installed
type, keeping its options) and checks again. `snp doctor --fast-path` explains for every hook whether it runs natively
and why not; `snp doctor --stash` checks the stashes of interrupted runs against the worktree (see below).

## Common problems

### Commits still run Python pre-commit

The hook script in `.git/hooks/` is still pre-commit's. Run `snp install` (or `snp doctor --fix` for every hook type).
`snp run` started from a terminal prints a hint when that is the case:
``hint: .git/hooks/pre-commit runs Python pre-commit, not snp; run `snp install` to switch``
(`SNP_NO_HINTS=1` silences it).

### `` `snp` not found.  Did you forget to activate your virtualenv? ``

The hook script could not find snp: the path it embeds no longer exists and no `snp` is on `PATH`. This happens when
snp was installed into a virtualenv or a versioned directory that was removed. Reinstall snp and run `snp install`
again, or set `SNP_BIN` to the executable.

### Commits fail in a GUI client but work in the terminal

GUI clients (on macOS: Tower, Fork, SourceTree, GitHub Desktop) start hooks with a minimal `PATH`. snp's script embeds
the absolute path of snp, so snp itself is found, but hooks with `language: system` may not find their tools. Use an
absolute `entry`, or set the `env` extension key.

### The first run is slow after switching

snp builds hook environments in its own cache; it never reuses pre-commit's. Run `snp install --install-hooks` once,
and in CI change the cache key (see [CI integration](ci.md)).

### `snp refuses to use pre-commit's cache directory …`

`SNP_HOME` points at pre-commit's cache. snp's cache has a different format; point `SNP_HOME` somewhere else (or unset
it).

### A run waits for "the stash" of another run

Two runs in the same worktree serialise on the stash of unstaged changes; the second waits up to 60 s
(`SNP_STASH_LOCK_TIMEOUT`) and then fails. A hook that itself calls `snp run` does not wait (the nested run sees
`_SNP_STASH_LOCK`).

### Unstaged changes after a crash or a killed run

snp journals the stash of unstaged changes before touching the worktree. When a run is interrupted before it could
restore them (`kill -9`, a power loss), every later run in that worktree prints, on stderr, where the changes are
saved and the exact command that restores them; snp never applies the patch by itself (you may have kept working).
Once you have run that command, the next run notices that the changes are back, says so once and stops printing the
notice. To dismiss a notice without restoring (or after restoring the work some other way), run
`snp doctor --fix --stash`: it names each patch it dismisses and keeps it, so `git apply --3way <patch>` still works
later (`snp cache gc` removes such a patch after 30 days).

### Warnings about old git

Below git 2.31 snp prints one warning per run (`SNP_ALLOW_OLD_GIT=1` silences it). Below git 2.19 a commit with
unstaged changes is refused before anything is changed; see [git versions](migration.md#git-versions).

### Windows: hooks with `#!/bin/bash` or `#!/usr/bin/env python`

Git for Windows runs hook scripts with its own `sh`. snp resolves shebangs of `script` hooks by basename on `PATH` when
the absolute interpreter does not exist (pre-commit fails with ``Executable `/bin/bash` not found``). Make sure Git's
`usr\bin` is on `PATH` or that the interpreter is installed. See [Platform support](platforms.md).

## Logs and crash reports

* `SNP_LOG=debug snp run` logs every decision snp takes (fast path, cache hits, scheduling) on stderr, or into
  `SNP_LOG_FILE`. Environment values are printed only for an allow-list of variables, so logs are safe to share.
* An unexpected error prints `An unexpected error has occurred: …`, exits with 3, and writes a crash log
  `snp.log` (`pre-commit.log` under the `pre-commit` alias) in the cache directory, as pre-commit does. Please attach
  it to a bug report at <https://github.com/grok-rs/snp/issues>.
* `snp self check` runs the built-in self-test of the installation.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | a hook failed or modified files, or an error pre-commit reports with exit 1 (invalid config, …) |
| 2 | invalid command line (usage error) |
| 3 | an unexpected error (crash log written) |
| 130 | interrupted (Ctrl-C) |

These are pre-commit's codes; snp never maps a failure to 0.
