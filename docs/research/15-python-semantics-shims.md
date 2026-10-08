# 15. Python-stdlib semantics shims (`pycompat`)

## Addendum: One Python-stdlib semantics shim spec with test vectors (splitlines, split/rsplit/strip whitespace, os.path, shlex.quote/join, repr)

Scope: every place where pre-commit 4.6.2 (`refs/pre-commit` @ `c429238`) leans on CPython string, codec, path,
shlex or repr semantics that a Rust port would get subtly wrong. Baseline oracle: **CPython 3.12.3, Unicode 15.0.0**
(the interpreter of the 4.6.2 oracle venv `venv-pc462/`), with a **3.13.14 / Unicode 15.1.0 delta**. Behavior marked
**OBS** was observed by running pre-commit 4.6.2 in throwaway repos under `tmp-pycompat/`. Source citations are relative
to `refs/pre-commit/` unless they name another tree. Scratchpad-relative paths below are under
`/tmp/claude-1000/-home-dromix-projects-snp/484ec4f1-c0f7-4123-b7cd-bc3e3cb59b63/scratchpad/`.

### A0. TL;DR — facts and decisions

1. **Python uses three different "whitespace" sets and four "line break" sets, and none equals a Rust std set.**
   `str.isspace()` = Python `re` str-pattern `\s` = `\t\n\x0b\x0c\r\x1c\x1d\x1e\x1f\x20\x85\xa0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000`.
   Rust `char::is_whitespace`/`str::trim`/`split_whitespace`/regex-crate `\s` (Unicode `White_Space`) lack exactly
   **U+001C..U+001F**. `bytes.strip()` = `b' \t\n\r\x0b\x0c'`; Rust `trim_ascii`/`is_ascii_whitespace` lack **`\x0b`**.
   `str.splitlines()` breaks on `\n \r \r\n \x0b \x0c \x1c \x1d \x1e \x85 \u2028 \u2029` (not `\x1f`);
   `bytes.splitlines()` only on `\n \r \r\n`; text-file iteration only on `\n` (after universal-newline translation);
   Rust `str::lines()` only on `\n` (dropping one preceding `\r`). All verified over all 1,114,112 code points.
2. **Golden vectors are committed**: `research/vectors/pycompat-vectors.cpython312.json` (59 sections, 3,256 cases incl.
   2,200 seeded random cases, plus whole-code-space character-class sets; deterministic; sha256 `8c642b5f…`), a 3.13
   delta file, the generator and the Unicode-table codegen. A **Rust reference prototype** (`research/pycompat-proto/`, rustc 1.98.0, edition 2024,
   `#![forbid(unsafe_code)]`) passes **all** vectors (`cargo +1.98.0 test --offline`: 14 tests green).
3. **Pre-push stdin (D1): parity, not 03 §20.4.** Decode strict UTF-8 eagerly (any bad byte → crash before line 1),
   split with Python `str.splitlines()`, fields with Python `rsplit(maxsplit=3)` (Unicode whitespace, runs collapse,
   remainder keeps *leading* whitespace), unpack lazily (a malformed line crashes only if no earlier line returned).
   Errors are exit 3 with `An unexpected error has occurred: ValueError: not enough values to unpack (expected 4, got N)`
   or `…: UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 14: invalid continuation byte` (OBS).
   03's "skip malformed lines" is **rejected**: it is fail-open for a guard.
4. **`REV_LINE_RE` (D2): parity.** `\s` must be the explicit Python class (or the regex-free matcher of §A4.6); Python
   `$` = "end or before a final `\n`". Group 6 is **always `"\n"`**: greedy `(.*)` swallows the `\r` of CRLF into
   group 5. Consequence (OBS): `autoupdate --freeze` (and un-freezing) on a CRLF config rewrites that one line with
   LF → mixed line endings. snp **deliberately deviates** (DEV-PY-1): re-append the `\r` (§A5).
5. **`repr()` must be exact**: quote choice (`'` unless the text has `'` and no `"`), escapes `\\ \' \t \n \r`,
   `\xNN` for C0/DEL/C1/NBSP…, `\uNNNN`/`\UNNNNNNNN` for other **non-printable** code points (Unicode-version
   dependent table, 711 ranges for UCD 15.0), 1-tuples `('a',)`, `%.200R` truncation in `int()` errors, f-string
   `{x=}` = `x=` + repr. Used in ~25 message sites (§A2.7).
6. **`os.path` is lexical and flavor-specific**: posix `normpath` keeps exactly two leading slashes; `abspath` is
   `normpath(join(cwd, p))` (NOT `std::path::absolute`, which keeps `..` on Unix); `relpath('')` raises
   `ValueError: no path specified` (OBS: `pre-commit run -c ''` → exit 3); ntpath `relpath` across drives raises
   `path is on mount 'D:', start on mount 'C:'` (by code: exit 3 for `run --files D:\x.py` from a `C:` repo);
   `commonpath` is case-insensitive on nt but returns the first path's case.
7. **`expanduser` differs from `std::env::home_dir` on both OSes**: POSIX honors `HOME=""` (`'~'`→`'/'`,
   `'~/.cache'`→`'/.cache'`; 07 §7 item 20 is wrong that it yields `''`); Windows uses `USERPROFILE` even when empty, then
   `HOMEDRIVE`+`HOMEPATH`, else returns the input unchanged, never `HOME`. Own implementation required.
8. **POSIX `realpath` is `_joinrealpath`, not "canonicalize the longest existing ancestor"** (07 §7 item 20):
   dangling links resolve to their target (`dangling`→`$ROOT/nonexistent`), `..` applies after resolution, loops stay
   unresolved, `file/x` → `…/f/x`. The ancestor recipe is only right for **Windows** realpath. Fixture vectors V-RP.
9. **Executable lookup**: 04a §14.18 is right, 06 §8 is wrong — `which` 8.0.6 skips empty PATH entries on Windows,
   absolutizes relative entries, expands `~`, tries the bare name **before** PATHEXT variants and re-cases the result;
   Python tries `name+ext` for every PATHEXT entry first, then the bare name, keeps relative results, uses PATHEXT
   whenever it is set (any OS). Fixture vectors V-FE.
10. **Text-mode I/O**: migrate-config, autoupdate and install use the **locale** encoding; migrate-config, install and
    autoupdate's reformat fallback write `os.linesep` (CRLF on Windows) while autoupdate's normal rewrite preserves
    endings (`newline=''`); cfgv reads configs as UTF-8 with universal newlines. snp: always UTF-8 (DEV-PY-3), keep
    each site's newline semantics.
11. **Unicode-version drift is real but tiny**: 3.12 vs 3.13 differ only in `isprintable` (U+2FFC..2FFF, U+31EF, CJK Ext I
    become printable) and in ntpath `isabs('/x')`/`commonpath` error order; pin UCD 15.0 (doc 10 Q7) and generate tables.

### A1. Artifacts and how to use them

| Artifact | Path (scratchpad-relative) | Notes |
|---|---|---|
| Golden vectors (3.12) | `research/vectors/pycompat-vectors.cpython312.json` | 994 KB, sha256 `8c642b5fdf8c2d9b86786aa8bbcefc81cd97ed22eb77396e5095261cd7710004`. Regeneration is byte-identical (verified). |
| 3.13 delta | `research/vectors/pycompat-delta-cpython313.json` | only entries whose 3.13.14 result differs (same section/index). |
| Generator | `scripts/gen_pycompat_vectors.py` | `python3 -I gen_pycompat_vectors.py OUT.json FIXTURE_DIR`, run from a directory with no `.py` files. Stdlib only. Re-states the pre-commit expressions inline (cited), imports nothing from `refs/`. Creates the realpath/find_executable fixtures under `FIXTURE_DIR`. |
| Unicode tables codegen | `scripts/gen_pycompat_tables.py` → `research/pycompat-proto/src/tables.rs` | `NONPRINTABLE` (711 ranges ≥ U+0080), `DECIMAL_ZEROS` (68 `Nd` zeros, each a run of 10). Asserts its own invariants. |
| Markdown renderer | `scripts/render_pycompat_tables.py` | produced every `V-*` table in this addendum from the JSON (no hand transcription). |
| Rust reference prototype | `research/pycompat-proto/` | `CARGO_TARGET_DIR=… cargo +1.98.0 test --offline` → 14 tests, all sections replayed, incl. fixtures recreated in a temp dir. Lints: `unsafe_code=forbid`, clippy `unwrap_used/expect_used/panic/indexing_slicing=deny` (lib clean). |

Destination in the snp repo (doc 10 §1 item 4, §7.5): `crates/snp-pycompat/tests/vectors/pycompat.cpython312.json`,
generator under `xtask/oracle/` (`cargo xtask oracle vectors`), tables committed as generated source with a
`// @generated` header and a CI check that regenerating yields no diff.

JSON conventions: bytes are lowercase hex in `*_hex` fields; strings that need lone surrogates (POSIX
`surrogateescape`) are code-point lists in `*_cps` (U+DC80..U+DCFF = undecodable byte 0x80..0xFF); results are
`{"ok": x}` or `{"error": "<ExcType>", "msg": "<str(exc)>"}`; values fed to `repr` are tagged
`{"t": "str|list|tuple|dict|int|float|bool|none", "v": …}` (float `v` is Python's `repr`, `10**20` is an exact JSON
integer — parse as f64 or arbitrary precision); fixtures use the placeholder `$ROOT`. Sections and fields:

| Section | n | Fields |
|---|---|---|
| `sets` | 10 sets | `str_isspace`, `re_str_backslash_s`, `str_splitlines_boundary`, `str_isprintable_false` (inclusive code-point ranges), `bytes_isspace`, `re_bytes_backslash_s`, `bytes_splitlines_boundary` (byte lists), `rust_white_space_property`, `str_isspace_minus_rust_white_space`, `rust_white_space_minus_str_isspace` |
| `str_splitlines` / `bytes_splitlines` | 70 / 34 | `in`/`in_hex`, `keepends`, `out`/`out_hex` |
| `str_split_none`, `str_split_none_maxsplit`, `str_rsplit_none_maxsplit`, `str_split_sep` | 18/15/42/19 | `in`, `maxsplit` (−1 = unlimited), `sep`, `out` |
| `str_strip_none`, `str_strip_chars`, `bytes_strip_none`, `bytes_strip_chars` | 19/7/13/9 | `in`, `chars`, `strip`, `lstrip`, `rstrip` (`*_hex` for bytes) |
| `zsplit`, `skip_env`, `pre_push_parse`, `merge_msg_conflicts` | 10/17/21/9 | composites of §A4.5 |
| `rev_line_re`, `random_rev_lines`, `rev_line_rewrite`, `autoupdate_original_lines`, `migrate_header_split`, `textwrap_indent` | 32/600/11/9/11/12 | §A4.6 |
| `text_read`, `text_write` | 9/5 | `newline_none`, `newline_empty`, `posix_hex`, `windows_hex` |
| `utf8_decode_strict`, `utf8_decode_backslashreplace` | 25/5 | §A4.3 |
| `repr`, `fstring_eq`, `oserror_str`, `errno_strerror_glibc`, `stdlib_calledprocesserror_str`, `keyerror_str`, `argparse_invalid_choice`, `parse_version` | 85/4/9/10/7/3/2/13 | §A4.7 |
| `shlex_quote`, `shlex_join`, `shlex_split` | 26/5/51 | §A4.8 |
| `posixpath_*` (normpath, abspath, join, relpath, commonpath, parts, expanduser), `posixpath_realpath_fixture` | 27/9/11/14/14/20/48, 29 cases | §A4.9; `cwd` given per case |
| `ntpath_*` (normpath, abspath_fallback, join, relpath, commonpath, parts, expanduser) | 26/12/17/10/9/15/35 | §A4.10; `real_windows_may_differ` marks fallback-only results |
| `exe_exists_pure`, `adjust_args_and_chdir`, `find_executable_posix_fixture` | 17/19/19 cases | §A4.11 |
| `random_str_ops`, `random_bytes_ops`, `random_posixpath`, `random_ntpath` | 400 each | seeded (`random.Random(20261008)`) alphabets rich in every boundary/space class |
| `decimal_zeros` | 68 | `unicodedata.decimal(c) == 0` code points |

### A2. Call-site inventory (pre-commit 4.6.2)

Each row: site, exact expression, where the input comes from, the `pycompat` shim (§A6) to use. "crash" = the exception
escapes to `error_handler` → `An unexpected error has occurred: <Type>: <str(exc)>`, exit 3.

#### A2.1 Line splitting

| Site | Expression | Input | Shim / note |
|---|---|---|---|
| `commands/hook_impl.py:128` | `for line in stdin.decode().splitlines():` | pre-push stdin (git) | `decode_utf8` + `str_splitlines` (D1) |
| `commands/hook_impl.py:150` | `ancestors.splitlines()[0]` (ancestors = `check_output(...).decode().strip()`) | `git rev-list` | `str_splitlines`; ASCII hex only, any splitter is equivalent |
| `commands/hook_impl.py:152` | `set(subprocess.check_output(cmd).decode().splitlines())` | `git rev-list --max-parents=0` | same |
| `git.py:104-111` | `[line.lstrip(b'#').strip().decode() for line in merge_msg.splitlines() if line.startswith((b'\t', b'#\t'))]` | `.git/MERGE_MSG` (binary read, `git.py:118`) | `bytes_splitlines` + `bytes_lstrip_set(b"#")` + `bytes_strip` + `decode_utf8`; eager list → one bad line crashes; error position is relative to the stripped line |
| `git.py:239-241` | `cmd_output('git', *NO_FS_MONITOR, 'tag', '--points-at', rev, cwd=git_repo)[1].splitlines()` | tag names | `str_splitlines` (tags may legally contain U+0085/U+2028/U+2029 bytes) |
| `commands/autoupdate.py:125-128` | `open(path, newline='')` … `original.splitlines(True)` | config file | `str_splitlines_keepends` on the raw text (no newline translation) |
| `commands/migrate_config.py:28` | `contents.splitlines(True)` after `open(config_file)` (universal newlines) | config file | `universal_newlines` then `str_splitlines_keepends` |
| `commands/migrate_config.py:44` | `textwrap.indent(rest, " " * 4)` | config tail | `textwrap_indent` (= `splitlines(True)` + prefix lines whose `strip()` is non-empty) |
| `languages/python.py:38-45` | `for line in f:` (text mode, UTF-8) | `pyvenv.cfg` | **not** splitlines: split after `\n` only, after `\r\n`/`\r`→`\n` |
| `languages/pygrep.py:25,55` | `for line_no, line in enumerate(f, start=1)` (binary) | hooked files | split after each `b'\n'` only (a CR-only file is one line) |
| `languages/pygrep.py:40-44` | `contents[:match.start()].count(b'\n')`, `match[0].split(b'\n')`, `contents.split(b'\n')[line_no]` | hooked files | byte `\n` only |
| `error_handler.py:50` | `sys.version.splitlines()` | — | N/A in snp (crash-log content is snp's own) |

#### A2.2 Whitespace split / strip (no separator argument)

| Site | Expression | Kind | Note |
|---|---|---|---|
| `commands/hook_impl.py:129-130` | `parts = line.rsplit(maxsplit=3)`; 4-way unpack | str | D1 |
| `commands/run.py:129-131` | `{skip.strip() for skip in environ.get('SKIP', '').split(',') if skip.strip()}` | str | V-SKIP; env value is surrogateescape-decoded on POSIX → strip bytes with UTF-8-aware classes (invalid bytes are non-space) |
| `git.py:58,62` | `abspath(cmd_output('git','rev-parse','--show-cdup')[1].strip())`, `--is-inside-git-dir` `.strip()` | str | Unicode strip of a path; harmless (`../` only) |
| `git.py:79-80, 89-90` | `out.strip()` of `--git-dir` / `--git-common-dir`, then `normpath(join(git_root, out))` | str | by code: a git dir path ending in a Unicode space (e.g. a worktree named `x\u3000`) is truncated — parity keeps the bug |
| `git.py:125`, `staged_files_only.py:52` | `cmd_output('git', 'write-tree')[1].strip()` | str | hex |
| `git.py:172` | `out.split()[0]` (`ls-remote`) | str | IndexError (crash) on empty output |
| `git.py:182` | `bool(out.strip())` (`git config core.hooksPath`, bytes) | bytes | ASCII-6 strip: a value consisting of `\x1c` is "set" |
| `git.py:207` | `os.path.join(repo, out.strip())` (`--git-path`) | str | |
| `staged_files_only.py:62,67`; `commands/run.py:327` | `diff_stdout.strip()`, `stdout.strip()` | bytes | emptiness tests |
| `commands/run.py:230-232` | `if out.strip(): … write_line_b(out.strip(), …)` | bytes | **hook output display**: leading/trailing `b' \t\n\r\x0b\x0c'` removed; `\x1c`, `\x85`, NBSP bytes kept |
| `commands/autoupdate.py:56,58,65` | `cmd_output(...)[1].strip()` (`describe`, `rev-parse`) | str | |
| `commands/autoupdate.py:149` | `new_rev_s.split(':', 1)[1].strip()` | str | splits the dumped `rev: …\n` |
| `commands/autoupdate.py:152` | `match[5].strip().startswith('# frozen:')` | str | Unicode strip (`\xa0# frozen:` counts) |
| `commands/hook_impl.py:146,164` | `.decode().strip()` of `rev-list`/`rev-parse` | str | |
| `commands/init_templatedir.py:34` | `realpath(expanduser(out.strip()))` | str | `git config init.templateDir` value |
| `commands/migrate_config.py:22` | `line.startswith(('#', '---')) or not line.strip()` | str | header-line test |
| `languages/python.py:31,72` | `cmd_output(...)[1].strip()` | str | interpreter output |
| `languages/python.py:41,45` | `k, v = line.split('=')` (exactly 2 parts else skip); `k.strip()`, `v.strip()` | str | |
| `languages/node.py:41`, `languages/golang.py:141`, `languages/lua.py:25` | `.strip()` of tool output | str | |
| `languages/r.py:37` | `out.rstrip('\n')` | str | set `{'\n'}` |
| `languages/pygrep.py:29` | `line.rstrip(b'\r\n')` | bytes | **set** `{\r,\n}`: `b'foo\r\r\n'`→`b'foo'` |
| `util.py:66` | `b'\n    ' + part.replace(b'\n', b'\n    ').rstrip()` | bytes | CalledProcessError body |
| `util.py:87` | `force_bytes(e).rstrip(b'\n') + b'\n'` | bytes | spawn-error output |
| `error_handler.py:29` | `git_version_b.decode(errors='backslashreplace').rstrip()` | str | crash log |
| `git.py:20` | `s.strip('\0')` (zsplit) | str | V-ZSPLIT |

#### A2.3 Explicit separators, partition, prefix tests (Rust std is identical when the separator is non-empty)

| Site | Expression | Note |
|---|---|---|
| `git.py:22` | `s.split('\0')` | interior empty items survive (`'a\0\0b\0'` → `['a', '', 'b']`) |
| `clientlib.py:50-52` | `tuple(int(p) for p in s.split('.'))` | `int()` semantics §A4.7; non-int → crash (OBS: `validate-config` with `minimum_pre_commit_version: "1.x"` → `An unexpected error has occurred: ValueError: invalid literal for int() with base 10: 'x'`, exit 3) |
| `parse_shebang.py:32,37` | `environ['PATHEXT'].split(os.pathsep)`; `environ.get('PATH', '').split(os.pathsep)` | unset/empty PATH → `['']` → the cwd is searched |
| `commands/install_uninstall.py:94-95` | `before, rest = contents.split(TEMPLATE_START)`; `_, after = rest.split(TEMPLATE_END)` | resource text |
| `languages/python.py:87-88` | `os.path.split(path.lower())`; `exe.partition('.exe')` | full Unicode `str.lower()` (`'İ'.lower()` == `'i̇'`, final sigma) = Rust `str::to_lowercase` |
| `languages/python.py:141` | `version.removeprefix('python').split('.')` + `int()` | ValueError caught there |
| `languages/rust.py:76,138`, `languages/dart.py:74` | `dep.partition(':')` | absent sep → `(s, '', '')` |
| `commands/hazmat.py:47-53` | `filename.startswith(f'{subdir}/')`, `.removeprefix(prefix)` | |

#### A2.4 Decode / encode (all strict UTF-8 unless stated)

| Site | Expression | Failure in pre-commit |
|---|---|---|
| `util.py:118-119` | `stdout_b.decode()` / `stderr_b.decode()` in every `cmd_output` | non-UTF-8 git output → crash; N indexes the **whole** stdout. OBS (`caf\xe9.txt` committed, `run --all-files`): `An unexpected error has occurred: UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 27: invalid continuation byte` (27 = 24 bytes of `.pre-commit-config.yaml\0` + `caf`), exit 3 |
| `commands/hook_impl.py:128,146,152,164` | `.decode()` | crash (V-PP) |
| `git.py:107` | per-line `.decode()` | crash, position relative to the stripped line (V-MM) |
| `output.py:10,33` | `stream.write(s.encode())` for every printed `str` | a POSIX path containing undecodable bytes (surrogates) → `UnicodeEncodeError: 'utf-8' codec can't encode character '\udcff' in position 5: surrogates not allowed` (`characters in position 5-6` for runs) |
| `xargs.py:66-69` | `len(' '.join(cmd).encode('utf-16le')) // 2` (win32) / `.encode(sys.getfilesystemencoding())` | POSIX: surrogates → UnicodeEncodeError (strict); Windows length = UTF-16 code units |
| `languages/fail.py:25-26` | `f'{entry}\n\n'.encode()`, `f.encode()` | |
| `languages/docker.py:70` | `hashlib.md5(s.encode())` | |
| `languages/pygrep.py:123` | `args.pattern.encode()` | |
| `error_handler.py:29` | `.decode(errors='backslashreplace')` | never fails |

#### A2.5 `os.path` (flavor = host OS)

| Function | Sites |
|---|---|
| `abspath` | `main.py:178,180,182,186` (config if it exists, files, commit-msg file, try-repo repo); `git.py:57` (`--show-cdup`); `git.py:187` (`init_repo` remote if `isdir`) |
| `relpath` | `main.py:191,193,195,199` (after `chdir(toplevel)`, `main.py:189-190`) |
| `normpath` | `parse_shebang.py:25` (`find_executable`); `git.py:80,90`; `prefix.py:9-10` (`Prefix.path`: `normpath(join(prefix_dir, *parts))`); `languages/node.py:95` (`\\?\` + normpath) |
| `join` | 76 sites; semantics only matter where a later part may be absolute/drive-qualified: `Prefix.path`, `lang_base.environment_dir` (`lang_base.py:91`: `f'{d}-{language_version}'`), `git.py:207`, `store.py:33-36` |
| `commonpath` | `lang_base.py:70` (`exe_exists`, ValueError swallowed); `languages/docker.py:60` (Linux only) |
| `expanduser` | `store.py:33` (`~/.cache`); `commands/clean.py:11` (`~/.pre-commit`); `lang_base.py:68` (`~`); `languages/python.py:165` (user `language_version`); `commands/init_templatedir.py:34` |
| `realpath` | `store.py:36` (store dir), `store.py:228` (`mark_config_used`), `commands/init_templatedir.py:34-35`, `languages/python.py:101` (N/A) |
| `split`/`dirname`/`basename`/`splitext` | `store.py:50`; `lang_base.py:79` (`dirname(homedir) == homedir`); `commands/install_uninstall.py:73`; `util.py:215`; `languages/coursier.py:49-50`; `languages/docker.py:74` (`basename(prefix_dir)` → image tag) |
| `exists`/`lexists`/`isfile`/`isdir`/`access` | `main.py:177,185,198`; `commands/run.py:75` (`lexists` filter on `--files`); `commands/hook_impl.py:41,54`; `parse_shebang.py:16,40,55-59`; `commands/install_uninstall.py:57,76,80,82,153,159`; … — all return False on any OSError (incl. `ENAMETOOLONG`, embedded NUL) like Rust `Path::exists`/`is_file`; `os.access(X_OK)` uses the **real** uid (Rust: `rustix::fs::access(p, Access::EXEC_OK)`); on Windows `os.access(X_OK)` is "exists" |
| `os.sep`/`os.altsep` tests | `parse_shebang.py:26,50`; `commands/run.py:123-124` (Windows: `f.replace('\\', '/')` before include/exclude matching) |

#### A2.6 shlex

| Site | Expression |
|---|---|
| `lang_base.py:174-178` | `cmd = shlex.split(entry)`; `pre-commit hazmat` rewrite; `(*cmd, *args)` |
| `languages/r.py:183` | `shlex.split(entry)` (no hazmat rewrite) |
| `clientlib.py:106` | warning text ``Hint: often `pre-commit autoupdate --repo {shlex.quote(repo)}` will fix this.`` |
| `clientlib.py:355` | meta-hook `entry` = `f'{shlex.quote(sys.executable)} -m pre_commit.meta_hooks.{modname}'` |
| `commands/install_uninstall.py:105-107` | `INSTALL_PYTHON={shlex.quote(sys.executable)}`, `ARGS=({shlex.join(args)})` |
| `languages/perl.py:25,28` | `--install_base {shlex.quote(venv)}`, `INSTALL_BASE={shlex.quote(venv)} ` |

#### A2.7 `repr` / `str(exception)` in user-visible text

| Site | Template | Repr'd value |
|---|---|---|
| `util.py:71` | `command: {self.cmd!r}` (pre-commit `CalledProcessError`) | tuple of str (the **normalized** argv) |
| `commands/hook_impl.py:189-210` | `hook-impl for {hook_type} expected 1, 2, or 3 arguments but got {len(args)}: {args}` (prepare-commit-msg), `… expected 1 or 2 arguments …` (pre-rebase), `… expected {expected} {'argument' if expected == 1 else 'arguments'} …`; OBS: `hook-impl for pre-push expected 2 arguments but got 1: ['origin']`, `hook-impl for pre-commit expected 0 arguments but got 2: ["it's", 'a']` (stderr, exit 1) | list (`args.rest[1:]`) |
| `lang_base.py:107-111` | ``for now, pre-commit does not support additional_dependencies for {lang} -- you selected `additional_dependencies: {additional_deps}` `` | list (from YAML) |
| `commands/hazmat.py:51` | `unexpected file without {prefix=}: {filename}` | `prefix='sub/'` |
| `clientlib.py:45` | `Type tag {tag!r} is not recognized.  Try upgrading identify and pre-commit?` | str |
| `clientlib.py:303-309` | `The {self.key!r} field of repo {dct["repo"]!r} appears to be a mutable reference …` | str |
| `clientlib.py:319-328` | `The {self.key!r} field in hook {dct.get("id")!r} is a regex, not a glob -- …` / `… normalizes slashes in the {key!r} field in hook {id!r} …` | str or `None` |
| `clientlib.py:338,344-346` | top-level variants | str |
| `clientlib.py:404` | `{self.key!r} cannot be overridden` | str |
| `meta_hooks/check_useless_excludes.py:42,63` | `The global exclude pattern {exclude!r} does not match any files`; `The exclude pattern {exclude!r} for {hook["id"]} does not match any files` | str |
| `languages/r.py:203` | `prefix_dir <- {prefix.prefix_dir!r}` (Python repr spliced into R source) | str |
| `error_handler.py:25` | `{msg}: {type(exc).__name__}: ` + `force_bytes(exc)` (`bytes(exc)` if supported, else `str(exc)`) | any exception: OSError (`[Errno N] msg: 'file'`), KeyError (`'k'`), `int()` ValueError (`%.200R`), ntpath relpath ValueError (`%r`), stdlib `CalledProcessError` (`str(tuple)`), UnicodeDecode/EncodeError |
| `staged_files_only.py:101-104` | `pre-commit failed to diff -- perhaps due to permissions?\n\n{e}` | CalledProcessError text (tuple repr) |
| `commands/autoupdate.py:78,203` | `[{self.repo}] {e}`; `str(e)` | InvalidManifestError text (cfgv messages, see doc 08) |
| `commands/hook_impl.py:143-165` | stdlib `subprocess.check_output` raises **stdlib** `CalledProcessError`: `Command '<str(tuple)>' returned non-zero exit status N.` (OBS, V-CPE) | tuple |
| argparse (CLI) | `argument --hook-type: invalid choice: 'x' (choose from 'a', 'b')` | value and each choice via repr (3.12 and 3.13) |
| cfgv / PyYAML | `{v!r} is not a valid python regex`, `At Hook(id='x')`, `could not determine a constructor for the tag '!x'` | see doc 08 §2.4, doc 07 §3.1.4 |

Not repr: `', '.join(...)` sites (`clientlib.py:104,156,182,363,372`, `hook.py:55`, `commands/autoupdate.py:99,181`).

#### A2.8 Text-mode file I/O

| Site | Mode | Read newline | Write newline | Encoding |
|---|---|---|---|---|
| cfgv `load_from_filename` (`refs/cfgv/cfgv.py:439-442`) | read config/manifest | universal (`\r\n`,`\r`→`\n`) | — | UTF-8; decode error → `ValidationError(str(e))` |
| `commands/migrate_config.py:115,129` | read, rewrite | universal | `os.linesep` (Windows CRLF) | **locale** |
| `commands/autoupdate.py:125` / `:135` / `:158` | read / reformat write / final write | none (`newline=''`) | `os.linesep` / none (`newline=''`) | **locale** |
| `commands/install_uninstall.py:92` | write hook script | — | `os.linesep` → **CRLF on Windows** | locale |
| `commands/try_repo.py:68`, `store.py:55,72`, `repository.py:101`, `languages/r.py:118`, `languages/dotnet.py:43` | write | — | `os.linesep` | locale |
| `repository.py:42`, `languages/dart.py:51` | read | universal | — | locale |
| `languages/python.py:38` | read `pyvenv.cfg` | universal | — | UTF-8 |
| `git.py:118`, `commands/install_uninstall.py:59`, pygrep, patch files, logs | binary | — | — | — |

### A3. Character classes (normative; verified for every code point, `sets` section)

| Name | Members | Python users | Closest Rust std | Difference |
|---|---|---|---|---|
| **PY-WS** | U+0009..U+000D, **U+001C..U+001F**, U+0020, U+0085, U+00A0, U+1680, U+2000..U+200A, U+2028, U+2029, U+202F, U+205F, U+3000 | `str.isspace/strip/lstrip/rstrip/split()/rsplit()`; `re` str-pattern `\s` (identical set) | `char::is_whitespace`, `str::trim*`, `split_whitespace`, regex-crate `\s` (= Unicode `White_Space`) | Rust lacks U+001C..U+001F (and has nothing Python lacks). U+180E, U+200B, U+FEFF are **not** spaces in either. |
| **PY-LB** | `\n` U+000A, `\x0b` U+000B, `\x0c` U+000C, `\r` U+000D (`\r\n` = one break), U+001C, U+001D, U+001E, U+0085, U+2028, U+2029 | `str.splitlines` | `str::lines` (`\n`, optional preceding `\r` dropped) | Rust misses lone `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, U+0085, U+2028/9. **U+001F is a space but not a line break.** |
| **B-WS** | `\t \n \x0b \x0c \r` and space | `bytes.strip/split()/isspace`; `re` bytes-pattern `\s` | `u8::is_ascii_whitespace`, `<[u8]>::trim_ascii` | Rust lacks `\x0b` |
| **B-LB** | `\n`, `\r`, `\r\n` | `bytes.splitlines` | — | `\x0b`, `\x0c`, `\x1c`, `\x85`, `\xc2\x85`, `\xe2\x80\xa8` are **not** breaks |
| **TEXT-LB** | `\n` after universal translation (`\r\n`,`\r`→`\n`) | iterating a text file (`for line in f`), `readlines()` | `BufRead::lines` (`\n`, strips one `\r`) | Rust does not split on lone `\r` |
| **BIN-LB** | after each `\n` | iterating a binary file | `BufRead::split(b'\n')` (drops the `\n`) | Python keeps the `\n` in the line |
| **SHLEX-WS** | `' \t\r\n'` | `shlex.split` | — | `\x0b`, `\x0c`, NBSP etc. are word characters |
| **PY-PRINTABLE** | not (Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs) ∪ {U+0020} | `repr()` (non-ASCII), `str.isprintable` | none (no GC tables in std) | UCD-version dependent: generate (711 non-printable ranges ≥ U+0080 for 15.0). 3.13/15.1 makes U+2FFC..U+2FFF, U+31EF, U+2EBF0..U+2EE5D printable. |
| **PY-DECIMAL** | `unicodedata.decimal` (Nd): 68 runs of 10 | `int()` | `char::to_digit(10)` (ASCII only) | `int('٣')` == 3, `int('²')` fails |

Rust: `const fn is_space(c) { matches!(c, '\t'..='\r' | '\x1c'..=' ' | '\u{85}' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}') }`;
`is_linebreak(c) = matches!(c, '\n'..='\r' | '\x1c'..='\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')`;
`is_bytes_space(b) = matches!(b, b'\t'..=b'\r' | b' ')`. For the `regex` crate, `\s` in a translated Python str pattern
must become `[\t\n\x0B\x0C\r\x1C-\x20\x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]`
and `\S` / `[^\s…]` its complement (`PY_STR_SPACE_CLASS`); this also applies to the general pyre translator (doc 07 §3.2).

### A4. Primitive semantics with golden vectors

All `V-*` tables are rendered mechanically from the 3.12 JSON; inputs and outputs are Python literals.

#### A4.1 `str.splitlines` / `bytes.splitlines`

Algorithm (`Objects/stringlib/split.h` `splitlines`): scan; at a PY-LB char end the line; `\r` immediately followed by
`\n` is one break; with `keepends` the break characters stay on the line; the text after the last break is emitted only
if non-empty (`'a\n'` → `['a']`, `'\n'` → `['']`, `''` → `[]`). `bytes.splitlines` is the same with B-LB.
Performance: for ASCII-only input only bytes `0x0A..=0x0D`, `0x1C..=0x1E` can break; for UTF-8 also the lead bytes
`0xC2` (U+0085 = `C2 85`) and `0xE2` (U+2028/9 = `E2 80 A8/A9`) — a 256-entry byte table or `memchr` for `\n`/`\r`
plus a one-pass "rare breaker" pre-check keeps it at memchr speed; never decode to `char`s on the hot path.

##### V-SL: `str.splitlines()` / `splitlines(True)` (section `str_splitlines`)

| input | splitlines() | splitlines(True) |
|---|---|---|
| `''` | `[]` | `[]` |
| `'\n'` | `['']` | `['\n']` |
| `'\n\n'` | `['', '']` | `['\n', '\n']` |
| `'a'` | `['a']` | `['a']` |
| `'a\n'` | `['a']` | `['a\n']` |
| `'a\nb'` | `['a', 'b']` | `['a\n', 'b']` |
| `'a\n\nb'` | `['a', '', 'b']` | `['a\n', '\n', 'b']` |
| `'a\r\nb'` | `['a', 'b']` | `['a\r\n', 'b']` |
| `'a\rb'` | `['a', 'b']` | `['a\r', 'b']` |
| `'a\n\rb'` | `['a', '', 'b']` | `['a\n', '\r', 'b']` |
| `'a\r\r\nb'` | `['a', '', 'b']` | `['a\r', '\r\n', 'b']` |
| `'\r'` | `['']` | `['\r']` |
| `'\r\n'` | `['']` | `['\r\n']` |
| `'\r\r'` | `['', '']` | `['\r', '\r']` |
| `'a\r\n'` | `['a']` | `['a\r\n']` |
| `'\n\r\n'` | `['', '']` | `['\n', '\r\n']` |
| `'a\x0bb'` | `['a', 'b']` | `['a\x0b', 'b']` |
| `'a\x0cb'` | `['a', 'b']` | `['a\x0c', 'b']` |
| `'a\x1cb'` | `['a', 'b']` | `['a\x1c', 'b']` |
| `'a\x1fb'` | `['a\x1fb']` | `['a\x1fb']` |
| `'a\x85b'` | `['a', 'b']` | `['a\x85', 'b']` |
| `'a\u2028b'` | `['a', 'b']` | `['a\u2028', 'b']` |
| `'a\x00b'` | `['a\x00b']` | `['a\x00b']` |
| `'a\tb'` | `['a\tb']` | `['a\tb']` |
| `'a\x85'` | `['a']` | `['a\x85']` |
| `'x\r\n\r\ny'` | `['x', '', 'y']` | `['x\r\n', '\r\n', 'y']` |
| `'refs/heads/a\u2028b 1111 refs/heads/x 0000\n'` | `['refs/heads/a', 'b 1111 refs/heads/x 0000']` | `['refs/heads/a\u2028', 'b 1111 refs/heads/x 0000\n']` |
| `'a\x0b\x0c\x1c\x1d\x1e\x85'` | `['a', '', '', '', '', '']` | `['a\x0b', '\x0c', '\x1c', '\x1d', '\x1e', '\x85']` |
| `'  rev: v1\r\n  rev: v2\n  rev: v3'` | `['  rev: v1', '  rev: v2', '  rev: v3']` | `['  rev: v1\r\n', '  rev: v2\n', '  rev: v3']` |

##### V-BSL: `bytes.splitlines()` (section `bytes_splitlines`)

| input | splitlines() | splitlines(True) |
|---|---|---|
| `b''` | `[]` | `[]` |
| `b'\n'` | `[b'']` | `[b'\n']` |
| `b'a\nb'` | `[b'a', b'b']` | `[b'a\n', b'b']` |
| `b'a\r\nb'` | `[b'a', b'b']` | `[b'a\r\n', b'b']` |
| `b'a\rb'` | `[b'a', b'b']` | `[b'a\r', b'b']` |
| `b'a\n\rb'` | `[b'a', b'', b'b']` | `[b'a\n', b'\r', b'b']` |
| `b'a\r\r\nb'` | `[b'a', b'', b'b']` | `[b'a\r', b'\r\n', b'b']` |
| `b'a\x0bb'` | `[b'a\x0bb']` | `[b'a\x0bb']` |
| `b'a\x0cb'` | `[b'a\x0cb']` | `[b'a\x0cb']` |
| `b'a\x1cb'` | `[b'a\x1cb']` | `[b'a\x1cb']` |
| `b'a\x85b'` | `[b'a\x85b']` | `[b'a\x85b']` |
| `b'a\xc2\x85b'` | `[b'a\xc2\x85b']` | `[b'a\xc2\x85b']` |
| `b'a\xe2\x80\xa8b'` | `[b'a\xe2\x80\xa8b']` | `[b'a\xe2\x80\xa8b']` |
| `b'#\tfoo.py\r\n#\tbar.py\n\tbaz.py'` | `[b'#\tfoo.py', b'#\tbar.py', b'\tbaz.py']` | `[b'#\tfoo.py\r\n', b'#\tbar.py\n', b'\tbaz.py']` |
| `b'a\r'` | `[b'a']` | `[b'a\r']` |
| `b'\r\n\r\n'` | `[b'', b'']` | `[b'\r\n', b'\r\n']` |
| `b'x\n'` | `[b'x']` | `[b'x\n']` |

#### A4.2 `split()` / `rsplit()` / `strip()` family

* `s.split(None, maxsplit)` (`split_whitespace` in stringlib): skip PY-WS; take a maximal non-WS run; repeat. When
  `maxsplit` is exhausted, skip the following whitespace and emit the **rest verbatim including trailing whitespace**
  (`'  a b  c  '.split(None, 2)` → `['a', 'b', 'c  ']`). Empty or all-WS input → `[]`.
* `s.rsplit(None, maxsplit)` mirrors it from the right; when exhausted, the remainder is `s[0 : i+1]` after skipping
  trailing WS, i.e. it **keeps leading whitespace** (`'  a b c d'.rsplit(maxsplit=3)` → `['  a', 'b', 'c', 'd']`).
  Result order is left-to-right.
* `strip/lstrip/rstrip()` remove PY-WS; with an argument the argument is a **set** of characters (`'a\r\n'.rstrip('\n')`
  → `'a\r'`); `strip('')` strips nothing. `bytes` variants use B-WS or the given byte set.
* `s.split(sep)` with a non-empty `sep` equals Rust `s.split(sep)` exactly (empty fields kept, `''.split(',')` == `['']`).
  `s.partition(sep)` = `split_once` with `(s, '', '')` when absent.

##### V-SPLIT: `str.split()` (sep=None)

| input | result |
|---|---|
| `''` | `[]` |
| `' '` | `[]` |
| `'  '` | `[]` |
| `'a'` | `['a']` |
| `' a '` | `['a']` |
| `'a  b'` | `['a', 'b']` |
| `'a\tb\nc'` | `['a', 'b', 'c']` |
| `'a\xa0b'` | `['a', 'b']` |
| `'a\u3000b'` | `['a', 'b']` |
| `'a\x1fb'` | `['a', 'b']` |
| `'a\x1cb'` | `['a', 'b']` |
| `'a\u200bb'` | `['a\u200bb']` |
| `'a\x85b'` | `['a', 'b']` |
| `'a\u180eb'` | `['a\u180eb']` |
| `'a\ufeffb'` | `['a\ufeffb']` |
| `'a\u2007b'` | `['a', 'b']` |
| `'a\u202fb'` | `['a', 'b']` |
| `'\t a b \n'` | `['a', 'b']` |

##### V-SPLITN: `str.split(None, maxsplit)`

| input | maxsplit | result |
|---|---|---|
| `'a b c'` | 0 | `['a b c']` |
| `'a b c'` | 1 | `['a', 'b c']` |
| `'a b c'` | 2 | `['a', 'b', 'c']` |
| `'  a b  c  '` | 0 | `['a b  c  ']` |
| `'  a b  c  '` | 1 | `['a', 'b  c  ']` |
| `'  a b  c  '` | 2 | `['a', 'b', 'c  ']` |
| `'a  b  c  d  e'` | 0 | `['a  b  c  d  e']` |
| `'a  b  c  d  e'` | 1 | `['a', 'b  c  d  e']` |
| `'a  b  c  d  e'` | 2 | `['a', 'b', 'c  d  e']` |
| `' x '` | 0 | `['x ']` |
| `' x '` | 1 | `['x']` |
| `' x '` | 2 | `['x']` |

##### V-RSPLIT: `str.rsplit(None, 3)` (pre-push field split)

| input | rsplit(maxsplit=3) |
|---|---|
| `'refs/heads/main <O1> refs/heads/main <O2>'` | `['refs/heads/main', '<O1>', 'refs/heads/main', '<O2>']` |
| `'HEAD@{1 day ago} 1111 refs/heads/x 0000'` | `['HEAD@{1 day ago}', '1111', 'refs/heads/x', '0000']` |
| `'a b c d'` | `['a', 'b', 'c', 'd']` |
| `'a b c'` | `['a', 'b', 'c']` |
| `'a b'` | `['a', 'b']` |
| `'a'` | `['a']` |
| `''` | `[]` |
| `'   '` | `[]` |
| `'a b c d e f'` | `['a b c', 'd', 'e', 'f']` |
| `'a  b  c  d  e'` | `['a  b', 'c', 'd', 'e']` |
| `'  a b c d'` | `['  a', 'b', 'c', 'd']` |
| `'  a b c d e'` | `['  a b', 'c', 'd', 'e']` |
| `'a b c d  '` | `['a', 'b', 'c', 'd']` |
| `'\ta\tb\tc\td\t'` | `['\ta', 'b', 'c', 'd']` |
| `'l\xa0ls r rs'` | `['l', 'ls', 'r', 'rs']` |
| `'l ls r\xa0x rs'` | `['l ls', 'r', 'x', 'rs']` |
| `'l ls r\x1fx rs'` | `['l ls', 'r', 'x', 'rs']` |
| `'l ls r\u3000x rs'` | `['l ls', 'r', 'x', 'rs']` |
| `'l ls r\x85x rs'` | `['l ls', 'r', 'x', 'rs']` |
| `'l ls r\u200bx rs'` | `['l', 'ls', 'r\u200bx', 'rs']` |
| `'a\x0bb c d'` | `['a', 'b', 'c', 'd']` |

##### V-SEP: `str.split(sep)` (== Rust `str::split(sep)` for non-empty sep)

| input | sep | result |
|---|---|---|
| `''` | `','` | `['']` |
| `','` | `','` | `['', '']` |
| `' a , b '` | `','` | `[' a ', ' b ']` |
| `'a,,b'` | `','` | `['a', '', 'b']` |
| `''` | `':'` | `['']` |
| `'/a::/b:'` | `':'` | `['/a', '', '/b', '']` |
| `';.EXE'` | `';'` | `['', '.EXE']` |
| `'a\x00b\x00'` | `'\x00'` | `['a', 'b', '']` |
| `'1..2'` | `'.'` | `['1', '', '2']` |
| `''` | `'.'` | `['']` |

##### V-STRIP: `str.strip()/lstrip()/rstrip()`

| input | strip() | lstrip() | rstrip() |
|---|---|---|---|
| `''` | `''` | `''` | `''` |
| `' '` | `''` | `''` | `''` |
| `'a'` | `'a'` | `'a'` | `'a'` |
| `'  a  '` | `'a'` | `'a  '` | `'  a'` |
| `'\ta\n'` | `'a'` | `'a\n'` | `'\ta'` |
| `'\r\n'` | `''` | `''` | `''` |
| `'a\xa0'` | `'a'` | `'a\xa0'` | `'a'` |
| `'\xa0a'` | `'a'` | `'a'` | `'\xa0a'` |
| `'a\x1f'` | `'a'` | `'a\x1f'` | `'a'` |
| `'a\x1c'` | `'a'` | `'a\x1c'` | `'a'` |
| `'a\x85'` | `'a'` | `'a\x85'` | `'a'` |
| `'\u3000a\u3000'` | `'a'` | `'a\u3000'` | `'\u3000a'` |
| `'a\u200b'` | `'a\u200b'` | `'a\u200b'` | `'a\u200b'` |
| `'a\ufeff'` | `'a\ufeff'` | `'a\ufeff'` | `'a\ufeff'` |
| `'\x0b\x0ca\x0c\x0b'` | `'a'` | `'a\x0c\x0b'` | `'\x0b\x0ca'` |
| `'a\x00'` | `'a\x00'` | `'a\x00'` | `'a\x00'` |
| `'\x00a\x00\x00'` | `'\x00a\x00\x00'` | `'\x00a\x00\x00'` | `'\x00a\x00\x00'` |
| `'a\u2028'` | `'a'` | `'a\u2028'` | `'a'` |
| `'a\u180e'` | `'a\u180e'` | `'a\u180e'` | `'a\u180e'` |

##### V-STRIPC: `str.strip(chars)` (chars is a set)

| input | chars | strip | lstrip | rstrip |
|---|---|---|---|---|
| `'\x00a\x00b\x00\x00'` | `'\x00'` | `'a\x00b'` | `'a\x00b\x00\x00'` | `'\x00a\x00b'` |
| `'a\n\n'` | `'\n'` | `'a'` | `'a\n\n'` | `'a'` |
| `'a\r\n'` | `'\n'` | `'a\r'` | `'a\r\n'` | `'a\r'` |
| `'\n'` | `'\n'` | `''` | `''` | `''` |
| `''` | `'\n'` | `''` | `''` | `''` |
| `'##a#'` | `'#'` | `'a'` | `'a#'` | `'##a'` |
| `'ab'` | `''` | `'ab'` | `'ab'` | `'ab'` |

##### V-BSTRIP: `bytes.strip()` and `bytes.strip(set)`

| input | chars | strip | lstrip | rstrip |
|---|---|---|---|---|
| `b''` | (None) | `b''` | `b''` | `b''` |
| `b' '` | (None) | `b''` | `b''` | `b''` |
| `b'  a  '` | (None) | `b'a'` | `b'a  '` | `b'  a'` |
| `b'\ta\n'` | (None) | `b'a'` | `b'a\n'` | `b'\ta'` |
| `b'a\x0b'` | (None) | `b'a'` | `b'a\x0b'` | `b'a'` |
| `b'a\x0c'` | (None) | `b'a'` | `b'a\x0c'` | `b'a'` |
| `b'a\x1c'` | (None) | `b'a\x1c'` | `b'a\x1c'` | `b'a\x1c'` |
| `b'a\x1f'` | (None) | `b'a\x1f'` | `b'a\x1f'` | `b'a\x1f'` |
| `b'a\x85'` | (None) | `b'a\x85'` | `b'a\x85'` | `b'a\x85'` |
| `b'a\xa0'` | (None) | `b'a\xa0'` | `b'a\xa0'` | `b'a\xa0'` |
| `b'a\xc2\xa0'` | (None) | `b'a\xc2\xa0'` | `b'a\xc2\xa0'` | `b'a\xc2\xa0'` |
| `b'\r\n'` | (None) | `b''` | `b''` | `b''` |
| `b'a\x00'` | (None) | `b'a\x00'` | `b'a\x00'` | `b'a\x00'` |
| `b'##\tfoo'` | `b'#'` | `b'\tfoo'` | `b'\tfoo'` | `b'##\tfoo'` |
| `b'#\t#foo'` | `b'#'` | `b'\t#foo'` | `b'\t#foo'` | `b'#\t#foo'` |
| `b'foo\r\n'` | `b'\r\n'` | `b'foo'` | `b'foo\r\n'` | `b'foo'` |
| `b'foo\r\r\n'` | `b'\r\n'` | `b'foo'` | `b'foo\r\r\n'` | `b'foo'` |
| `b'foo\n\r'` | `b'\r\n'` | `b'foo'` | `b'foo\n\r'` | `b'foo'` |
| `b'foo\r'` | `b'\r\n'` | `b'foo'` | `b'foo\r'` | `b'foo'` |
| `b'\r\n'` | `b'\r\n'` | `b''` | `b''` | `b''` |
| `b'foo \r\n'` | `b'\r\n'` | `b'foo '` | `b'foo \r\n'` | `b'foo '` |
| `b'foo\n'` | `b'\n'` | `b'foo'` | `b'foo\n'` | `b'foo'` |

#### A4.3 Strict UTF-8 decoding and its error text

* `b.decode()` is strict UTF-8 with codec name `'utf-8'`; a BOM is kept as U+FEFF (it is not `utf-8-sig`).
* Error = the **first** invalid sequence, reported with CPython's maximal-subpart range, which is exactly Rust's
  `Utf8Error`: `start = valid_up_to()`; `error_len() == None` → reason `unexpected end of data`, `end = len(b)`;
  `Some(n)` → `end = start + n`, reason `invalid start byte` if `n == 1` and `b[start]` ∉ `00..7F ∪ C2..F4`, else
  `invalid continuation byte`.
* Text: `'utf-8' codec can't decode byte 0x{b[start]:02x} in position {start}: {reason}` when `end - start == 1`,
  else `'utf-8' codec can't decode bytes in position {start}-{end-1}: {reason}`. Crash line:
  `An unexpected error has occurred: UnicodeDecodeError: <that text>`.
* `decode(errors='backslashreplace')`: each byte of each error range becomes `\xNN` (lowercase hex).
* Encoding direction (output, `xargs` length on POSIX): a surrogate-escaped character fails with
  `'utf-8' codec can't encode character '\udcff' in position 5: surrogates not allowed` /
  `… encode characters in position 5-6: …`.

##### V-UTF8: `bytes.decode()` (strict) — `str(UnicodeDecodeError)`

| bytes | result |
|---|---|
| `b''` | `''` |
| `b'ok'` | `'ok'` |
| `b'\xff'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` |
| `b'\x80'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0x80 in position 0: invalid start byte` |
| `b'a\xc3'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc3 in position 1: unexpected end of data` |
| `b'\xc3('` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc3 in position 0: invalid continuation byte` |
| `b'\xe2\x82'` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: unexpected end of data` |
| `b'\xe2(\xa1'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe2 in position 0: invalid continuation byte` |
| `b'\xe2\x82('` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte` |
| `b'\xf0\x9f\x98'` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-2: unexpected end of data` |
| `b'\xf0(\x8c('` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xf0 in position 0: invalid continuation byte` |
| `b'\xc0\xaf'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc0 in position 0: invalid start byte` |
| `b'\xc1\xbf'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xc1 in position 0: invalid start byte` |
| `b'\xe0\x80\xaf'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe0 in position 0: invalid continuation byte` |
| `b'\xe0\xa0'` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: unexpected end of data` |
| `b'\xed\xa0\x80'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte` |
| `b'\xf4\x90\x80\x80'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xf4 in position 0: invalid continuation byte` |
| `b'\xf5\x80'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xf5 in position 0: invalid start byte` |
| `b'\xf8\x88\x80\x80\x80'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xf8 in position 0: invalid start byte` |
| `b'ok\xffok'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 2: invalid start byte` |
| `b'abc\xe2\x82'` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 3-4: unexpected end of data` |
| `b'\xe2\x82abc'` | `UnicodeDecodeError: 'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte` |
| `b'\xf0\x9f\x98\x80\xff'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 4: invalid start byte` |
| `b'caf\xe9.txt\x00'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 3: invalid continuation byte` |
| `b'\xef\xbb\xbfbom'` | `'\ufeffbom'` |

##### V-BSR: `bytes.decode(errors='backslashreplace')`

| bytes | result |
|---|---|
| `b'git version 2.45.0\n'` | `'git version 2.45.0\n'` |
| `b'git version \xff\xfe\n'` | `'git version \\xff\\xfe\n'` |
| `b'\xe2\x82abc'` | `'\\xe2\\x82abc'` |
| `b'\xf0\x9f\x98'` | `'\\xf0\\x9f\\x98'` |
| `b'\xc3\xa9\x80'` | `'é\\x80'` |

#### A4.4 Text-mode newline translation and `textwrap.indent`

* Read, `newline=None` (default): `\r\n` → `\n`, then lone `\r` → `\n` (also across buffer boundaries). Nothing else
  is translated (`\x85`, U+2028 survive). Read, `newline=''`: no translation.
* Write, `newline=None`: every `\n` → `os.linesep` (Windows: an existing `\r\n` becomes `\r\r\n`). `newline=''`: none.
* `textwrap.indent(text, prefix)` = for each `text.splitlines(True)` line, prefix it iff `line.strip()` is non-empty
  (3.13 rewrote it as `not line.isspace()`: same result).

##### V-TIO: text-mode read (`newline=None` vs `newline=''`)

| file bytes | newline=None | newline='' |
|---|---|---|
| `b'a\r\nb\rc\nd'` | `'a\nb\nc\nd'` | `'a\r\nb\rc\nd'` |
| `b'\r'` | `'\n'` | `'\r'` |
| `b'\r\r\n'` | `'\n\n'` | `'\r\r\n'` |
| `b'\n\r'` | `'\n\n'` | `'\n\r'` |
| `b'a\xc2\x85b\xe2\x80\xa8c'` | `'a\x85b\u2028c'` | `'a\x85b\u2028c'` |
| `b'\xef\xbb\xbfa\r\n'` | `'\ufeffa\n'` | `'\ufeffa\r\n'` |
| `b'a\r\n'` | `'a\n'` | `'a\r\n'` |
| `b''` | `''` | `''` |
| `b'x\x0cy\x0bz'` | `'x\x0cy\x0bz'` | `'x\x0cy\x0bz'` |

##### V-TIOW: text-mode write (`open(f, "w")`)

| str | POSIX bytes | Windows bytes (os.linesep) |
|---|---|---|
| `'a\nb\n'` | `b'a\nb\n'` | `b'a\r\nb\r\n'` |
| `'a\r\nb'` | `b'a\r\nb'` | `b'a\r\r\nb'` |
| `'a\rb'` | `b'a\rb'` | `b'a\rb'` |
| `''` | `b''` | `b''` |
| `'no-eol'` | `b'no-eol'` | `b'no-eol'` |

#### A4.5 pre-commit composites over the string primitives

`zsplit(s)` (`git.py:19-24`): `s.strip('\0')`; empty → `[]`; else `split('\0')`. `_get_skips` (`run.py:129-131`): a
set; order never observable. Pre-push: §A5 D1. Merge message: eager decode per conflict line.

##### V-ZSPLIT: `git.zsplit` (`git.py:19-24`)

| input | result |
|---|---|
| `''` | `[]` |
| `'\x00'` | `[]` |
| `'\x00\x00'` | `[]` |
| `'a'` | `['a']` |
| `'a\x00'` | `['a']` |
| `'a\x00b\x00'` | `['a', 'b']` |
| `'\x00a\x00'` | `['a']` |
| `'a\x00\x00b\x00'` | `['a', '', 'b']` |
| `' a \x00 b\x00'` | `[' a ', ' b']` |
| `'a b\x00'` | `['a b']` |

##### V-SKIP: `SKIP` parsing (`run.py:129-131`), set shown sorted

| SKIP | set |
|---|---|
| `''` | `[]` |
| `' '` | `[]` |
| `','` | `[]` |
| `'a'` | `['a']` |
| `'a,b'` | `['a', 'b']` |
| `' a , b '` | `['a', 'b']` |
| `'a,,b'` | `['a', 'b']` |
| `',a,'` | `['a']` |
| `'a,a'` | `['a']` |
| `'a b,c'` | `['a b', 'c']` |
| `'a\xa0,b'` | `['a', 'b']` |
| `'\xa0'` | `[]` |
| `'a\x1f'` | `['a']` |
| `'a\x1c,b'` | `['a', 'b']` |
| `'a\u200b'` | `['a\u200b']` |
| `'a\tb'` | `['a\tb']` |
| `'flake8, mypy ,\tblack\n'` | `['black', 'flake8', 'mypy']` |

##### V-PP: pre-push stdin parsing (`hook_impl.py:128-130`; `<O1>`/`<O2>` = 40×`1`/`2`, `<Z40>` = 40×`0`; input shown decoded with backslashreplace)

| stdin bytes | per line: fields, or the exception that pre-commit raises (exit 3) |
|---|---|
| `'refs/heads/main <O1> refs/heads/main <O2>\n'` | `['refs/heads/main', '<O1>', 'refs/heads/main', '<O2>']` |
| `'refs/heads/a <O1> refs/heads/a <Z40>\nrefs/heads/b <O2> refs/heads/b <Z40>\n'` | `['refs/heads/a', '<O1>', 'refs/heads/a', '<Z40>'] ; ['refs/heads/b', '<O2>', 'refs/heads/b', '<Z40>']` |
| `'(delete) <Z40> refs/heads/gone <O2>\n'` | `['(delete)', '<Z40>', 'refs/heads/gone', '<O2>']` |
| `'HEAD@{1 day ago} <O1> refs/heads/x <Z40>\n'` | `['HEAD@{1 day ago}', '<O1>', 'refs/heads/x', '<Z40>']` |
| `'refs/heads/main <O1> refs/heads/main <O2>\r\n'` | `['refs/heads/main', '<O1>', 'refs/heads/main', '<O2>']` |
| `'refs/heads/main <O1> refs/heads/main <O2>'` | `['refs/heads/main', '<O1>', 'refs/heads/main', '<O2>']` |
| `''` | `(no lines)` |
| `'\n'` | `ValueError: not enough values to unpack (expected 4, got 0)` |
| `'refs/heads/main <O1> refs/heads/main <O2>\n\n'` | `['refs/heads/main', '<O1>', 'refs/heads/main', '<O2>'] ; ValueError: not enough values to unpack (expected 4, got 0)` |
| `'refs/heads/main <O1> refs/heads/main\n'` | `ValueError: not enough values to unpack (expected 4, got 3)` |
| `'refs/heads/café <O1> refs/heads/x <Z40>\n'` | `['refs/heads/café', '<O1>', 'refs/heads/x', '<Z40>']` |
| `'refs/heads/caf\\xe9 <O1> refs/heads/x <Z40>\n'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 14: invalid continuation byte` |
| `'refs/heads/a\u2028b <O1> refs/heads/a\u2028b <Z40>\n'` | `ValueError: not enough values to unpack (expected 4, got 1)` |
| `'refs/heads/a\x85b <O1> refs/heads/a\x85b <Z40>\n'` | `ValueError: not enough values to unpack (expected 4, got 1)` |
| `'refs/heads/a\xa0b <O1> refs/heads/a\xa0b <Z40>\n'` | `['refs/heads/a\xa0b <O1>', 'refs/heads/a', 'b', '<Z40>']` |
| `'refs/heads/a\u3000b <O1> refs/heads/x <Z40>\n'` | `['refs/heads/a\u3000b', '<O1>', 'refs/heads/x', '<Z40>']` |
| `'refs/heads/x <O1> refs/heads/a\x1fb <Z40>\n'` | `['refs/heads/x <O1>', 'refs/heads/a', 'b', '<Z40>']` |
| `'  refs/heads/x <O1> refs/heads/x <Z40>  \n'` | `['  refs/heads/x', '<O1>', 'refs/heads/x', '<Z40>']` |
| `'refs/heads/x\t<O1>\trefs/heads/x\t<Z40>\n'` | `['refs/heads/x', '<O1>', 'refs/heads/x', '<Z40>']` |
| `'refs/heads/x <O1> refs/heads/x <Z40>\x0crefs/heads/y <O2> refs/heads/y <Z40>\n'` | `['refs/heads/x', '<O1>', 'refs/heads/x', '<Z40>'] ; ['refs/heads/y', '<O2>', 'refs/heads/y', '<Z40>']` |
| `'refs/heads/x <O1> refs/heads/x <Z40>\rrefs/heads/y <O2> refs/heads/y <Z40>\n'` | `['refs/heads/x', '<O1>', 'refs/heads/x', '<Z40>'] ; ['refs/heads/y', '<O2>', 'refs/heads/y', '<Z40>']` |

##### V-MM: `parse_merge_msg_for_conflicts` (`git.py:104-111`)

| MERGE_MSG bytes | result |
|---|---|
| `b"Merge branch 'foo'\n\n# Conflicts:\n#\tconflict_file\n#\tother\n"` | `['conflict_file', 'other']` |
| `b"Merge branch 'foo'\n\nConflicts:\n\tconflict_file\n"` | `['conflict_file']` |
| `b'Merge\r\n\r\n# Conflicts:\r\n#\ta.txt\r\n#\tb c.txt \r\n'` | `['a.txt', 'b c.txt']` |
| `b'#\t  spaced  \n##\tdouble\n# \tnot\n\t\x0bvt\x0b\n#\tx\x0cy\n'` | `['spaced', 'vt', 'x\x0cy']` |
| `b'#\tcaf\xc3\xa9\n'` | `['café']` |
| `b'#\tcaf\xe9\n'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 3: unexpected end of data` |
| `b'#\ta\x85b\n#\tc\x1cd\n'` | `UnicodeDecodeError: 'utf-8' codec can't decode byte 0x85 in position 1: invalid start byte` |
| `b'#\tfile\rwith-cr\n'` | `['file']` |
| `b''` | `[]` |

#### A4.6 `REV_LINE_RE`, rev-line rewrite, migrate-config header split

`REV_LINE_RE = re.compile(r'^(\s+)rev:(\s*)([\'"]?)([^\s#]+)(.*)(\r?\n)$')` (`commands/autoupdate.py:116`) is applied
with `.match` to each `splitlines(True)` line of the file read with `newline=''`. Exact regex-free equivalent
(normative; verified against 632 vectors incl. 600 random lines, prototype `rev_line_match_manual`):

1. g1 = the maximal leading PY-WS run; must be non-empty (`rev` cannot be WS, so no backtracking is possible).
2. Literal `rev:`; g2 = the maximal PY-WS run after it.
3. g3 = `'` or `"` **only if** the next char exists and is neither PY-WS nor `#`; otherwise g3 = `''` (Python
   backtracks and the quote becomes part of g4).
4. g4 = maximal run of chars that are neither PY-WS nor `#`; must be non-empty. (A closing quote is part of g4:
   `"    rev: 'v1.0.0'\n"` → g4 = `"v1.0.0'"`.)
5. The rest must contain a `\n`: g5 = everything before the first `\n` (`.` matches `\r`, `\x85`, U+2028…), and after
   that `\n` only end-of-string or exactly one more `\n` may follow (Python `$`). **g6 is always `"\n"`**; a CRLF
   line's `\r` is the last char of g5.
6. Lines ending in another PY-LB (`\x0b \x0c \x1c-\x1e \x85 \u2028 \u2029`) or in lone `\r`, and a last line without
   `\n`, never match. If the number of matching lines ≠ the number of non-`local`/`meta` repos (`autoupdate.py:128-137`,
   `config_repos` at `:174-177`; a `rev:` line inside a local repo therefore forces the reformat), the file is rewritten as `yaml_dump(yaml_load(original))` (comments lost, text-mode
   write) and re-scanned once; a second mismatch is `AssertionError: could not find rev lines` (exit 3).

Regex form for the `regex` crate (also verified): `\A([S]+)rev:([S]*)(['"]?)([^S#]+)(.*)(\r?\n)\n?\z` with `S` =
`PY_STR_SPACE_CLASS` and the trailing `\n?` standing for Python's `$` (the match span is never used).

Rewrite (`autoupdate.py:146-156`): `comment = '  # frozen: <tag>'` if freezing; else `''` if
`g5.strip().startswith('# frozen:')`; else `g5`. Line = `g1 + 'rev:' + g2 + new_rev + comment + g6`, where `new_rev` =
`yaml_dump({'rev': rev}, default_style=g3).split(':', 1)[1].strip()` (YAML emitter: doc 07 §3.1.5). Because g6 is
`\n`, whenever `comment` replaces g5 the `\r` is lost (V-REVW rows 3-4, 7; OBS on 4.6.2 with a CRLF config:
`rev: <sha>  # frozen: v2.0.0\n` and, after un-freezing, `rev: v2.0.0\n` amid `\r\n` lines).

`_migrate_map` (`migrate_config.py:25-46`): header = the longest prefix of `splitlines(True)` lines that start with
`#` or `---` or are PY-WS-only (an all-NBSP or `\x1f` line counts as blank; a ZWSP line does not).

##### V-REV: `REV_LINE_RE.match(line).groups()` (`autoupdate.py:116`)

| line | groups 1..6 (None = no match) |
|---|---|
| `'    rev: v1.0.0\n'` | `('    ', ' ', '', 'v1.0.0', '', '\n')` |
| `'    rev: v1.0.0\r\n'` | `('    ', ' ', '', 'v1.0.0', '\r', '\n')` |
| `'    rev: v1.0.0'` | `None` |
| `"    rev: 'v1.0.0'\n"` | `('    ', ' ', "'", "v1.0.0'", '', '\n')` |
| `'    rev: "v1.0.0"\n'` | `('    ', ' ', '"', 'v1.0.0"', '', '\n')` |
| `'-   rev: v1\n'` | `None` |
| `'  - rev: v1\n'` | `None` |
| `'rev: v1\n'` | `None` |
| `'    rev:v1\n'` | `('    ', '', '', 'v1', '', '\n')` |
| `'    rev:\tv1\n'` | `('    ', '\t', '', 'v1', '', '\n')` |
| `'    rev: v1  # frozen: v1.0.0\n'` | `('    ', ' ', '', 'v1', '  # frozen: v1.0.0', '\n')` |
| `'    rev: abc  # frozen: v1\r\n'` | `('    ', ' ', '', 'abc', '  # frozen: v1\r', '\n')` |
| `'    rev: v1 # comment\n'` | `('    ', ' ', '', 'v1', ' # comment', '\n')` |
| `'    rev: #x\n'` | `None` |
| `'    rev: \n'` | `None` |
| `'\xa0\xa0rev: v1\n'` | `('\xa0\xa0', ' ', '', 'v1', '', '\n')` |
| `'\x1f rev: v1\n'` | `('\x1f ', ' ', '', 'v1', '', '\n')` |
| `'    rev:\xa0v1\n'` | `('    ', '\xa0', '', 'v1', '', '\n')` |
| `'    rev: v1\xa0\n'` | `('    ', ' ', '', 'v1', '\xa0', '\n')` |
| `'    rev: v1\x1f\n'` | `('    ', ' ', '', 'v1', '\x1f', '\n')` |
| `'    rev: v1\u3000x\n'` | `('    ', ' ', '', 'v1', '\u3000x', '\n')` |
| `'    rev: v1\r\r\n'` | `('    ', ' ', '', 'v1', '\r\r', '\n')` |
| `'    rev: v1\n\n'` | `('    ', ' ', '', 'v1', '', '\n')` |
| `'    rev: v1\x85'` | `None` |
| `'    rev: v1\u2028'` | `None` |
| `'    rev: vé\n'` | `('    ', ' ', '', 'vé', '', '\n')` |
| `"    rev: 'v1'  \n"` | `('    ', ' ', "'", "v1'", '  ', '\n')` |
| `'    rev: "v1\'\n'` | `('    ', ' ', '"', "v1'", '', '\n')` |
| `'\trev: v1\n'` | `('\t', ' ', '', 'v1', '', '\n')` |
| `'    revision: v1\n'` | `None` |
| `'    rev: [v1]\n'` | `('    ', ' ', '', '[v1]', '', '\n')` |
| `'    rev: v1\r'` | `None` |

##### V-REVW: rev-line rewrite (`autoupdate.py:146-156`; `new_rev` = rendered YAML scalar)

| line | new_rev | frozen | result |
|---|---|---|---|
| `'    rev: v1.0.0\n'` | `'v2.0.0'` | `None` | `'    rev: v2.0.0\n'` |
| `'    rev: v1.0.0\r\n'` | `'v2.0.0'` | `None` | `'    rev: v2.0.0\r\n'` |
| `'    rev: v1.0.0\r\n'` | `'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'` | `'v2.0.0'` | `'    rev: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # frozen: v2.0.0\n'` |
| `'    rev: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # frozen: v1\r\n'` | `'v2'` | `None` | `'    rev: v2\n'` |
| `'    rev: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # frozen: v1\n'` | `'v2'` | `None` | `'    rev: v2\n'` |
| `'    rev: v1  # keep me\n'` | `'v2'` | `None` | `'    rev: v2  # keep me\n'` |
| `'    rev: v1  # keep me\r\n'` | `'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'` | `'v2'` | `'    rev: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  # frozen: v2\n'` |
| `"    rev: 'v1'\n"` | `"'v2'"` | `None` | `"    rev: 'v2'\n"` |
| `'    rev: v1 #frozen: x\n'` | `'v2'` | `None` | `'    rev: v2 #frozen: x\n'` |
| `'    rev: v1\t# frozen: x\n'` | `'v2'` | `None` | `'    rev: v2\n'` |
| `'    rev: v1\xa0# frozen: x\n'` | `'v2'` | `None` | `'    rev: v2\n'` |

##### V-AUL: `_original_lines` rev-line detection (`autoupdate.py:120-130`)

| file text (newline='') | lines | rev idxs |
|---|---|---|
| `'repos:\n-   repo: r\n    rev: v1\n    hooks:\n    -   id: x\n'` | `['repos:\n', '-   repo: r\n', '    rev: v1\n', '    hooks:\n', '    -   id: x\n']` | `[2]` |
| `'repos:\r\n-   repo: r\r\n    rev: v1\r\n    hooks: []\r\n'` | `['repos:\r\n', '-   repo: r\r\n', '    rev: v1\r\n', '    hooks: []\r\n']` | `[2]` |
| `'repos:\n-   repo: r\n    rev: v1'` | `['repos:\n', '-   repo: r\n', '    rev: v1']` | `[]` |
| `'repos:\n-   repo: r\n    rev: v1\x0c\n'` | `['repos:\n', '-   repo: r\n', '    rev: v1\x0c', '\n']` | `[]` |
| `'repos:\n-   repo: r\n    rev: v1\x85    rev: v2\n'` | `['repos:\n', '-   repo: r\n', '    rev: v1\x85', '    rev: v2\n']` | `[3]` |
| `'repos:\n-   repo: r\n    rev: v1\u2028\n'` | `['repos:\n', '-   repo: r\n', '    rev: v1\u2028', '\n']` | `[]` |
| `'repos:\r-   repo: r\r    rev: v1\r'` | `['repos:\r', '-   repo: r\r', '    rev: v1\r']` | `[]` |
| `'repos:\n-   {repo: r, rev: v1, hooks: []}\n'` | `['repos:\n', '-   {repo: r, rev: v1, hooks: []}\n']` | `[]` |
| `'repos:\n- repo: r\n  rev: v1 # c\n  hooks: []\n'` | `['repos:\n', '- repo: r\n', '  rev: v1 # c\n', '  hooks: []\n']` | `[2]` |

##### V-MIG: `_migrate_map` header split + `textwrap.indent(rest, "    ")` (`migrate_config.py:21-44`)

| contents | header | rest | indented rest |
|---|---|---|---|
| `'-   repo: r\n    hooks: []\n'` | `''` | `'-   repo: r\n    hooks: []\n'` | `'    -   repo: r\n        hooks: []\n'` |
| `'# comment\n---\n\n-   repo: r\n'` | `'# comment\n---\n\n'` | `'-   repo: r\n'` | `'    -   repo: r\n'` |
| `'#c\n   \n\t\n-   repo: r\n'` | `'#c\n   \n\t\n'` | `'-   repo: r\n'` | `'    -   repo: r\n'` |
| `'\xa0\n-   repo: r\n'` | `'\xa0\n'` | `'-   repo: r\n'` | `'    -   repo: r\n'` |
| `'\x1f\n-   repo: r\n'` | `'\x1f\n'` | `'-   repo: r\n'` | `'    -   repo: r\n'` |
| `'\u200b\n-   repo: r\n'` | `''` | `'\u200b\n-   repo: r\n'` | `'    \u200b\n    -   repo: r\n'` |
| `'#c\x0c-   repo: r\n'` | `'#c\x0c'` | `'-   repo: r\n'` | `'    -   repo: r\n'` |
| `'# only comments\n'` | `'# only comments\n'` | `''` | `''` |
| `''` | `''` | `''` | `''` |
| `'- a\n\n- b\n   \n- c'` | `''` | `'- a\n\n- b\n   \n- c'` | `'    - a\n\n    - b\n   \n    - c'` |
| `'- a\n\x0b- b\n'` | `''` | `'- a\n\x0b- b\n'` | `'    - a\n\x0b    - b\n'` |

#### A4.7 `repr()`, exception texts, `int()`

`repr(str)` (`unicode_repr`):

1. Quote = `"` iff the text contains `'` and no `"`; else `'`.
2. Per code point: the quote char and `\` → backslash + char; `\t` `\n` `\r` → `\t` `\n` `\r`; other `< U+0020` and
   U+007F → `\xNN`; other ASCII → itself; non-ASCII **printable** (PY-PRINTABLE) → itself; else `\xNN` (≤ U+00FF),
   `\uNNNN` (≤ U+FFFF), `\UNNNNNNNN`. Hex is lowercase. Lone surrogates (POSIX `surrogateescape` bytes, Windows
   unpaired UTF-16) → `\udcNN`/`\uNNNN`.
3. A POSIX `OsStr` is shown as Python would after `os.fsdecode`: valid UTF-8 as text, each byte of an invalid range as
   U+DC00+byte (prototype `repr_fsbytes`).

Containers: `list` → `[` + `', '`-joined item reprs + `]`; `tuple` → same with `()` and a trailing `,` for exactly one
item (`('a',)`, `((),)`); `dict` → `{k: v, …}` in insertion order; `None`/`True`/`False`; `int` decimal (arbitrary
size: YAML ints exceed i64); `float` = shortest round-trip digits, positional when `-4 < decpt <= 16` (`decpt` =
exponent of the leading digit + 1), else `d[.ddd]e±XX` with at least two exponent digits; always a `.0` for
integral positional values; `inf`, `-inf`, `nan`, `-0.0`. Recipe without deps: `format!("{:e}", x.abs())` gives the
shortest digits and exponent; re-layout per the rule (prototype `repr_float`, verified on the vectors; doc 09 App. B
has more float vectors).

Exception texts: `str(OSError(errno, strerror, filename[, None, filename2]))` = `[Errno {errno}] {strerror}: {repr(filename)}[ -> {repr(filename2)}]`,
without filename `[Errno N] strerror`; Windows `[WinError N] <FormatMessage text>`; strerror text is libc's (glibc
strings in `errno_strerror_glibc`; Rust `io::Error::from_raw_os_error(n).to_string()` minus ` (os error n)` gives the
same libc text). `subprocess.Popen` on POSIX attaches the **executable as passed** as `filename` (ENOENT/EACCES/
ENOEXEC), or the `cwd` for a failed `chdir`. `str(KeyError(k))` = `repr(k)`. `str(subprocess.CalledProcessError)`
(stdlib): `Command '{str(cmd)}' returned non-zero exit status {rc}.`; negative rc: `Command '{cmd}' died with <Signals.SIGKILL: 9>.`
or `… died with unknown signal {n}.`. `int(s)` failure: `invalid literal for int() with base 10: {repr(s)[:200]}`
(the repr is cut at 200 **characters**, no closing quote; the literal is the original, unstripped string).

`int(s)` (base 10): strip PY-WS at both ends; optional single `+`/`-` directly before the digits; PY-DECIMAL digits
(any script, mixed allowed: `'12٠'` → 120); single `_` only between digits (`1_0` ok; `_1`, `1_`, `1__0`, `+_1`
fail); no prefixes, no `.`; no inner whitespace (`'- 1'` fails).

##### V-REPR: `repr()` (input shown as `ascii()` to make it unambiguous)

| input (ascii()) | repr() output |
|---|---|
| `''` | `''` |
| `'a'` | `'a'` |
| `"it's"` | `"it's"` |
| `'say "hi"'` | `'say "hi"'` |
| `'both \'"'` | `'both \'"'` |
| `'back\\slash'` | `'back\\slash'` |
| `'^\\.py('` | `'^\\.py('` |
| `'tab\t'` | `'tab\t'` |
| `'nl\n'` | `'nl\n'` |
| `'cr\r'` | `'cr\r'` |
| `'nul\x00'` | `'nul\x00'` |
| `'bel\x07'` | `'bel\x07'` |
| `'\x1b[31m'` | `'\x1b[31m'` |
| `'del\x7f'` | `'del\x7f'` |
| `'\x80'` | `'\x80'` |
| `'\x9f'` | `'\x9f'` |
| `'\xa0'` | `'\xa0'` |
| `'\xad'` | `'\xad'` |
| `'\xe9'` | `'é'` |
| `'caf\xe9'` | `'café'` |
| `'\xdf'` | `'ß'` |
| `'\u0378'` | `'\u0378'` |
| `'\u200b'` | `'\u200b'` |
| `'\u200e'` | `'\u200e'` |
| `'\u2028'` | `'\u2028'` |
| `'\u2029'` | `'\u2029'` |
| `'\u3000'` | `'\u3000'` |
| `'\ufeff'` | `'\ufeff'` |
| `'\ufffd'` | `'�'` |
| `'\uffff'` | `'\uffff'` |
| `'\ue000'` | `'\ue000'` |
| `'\U0001f600'` | `'😀'` |
| `'\U0001fae9'` | `'\U0001fae9'` |
| `'\U000e0001'` | `'\U000e0001'` |
| `'\U0010ffff'` | `'\U0010ffff'` |
| `'\U000f0000'` | `'\U000f0000'` |
| `'C:\\Users\\a b\\python.exe'` | `'C:\\Users\\a b\\python.exe'` |
| `'/tmp/it\'s "x"'` | `'/tmp/it\'s "x"'` |
| `"'"` | `"'"` |
| `'"'` | `'"'` |
| `'\\'` | `'\\'` |
| `'a\x0bb\x0cc'` | `'a\x0bb\x0cc'` |
| `'x\x85y'` | `'x\x85y'` |
| `'\U0001f1fa\U0001f1f8'` | `'🇺🇸'` |
| `'e\u0301'` | `'é'` |
| `'\u0600'` | `'\u0600'` |
| `'\u061c'` | `'\u061c'` |
| `'\u1680'` | `'\u1680'` |
| `'\U00013430'` | `'\U00013430'` |
| `'\U0001d173'` | `'\U0001d173'` |
| `'sub/'` | `'sub/'` |
| `'hook-impl'` | `'hook-impl'` |
| `'\udcff'` | `'\udcff'` |
| `'/tmp/\udcff\udc80'` | `'/tmp/\udcff\udc80'` |
| `[]` | `[]` |
| `['a']` | `['a']` |
| `['a', 'b']` | `['a', 'b']` |
| `["it's"]` | `["it's"]` |
| `['a', ['b', ('c',)]]` | `['a', ['b', ('c',)]]` |
| `[1, 'a']` | `[1, 'a']` |
| `[True, False, None]` | `[True, False, None]` |
| `[1.0, 1e+16, 1.5e-07, -0.0]` | `[1.0, 1e+16, 1.5e-07, -0.0]` |
| `()` | `()` |
| `('a',)` | `('a',)` |
| `('a', 'b')` | `('a', 'b')` |
| `('/usr/bin/git', 'write-tree')` | `('/usr/bin/git', 'write-tree')` |
| `('git', 'diff-index', '--ignore-submodules', '--binary', '--exit-code', '--no-color', '--no-ext-diff', 'deadbeef', '--')` | `('git', 'diff-index', '--ignore-submodules', '--binary', '--exit-code', '--no-color', '--no-ext-diff', 'deadbeef', '--')` |
| `((),)` | `((),)` |
| `([],)` | `([],)` |
| `{}` | `{}` |
| `{'a': 1}` | `{'a': 1}` |
| `{'k': ['v']}` | `{'k': ['v']}` |
| `0` | `0` |
| `-1` | `-1` |
| `100000000000000000000` | `100000000000000000000` |
| `True` | `True` |
| `None` | `None` |
| `1.0` | `1.0` |
| `0.1` | `0.1` |
| `1e+16` | `1e+16` |
| `1e-05` | `1e-05` |
| `inf` | `inf` |
| `nan` | `nan` |
| `['--hook-type=pre-commit', 'x y']` | `['--hook-type=pre-commit', 'x y']` |
| `['pyyaml==6.0', "it's"]` | `['pyyaml==6.0', "it's"]` |

##### V-FEQ: `f'{prefix=}'` (`hazmat.py:51`)

| prefix | rendered |
|---|---|
| `'sub/'` | `prefix='sub/'` |
| `"it's/"` | `prefix="it's/"` |
| `'a\\b/'` | `prefix='a\\b/'` |
| `'tab\t/'` | `prefix='tab\t/'` |

##### V-OSE: `str(OSError(*args))`

| args | str() |
|---|---|
| `(2, 'No such file or directory', './x.sh')` | `[Errno 2] No such file or directory: './x.sh'` |
| `(13, 'Permission denied', '/repo/hook.sh')` | `[Errno 13] Permission denied: '/repo/hook.sh'` |
| `(8, 'Exec format error', './noshebang.sh')` | `[Errno 8] Exec format error: './noshebang.sh'` |
| `(7, 'Argument list too long', '/usr/bin/git')` | `[Errno 7] Argument list too long: '/usr/bin/git'` |
| `(2, 'No such file or directory')` | `[Errno 2] No such file or directory` |
| `(2, 'No such file or directory', "it's")` | `[Errno 2] No such file or directory: "it's"` |
| `(2, 'No such file or directory', 'a', None, 'b')` | `[Errno 2] No such file or directory: 'a' -> 'b'` |
| `('just a message',)` | `just a message` |
| `()` | *(empty string)* |

##### V-CPE: `str(subprocess.CalledProcessError(rc, cmd))` (stdlib; `hook_impl.py:143-165`)

| rc | cmd | str() |
|---|---|---|
| 128 | `('git', 'rev-list', 'refs/heads/a', '--topo-order', '--reverse', '--not', '--remotes=origin')` | `Command '('git', 'rev-list', 'refs/heads/a', '--topo-order', '--reverse', '--not', '--remotes=origin')' returned non-zero exit status 128.` |
| 1 | `('git', 'rev-parse', 'x^')` | `Command '('git', 'rev-parse', 'x^')' returned non-zero exit status 1.` |
| -9 | `('git', 'x')` | `Command '('git', 'x')' died with <Signals.SIGKILL: 9>.` |
| -15 | `('git', 'x')` | `Command '('git', 'x')' died with <Signals.SIGTERM: 15>.` |
| -99 | `('git',)` | `Command '('git',)' died with unknown signal 99.` |
| 1 | `'git x'` | `Command 'git x' returned non-zero exit status 1.` |
| 2 | `['git', "it's"]` | `Command '['git', "it's"]' returned non-zero exit status 2.` |

##### V-INT: `clientlib.parse_version` = `tuple(int(p) for p in s.split("."))`

| s | result |
|---|---|
| `'1.2.3'` | `(1, 2, 3)` |
| `'4.6.2'` | `(4, 6, 2)` |
| `'1'` | `(1,)` |
| `'1. 2'` | `(1, 2)` |
| `' 1.2 '` | `(1, 2)` |
| `'1.x'` | `ValueError: invalid literal for int() with base 10: 'x'` |
| `''` | `ValueError: invalid literal for int() with base 10: ''` |
| `'1..2'` | `ValueError: invalid literal for int() with base 10: ''` |
| `'1_0.2'` | `(10, 2)` |
| `'٣.1'` | `(3, 1)` |
| `'1.2-dev'` | `ValueError: invalid literal for int() with base 10: '2-dev'` |
| `"it's"` | `ValueError: invalid literal for int() with base 10: "it's"` |
| `'1.2\n'` | `(1, 2)` |

#### A4.8 shlex

* `quote(s)`: `''` → `"''"`; unchanged iff every char is ASCII `[A-Za-z0-9_@%+=:,./-]` (non-ASCII letters are
  unsafe: `'é'` → `"'é'"`); else `'` + `s.replace("'", "'\"'\"'")` + `'`. `join(xs)` = `' '.join(map(quote, xs))`.
* `split(s)` = `shlex(s, posix=True)` with `whitespace_split=True`, `commenters=''`; SHLEX-WS separators; `'…'` literal;
  `"…"`: backslash escapes only `"` and `\`, otherwise kept (`"a\$b"` → `a\$b`); outside quotes backslash escapes any
  char incl. newline (`a\<LF>b` → `a<LF>b`); `''`/`""` produce an empty token; `#` is an ordinary char. Errors
  (`ValueError`): `No closing quotation` (EOF in quotes), `No escaped character` (EOF after `\`, also inside `"…"`).
  State machine: prototype `shlex::split` (verified on 51 + 400 random vectors). Same conclusions as doc 02 §21.7 /
  04b §5.3 / 07 §3.3.

##### V-SQ: `shlex.quote`

| s | quote(s) |
|---|---|
| `''` | `"''"` |
| `'a'` | `'a'` |
| `'a b'` | `"'a b'"` |
| `"it's"` | `'\'it\'"\'"\'s\''` |
| `'é'` | `"'é'"` |
| `'a$b'` | `"'a$b'"` |
| `'-x'` | `'-x'` |
| `'=x'` | `'=x'` |
| `'~'` | `"'~'"` |
| `'*'` | `"'*'"` |
| `'!'` | `"'!'"` |
| `'a\nb'` | `"'a\nb'"` |
| `'@%+=:,./-_'` | `'@%+=:,./-_'` |
| `'C:\\Python\\python.exe'` | `"'C:\\Python\\python.exe'"` |
| `'/usr/bin/python3'` | `'/usr/bin/python3'` |
| `"'"` | `'\'\'"\'"\'\''` |
| `'"'` | `'\'"\''` |
| `'a"b'` | `'\'a"b\''` |
| `'\t'` | `"'\t'"` |
| `'--config=.pre-commit-config.yaml'` | `'--config=.pre-commit-config.yaml'` |
| `'--config=my config.yaml'` | `"'--config=my config.yaml'"` |
| `'/home/u/.cache/pre-commit/repoabc/py_env-python3'` | `'/home/u/.cache/pre-commit/repoabc/py_env-python3'` |
| `'x\x00y'` | `"'x\x00y'"` |
| `'A-Z_a-z0-9'` | `'A-Z_a-z0-9'` |
| `'été'` | `"'été'"` |
| `'#'` | `"'#'"` |

##### V-SJ: `shlex.join`

| args | join(args) |
|---|---|
| `[]` | `''` |
| `['']` | `"''"` |
| `['hook-impl', '--config=.pre-commit-config.yaml', '--hook-type=pre-commit']` | `'hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit'` |
| `['hook-impl', '--config=a b.yaml', '--hook-type=pre-push', '--skip-on-missing-config']` | `"hook-impl '--config=a b.yaml' --hook-type=pre-push --skip-on-missing-config"` |
| `['a b', "c'd", '', '$x']` | `'\'a b\' \'c\'"\'"\'d\' \'\' \'$x\''` |

##### V-SS: `shlex.split` (posix, whitespace_split, no comments)

| s | result |
|---|---|
| `''` | `[]` |
| `' '` | `[]` |
| `'a'` | `['a']` |
| `'a b'` | `['a', 'b']` |
| `'  a   b  '` | `['a', 'b']` |
| `'a\tb\nc'` | `['a', 'b', 'c']` |
| `"'a b'"` | `['a b']` |
| `'"a b"'` | `['a b']` |
| `"a'b'c"` | `['abc']` |
| `'a"b"c'` | `['abc']` |
| `'"a\\"b"'` | `['a"b']` |
| `'"a\\\\b"'` | `['a\\b']` |
| `'"a\\$b"'` | `['a\\$b']` |
| `'"a\\b"'` | `['a\\b']` |
| `` '"a\\`b"' `` | `` ['a\\`b'] `` |
| `'"a\\\nb"'` | `['a\\\nb']` |
| `"'a\\b'"` | `['a\\b']` |
| `'a\\ b'` | `['a b']` |
| `'a\\\\b'` | `['a\\b']` |
| `'a\\$b'` | `['a$b']` |
| `'a\\\nb'` | `['a\nb']` |
| `'\\'` | `ValueError: No escaped character` |
| `'"unterminated'` | `ValueError: No closing quotation` |
| `"'unterminated"` | `ValueError: No closing quotation` |
| `'a # b'` | `['a', '#', 'b']` |
| `'a#b'` | `['a#b']` |
| `'#a'` | `['#a']` |
| `"''"` | `['']` |
| `'""'` | `['']` |
| `"'' x"` | `['', 'x']` |
| `"a''b"` | `['ab']` |
| `'pre-commit hazmat cd sub cmd'` | `['pre-commit', 'hazmat', 'cd', 'sub', 'cmd']` |
| `'\xa0a'` | `['\xa0a']` |
| `'a\xa0b'` | `['a\xa0b']` |
| `'a\x1fb'` | `['a\x1fb']` |
| `'a\x0bb'` | `['a\x0bb']` |
| `'a\x0cb'` | `['a\x0cb']` |
| `'a\rb'` | `['a', 'b']` |
| `'é ü'` | `['é', 'ü']` |
| `'\u3000a'` | `['\u3000a']` |
| `'python -c "import sys; print(sys.argv)"'` | `['python', '-c', 'import sys; print(sys.argv)']` |
| `'~/bin/x'` | `['~/bin/x']` |
| `'a;b'` | `['a;b']` |
| `'a\|b'` | `['a\|b']` |
| `'a&&b'` | `['a&&b']` |
| `'$HOME'` | `['$HOME']` |
| `'"$HOME"'` | `['$HOME']` |
| `'a\\'` | `ValueError: No escaped character` |
| `'"a\\'` | `ValueError: No escaped character` |
| `"it's"` | `ValueError: No closing quotation` |
| `"'it'\\''s'"` | `["it's"]` |

#### A4.9 `posixpath` (Linux and macOS; operate on bytes)

* `normpath`: `''`→`'.'`; initial slashes: exactly 2 are kept, 1 or ≥3 → 1; drop `''`/`'.'` components; `..` pops a
  previous non-`..` component, is dropped at the root, kept when leading a relative path; join with `/`; empty → `.`.
* `join(a, *p)`: an absolute later part replaces; otherwise add `/` unless the path is empty or ends in `/`
  (`join('a', '')` → `'a/'`).
* `split(p)`: tail = after the last `/`; head has trailing `/` removed unless it is all slashes. `dirname`/`basename`
  are its halves (`basename('a/b/')` == `''`, unlike Rust `Path::file_name`).
* `splitext(p)`: last `.` after the last `/`, ignoring leading dots of the name (`'...md'` → no extension, unlike Rust
  `Path::extension` → `"md"`); the extension includes the dot (`'foo.'` → `('foo', '.')`).
* `isabs`: starts with `/`. `abspath(p)` = `normpath(p if isabs else join(getcwd(), p))` — **lexical**; `getcwd()` is the
  physical cwd (same as `std::env::current_dir`).
* `relpath(path, start='.')`: `''` → `ValueError('no path specified')`; compare `abspath` component lists (empty
  components dropped), then `..`×rest + tail, `.` if equal.
* `commonpath(paths)`: empty → `ValueError('commonpath() arg is an empty sequence')`; mixed abs/rel →
  `ValueError("Can't mix absolute and relative paths")`; compares components after dropping `''` and `'.'`; **does
  not** resolve `..` (`['/a/../b', '/a/b']` → `'/a'`).
* `expanduser(p)`: only a leading `~`/`~user` up to the first `/`; `~` uses `os.environ['HOME']` **if the key exists,
  even empty**, else the passwd entry of the real uid (unknown → unchanged); `~user` → passwd (`getpwnam`, unknown →
  unchanged); `userhome.rstrip('/')` then `(userhome + rest) or '/'`.
* `realpath(p)` (non-strict; `posixpath.py` `_joinrealpath`): walk components left to right on a growing path;
  `''`/`.` skipped; `..` → `split(path)` (or `..` when the path is empty / ends in `..`); for a name, `lstat`
  (**any** OSError → "not a link"); a link is resolved recursively with `os.readlink` and a `seen` map; a link already
  being resolved (loop) returns `join(link_path, rest)` **unresolved**; the result goes through `abspath`. Never
  fails for missing paths; an error from `os.readlink` (after `lstat` said "link") propagates.

##### V-PN: `posixpath.normpath`

| p | normpath(p) |
|---|---|
| `''` | `'.'` |
| `'.'` | `'.'` |
| `'..'` | `'..'` |
| `'/'` | `'/'` |
| `'//'` | `'//'` |
| `'///'` | `'/'` |
| `'////a'` | `'/a'` |
| `'//a//b'` | `'//a/b'` |
| `'/..'` | `'/'` |
| `'/../a'` | `'/a'` |
| `'../..'` | `'../..'` |
| `'a/..'` | `'.'` |
| `'a/../..'` | `'..'` |
| `'./a/./b/.'` | `'a/b'` |
| `'a//b'` | `'a/b'` |
| `'a/b/'` | `'a/b'` |
| `'a/b/../../..'` | `'..'` |
| `'/a/b/../../../c'` | `'/c'` |
| `'~'` | `'~'` |
| `'~/x/../y'` | `'~/y'` |
| `'a/./../b'` | `'b'` |
| `'.a/..b/...'` | `'.a/..b/...'` |
| `'/./'` | `'/'` |
| `'a\\b'` | `'a\\b'` |
| `'C:\\x'` | `'C:\\x'` |
| `'/a/b/./c/'` | `'/a/b/c'` |
| `'..//./..'` | `'../..'` |

##### V-PA: `posixpath.abspath` (cwd `'/home/u/repo/sub'`)

| p | abspath(p) |
|---|---|
| `''` | `'/home/u/repo/sub'` |
| `'.'` | `'/home/u/repo/sub'` |
| `'x'` | `'/home/u/repo/sub/x'` |
| `'../x'` | `'/home/u/repo/x'` |
| `'../../../../../x'` | `'/x'` |
| `'/abs/./x/..'` | `'/abs'` |
| `'//x'` | `'//x'` |
| `'a/b/'` | `'/home/u/repo/sub/a/b'` |
| `'~'` | `'/home/u/repo/sub/~'` |

##### V-PJ: `posixpath.join`

| args | join(*args) |
|---|---|
| `['a', 'b']` | `'a/b'` |
| `['a/', 'b']` | `'a/b'` |
| `['a', '/b']` | `'/b'` |
| `['a', '']` | `'a/'` |
| `['', 'b']` | `'b'` |
| `['', '']` | `''` |
| `['/', 'x']` | `'/x'` |
| `['a', 'b', '/c', 'd']` | `'/c/d'` |
| `['a//', 'b']` | `'a//b'` |
| `['/r', 'py_env-/usr/bin/python3']` | `'/r/py_env-/usr/bin/python3'` |
| `['/r', 'py_env-3.12']` | `'/r/py_env-3.12'` |

##### V-PR: `posixpath.relpath` (cwd `'/home/u/repo/sub'`)

| path | start | relpath |
|---|---|---|
| `'/home/u/repo/sub/x'` | `None` | `'x'` |
| `'/home/u/repo/x'` | `None` | `'../x'` |
| `'/home/u/repo'` | `None` | `'..'` |
| `'/home/u/repo/sub'` | `None` | `'.'` |
| `'/'` | `None` | `'../../../..'` |
| `'/etc/passwd'` | `'/home/u/repo'` | `'../../../etc/passwd'` |
| `'/home/u/repo/a/b'` | `'/home/u/repo'` | `'a/b'` |
| `'/home/u/repo'` | `'/home/u/repo'` | `'.'` |
| `'x'` | `None` | `'x'` |
| `''` | `None` | `ValueError: no path specified` |
| `'/home/u/repo/.pre-commit-config.yaml'` | `'/home/u/repo'` | `'.pre-commit-config.yaml'` |
| `'/a//b/./c'` | `'/a/b'` | `'c'` |
| `'//a'` | `'/a'` | `'.'` |
| `'/a'` | `'/a/'` | `'.'` |

##### V-PC: `posixpath.commonpath`

| paths | result |
|---|---|
| `['/usr/bin/node', '/home/u']` | `'/'` |
| `['/home/u/.nvm/bin/node', '/home/u']` | `'/home/u'` |
| `['/home/u', '/home/u']` | `'/home/u'` |
| `['/home/user2/x', '/home/u']` | `'/home'` |
| `['bin/node', '/home/u']` | `ValueError: Can't mix absolute and relative paths` |
| `['/', '/home/u']` | `'/'` |
| `[]` | `ValueError: commonpath() arg is an empty sequence` |
| `['a/b', 'a/c']` | `'a'` |
| `['/a/./b', '/a/b/c']` | `'/a/b'` |
| `['/a/../b', '/a/b']` | `'/a'` |
| `['//a', '/a']` | `'/a'` |
| `['/home/u/', '/home/u']` | `'/home/u'` |
| `['', '']` | `''` |
| `['/x', '']` | `ValueError: Can't mix absolute and relative paths` |

##### V-PP2: `posixpath` split family

| p | dirname | basename | splitext | isabs |
|---|---|---|---|---|
| `''` | `''` | `''` | `('', '')` | False |
| `'/'` | `'/'` | `''` | `('/', '')` | True |
| `'//'` | `'//'` | `''` | `('//', '')` | True |
| `'a'` | `''` | `'a'` | `('a', '')` | False |
| `'/a'` | `'/'` | `'a'` | `('/a', '')` | True |
| `'a/'` | `'a'` | `''` | `('a/', '')` | False |
| `'/a/b'` | `'/a'` | `'b'` | `('/a/b', '')` | True |
| `'a/b/'` | `'a/b'` | `''` | `('a/b/', '')` | False |
| `'//a//b'` | `'//a'` | `'b'` | `('//a//b', '')` | True |
| `'a.b.c'` | `''` | `'a.b.c'` | `('a.b', '.c')` | False |
| `'.bashrc'` | `''` | `'.bashrc'` | `('.bashrc', '')` | False |
| `'..'` | `''` | `'..'` | `('..', '')` | False |
| `'foo.'` | `''` | `'foo.'` | `('foo', '.')` | False |
| `'...md'` | `''` | `'...md'` | `('...md', '')` | False |
| `'a..md'` | `''` | `'a..md'` | `('a.', '.md')` | False |
| `'dir.d/file'` | `'dir.d'` | `'file'` | `('dir.d/file', '')` | False |
| `'/a/b.tar.gz'` | `'/a'` | `'b.tar.gz'` | `('/a/b.tar', '.gz')` | True |
| `'C:\\x\\y.exe'` | `''` | `'C:\\x\\y.exe'` | `('C:\\x\\y', '.exe')` | False |
| `'app.json'` | `''` | `'app.json'` | `('app', '.json')` | False |
| `'/x/coursier-app.json'` | `'/x'` | `'coursier-app.json'` | `('/x/coursier-app', '.json')` | True |

##### V-PE: `posixpath.expanduser` by `HOME` value

| HOME | `~` | `~/` | `~/.cache` | `x/~` | `~~` | `~/~` |
|---|---|---|---|---|---|---|
| `'/home/u'` | `'/home/u'` | `'/home/u/'` | `'/home/u/.cache'` | `'x/~'` | `'~~'` | `'/home/u/~'` |
| `'/home/u/'` | `'/home/u'` | `'/home/u/'` | `'/home/u/.cache'` | `'x/~'` | `'~~'` | `'/home/u/~'` |
| `''` | `'/'` | `'/'` | `'/.cache'` | `'x/~'` | `'~~'` | `'/~'` |
| `'/'` | `'/'` | `'/'` | `'/.cache'` | `'x/~'` | `'~~'` | `'/~'` |
| `'//'` | `'/'` | `'/'` | `'/.cache'` | `'x/~'` | `'~~'` | `'/~'` |
| `'rel/home'` | `'rel/home'` | `'rel/home/'` | `'rel/home/.cache'` | `'x/~'` | `'~~'` | `'rel/home/~'` |

##### V-RP: `posixpath.realpath` (non-strict) on fixture: dirs `a`, `a/b`; file `a/f`; symlinks `link_a`→`a`, `link_abs`→`$ROOT/a/b`, `link_dotdot`→`a/b/..`, `loop1`→`loop2`, `loop2`→`loop1`, `self`→`self`, `dangling`→`nonexistent`, `link_file`→`a/f`, `chain`→`link_a`, `a/b/up`→`../..`; cwd `$ROOT`

| input | realpath |
|---|---|
| `'$ROOT'` | `'$ROOT'` |
| `'$ROOT/a'` | `'$ROOT/a'` |
| `'$ROOT/link_a'` | `'$ROOT/a'` |
| `'$ROOT/link_a/b'` | `'$ROOT/a/b'` |
| `'$ROOT/link_a/..'` | `'$ROOT'` |
| `'$ROOT/link_abs'` | `'$ROOT/a/b'` |
| `'$ROOT/link_abs/..'` | `'$ROOT/a'` |
| `'$ROOT/link_dotdot'` | `'$ROOT/a'` |
| `'$ROOT/loop1'` | `'$ROOT/loop1'` |
| `'$ROOT/loop1/x'` | `'$ROOT/loop1/x'` |
| `'$ROOT/self'` | `'$ROOT/self'` |
| `'$ROOT/dangling'` | `'$ROOT/nonexistent'` |
| `'$ROOT/dangling/x'` | `'$ROOT/nonexistent/x'` |
| `'$ROOT/link_file'` | `'$ROOT/a/f'` |
| `'$ROOT/link_file/x'` | `'$ROOT/a/f/x'` |
| `'$ROOT/chain/b'` | `'$ROOT/a/b'` |
| `'$ROOT/missing/../a'` | `'$ROOT/a'` |
| `'$ROOT/missing/x'` | `'$ROOT/missing/x'` |
| `'$ROOT/a/b/up'` | `'$ROOT'` |
| `'$ROOT/a/b/up/a'` | `'$ROOT/a'` |
| `'$ROOT/./a//b/'` | `'$ROOT/a/b'` |
| `'link_a'` | `'$ROOT/a'` |
| `'.'` | `'$ROOT'` |
| `''` | `'$ROOT'` |
| `'missing'` | `'$ROOT/missing'` |
| `'$ROOT/a/f/..'` | `'$ROOT/a'` |
| `'$ROOT/missing/../link_a'` | `'$ROOT/a'` |
| `'$ROOT/missing/../dangling'` | `'$ROOT/nonexistent'` |
| `'$ROOT/link_a/../link_file'` | `'$ROOT/a/f'` |

#### A4.10 `ntpath` (Windows; all lexical functions testable on Linux)

* `splitroot(p)` (3.12; `/` treated as `\`): UNC `\\server\share` / `\\?\UNC\server\share` → drive = everything
  before the separator that follows the share name (no such separator → the whole input is the drive); `\\?\X:` and
  `\\.\dev` are device drives by the same rule; `\x` → root only; `X:` (any first char, colon second) → drive, then
  an optional root.
  `splitdrive(p)` = `(drive, root+tail)`.
* `normpath`: `/`→`\`; `splitroot`; then the posix algorithm on `\` components except `..` at the start of a rooted
  path is dropped and a drive-relative `C:..` keeps `..`; `\\?\` paths are normalized like any other in 3.12
  (V-NN); empty without drive/root → `.`.
* `join`: a later rooted part replaces drive+root (keeps the old drive if it has none); a later different drive
  (case-insensitive) replaces everything; same drive in another case adopts the later spelling; add `\` unless the
  path ends in `\`/`/`; UNC drive + relative tail gets a `\`.
* `split`/`dirname`/`basename`/`splitext`: on both separators; dirname keeps drive+root (`'//srv/share/x'` →
  `'//srv/share/'`).
* `isabs`: 3.12: `\x` and `/x` are absolute (legacy); **3.13+**: only drive+root or UNC. pre-commit never calls it
  directly; it only matters inside `abspath`'s fallback.
* `normcase`: `/`→`\` + lowercase. CPython on Windows uses `LCMapStringEx(LOCALE_NAME_INVARIANT, LCMAP_LOWERCASE)`
  (simple 1:1 mapping per UTF-16 unit); the Linux fallback is `str.lower()` (full mapping). Identical for ASCII.
* `abspath`: on real Windows `_getfullpathname(normpath(p))` (`GetFullPathNameW`): resolves root-relative `\x`
  against the current drive, drive-relative `D:x` against that drive's cwd (`=D:` env), strips trailing dots/spaces
  of the last component, maps reserved device names. The JSON vectors are the **fallback** (`normpath(join(cwd, p))`)
  and flag every case where the real result may differ (`real_windows_may_differ`). Rust: `std::path::absolute`
  calls `GetFullPathNameW` on Windows → use `absolute(nt_normpath(p))`.
* `relpath(path, start='.')`: `abspath(normpath(…))` of both, drives compared with `normcase`; different →
  `ValueError("path is on mount %r, start on mount %r" % (path_drive, start_drive))`; components compared with
  `normcase`; result joined with `\`. UNC drive appears as `'\\\\srv\\share'` in the message (repr doubles `\`).
* `commonpath`: lowercases (`str.lower`) copies for comparison; 3.12 order of checks: empty → `commonpath() arg is an
  empty sequence`, roots differ → `Can't mix absolute and relative paths`, drives differ → `Paths don't have the same
  drive` (3.13 checks drive first and says "empty iterable"); result = drive + root of the **first** path (original
  case, `/`→`\`) + its common components. pre-commit only calls it in `exe_exists`, which swallows `ValueError`.
* `expanduser`: `~` up to the first `\` or `/`; home = `USERPROFILE` if the key exists (even empty) else
  `join(HOMEDRIVE or '', HOMEPATH)` if `HOMEPATH` exists else return unchanged; `HOME` ignored; `~user`: if
  `USERNAME != user`, require `basename(home) == USERNAME` and use `join(dirname(home), user)`, else unchanged. No
  separator normalization (`'C:\\Users\\u/.cache'`).
* `realpath` (Windows only, `ntpath.py` `realpath`): `normpath` **first** (`..` is lexical, unlike POSIX); `\\.\NUL`
  for `devnull`; relative → join cwd; `_getfinalpathname` (= `GetFinalPathNameByHandleW`: resolves links/junctions,
  real on-disk case, SUBST/mapped drives to targets); on failure with an allowed winerror (1, 2, 3, 5, 21, 32, 50, 53,
  65, 67, 87, 123, 161, 1920, 1921) walk up: `_readlink_deep` on the failing component (dangling link → target), else
  pop the last component into a tail and retry; finally strip `\\?\` (`\\?\UNC\` → `\\`) if the stripped path resolves
  to the same final path. Rust: `std::fs::canonicalize` + `dunce::simplified`, with the walk-up and `read_link` for
  missing paths (this is what 07 §7 item 20 describes; it is wrong only for POSIX).

##### V-NN: `ntpath.normpath`

| p | normpath(p) |
|---|---|
| `''` | `'.'` |
| `'.'` | `'.'` |
| `'..'` | `'..'` |
| `'\\'` | `'\\'` |
| `'/'` | `'\\'` |
| `'C:'` | `'C:'` |
| `'C:\\'` | `'C:\\'` |
| `'c:/'` | `'c:\\'` |
| `'C:..'` | `'C:..'` |
| `'C:\\..'` | `'C:\\'` |
| `'C:\\a\\..\\..\\b'` | `'C:\\b'` |
| `'C:a\\..\\..'` | `'C:..'` |
| `'a/b\\c'` | `'a\\b\\c'` |
| `'a//b'` | `'a\\b'` |
| `'\\\\server\\share'` | `'\\\\server\\share'` |
| `'\\\\server\\share\\..\\x'` | `'\\\\server\\share\\x'` |
| `'//server/share/a/../b'` | `'\\\\server\\share\\b'` |
| `'\\\\?\\C:\\a\\..\\b'` | `'\\\\?\\C:\\b'` |
| `'\\\\.\\pipe\\x'` | `'\\\\.\\pipe\\x'` |
| `'\\a\\..\\..'` | `'\\'` |
| `'a\\b\\'` | `'a\\b'` |
| `'C:\\a.\\b '` | `'C:\\a.\\b '` |
| `'..\\..\\a'` | `'..\\..\\a'` |
| `'\\\\'` | `'\\\\'` |
| `'\\\\server'` | `'\\\\server'` |
| `'C:\\Users\\u\\.cache\\pre-commit\\repo\\node_env-default'` | `'C:\\Users\\u\\.cache\\pre-commit\\repo\\node_env-default'` |

##### V-NJ: `ntpath.join`

| args | join(*args) |
|---|---|
| `['a', 'b']` | `'a\\b'` |
| `['a\\', 'b']` | `'a\\b'` |
| `['a/', 'b']` | `'a/b'` |
| `['a', '\\b']` | `'\\b'` |
| `['C:\\a', 'b']` | `'C:\\a\\b'` |
| `['C:\\a', 'D:b']` | `'D:b'` |
| `['C:\\a', 'D:\\b']` | `'D:\\b'` |
| `['C:\\a', '\\b']` | `'C:\\b'` |
| `['c:\\a', 'C:b']` | `'C:\\a\\b'` |
| `['C:', 'b']` | `'C:b'` |
| `['C:', '\\b']` | `'C:\\b'` |
| `['a', '']` | `'a\\'` |
| `['', 'b']` | `'b'` |
| `['\\\\srv\\share', 'x']` | `'\\\\srv\\share\\x'` |
| `['\\\\srv\\share\\', '\\x']` | `'\\\\srv\\share\\x'` |
| `['C:\\r', 'py_env-C:\\Python312\\python.exe']` | `'C:\\r\\py_env-C:\\Python312\\python.exe'` |
| `['C:\\r', 'node_env-default']` | `'C:\\r\\node_env-default'` |

##### V-NA: `ntpath.abspath` fallback (cwd `'C:\\Users\\u\\repo\\sub'`); `real≠` = real Windows (GetFullPathNameW) may differ

| p | abspath(p) | real≠ |
|---|---|---|
| `''` | `'C:\\Users\\u\\repo\\sub'` |  |
| `'.'` | `'C:\\Users\\u\\repo\\sub'` |  |
| `'x'` | `'C:\\Users\\u\\repo\\sub\\x'` |  |
| `'..\\x'` | `'C:\\Users\\u\\repo\\x'` |  |
| `'../x'` | `'C:\\Users\\u\\repo\\x'` |  |
| `'C:\\abs\\x\\..'` | `'C:\\abs'` |  |
| `'D:\\x'` | `'D:\\x'` |  |
| `'D:x'` | `'D:x'` | yes |
| `'\\x'` | `'\\x'` | yes |
| `'/x'` | `'\\x'` | yes |
| `'\\\\srv\\share\\x'` | `'\\\\srv\\share\\x'` |  |
| `'a/b/'` | `'C:\\Users\\u\\repo\\sub\\a\\b'` |  |

##### V-NR: `ntpath.relpath` (cwd `'C:\\Users\\u\\repo\\sub'`)

| path | start | relpath | real≠ |
|---|---|---|---|
| `'C:\\Users\\u\\repo\\sub\\x'` | `None` | `'x'` |  |
| `'C:\\Users\\u\\repo\\x'` | `None` | `'..\\x'` |  |
| `'c:\\users\\U\\REPO\\x'` | `'C:\\Users\\u\\repo'` | `'x'` |  |
| `'D:\\x'` | `None` | `ValueError: path is on mount 'D:', start on mount 'C:'` |  |
| `'D:\\x'` | `'C:\\Users\\u\\repo'` | `ValueError: path is on mount 'D:', start on mount 'C:'` |  |
| `'C:/Users/u/repo/a/b'` | `'C:\\Users\\u\\repo'` | `'a\\b'` |  |
| `'\\\\srv\\share\\x'` | `'C:\\'` | `ValueError: path is on mount '\\\\srv\\share', start on mount 'C:'` |  |
| `'x'` | `None` | `'x'` |  |
| `''` | `None` | `ValueError: no path specified` |  |
| `'C:\\Users\\u\\repo'` | `'C:\\Users\\u\\repo'` | `'.'` |  |

##### V-NC: `ntpath.commonpath` (3.12)

| paths | result |
|---|---|
| `['C:\\Program Files\\nodejs\\node.exe', 'C:\\Users\\u']` | `'C:\\'` |
| `['C:\\Users\\u\\AppData\\x.exe', 'C:\\Users\\u']` | `'C:\\Users\\u'` |
| `['c:\\users\\U\\x.exe', 'C:\\Users\\u']` | `'c:\\users\\U'` |
| `['D:\\tools\\node.exe', 'C:\\Users\\u']` | `ValueError: Paths don't have the same drive` |
| `['bin\\node.exe', 'C:\\Users\\u']` | `ValueError: Can't mix absolute and relative paths` |
| `['C:/a/b', 'C:\\a\\c']` | `'C:\\a'` |
| `['\\a\\b', 'C:\\a']` | `ValueError: Paths don't have the same drive` |
| `['\\\\srv\\share\\a', '\\\\SRV\\Share\\b']` | `'\\\\srv\\share\\'` |
| `[]` | `ValueError: commonpath() arg is an empty sequence` |

##### V-NP: `ntpath` split family (3.12)

| p | dirname | basename | splitext | splitdrive | isabs | normcase |
|---|---|---|---|---|---|---|
| `''` | `''` | `''` | `('', '')` | `('', '')` | False | `''` |
| `'C:'` | `'C:'` | `''` | `('C:', '')` | `('C:', '')` | False | `'c:'` |
| `'C:\\'` | `'C:\\'` | `''` | `('C:\\', '')` | `('C:', '\\')` | True | `'c:\\'` |
| `'C:x'` | `'C:'` | `'x'` | `('C:x', '')` | `('C:', 'x')` | False | `'c:x'` |
| `'C:\\a\\b.exe'` | `'C:\\a'` | `'b.exe'` | `('C:\\a\\b', '.exe')` | `('C:', '\\a\\b.exe')` | True | `'c:\\a\\b.exe'` |
| `'a/b\\c.txt'` | `'a/b'` | `'c.txt'` | `('a/b\\c', '.txt')` | `('', 'a/b\\c.txt')` | False | `'a\\b\\c.txt'` |
| `'\\a'` | `'\\'` | `'a'` | `('\\a', '')` | `('', '\\a')` | True | `'\\a'` |
| `'/a'` | `'/'` | `'a'` | `('/a', '')` | `('', '/a')` | True | `'\\a'` |
| `'\\\\srv\\share'` | `'\\\\srv\\share'` | `''` | `('\\\\srv\\share', '')` | `('\\\\srv\\share', '')` | True | `'\\\\srv\\share'` |
| `'\\\\srv\\share\\x\\y'` | `'\\\\srv\\share\\x'` | `'y'` | `('\\\\srv\\share\\x\\y', '')` | `('\\\\srv\\share', '\\x\\y')` | True | `'\\\\srv\\share\\x\\y'` |
| `'//srv/share/x'` | `'//srv/share/'` | `'x'` | `('//srv/share/x', '')` | `('//srv/share', '/x')` | True | `'\\\\srv\\share\\x'` |
| `'C:\\a\\'` | `'C:\\a'` | `''` | `('C:\\a\\', '')` | `('C:', '\\a\\')` | True | `'c:\\a\\'` |
| `'x.'` | `''` | `'x.'` | `('x', '.')` | `('', 'x.')` | False | `'x.'` |
| `'C:\\Python312\\python.EXE'` | `'C:\\Python312'` | `'python.EXE'` | `('C:\\Python312\\python', '.EXE')` | `('C:', '\\Python312\\python.EXE')` | True | `'c:\\python312\\python.exe'` |
| `'\\\\?\\C:\\x'` | `'\\\\?\\C:\\'` | `'x'` | `('\\\\?\\C:\\x', '')` | `('\\\\?\\C:', '\\x')` | True | `'\\\\?\\c:\\x'` |

##### V-NE: `ntpath.expanduser` by environment

| env | `~` | `~/.cache` | `~\.cache` | `~other\x` |
|---|---|---|---|---|
| `{"USERPROFILE": "C:\\Users\\u"}` | `'C:\\Users\\u'` | `'C:\\Users\\u/.cache'` | `'C:\\Users\\u\\.cache'` | `'~other\\x'` |
| `{"USERPROFILE": ""}` | `''` | `'/.cache'` | `'\\.cache'` | `'~other\\x'` |
| `{"HOMEDRIVE": "D:", "HOMEPATH": "\\home\\u"}` | `'D:\\home\\u'` | `'D:\\home\\u/.cache'` | `'D:\\home\\u\\.cache'` | `'~other\\x'` |
| `{"HOMEPATH": "\\home\\u"}` | `'\\home\\u'` | `'\\home\\u/.cache'` | `'\\home\\u\\.cache'` | `'~other\\x'` |
| `{}` | `'~'` | `'~/.cache'` | `'~\\.cache'` | `'~other\\x'` |
| `{"HOME": "/home/posix"}` | `'~'` | `'~/.cache'` | `'~\\.cache'` | `'~other\\x'` |
| `{"USERNAME": "u", "USERPROFILE": "C:\\Users\\u"}` | `'C:\\Users\\u'` | `'C:\\Users\\u/.cache'` | `'C:\\Users\\u\\.cache'` | `'C:\\Users\\other\\x'` |

#### A4.11 Executable lookup and the path composites

`find_executable(exe, env)` (`parse_shebang.py:22-43`): `exe = normpath(exe)` (so `'bin1/../tool3'` → `'tool3'`, and a
bare name stays bare); if `os.sep` in it → return it **unchecked**; `exts = PATHEXT.split(os.pathsep)` **whenever the
key exists (any OS)**, candidates = `[exe+e for e in exts] + [exe]`; for each `dir` in `PATH.split(os.pathsep)`
(unset → `['']`) — **directory-major** — for each candidate: `join(dir, cand)`; return the first that `isfile` and
`access(X_OK)`; the returned string is the join as written (relative stays relative, case as in PATH/PATHEXT, e.g.
`node.EXE`). `normexe` (`:46-62`) calls it only when `orig` has no `os.sep` (nor `/` on Windows); otherwise
``Executable `{orig}` is a directory`` / ``… not found`` / ``… is not executable``. `exe_exists` (`lang_base.py:63-83`) adds the
shim/home test (V-EXE): `not re.search(r'[/\\]shims[/\\]', found) and (dirname(home) == home or commonpath((found, home)) != home)`
with `ValueError` → `None` (so a relative `found`, or one on another drive, counts as "outside home"; on Windows the
string comparison is case-sensitive although `commonpath` is not: `'C:\\users\\u\\x\\node.EXE'` vs home
`'C:\\Users\\u'` → `True`).

`_adjust_args_and_chdir` (`main.py:175-199`): `config` → `abspath` only if it exists; `files`/`commit_msg_filename`
→ always `abspath`; try-repo `repo` → `abspath` if it exists; `chdir(toplevel)`; then `relpath` of each (so a missing
config stays as typed but is now relative to the toplevel). Windows `Classifier` then replaces `\` by `/`
(`commands/run.py:123-124`).

`Store` dir (`store.py:25-36`): `PRE_COMMIT_HOME or join(XDG_CACHE_HOME or expanduser('~/.cache'), 'pre-commit')`
(empty values count as unset), then `realpath`; computed **before** the chdir (`main.py:380` vs `:383`), so a relative
`PRE_COMMIT_HOME` is relative to the invocation cwd. `HOME=''` → `/.cache/pre-commit`. Windows: `USERPROFILE` +
`/.cache` + `\pre-commit`, normalized to backslashes by `realpath`.

`init-templatedir` (`commands/init_templatedir.py:29-38`): `configured = realpath(expanduser(git_config_value.strip()))`
(no `%(prefix)` support; only `~`), compared as strings with `realpath(directory)`.

##### V-EXE: `lang_base.exe_exists` pure part (after `find_executable` → `found`, `expanduser("~")` → `home`)

| flavor | found | home | exe_exists |
|---|---|---|---|
| posix | `'/usr/bin/node'` | `'/home/u'` | True |
| posix | `'/home/u/.nvm/versions/node/v20/bin/node'` | `'/home/u'` | False |
| posix | `'/home/u/.pyenv/shims/python'` | `'/home/u'` | False |
| posix | `'/opt/x/shims/ruby'` | `'/home/u'` | False |
| posix | `'/usr/bin/node'` | `'/'` | True |
| posix | `'/home/u/bin/node'` | `'/'` | True |
| posix | `'bin/node'` | `'/home/u'` | True |
| posix | `'/home/u/bin/node'` | `'/home/u/'` | True |
| posix | `'/home/u2/node'` | `'/home/u'` | True |
| posix | `'/home/u/node'` | `''` | True |
| nt | `'C:\\Program Files\\nodejs\\node.EXE'` | `'C:\\Users\\u'` | True |
| nt | `'C:\\Users\\u\\AppData\\Roaming\\npm\\node.EXE'` | `'C:\\Users\\u'` | False |
| nt | `'c:\\users\\U\\scoop\\shims\\node.EXE'` | `'C:\\Users\\u'` | False |
| nt | `'C:\\users\\u\\x\\node.EXE'` | `'C:\\Users\\u'` | True |
| nt | `'D:\\tools\\node.EXE'` | `'C:\\Users\\u'` | True |
| nt | `'C:\\Users\\u/bin\\node.EXE'` | `'C:\\Users\\u'` | False |
| nt | `'C:\\x\\node.EXE'` | `'C:\\'` | True |

##### V-ADJ: `_adjust_args_and_chdir` (`main.py:175-199`): abspath in cwd, then relpath from toplevel

| flavor | cwd | toplevel | kind | arg | exists | result | Classifier (nt `\`→`/`) |
|---|---|---|---|---|---|---|---|
| posix | `'/r/sub'` | `'/r'` | config | `'.pre-commit-config.yaml'` | False | `'.pre-commit-config.yaml'` |  |
| posix | `'/r/sub'` | `'/r'` | config | `'my.yaml'` | True | `'sub/my.yaml'` |  |
| posix | `'/r/sub'` | `'/r'` | config | `'../.pre-commit-config.yaml'` | True | `'.pre-commit-config.yaml'` |  |
| posix | `'/r/sub'` | `'/r'` | config | `'/etc/x.yaml'` | True | `'../etc/x.yaml'` |  |
| posix | `'/r/sub'` | `'/r'` | config | `''` | False | `ValueError: no path specified` |  |
| posix | `'/r/sub'` | `'/r'` | files | `'a.py'` | True | `'sub/a.py'` |  |
| posix | `'/r/sub'` | `'/r'` | files | `'../b.py'` | True | `'b.py'` |  |
| posix | `'/r/sub'` | `'/r'` | files | `'missing.py'` | False | `'sub/missing.py'` |  |
| posix | `'/r/sub'` | `'/r'` | files | `'/outside/c.py'` | True | `'../outside/c.py'` |  |
| posix | `'/r/sub'` | `'/r'` | files | `'d/../e.py'` | True | `'sub/e.py'` |  |
| posix | `'/r/sub'` | `'/r'` | files | `''` | False | `'sub'` |  |
| posix | `'/r'` | `'/r'` | files | `'./x//y.py'` | True | `'x/y.py'` |  |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | config | `'.pre-commit-config.yaml'` | False | `'.pre-commit-config.yaml'` |  |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | config | `'my.yaml'` | True | `'sub\\my.yaml'` |  |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | files | `'a/b.py'` | True | `'sub\\a\\b.py'` | `'sub/a/b.py'` |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | files | `'..\\c.py'` | True | `'c.py'` | `'c.py'` |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | files | `'D:\\x.py'` | True | `ValueError: path is on mount 'D:', start on mount 'C:'` |  |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | files | `'c:\\R\\SUB\\x.py'` | True | `'SUB\\x.py'` | `'SUB/x.py'` |
| nt | `'C:\\r\\sub'` | `'C:\\r'` | config | `'D:\\cfg.yaml'` | True | `ValueError: path is on mount 'D:', start on mount 'C:'` |  |

##### V-FE: `parse_shebang.find_executable` (POSIX) on fixture: `bin1/tool` (755), `bin2/tool` (644), `rel/tool2` (755), `tool3` (755), `extd/tool.sh` (755), `extd/tool` (755), `bin1/both` (755); dir `bin3/tool/`; cwd `$ROOT`

| exe | env | result |
|---|---|---|
| `'tool'` | `{"PATH": "$ROOT/bin2:$ROOT/bin1"}` | `'$ROOT/bin1/tool'` |
| `'tool'` | `{"PATH": "$ROOT/bin3:$ROOT/bin1"}` | `'$ROOT/bin1/tool'` |
| `'tool'` | `{"PATH": "$ROOT/bin2"}` | `None` |
| `'tool2'` | `{"PATH": "rel"}` | `'rel/tool2'` |
| `'tool2'` | `{"PATH": "./rel/"}` | `'./rel/tool2'` |
| `'tool3'` | `{"PATH": ""}` | `'tool3'` |
| `'tool3'` | `{}` | `'tool3'` |
| `'tool3'` | `{"PATH": ":"}` | `'tool3'` |
| `'tool3'` | `{"PATH": "$ROOT/bin1:"}` | `'tool3'` |
| `'tool3'` | `{"PATH": "$ROOT/bin1"}` | `None` |
| `'tool'` | `{"PATH": "$ROOT/extd", "PATHEXT": ".sh:.py"}` | `'$ROOT/extd/tool.sh'` |
| `'tool'` | `{"PATH": "$ROOT/extd", "PATHEXT": ""}` | `'$ROOT/extd/tool'` |
| `'tool'` | `{"PATH": "$ROOT/bin1:$ROOT/extd", "PATHEXT": ".sh"}` | `'$ROOT/bin1/tool'` |
| `'a/b'` | `{"PATH": "$ROOT/bin1"}` | `'a/b'` |
| `'./tool3'` | `{"PATH": "$ROOT/bin1"}` | `None` |
| `'bin1/../tool3'` | `{"PATH": ""}` | `'tool3'` |
| `'tool/..'` | `{"PATH": "$ROOT/bin1"}` | `None` |
| `''` | `{"PATH": "$ROOT/bin1"}` | `None` |
| `'nonexistent'` | `{"PATH": "$ROOT/bin1"}` | `None` |

### A5. Decisions where earlier docs differ (parity vs deliberate deviation)

| # | Topic | Earlier doc(s) | Evidence | Decision |
|---|---|---|---|---|
| D1 | pre-push stdin parsing | 03 §20.4 proposes split on `\n`, strip `\r`, `rsplitn(4, ' ')` over ASCII whitespace, skip malformed lines. 02 §16.3 and 03 §6.7 describe Python correctly. | V-PP; OBS (4.6.2): 3-field line → `ValueError: not enough values to unpack (expected 4, got 3)`, exit 3; empty line → `got 0`; U+2028 inside a ref → `got 1`; Latin-1 ref → `UnicodeDecodeError … position 14: invalid continuation byte`; NBSP inside the remote ref → fields shift, then `git rev-list refs/heads/a …` fails → `CalledProcessError: Command '('git', 'rev-list', 'refs/heads/a', '--topo-order', '--reverse', '--not', '--remotes=origin')' returned non-zero exit status 128.` (git's own `fatal: ambiguous argument …` stderr leaks first). | **Parity.** `decode_utf8(stdin)?` (eager) → `str_splitlines` → per line `rsplit_whitespace(line, Some(3))` → 4-way destructure, else the exact ValueError (lazy, after earlier lines were acted on). Rationale: identical for every input git produces; for anything else pre-commit **blocks the push** (exit 3) while 03's skip silently lets an unchecked push through — fail-open is unacceptable for a guard; `rsplitn(4, ' ')` is also wrong on trailing spaces (`'a b c d '` → `['', 'd', 'c', 'a b']`) and on tabs. Exception type and text are part of the contract (Tier B allows free-form only if doc 10 classifies them so). |
| D2 | `REV_LINE_RE` `\s` and `$` | 03 §20.6: "port exactly with the `regex` crate … `\s` must match Python's Unicode `\s`" (correct intent, unachievable with `\s`). 10 §5.11 quotes the pattern. | `sets.re_str_backslash_s` == PY-WS ≠ `White_Space`; prototype test `rust_std_pitfalls_are_real`; V-REV row `'\x1f rev: v1\n'` matches in Python. | **Parity** via the regex-free matcher (§A4.6) or the explicit-class regex. Never write `\s` in a pattern that emulates a Python str regex. |
| D3 | CRLF `\r` loss on frozen-comment change | not covered anywhere | V-REVW rows with `\r\n`; OBS 4.6.2 `autoupdate --freeze` then plain `autoupdate` on a CRLF file. | **Deliberate deviation DEV-PY-1**: when `comment` replaces g5 and g5 ends with `\r`, append `\r` before g6. Output becomes consistently CRLF; register in the deviations list and normalize this line in differential tests. |
| D4 | `expanduser` with `HOME=""` | 07 §7 item 20: "`expanduser('~')` with `HOME=""` yields `''`" | V-PE: `'~'` → `'/'`, `'~/.cache'` → `'/.cache'`; Rust 1.98 `std::env::home_dir()` with `HOME=` returns the passwd home (probe). | **Parity**, own `expanduser`; call the passwd lookup only when `HOME` is **unset**. Correct 07 §7 item 20's statement. |
| D5 | Windows home lookup | 07 §7 item 17: "`std::env::home_dir` follows `USERPROFILE`, close to Python `ntpath.expanduser`" | Rust: `USERPROFILE` if set **and non-empty**, else `GetUserProfileDirectoryW`; Python: `USERPROFILE` even empty, else `HOMEDRIVE`+`HOMEPATH`, else unchanged (V-NE). | **Parity**, own env-based implementation; never call `home_dir` on Windows. |
| D6 | POSIX `realpath` | 07 §7 item 20: "canonicalize the longest existing ancestor and append the rest lexically" | V-RP: `dangling` → `$ROOT/nonexistent` (ancestor recipe: `$ROOT/dangling`), `missing/../link_a` resolves `link_a` after the lexical pop, loops left unresolved, `link_file/x` → `$ROOT/a/f/x`. By code, a dangling-symlink `PRE_COMMIT_HOME` makes pre-commit create the store at the link **target** (`store.py:36,70-71`). | **Parity**: port `_joinrealpath` on POSIX (prototype `posixpath::realpath`, fixture-verified); the ancestor recipe is the Windows algorithm only (§A4.10). |
| D7 | Executable lookup | 06 §8 "PATHEXT via `which`"; 04a §14.18 "do not use `which`" | `which` 8.0.6 `src/finder.rs`: `new_paths` filters empty PATH entries on Windows (`.filter(\|p\| !is_windows \|\| !p.as_os_str().is_empty())`), applies `tilde_expansion` and `to_absolute(cwd)` to relative entries, `PathsIter::next` yields the bare name **before** `name+ext`, skips PATHEXT when the name already has an executable extension, `correct_casing` re-cases on Windows; PATHEXT is consulted only on Windows. Python: V-FE. | **04a is right; 06 §8 is wrong.** Own ~40-line `find_executable` (+ `rustix::fs::access` on Unix). |
| D8 | Env-dir naming | 03 §20.5 item 9 (`<ENV>-<version>` only for `^[A-Za-z0-9._+-]{1,64}$`, else `<ENV>-h<hash>`) vs 04a §14.2 (literal name; nested dirs for absolute interpreter paths "still work"; reject escapes) | `posixpath.join('/r', 'py_env-/usr/bin/python3')` → `'/r/py_env-/usr/bin/python3'` (nested, works on POSIX); `normpath` collapses `..` (escape possible); `ntpath.join('C:\\r', 'py_env-C:\\Python312\\python.exe')` → `'C:\\r\\py_env-C:\\Python312\\python.exe'` — a `:` inside a non-drive component, which Win32 rejects or treats as an NTFS stream (needs a Windows CI check). | Path semantics favor **03 §20.5**: keep the literal name for safe versions (all common values: `default`, `system`, `python3.12`, `3.12`, `18.17.0`), hash otherwise. 04a's "still works" holds on POSIX only. Owners of 03/04a decide; this addendum only supplies the evidence. |
| D9 | Store location | 03 §20.5 / 10 Q5 / 06 §8 / 04a §14.10 (policy conflict) | Only the semantics are in scope: §A4.11 store paragraph. | No decision here (policy). Whatever the policy, compute pre-commit's path with these shims when interop is needed. |
| D10 | Locale encoding of migrate-config/autoupdate/install | none | `open()` without `encoding` (A2.8); Windows ANSI code page unless UTF-8 mode; cp1252 cannot decode bytes 0x81, 0x8D, 0x8F, 0x90, 0x9D; PEP 686 makes UTF-8 the default in 3.15. | **Deliberate deviation DEV-PY-3**: snp reads and writes these files as UTF-8 bytes on every OS (identical on Linux/macOS with a UTF-8 locale). Keep each site's newline semantics (A2.8). |
| D11 | Hook-script newline on Windows | 03 §20.3 proposes an LF POSIX-sh script | pre-commit writes `hook-tmpl` with `open(…, 'w')` → CRLF on Windows (A2.8). | Out of scope for parity (snp's own template); 03's LF choice stands. |
| D12 | Non-UTF-8 paths and output | 02/03 use `OsStr`/bytes | A2.4: pre-commit crashes on undecodable git output and on printing surrogate-escaped paths. | Recommendation (owner: run-pipeline doc): handle paths as bytes end-to-end and print raw bytes (DEV-PY-2, crash → works). Where parity is required (stdin, MERGE_MSG, config), use `decode_utf8` and the exact crash text. |
| D13 | Version drift | 10 Q7 (UCD 15.0 vs 15.1) | delta file: only `isprintable` ranges and ntpath `isabs`/`commonpath`/`abspath`-fallback cases. | Pin CPython 3.12 / UCD 15.0 tables; never surface `commonpath` messages (always caught). |

Contradiction triage for the other reported conflicts: **Hook stdin** (02 §21.3 / 06 §8 vs 04a §14.3), **YAML
error-text parity**, **regex engine**, **async runtime**, **toolchain pin**, **hazmat failure exit code**,
**PRE_COMMIT_COLOR**, **argparse error prefix**, **install-state files**, **crash log**, **CPU count**, **fsmonitor
flag**, **stash restore algorithm**, **in-process emulation granularity** — none depends on string/path/repr
semantics, so this addendum does not rule on them (the argparse one does involve repr: invalid-choice messages repr
both the value and each choice, V `argparse_invalid_choice`).

### A6. `pycompat` Rust module API (normative)

Crate `snp-pycompat` (doc 07 §4.1 layout). The prototype implements a verified subset with simpler signatures (Vec-returning
splitters, `&str` ntpath); the API below is the production target. `#![forbid(unsafe_code)]`, no panics on any input (clippy
`unwrap_used/expect_used/panic/indexing_slicing = deny`), MSRV/toolchain 1.98. Dependencies: none for the core; `rustix`
(`fs`) on Unix for `access`; generated tables in-tree. Every function names the Python expression it reproduces in
its doc comment and has a vector section.

```rust
pub mod chars {
    pub const fn is_space(c: char) -> bool;              // PY-WS: str.isspace / re str-pattern \s
    pub const fn is_linebreak(c: char) -> bool;          // PY-LB (caller merges "\r\n")
    pub const fn is_bytes_space(b: u8) -> bool;          // B-WS
    pub fn is_printable(c: char) -> bool;                // PY-PRINTABLE, UCD 15.0 table, binary search
    pub fn decimal_value(c: char) -> Option<u32>;        // unicodedata.decimal
    pub const PY_STR_SPACE_CLASS: &str;                  // regex-crate class body for Python `\s`
    pub const UNICODE_VERSION: &str;                     // "15.0.0"
}
pub mod text {
    pub fn splitlines(s: &str) -> SplitLines<'_>;                 // Iterator<Item = &str>, zero-copy
    pub fn splitlines_keepends(s: &str) -> SplitLines<'_>;
    pub fn bytes_splitlines(b: &[u8]) -> BytesSplitLines<'_>;     // + bytes_splitlines_keepends
    pub fn split_whitespace(s: &str, maxsplit: Option<usize>) -> SplitWs<'_>;   // Iterator
    pub fn rsplit_whitespace(s: &str, maxsplit: Option<usize>) -> Vec<&str>;    // left-to-right
    pub fn rsplit4(s: &str) -> Result<[&str; 4], UnpackError>;     // pre-push fast path, no alloc
    pub fn strip(s: &str) -> &str; pub fn lstrip(s: &str) -> &str; pub fn rstrip(s: &str) -> &str;
    pub fn strip_chars<'a>(s: &'a str, set: &str) -> &'a str;     // + lstrip_chars / rstrip_chars
    pub fn bytes_strip(b: &[u8]) -> &[u8];                        // + bytes_lstrip / bytes_rstrip
    pub fn bytes_strip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8];  // + l/r variants
    pub fn strip_fsbytes(b: &[u8]) -> &[u8];  // str.strip() of os.fsdecode(b): UTF-8-aware PY-WS, invalid bytes are non-space
    pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str);
}
pub mod codec {
    pub fn decode_utf8(b: &[u8]) -> Result<&str, Utf8DecodeError>;      // Display = CPython text
    pub fn decode_utf8_backslashreplace(b: &[u8]) -> Cow<'_, str>;
    pub fn fsdecode_chars(b: &[u8]) -> impl Iterator<Item = PyChar> + Clone + '_;  // surrogateescape view
    pub struct Utf8DecodeError { pub start: usize, pub end: usize, pub first_byte: u8, pub reason: Utf8Reason }
    pub struct Utf8EncodeError { /* "can't encode character '\udcff' in position N: surrogates not allowed" */ }
}
pub mod repr {
    pub fn repr_str(s: &str) -> String;  pub fn write_repr_str(out: &mut String, s: &str);
    pub fn repr_os(s: &OsStr) -> String;               // Unix: fsdecode view; Windows: WTF-16 with \uXXXX for lone surrogates
    pub fn repr_str_tuple<S: AsRef<str>>(xs: &[S]) -> String;  pub fn repr_str_list<S: AsRef<str>>(xs: &[S]) -> String;
    pub enum PyValue { None, Bool(bool), Int(PyInt), Float(f64), Str(String), List(Vec<PyValue>), Tuple(Vec<PyValue>), Dict(Vec<(PyValue, PyValue)>) }
    pub enum PyInt { Small(i128), Decimal(Box<str>) }   // YAML ints are unbounded; repr = decimal text
    pub fn repr(v: &PyValue) -> String;  pub fn repr_float(x: f64) -> String;
    pub fn fstring_eq(name: &str, v: &str) -> String;                  // f'{name=}'
    pub fn oserror_str(errno: i32, strerror: &str, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String;
    pub fn oserror_from_io(e: &io::Error, filename: Option<&OsStr>) -> String;  // [Errno N] / [WinError N]
    pub fn stdlib_called_process_error_str(rc: i32, cmd_str: &str) -> String;
    pub fn int_literal_error(literal: &str) -> String;                 // %.200R truncation
}
pub mod int { pub fn parse_int(s: &str) -> Result<i128, IntParseError>; pub fn parse_version(s: &str) -> Result<Vec<i128>, IntParseError>; }
pub mod shlex {
    pub fn quote(s: &str) -> Cow<'_, str>;
    pub fn join<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> String;
    pub fn split(s: &str) -> Result<Vec<String>, ShlexError>;           // NoClosingQuotation | NoEscapedCharacter
}
pub mod posixpath {   // &[u8] in, Vec<u8>/slices out; compiled on every OS
    pub fn normpath(p: &[u8]) -> Vec<u8>;  pub fn join(a: &[u8], rest: &[&[u8]]) -> Vec<u8>;
    pub fn split(p: &[u8]) -> (&[u8], &[u8]);  pub fn dirname(p: &[u8]) -> &[u8];  pub fn basename(p: &[u8]) -> &[u8];
    pub fn splitext(p: &[u8]) -> (&[u8], &[u8]);  pub fn isabs(p: &[u8]) -> bool;
    pub fn abspath(p: &[u8], cwd: &[u8]) -> Vec<u8>;
    pub fn relpath(p: &[u8], start: Option<&[u8]>, cwd: &[u8]) -> Result<Vec<u8>, PathError>;
    pub fn commonpath(ps: &[&[u8]]) -> Result<Vec<u8>, PathError>;
    pub fn expanduser<'a>(p: &'a [u8], home_env: Option<&[u8]>, pw_current: impl FnOnce() -> Option<Vec<u8>>,
                          pw_named: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> Cow<'a, [u8]>;
    #[cfg(unix)] pub fn realpath(p: &[u8], cwd: &[u8]) -> io::Result<Vec<u8>>;
}
pub mod ntpath {      // generic over code units: u8 (UTF-8 text, tests) and u16 (OsStr::encode_wide); compiled on every OS
    pub fn splitroot<U: Unit>(p: &[U]) -> (&[U], &[U], &[U]);  pub fn splitdrive<U: Unit>(p: &[U]) -> (&[U], &[U]);
    pub fn normpath<U: Unit>(p: &[U]) -> Vec<U>;  pub fn join<U: Unit>(a: &[U], rest: &[&[U]]) -> Vec<U>;
    pub fn split / dirname / basename / splitext / isabs_312 / normcase / commonpath / expanduser(p, env: impl Fn(&str) -> Option<OsString>);
    pub fn abspath_fallback(p, cwd) -> Vec<U>;                 // tests only
    #[cfg(windows)] pub fn abspath(p: &OsStr) -> io::Result<PathBuf>;   // std::path::absolute(normpath(p))
    #[cfg(windows)] pub fn relpath(p: &OsStr, start: Option<&OsStr>) -> Result<OsString, PathError>;
    #[cfg(windows)] pub fn realpath(p: &OsStr) -> io::Result<PathBuf>;  // §A4.10 algorithm
}
pub mod ospath { /* cfg-selected facade over posixpath/ntpath taking &OsStr/&Path; what callers use */ }
pub mod textio {
    pub fn universal_newlines(s: &str) -> Cow<'_, str>;   pub fn windows_text_write(s: &str) -> Cow<'_, str>;
    pub fn textwrap_indent(text: &str, prefix: &str) -> String;
}
pub enum PathError { NoPathSpecified, EmptySequence, MixAbsRel, DifferentDrives, DifferentMounts { path_drive: OsString, start_drive: OsString } }
// Display of every error = the Python str(exc); a `py_type()` method returns "ValueError" / "UnicodeDecodeError" … for the crash line.
```

Composites live in their consumer crates but must call these shims: `zsplit` (bytes iterator over `-z` output; strip
leading/trailing NULs; interior empties kept), `get_skips` (`text::strip_fsbytes` on `var_os("SKIP")`), pre-push
(`decode_utf8` → `splitlines` → `rsplit4`), `merge_msg_conflicts`, `rev_line_match` (§A4.6 manual form),
`migrate_header_split`, `exe_exists`, `adjust_args` (`ospath::abspath`/`relpath` with explicit cwd values; snp never
`chdir`s — `std::env::set_current_dir` is process-global and races in tests).

Performance and memory rules: return borrowed slices/iterators (`SplitLines` is a `FusedIterator` with
`DoubleEndedIterator` where cheap); `Cow` for transforms that are usually identity (`universal_newlines`, `quote`,
`decode_utf8_backslashreplace`, `expanduser`); ASCII fast paths (`s.is_ascii()` → byte scanning); `memchr` for NUL/`\n`
scans on large git outputs; `LazyLock` for any compiled regex; tables are `static` sorted ranges with binary search
(711 entries ≈ 5.7 KB). No allocation per item in `zsplit`/`splitlines`. `repr_str` pre-sizes `len + 2`.

### A7. Test-kit integration

* Replay: one `#[test]` per JSON section family (the prototype's `tests/vectors.rs` is a working template: hex/`*_cps`
  decoding, tagged repr values, `{"ok"|"error"}` outcomes, fixtures recreated under a temp dir with `$ROOT`
  substitution, `cwd` passed explicitly instead of `chdir`).
* Fixtures: realpath (dirs `a`, `a/b`; file `a/f`; 10 symlinks listed in V-RP) and find_executable (modes in V-FE)
  must be recreated with the same names; canonicalize the temp root first (macOS `/var` → `/private/var`).
* Windows CI: run the `ntpath_*` lexical vectors everywhere; on `windows-2025` additionally assert
  `ospath::abspath`/`relpath`/`realpath` against a live `python -I` oracle for the `real_windows_may_differ` rows
  (nightly live-oracle mode, doc 10 §7.5) and the env-dir `:` question (D8).
* Property tests (proptest 1.11): `shlex::split(&shlex::quote(s)) == [s]` for `s` without NUL; `split_whitespace(s,
  None)` never yields empty or space-containing items; `rsplit_whitespace(s, Some(n)).len() <= n + 1`;
  `normpath(normpath(p)) == normpath(p)` (both flavors); `relpath(abspath(p, cwd), Some(cwd), cwd) == normpath(p)` for relative `p`
  whose `normpath` does not start with `..`; `decode_utf8` agrees with `std::str::from_utf8` on success; `splitlines` concatenation with
  `keepends` reproduces the input. Fuzz targets: `decode_utf8`, `shlex::split`, `rev_line_match`, `rsplit4`,
  `ntpath::splitroot` (no panics).
* Live-oracle extension: the generator's functions are pure; a JSON-lines worker can expose them (`python -I worker.py`)
  for nightly differential fuzzing with proptest-generated inputs.

### A8. Pitfall checklist (each has a test in the prototype or a vector)

| Rust idiom | Python it does NOT match | Use instead |
|---|---|---|
| `str::lines()` | `str.splitlines()` (misses `\r`, `\x0b`, `\x0c`, `\x1c-\x1e`, U+0085, U+2028/9) and `bytes.splitlines()` (misses lone `\r`) | `text::splitlines`, `text::bytes_splitlines` |
| `str::trim()`, `split_whitespace()`, `char::is_whitespace`, regex `\s` | PY-WS (`\x1c-\x1f`) | `text::strip`, `text::split_whitespace`, `PY_STR_SPACE_CLASS` |
| `<[u8]>::trim_ascii()`, `split_ascii_whitespace`, `u8::is_ascii_whitespace` | B-WS (`\x0b`) | `text::bytes_strip` |
| `rsplitn(4, ' ')` | `rsplit(maxsplit=3)` (runs, tabs, trailing WS, leading WS kept in remainder) | `text::rsplit4` |
| `trim_end_matches("\r\n")` (suffix) | `rstrip('\r\n')` (set) | `bytes_rstrip_set` |
| `{:?}` / `{}` on `&str` | `repr()` | `repr::repr_str` |
| `format!("{}", f64)` | `repr(float)` (`1e+16`, `1e-05`, `1.0`) | `repr::repr_float` |
| `std::path::absolute` on Unix | `abspath` (Rust keeps `..`: `absolute("a/../b")` = `$CWD/a/../b`; `//x/..` stays) | `posixpath::abspath` |
| `Path::components()` / `PathBuf::push` | `normpath` (`//a` → `/a` in Rust; Python keeps `//a`) | `posixpath::normpath` |
| `Path::file_name()` / `extension()` | `basename('a/b/') == ''`; `splitext('...md') == ('...md', '')` | `posixpath::basename/splitext` |
| `fs::canonicalize` | non-strict POSIX `realpath` | `posixpath::realpath` |
| `std::env::home_dir()` | `expanduser` (`HOME=""`, Windows `USERPROFILE=""`, `HOMEPATH`) | `*::expanduser` with env lookups |
| `which::which` | `find_executable` | own port |
| `char::to_digit(10)` | `int()` digits (any `Nd`) | `int::parse_int` |
| `shlex` / `shell-words` crates | `shlex.split` (`#`, `\$` in `"…"`) | `shlex::split` |
| `BufRead::lines()` on config text | universal newlines (`\r` alone is a break) | `textio::universal_newlines` + split |
| `std::env::set_current_dir` | pre-commit's `os.chdir(toplevel)` | explicit `cwd` arguments |

### A9. Open questions

1. **Pre-push parity tier.** D1 keeps exit 3 and the exact `ValueError`/`UnicodeDecodeError` text. Is a friendlier
   snp message with the same exit code acceptable (doc 10 Tier B), or must the crash line be byte-identical?
2. **DEV-PY-1 (CRLF rev line).** Confirm the deviation; alternatively keep parity behind a `compat=strict` switch.
3. **Windows env dir with `:`** (D8): does `os.makedirs('…\\py_env-C:\\Python312\\python.exe')` fail on Windows
   (ERROR_INVALID_NAME) or create an ADS? Needs a `windows-2025` run of pre-commit 4.6.2 with
   `language_version: C:\Python312\python.exe`.
4. **UCD pinning.** Users run pre-commit on 3.10–3.14 (UCD 13.0–16.0); only `repr` of newly assigned or
   reclassified code points differs. Pin 15.0 (3.12, the oracle) or track the newest CPython?
5. **`normcase` on Windows** uses `LCMapStringEx` invariant lowercasing, not `str.lower()`; for non-ASCII drive or
   path components (`İ`) results could differ from the Linux-generated vectors. Accept ASCII-exactness only?
6. **Non-UTF-8 everywhere (D12):** confirm the deviation for git `-z` outputs and printed paths, and decide the exact
   policy for non-UTF-8 `SKIP` values (surrogateescape semantics are specified in `strip_fsbytes`).
7. **Locale encoding (D10):** confirm DEV-PY-3 (always UTF-8) for migrate-config/autoupdate on Windows.
