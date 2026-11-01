# Compatibility

snp's reference is **pre-commit 4.6.2** with the dependencies it pins (cfgv 3.5.0, identify 2.6.20, PyYAML 6.0.3 with
libyaml). The normative policy is `docs/COMPATIBILITY.md` in the repository; this page summarises what it means for
you.

## The contract

* **Inputs.** Every `.pre-commit-config.yaml` and `.pre-commit-hooks.yaml` that pre-commit 4.6.2 accepts is accepted
  with the same meaning; every one it rejects is rejected with the same message — except where pre-commit crashes.
* **Bytes.** For a config that uses no snp extension, invoked as `pre-commit`, snp's stdout, stderr and exit code are
  byte-identical to pre-commit's. Invoked as `snp`, they are identical up to the tool's name (see [Brands](#brands)).
* **Side effects.** The working tree (bytes and modes), the index, refs, the stash list and the behaviour of installed
  hook scripts are identical.
* **Divergences are registered.** snp differs only where pre-commit crashes or hangs, loses or corrupts data, or has a
  security bug, or where an owner decision forces it (its own cache, its own name, no `unsafe` code, performance,
  extensions that are active only when used). Each difference has an ID, a rationale and a differential test that
  fails when it stops reproducing: see the [DIVERGENCES](divergences.md) book.
* **Fail closed.** No divergence turns a failure into success unless every selected hook actually ran and passed.
* **Extensions are additive.** snp-only keys, options and commands never change what a config that does not use them
  produces, and never break an abbreviation of a pre-commit option (`--all` still means `--all-files`).

How this is proven: every scenario of the test corpus runs through both snp and pre-commit 4.6.2 (as the oracle), in
both brands, and the outputs, trees, indexes and refs are compared.

## Brands

The binary looks at the name it was invoked under (`argv[0]`, without `.exe` on Windows):

| Invoked as | Brand | Behaviour |
|---|---|---|
| `pre-commit` (the alias, a copy or a link) | **compat** | pre-commit 4.6.2 byte for byte: `--version` prints `pre-commit 4.6.2`, the help and error texts are pre-commit's, snp-only commands and options are rejected with pre-commit's `invalid choice` / `unrecognized arguments` errors. |
| anything else (`snp`) | **snp** | the same behaviour and layout, every snp command and option available, and the tool named `snp` in its own messages. |

In the snp brand the following texts name `snp` instead of `pre-commit` (the complete list is in
`docs/COMPATIBILITY.md` §3.3):

| pre-commit | snp |
|---|---|
| `pre-commit 4.6.2` (`--version`) | `snp 2.0.0` |
| `pre-commit installed at .git/hooks/pre-commit` | `snp installed at .git/hooks/pre-commit` |
| ``reproduce locally with: `pre-commit run --all-files`.`` | ``reproduce locally with: `snp run --all-files`.`` |
| ``run `pre-commit autoupdate` `` hints | ``run `snp autoupdate` `` |
| `pre-commit version X is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`.` | `pre-commit version X is required but snp 2.0.0 is compatible with pre-commit 4.6.2.  Perhaps upgrade snp.` |
| `usage: pre-commit run …`, `pre-commit: error: …` | `usage: snp run …`, `snp: error: …` (wrapped for the shorter name exactly as argparse would) |
| crash log `pre-commit.log`, `pre-commit version: 4.6.2` | `snp.log`, `snp version: 2.0.0` |

Never changed in either brand: file names (`.pre-commit-config.yaml`, `.pre-commit-hooks.yaml`), environment variable
names (`PRE_COMMIT_*`, `SKIP`), the lines `pre-commit hook(s) made changes.` and `Your pre-commit configuration is
unstaged.` (CI scripts grep for them), and `https://pre-commit.com/…` URLs.

Environment variables (`SNP_*`) work in both brands. Hook scripts written through the alias keep the compat brand on
every commit.

## What is compared how

| Tier | What | Examples |
|---|---|---|
| **A** byte-exact | everything not listed below | hook output, `Passed`/`Failed` lines, error messages, exit codes, files written |
| **B** semantic | texts that embed Python internals | the body of a crash log below its version block, Python tracebacks (compared by their last line), localized Windows `[WinError N]` texts |
| **C** ignored | values that differ by design or by chance | durations, temporary file names and process ids, cache root paths (the caches differ), installer chatter between `Installing environment` lines |

Some outputs of pre-commit are themselves not deterministic (the order of a Python `set`, partition boundaries a few
bytes from the command-line limit); snp always produces one of pre-commit's possible outputs.

## Not divergences

Things that look different but are not:

* `try-repo` uses a throwaway cache, as pre-commit does, so its `Initializing`/`Installing` lines print on every run.
* Native fast-path hooks parse their arguments with pre-commit's argparse rules per partition, so file names that look
  like options behave as in Python.
* Hook ids containing `:` are plain ids.
* Every optimisation snp makes (stat-based change detection, parallel clones and installs with an ordered transcript,
  a cached `git --version`, …) is exact: it has an equivalence test against the plain path and never changes output.

## Python behaviour reproduced

Where pre-commit's behaviour comes from Python itself, snp reproduces the Python version pre-commit's users see:
regular expressions follow CPython 3.12's `re` (with its exact error messages), argparse texts follow CPython 3.13, and
`repr`, `shlex`, path handling, the deterministic shuffle of file lists and text decoding follow CPython. These are
verified against vectors generated by real interpreters.
